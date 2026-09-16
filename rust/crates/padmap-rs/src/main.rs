//! padmap's republisher, in Rust.
//!
//!     padmap list          what is plugged in
//!     padmap setup         assign player order by pressing a button
//!     padmap map           record which button is which
//!     padmap calibrate     measure where each controller's sticks rest
//!     padmap forget        delete stored controller profiles
//!     padmap run           republish the assigned pads and keep them alive
//!     padmap serve         the same, as a daemon a client drives
//!     padmap launch        republish, then start RetroArch
//!     padmap play          resolve mappings for the game about to run
//!     padmap hide          udev rules that hide the physical pads
//!     padmap ensure-daemon start the daemon, or restart a stale one
//!     padmap clean-config  strip padmap values out of retroarch.cfg
//!     padmap emit          write the emulator config files, from JSON
//!                          (--cemu-dir, --ares-settings, --ryujinx-config,
//!                          --env-file to say where)
//!     padmap exec          run a program with padmap's mappings set
//!     padmap sdl-mapping <guid>
//!                          what SDL's built-in database says about a GUID
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
use padmap_core::emit::{self, Identity};
use padmap_core::hide;
use std::path::PathBuf;

mod commands;

use padmap_input::{
    artefacts, assignments, clone, emulators, lizard, pad, profiles, reactor, republish, runtime,
    triton,
};

/// How often the tick runs. Matches the Python's `TICK_SECONDS`.
const TICK: Duration = Duration::from_millis(20);

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let mut args = std::env::args().skip(1);
    let command = args.next();
    let rest: Vec<String> = args.collect();
    match command.as_deref() {
        Some("list") => cmd_list(),
        Some("hide") => cmd_hide(&rest),
        Some("run") => cmd_run(),
        Some("serve") => cmd_serve(),
        Some("emit") => cmd_emit(&rest),
        Some("exec") => cmd_exec(rest),
        Some("sdl-mapping") => cmd_sdl_mapping(rest.into_iter().next()),
        Some("play") => commands::cmd_play(rest),
        Some("setup") => {
            let wanted = flag_value(&rest, &["-n", "--players"])
                .map(|value| parse_number(&value, "--players"));
            commands::cmd_setup(wanted)
        }
        Some("forget") => commands::cmd_forget(rest.iter().any(|arg| arg == "--all")),
        Some("calibrate") => {
            commands::cmd_calibrate(rest.iter().any(|arg| arg == "-f" || arg == "--force"))
        }
        Some("map") => commands::cmd_map(
            flag_value(&rest, &["--layout"]),
            flag_value(&rest, &["--pad"]),
            flag_value(&rest, &["--scope"]).unwrap_or_default(),
        ),
        Some("clean-config") => commands::cmd_clean_config(
            flag_value(&rest, &["--config"]),
            rest.iter().any(|arg| arg == "--dry-run"),
        ),
        Some("ensure-daemon") => commands::cmd_ensure_daemon(
            rest.iter().any(|arg| arg == "--check"),
            flag_value(&rest, &["--timeout"])
                .map(|value| parse_number::<f64>(&value, "--timeout"))
                .unwrap_or(10.0),
        ),
        Some("launch") => {
            // `--log` takes an optional value, so it is read before the rest
            // is handed to RetroArch verbatim.
            let log = rest.iter().position(|arg| arg == "--log").map(|at| {
                rest.get(at + 1)
                    .filter(|value| !value.starts_with('-'))
                    .cloned()
            });
            let passthrough: Vec<String> =
                rest.iter().filter(|arg| *arg != "--log").cloned().collect();
            commands::cmd_launch(passthrough, log).map(|code| std::process::exit(code))
        }
        Some(other) => {
            eprintln!("padmap: unknown command {other:?}");
            usage();
            std::process::exit(2);
        }
        None => {
            usage();
            std::process::exit(2);
        }
    }
}

/// The value after a flag, or `None` if the flag was not given.
///
/// A flag that *is* given and has no value is refused rather than ignored:
/// `--layout` with the value forgotten would otherwise read as "no layout"
/// and walk the generic pad, and `--config` as "the default config", which is
/// the user's own retroarch.cfg.
fn flag_value(args: &[String], names: &[&str]) -> Option<String> {
    let at = args.iter().position(|arg| names.contains(&arg.as_str()))?;
    match args.get(at + 1) {
        Some(value) if !value.starts_with('-') => Some(value.clone()),
        _ => {
            eprintln!("padmap: {} expects a value", args[at]);
            std::process::exit(2);
        }
    }
}

/// A number, or a refusal naming the flag -- not a silent zero.
fn parse_number<T: std::str::FromStr>(value: &str, flag: &str) -> T {
    match value.parse() {
        Ok(parsed) => parsed,
        Err(_) => {
            eprintln!("padmap: {flag} expects a number, not {value:?}");
            std::process::exit(2);
        }
    }
}

fn usage() {
    eprintln!(
        "usage: padmap list | setup | map | calibrate | forget | run | serve | \
         launch | play | hide | ensure-daemon | clean-config | \
         emit [--cemu-dir D] [--ares-settings F] [--ryujinx-config F] \
         [--env-file F] | \
         exec -- <program> [args...] | sdl-mapping <guid>"
    );
}

/// The daemon.
///
/// SIGTERM has to release the grabs: without a handler an open session keeps
/// EVIOCGRAB on every pad as the process dies, leaving the machine with no
/// working controllers. Ask the loop to exit instead, so the normal teardown
/// runs -- and install the handlers *after* restore, so a restart interrupted
/// mid-restore still tears down through the same path.
fn cmd_serve() -> Result<()> {
    let mut server = padmap_daemon::server::Server::new().context("preparing the daemon")?;
    server.start().context("starting the daemon")?;
    server.restore();
    let stop = Arc::new(AtomicBool::new(false));
    install_signal_handlers(&stop)?;
    server.run(&stop);
    info!("terminating");
    server.close();
    Ok(())
}

/// Print SDL's own mapping line for a GUID, or nothing.
///
/// A process of its own on purpose: `SDL_Init` starts threads and enumerates
/// every joystick, and the daemon that asks holds those same devices grabbed.
/// See `padmap_input::sdlprobe`. Exit 0 with empty output is "SDL has never
/// heard of it", which is the ordinary answer and not a failure.
fn cmd_sdl_mapping(guid: Option<String>) -> Result<()> {
    let Some(guid) = guid else {
        eprintln!("usage: padmap-rs sdl-mapping <guid>");
        std::process::exit(2);
    };
    match padmap_input::sdlprobe::builtin_mapping(&guid) {
        Ok(Some(line)) => println!("{line}"),
        Ok(None) => {}
        Err(error) => anyhow::bail!("{error}"),
    }
    Ok(())
}

/// Write the emulator config files for the pads described on stdin.
///
/// The Python daemon is still the one that runs, and it calls this after every
/// republish. Stdin rather than a state file so the caller's view is the one
/// that is written -- reading `assignments.json` back would answer for whatever
/// is on disk now, which during a hotplug is not what the caller just
/// published.
///
/// Always exits 0 on a well-formed request. An emulator that is not installed
/// is a skip, and a daemon must not learn to treat that as a failure.
fn cmd_emit(args: &[String]) -> Result<()> {
    let mut body = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut body)
        .context("reading the pad list from stdin")?;
    let pads: Vec<emulators::Published> =
        serde_json::from_str(&body).context("parsing the pad list")?;

    // A caller that keeps each game in an environment of its own -- its own
    // state directory, its own config -- is not writing to the user's home,
    // and two variants of one game must not share a Ryujinx configuration.
    // An absent flag keeps the default location, so overriding one leaves the
    // others alone.
    let destinations = emulators::Destinations {
        cemu_dir: flag_value(args, &["--cemu-dir"]).map(PathBuf::from),
        ares_settings: flag_value(args, &["--ares-settings"]).map(PathBuf::from),
        ryujinx_config: flag_value(args, &["--ryujinx-config"]).map(PathBuf::from),
        env_file: flag_value(args, &["--env-file"]).map(PathBuf::from),
    };
    let written = emulators::publish(&pads, &destinations);
    for path in &written.paths {
        println!("{}", path.display());
    }
    for (target, why) in &written.skipped {
        info!("{target}: not written ({why})");
    }
    Ok(())
}

/// Run a program with padmap's mappings already in its environment.
///
/// For Cemu and anything else that reads no controller database: the pads are
/// simply absent from its device list until SDL is told about them, and SDL
/// reads its database once at startup. `padmap-rs exec -- Cemu` is the whole
/// of the fix, and needs nothing from the program being run.
fn cmd_exec(args: Vec<String>) -> Result<()> {
    let args: Vec<String> = args.into_iter().skip_while(|arg| arg == "--").collect();
    let Some((program, rest)) = args.split_first() else {
        eprintln!("usage: padmap-rs exec -- <program> [args...]");
        std::process::exit(2);
    };

    // The file the daemon wrote at its last republish, which is the same value
    // it put in every emulator's config. Read rather than recomputed: this
    // process has no pads open and opening them would grab them away from the
    // daemon that does.
    let value = match std::fs::read_to_string(emulators::env_path()) {
        Ok(text) => emulators::value_from_script(&text).unwrap_or_default(),
        Err(error) => {
            warn!(
                "no mappings at {} ({error}); {program} will see whatever SDL \
                 already knows",
                emulators::env_path().display()
            );
            String::new()
        }
    };

    let mut command = std::process::Command::new(program);
    command.args(rest);
    if !value.is_empty() {
        command.env(emulators::CONFIG_ENV, value);
    }
    let status = command
        .status()
        .with_context(|| format!("running {program}"))?;
    std::process::exit(status.code().unwrap_or(1));
}

fn cmd_list() -> Result<()> {
    let pads = pad::discover(pad::Filter::default()).context("enumerating input devices")?;
    if pads.is_empty() {
        println!("No joypads found.");
        // The case that matters most: a controller is plugged in and the
        // kernel is not treating it as one, so "none found" is true and
        // useless.
        report_dormant();
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

    report_dormant();

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

/// Say when a controller is present but the kernel is not driving it as one.
///
/// "No joypads found" is true and useless when the controller is sitting there
/// pretending to be a keyboard. Nothing else on the machine will say so: the
/// device enumerates perfectly and every layer below this one is behaving
/// correctly.
fn report_dormant() {
    // padmap drives everything it has a protocol for (see `triton.rs`), so
    // what is left here is a model with no driver, or a receiver with nothing
    // paired into it. Neither needs more than a line.
    let driven: Vec<PathBuf> = triton::slots(true)
        .into_iter()
        .map(|pad| pad.path)
        .collect();
    for device in lizard::dormant() {
        if device.channels.iter().any(|node| driven.contains(node)) {
            continue;
        }
        let Some(model) = device.late_model() else {
            continue;
        };
        let what = if model.receiver {
            " (nothing paired to it)"
        } else {
            ""
        };
        println!(
            "\n{} ({:04x}:{:04x}) is not reporting as a controller{what}.",
            if device.name.is_empty() {
                "An unnamed device"
            } else {
                &device.name
            },
            device.vid,
            device.pid
        );
    }
}

/// Print the udev rules that hide the physical pads.
///
/// Printed rather than installed: writing them needs root, and a command that
/// silently asks for a password on a machine plugged into a television is
/// worse than one that shows you what to write. The Python's `padmap hide`
/// installs them when it is already root; this does not yet.
fn cmd_hide(args: &[String]) -> Result<()> {
    let pads = pad::discover(pad::Filter::default()).context("enumerating input devices")?;
    let hideable: Vec<hide::Hideable> = pads
        .iter()
        .map(|pad| hide::Hideable {
            name: pad.name.clone(),
            vid: pad.vid,
            pid: pad.pid,
        })
        .collect();
    let targets = hide::targets(&hideable, &[]);
    if targets.is_empty() {
        println!("No pads with usable ids; there is nothing safe to match on.");
        return Ok(());
    }
    let rules = hide::generate_rules(&targets);
    // Running as root is taken as the instruction to install: there is no
    // other reason to run this with privileges, and printing a script for
    // someone who already typed `sudo` to paste back into the same shell is a
    // step that exists only to be got wrong.
    let root = rustix::process::geteuid().is_root();
    if args.iter().any(|arg| arg == "--print") || !root {
        if args.iter().any(|arg| arg == "--install") && !root {
            eprintln!("Installing needs root. Re-run:  sudo padmap hide\n");
        }
        print!("{rules}");
        eprintln!();
        eprintln!("Write that to {} and reload:", commands::RUNTIME_RULES_PATH);
        eprintln!("  sudo udevadm control --reload-rules");
        eprintln!("  sudo udevadm trigger --subsystem-match=input");
        eprintln!();
        eprintln!("While it is installed and padmap is NOT running, these");
        eprintln!("controllers are invisible. Delete the file to undo it.");
        return Ok(());
    }

    println!("Hiding {} adapter(s):", targets.len());
    for target in &targets {
        println!("  {:04x}:{:04x}  {}", target.vid, target.pid, target.name);
    }
    println!();
    let (changed, messages) = commands::install_rules(&rules);
    for message in &messages {
        println!("  {message}");
    }
    let failed = messages
        .iter()
        .any(|message| message.contains("could not") || message.contains("failed"));
    if changed && !failed {
        println!(
            "\nCaution: while these rules are active and padmap is not running,\n\
             these controllers are invisible entirely.\nUndo with:  sudo rm {} && \
             sudo udevadm control --reload-rules",
            commands::RUNTIME_RULES_PATH
        );
    }
    if failed {
        std::process::exit(1);
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

    // The two artefacts other programs read. Written before the loop starts,
    // so a consumer launched immediately afterwards finds them already there:
    // SDL reads its database once, at startup, and a mapping that lands later
    // does nothing until that program is restarted.
    if let Err(error) = publish_artefacts(&vpads) {
        // Not fatal. A pad that is republished but unmapped still works as a
        // pad; one that is not republished at all does not exist.
        warn!("could not write the mapping files: {error}");
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
                // `run` has no socket and no session; those kinds are the
                // daemon's, and are never registered here.
                reactor::Watched::Listener
                | reactor::Watched::Client(_)
                | reactor::Watched::Session(_) => {}
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

/// Write the SDL database and the RetroArch autoconfig for these pads.
///
/// Both come from the stored profile: a capture the user performed, resolved
/// under no console context, because at republish time nothing knows what is
/// about to run. A launcher regenerates the autoconfig with the real context
/// immediately before starting a game.
fn publish_artefacts(vpads: &[padmap_input::VirtualPad]) -> Result<()> {
    let mut sdl_lines = BTreeMap::new();
    let mut profiles_out = BTreeMap::new();
    let mut identities = BTreeMap::new();
    let mut published: Vec<emulators::Published> = Vec::new();

    for vpad in vpads {
        let identity = Identity {
            bustype: vpad.identity.bustype,
            vendor: vpad.identity.vendor,
            product: vpad.identity.product,
            version: vpad.identity.version,
        };
        identities.insert(vpad.player, identity);

        let stored = profiles::load(&vpad.pad, None);
        let (_scope, mapping) = stored
            .as_ref()
            .map(|profile| profile.resolve("", ""))
            .unwrap_or_default();
        let bindings = mapping.resolved();

        // A pad nobody has mapped still has to be usable, or the user cannot
        // reach whatever would let them map it. The face buttons in the guess
        // really are a guess; the d-pad and sticks come from the pad's own
        // capabilities and are not.
        let line = if bindings.is_empty() {
            let (keys, axis_codes) = vpad.source.capabilities();
            let spans = vpad.source.axis_spans();
            let guessed = padmap_core::guess::guessed_fields(&keys, &axis_codes, Some(&spans));
            emit::sdl_line(
                &emit::virtual_guid(vpad.player, identity),
                &emit::virtual_name(vpad.player),
                &guessed,
            )
        } else {
            emit::sdl_line_for(vpad.player, identity, &bindings, None)
        };
        // Cemu, ares and Ryujinx each need something the SDL database cannot
        // give them: see `emulators`. They take the *clone's* capabilities,
        // not the controller's, because SDL opens the clone.
        let (keys, axes) = vpad.source.capabilities();
        published.push(emulators::Published {
            player: vpad.player,
            guid: emit::virtual_guid(vpad.player, identity),
            name: emit::virtual_name(vpad.player),
            keys,
            axes,
            sdl_line: line.clone(),
        });

        sdl_lines.insert(vpad.player, line);
        profiles_out.insert(
            vpad.player,
            emit::retroarch_profile(
                vpad.player,
                identity,
                &bindings,
                "",
                &mapping.layout,
                "",
                "",
            ),
        );
    }

    // A slot with no identity of its own falls back to padmap's, which is
    // only ever asked about players that are not attached right now.
    let fallback = Identity {
        bustype: 0x06,
        vendor: clone::PADMAP_VID,
        product: clone::PADMAP_PID,
        version: clone::PADMAP_VERSION,
    };
    let database = artefacts::write_sdl_database(
        &sdl_lines,
        &BTreeMap::new(),
        |player| identities.get(&player).copied().unwrap_or(fallback),
        None,
    )?;
    let written = artefacts::write_autoconfig(&profiles_out, None)?;

    info!("SDL mappings: {}", database.display());
    if let Some(first) = written.first() {
        if let Some(dir) = first.parent() {
            info!("RetroArch autoconfig: {}", dir.display());
        }
    }

    // Best-effort by design. Most machines have none of these three
    // installed, and "ares has never run" must read as an ordinary skip
    // rather than as the mapping files having failed.
    let emulators = emulators::publish(&published, &emulators::Destinations::default());
    for path in &emulators.paths {
        info!("emulator config: {}", path.display());
    }
    for (target, why) in &emulators.skipped {
        info!("{target}: not written ({why})");
    }
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
