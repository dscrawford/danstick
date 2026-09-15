//! padmap's republisher, in Rust.
//!
//!     padmap-rs list     what is plugged in
//!     padmap-rs run      republish the assigned pads and keep them alive
//!
//! Deliberately not the whole of `padmap`. The daemon's socket protocol, the
//! assignment session, the mapping wizard and every offline command stay in
//! Python for now; this is the forwarding path and the profile store it reads.
//!
//! It reads and writes the same `assignments.json` the Python does, so the two
//! can be swapped for each other while the port is in progress -- which is also
//! what makes `tools/latency.py --command` an A/B rather than an anecdote.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use log::{info, warn};
use padmap_input::{assignments, clone, pad, profiles, reactor, republish, runtime};

/// How often the tick runs. Matches the Python's `TICK_SECONDS`.
const TICK: Duration = Duration::from_millis(20);

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("list") => cmd_list(),
        Some("run") => cmd_run(),
        Some(other) => {
            eprintln!("padmap-rs: unknown command {other:?}");
            usage();
            std::process::exit(2);
        }
        None => {
            usage();
            std::process::exit(2);
        }
    }
}

fn usage() {
    eprintln!("usage: padmap-rs list | run");
}

fn cmd_list() -> Result<()> {
    let pads = pad::discover(pad::Filter::default()).context("enumerating input devices")?;
    if pads.is_empty() {
        println!("No joypads found.");
        return Ok(());
    }

    // Indices count only the pads RetroArch can see, so a hidden pad gets no
    // index rather than silently shifting the ones below it.
    println!(
        "{} pad(s), in the order RetroArch would enumerate them:\n",
        pads.len()
    );
    let mut index = 0;
    let mut hidden = 0;
    for pad in &pads {
        let label = if pad.retroarch_visible {
            let label = format!("[{index}]");
            index += 1;
            label
        } else {
            hidden += 1;
            " -- ".to_owned()
        };
        println!("  {label} {}", pad.name);
        println!(
            "        {}   phys={}",
            pad.path.display(),
            if pad.phys.is_empty() {
                "(none)"
            } else {
                &pad.phys
            }
        );
    }
    if hidden > 0 {
        println!("\n{hidden} pad(s) marked -- are hidden from RetroArch by udev");
        println!("rules (ID_INPUT_JOYSTICK cleared). padmap can still republish");
        println!("them; RetroArch sees only the virtual pads.");
    }

    let groups = pad::ambiguous_groups(&pads);
    if !groups.is_empty() {
        println!("\nIndistinguishable by every static attribute:");
        for group in &groups {
            let nodes: Vec<&str> = group.iter().map(|pad| pad.event()).collect();
            println!("  {}", group[0].name);
            println!("    {}", nodes.join(", "));
        }
        println!("\nNo config-file scheme can tell these apart. This is why");
        println!("assignment is done by pressing a button.");
    }
    Ok(())
}

fn cmd_run() -> Result<()> {
    let path = runtime::assignments_path();
    let saved = assignments::load(&path).context("reading the saved controller order")?;
    if saved.is_empty() {
        println!("No assignments. Run `padmap setup` first.");
        std::process::exit(1);
    }

    let pads = pad::discover(pad::Filter::default()).context("enumerating input devices")?;
    let (found, missing) = assignments::resolve(&saved, &pads);
    for gone in &missing {
        println!("  warning: {} ({}) is gone", gone.name, gone.path.display());
    }
    if found.is_empty() {
        println!("None of the assigned pads are attached.");
        std::process::exit(1);
    }

    let mode = clone::IdentityMode::from_env();

    let mut vpads = Vec::new();
    for (player, pad) in found {
        // The same store the Python writes, read by the same filename. A pad
        // with no profile is forwarded verbatim, which is correct for a
        // controller that centres itself; one with a measured resting position
        // is corrected in transit, which is the only place every consumer
        // benefits at once.
        let axes: BTreeMap<u16, padmap_core::calibration::AxisCalibration> =
            profiles::load(pad, None)
                .map(|stored| stored.axes)
                .unwrap_or_default();
        if !axes.is_empty() {
            info!(
                "player {player}: {} calibrated axis/axes from the profile store",
                axes.len()
            );
        }
        match clone::create(pad, player, mode, &axes, true) {
            Ok(vpad) => vpads.push(vpad),
            Err(error) => warn!("player {player}: {error}"),
        }
    }
    if vpads.is_empty() {
        anyhow::bail!("no pad could be republished");
    }

    let mut republisher = republish::Republisher::new(vpads);
    let mut reactor = reactor::Reactor::new(TICK).context("creating the event loop")?;
    for (index, vpad) in republisher.pads.iter().enumerate() {
        use std::os::fd::AsFd;
        reactor
            .watch(vpad.source.as_fd(), reactor::Watched::Source(index))
            .context("watching a pad")?;
        reactor
            .watch(vpad.clone.as_fd(), reactor::Watched::Clone(index))
            .context("watching a clone")?;
    }

    println!(
        "\nRepublishing {} pad(s). Ctrl-C to stop.\n",
        republisher.pads.len()
    );

    let stop = Arc::new(AtomicBool::new(false));
    install_signal_handlers(&stop)?;

    let mut late_ticks: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        let ready = match reactor.wait() {
            Ok(ready) => ready,
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error).context("waiting for input"),
        };
        for what in ready.iter() {
            match what {
                reactor::Watched::Source(index) => {
                    let pumped = republisher.forward(index);
                    if pumped.gone {
                        drop_pad(&reactor, &republisher, index);
                    }
                }
                reactor::Watched::Clone(index) => republisher.feedback(index),
                reactor::Watched::Tick => {
                    let expiries = reactor.take_tick();
                    // Nothing periodic belongs on this loop yet, and that is
                    // the point: the tick exists so that when something does,
                    // it runs 50 times a second and not once per event.
                    if expiries > 1 {
                        late_ticks += expiries - 1;
                    }
                }
            }
        }
    }

    if late_ticks > 0 {
        info!("{late_ticks} tick(s) were late; the loop fell behind that many periods");
    }
    for vpad in &republisher.pads {
        if vpad.dropped > 0 {
            info!(
                "player {}: the clone refused {} frame(s)",
                vpad.player, vpad.dropped
            );
        }
    }
    republisher.close();
    Ok(())
}

fn drop_pad(reactor: &reactor::Reactor, republisher: &republish::Republisher, index: usize) {
    use std::os::fd::AsFd;
    let Some(vpad) = republisher.pads.get(index) else {
        return;
    };
    // A dead node reports readable forever; left registered, the loop spins on
    // it for as long as the daemon runs. That is how the Python filled a 3.1GB
    // tmpfs with one warning per wakeup.
    let _ = reactor.unwatch(vpad.source.as_fd());
}

fn install_signal_handlers(stop: &Arc<AtomicBool>) -> Result<()> {
    // SIGTERM has to release the grabs. Without it the pads stay held by a
    // process the user no longer thinks exists, and the only cure is a replug.
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(stop))
            .with_context(|| format!("installing a handler for signal {signal}"))?;
    }
    Ok(())
}
