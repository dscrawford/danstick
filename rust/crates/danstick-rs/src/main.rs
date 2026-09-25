//! danstick's republisher, in Rust.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use danstick_core::emit::{self, Identity};
use danstick_core::hide;
use log::{info, warn};
use std::path::PathBuf;

mod commands;

use danstick_input::{
    artefacts, assignments, clone, emulators, isolate, lizard, pad, profiles, reactor, republish,
    runtime, triton,
};

const TICK: Duration = Duration::from_millis(20);

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    danstick_input::runtime::adopt_padmap_dirs();

    let mut args = std::env::args().skip(1);
    let command = args.next();
    let rest: Vec<String> = args.collect();
    match command.as_deref() {
        Some("list") => {
            if rest.iter().any(|arg| arg == "--json") {
                commands::cmd_list_json()
            } else {
                cmd_list()
            }
        }
        Some("hide") => cmd_hide(&rest),
        Some("run") => cmd_run(),
        Some("serve") => cmd_serve(&rest),
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
        Some("tune") => {
            if rest.iter().any(|arg| arg == "-h" || arg == "--help") {
                print!("{TUNE_USAGE}");
                return Ok(());
            }
            let request = tune_request(&rest)?;
            commands::cmd_tune(
                flag_value(&rest, &["--pad"]),
                request,
                rest.iter().any(|arg| arg == "--show"),
            )
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
            lifetime_flags(&rest),
        ),
        Some("launch") => {
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
            eprintln!("danstick: unknown command {other:?}");
            usage();
            std::process::exit(2);
        }
        None => {
            usage();
            std::process::exit(2);
        }
    }
}

fn flag_value(args: &[String], names: &[&str]) -> Option<String> {
    let at = args.iter().position(|arg| names.contains(&arg.as_str()))?;
    match args.get(at + 1) {
        Some(value) if !value.starts_with('-') => Some(value.clone()),
        _ => {
            eprintln!("danstick: {} expects a value", args[at]);
            std::process::exit(2);
        }
    }
}

fn parse_number<T: std::str::FromStr>(value: &str, flag: &str) -> T {
    match value.parse() {
        Ok(parsed) => parsed,
        Err(_) => {
            eprintln!("danstick: {flag} expects a number, not {value:?}");
            std::process::exit(2);
        }
    }
}

fn usage() {
    eprintln!(
        "usage: danstick list [--json] | setup | map | calibrate | tune | forget | run | \
         serve [--fresh] [--follow PID] [--slots fixed|on-demand] [--slot-count N] [--on-leave stay|destroy] [--layout position|label] | \
         launch | play | hide | ensure-daemon [--check] [--fresh] [--follow PID] | clean-config | \
         emit [--keyboard N] [--cemu-dir D] [--dolphin-dir D] [--ares-settings F] \
         [--ryujinx-config F] \
         [--env-file F] | \
         exec -- <program> [args...] | sdl-mapping <guid>"
    );
}

/// `--fresh` / `--follow PID` from the command line, `DANSTICK_NO_RESTORE=1` from
/// the environment: how long the daemon lives and whether it starts unseated.
fn lifetime_flags(args: &[String]) -> commands::Lifetime {
    commands::Lifetime {
        fresh: args.iter().any(|arg| arg == "--fresh")
            || std::env::var_os("DANSTICK_NO_RESTORE").is_some_and(|v| !v.is_empty() && v != "0"),
        follow: flag_value(args, &["--follow"])
            .map(|value| parse_number::<u32>(&value, "--follow")),
    }
}

/// SIGTERM must release grabs lest the machine be left with no working controllers.
fn cmd_serve(args: &[String]) -> Result<()> {
    let lifetime = lifetime_flags(args);
    let mut server = danstick_daemon::server::Server::new().context("preparing the daemon")?;
    let slots = danstick_core::slots::Change {
        mode: flag_value(args, &["--slots"]),
        count: flag_value(args, &["--slot-count"])
            .map(|value| parse_number::<i64>(&value, "--slot-count")),
        on_leave: flag_value(args, &["--on-leave"]),
        layout: flag_value(args, &["--layout"]),
    };
    if let Err(why) = server.configure_slots(&slots) {
        anyhow::bail!("{why}");
    }
    server.start().context("starting the daemon")?;
    if let Some(pid) = lifetime.follow {
        server.follow(pid);
    }
    if lifetime.fresh {
        info!("starting unseated: saved seats are not restored");
    } else {
        server.restore();
    }
    let stop = Arc::new(AtomicBool::new(false));
    install_signal_handlers(&stop)?;
    server.run(&stop);
    info!("terminating");
    server.close();
    Ok(())
}

fn cmd_sdl_mapping(guid: Option<String>) -> Result<()> {
    let Some(guid) = guid else {
        eprintln!("usage: danstick-rs sdl-mapping <guid>");
        std::process::exit(2);
    };
    match danstick_input::sdlprobe::builtin_mapping(&guid) {
        Ok(Some(line)) => println!("{line}"),
        Ok(None) => {}
        Err(error) => anyhow::bail!("{error}"),
    }
    Ok(())
}

fn cmd_emit(args: &[String]) -> Result<()> {
    let mut body = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut body)
        .context("reading the pad list from stdin")?;
    let pads: Vec<emulators::Published> =
        serde_json::from_str(&body).context("parsing the pad list")?;

    let destinations = emulators::Destinations {
        cemu_dir: flag_value(args, &["--cemu-dir"]).map(PathBuf::from),
        dolphin_dir: flag_value(args, &["--dolphin-dir"]).map(PathBuf::from),
        ares_settings: flag_value(args, &["--ares-settings"]).map(PathBuf::from),
        ryujinx_config: flag_value(args, &["--ryujinx-config"]).map(PathBuf::from),
        env_file: flag_value(args, &["--env-file"]).map(PathBuf::from),
    };
    let seat =
        flag_value(args, &["--keyboard"]).map(|value| parse_number::<u32>(&value, "--keyboard"));
    let written = emulators::publish(&pads, &destinations, seat);
    for path in &written.paths {
        println!("{}", path.display());
    }
    for (target, why) in &written.skipped {
        info!("{target}: not written ({why})");
    }
    Ok(())
}

/// Whether bwrap is on PATH. Without it there is no sandbox to run in, and a
/// game that starts seeing too many pads beats one that does not start.
fn which_bwrap() -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("bwrap"))
            .find(|candidate| candidate.is_file())
    })
}

/// What `exec --reserve` changed on the daemon, to put back when the game ends.
#[derive(Debug, Default)]
struct Borrowed {
    /// The identity the daemon had before it was asked for the 360's.
    identity: Option<String>,
    /// How many seats were reserved before, restored with `reserve`.
    reserved: Option<u32>,
}

/// Make seats 1..=`seats` exist before the game starts: the seated ones as they
/// are, the rest reserved. Reserving needs the 360 identity, and a daemon that
/// publishes another is switched in place, keeping every seat. Nothing here is
/// fatal -- a game with fewer seats than it wanted still runs.
fn borrow_seats(seats: u32) -> Borrowed {
    let mut borrowed = Borrowed::default();
    let seats = seats.min(danstick_core::retroarch::MAX_PLAYERS);
    if seats == 0 {
        return borrowed;
    }
    let Some(state) = commands::daemon_state(3.0) else {
        warn!("--reserve {seats}: no daemon is running, so no seats are reserved");
        return borrowed;
    };
    // Fixed slots already stand for every launch; there is nothing to borrow.
    if state["slot_mode"] == "fixed" && state["slot_count"].as_u64() >= Some(u64::from(seats)) {
        info!("--reserve {seats}: the daemon's fixed slots already cover it");
        return borrowed;
    }
    let identity = state["identity"].as_str().unwrap_or_default().to_owned();
    let reserved_before = state["reserved"]
        .as_array()
        .and_then(|seats| {
            seats
                .iter()
                .filter_map(|seat| seat["player"].as_u64())
                .max()
        })
        .unwrap_or(0) as u32;
    // Either 360 identity's layout is known before the pad; only the others are switched.
    if !matches!(identity.as_str(), "xbox360" | "xbox360-numbered") {
        let asked = serde_json::json!({"cmd": "identity", "mode": "xbox360"});
        match commands::daemon_ask_until(&asked, |state| state["identity"] == "xbox360", 15.0) {
            Ok(_) => {
                info!("--reserve {seats}: the daemon publishes the 360 identity until this ends");
                borrowed.identity = Some(identity);
            }
            Err(error) => {
                warn!("--reserve {seats}: no 360 identity ({error}); no seats reserved");
                return borrowed;
            }
        }
    }
    let asked = serde_json::json!({"cmd": "reserve", "players": seats});
    match commands::daemon_ask_until(&asked, |state| commands::covers_seats(state, seats), 15.0) {
        Ok(_) => {
            info!("--reserve {seats}: every seat exists before the launch");
            borrowed.reserved = Some(reserved_before);
        }
        Err(error) => warn!("--reserve {seats}: {error}; the game may not see every seat"),
    }
    borrowed
}

/// Give back what `borrow_seats` took. A daemon that has already ended with
/// the session has nothing to give back to, and that is fine.
fn hand_back(borrowed: Borrowed) {
    if let Some(before) = borrowed.reserved {
        let asked = serde_json::json!({"cmd": "reserve", "players": before});
        let _ = commands::daemon_ask_until(
            &asked,
            |state| {
                state["reserved"].as_array().is_none_or(|seats| {
                    seats
                        .iter()
                        .all(|seat| seat["player"].as_u64().unwrap_or(0) <= u64::from(before))
                })
            },
            5.0,
        );
    }
    if let Some(identity) = borrowed.identity {
        let asked = serde_json::json!({"cmd": "identity", "mode": identity});
        let _ = commands::daemon_ask_until(&asked, |state| state["identity"] == identity, 15.0);
    }
}

fn cmd_exec(args: Vec<String>) -> Result<()> {
    let (reserve, args) = match commands::exec_args(args) {
        Ok(parsed) => parsed,
        Err(why) => {
            eprintln!("{why}\nusage: danstick-rs exec [--reserve N] -- <program> [args...]");
            std::process::exit(2);
        }
    };
    let Some((program, rest)) = args.split_first() else {
        std::process::exit(2);
    };
    // Before anything reads the files or /dev/input: the seats have to exist
    // when the bind plan is built, and their mappings have to be in `env.sh`.
    let borrowed = reserve.map(borrow_seats).unwrap_or_default();

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

    // danstick's pads and nothing else. Every consumer is meant to read the
    // clones -- the mapping, the player order, the motion and the remapping
    // all live there -- and nothing stopped a game from opening the physical
    // pad as well and binding whichever SDL saw first. See `isolate`.
    //
    // DANSTICK_NO_ISOLATE=1 turns it off, for somebody who has to reach a
    // controller danstick has not republished.
    let mut argv: Vec<String> = std::iter::once(program.clone())
        .chain(rest.iter().cloned())
        .collect();
    if std::env::var("DANSTICK_NO_ISOLATE").unwrap_or_default() != "1" {
        let raw: Vec<std::path::PathBuf> = pad::discover(pad::Filter::default())
            .map(|pads| pads.into_iter().map(|pad| pad.path).collect())
            .unwrap_or_default();
        let plan = isolate::plan(&isolate::event_nodes(), &raw, &isolate::hidraw_nodes());
        if !plan.worth_it() {
            // No clone published: hiding this game's controllers would leave
            // it with none at all, which is worse than the problem.
            info!("no virtual pads published; {program} will see the controllers as they are");
        } else if which_bwrap().is_none() {
            warn!("bwrap is not here; {program} will see the physical pads as well as danstick's");
        } else {
            info!(
                "{program} sees {} input node(s): danstick's pads, the keyboard and the mouse",
                plan.keep.len()
            );
            argv = isolate::bwrap_argv(&plan, &argv, "bwrap");
        }
    }

    let mut command = std::process::Command::new(&argv[0]);
    command.args(&argv[1..]);
    if !value.is_empty() {
        command.env(emulators::CONFIG_ENV, value);
    }
    let status = command.status();
    hand_back(borrowed);
    let status = status.with_context(|| format!("running {program}"))?;
    std::process::exit(status.code().unwrap_or(1));
}

fn cmd_list() -> Result<()> {
    let found = pad::discover_all(pad::Filter::default()).context("enumerating input devices")?;
    let pads = found.pads;
    if pads.is_empty() {
        println!("No joypads found.");
        report_dormant();
        return Ok(());
    }

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
        println!("rules (ID_INPUT_JOYSTICK cleared). danstick can still republish");
        println!("them; RetroArch sees only the virtual pads.");
    }
    report_dropped(&found.dropped);

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

/// Say what discovery left out, so a pad that vanished did not vanish silently.
fn report_dropped(dropped: &[pad::Dropped]) {
    if dropped.is_empty() {
        return;
    }
    println!("\nLeft out, on purpose:");
    for entry in dropped {
        println!(
            "  {} ({:04x}:{:04x})",
            entry.pad.name, entry.pad.vid, entry.pad.pid
        );
        println!("        {}   {}", entry.pad.path.display(), entry.reason);
    }
}

/// Report dormant controllers the kernel isn't driving as joypads.
fn report_dormant() {
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
    let root = rustix::process::geteuid().is_root();
    if args.iter().any(|arg| arg == "--print") || !root {
        if args.iter().any(|arg| arg == "--install") && !root {
            eprintln!("Installing needs root. Re-run:  sudo danstick hide\n");
        }
        print!("{rules}");
        eprintln!();
        eprintln!("Write that to {} and reload:", commands::RUNTIME_RULES_PATH);
        eprintln!("  sudo udevadm control --reload-rules");
        eprintln!("  sudo udevadm trigger --subsystem-match=input");
        eprintln!();
        eprintln!("While it is installed and danstick is NOT running, these");
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
            "\nCaution: while these rules are active and danstick is not running,\n\
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
        println!("No assignments. Run `danstick setup` first.");
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
        let axes: BTreeMap<u16, danstick_core::calibration::AxisCalibration> =
            profiles::load(pad, None)
                .map(|stored| stored.axes)
                .unwrap_or_default();
        if !axes.is_empty() {
            info!(
                "player {player}: {} calibrated axis/axes from the profile store",
                axes.len()
            );
        }
        let tuning = danstick_daemon::publish::tuning_for(pad);
        let mapping = danstick_daemon::publish::resolved(pad, "", "").1;
        match clone::create(pad, player, mode, &axes, tuning, true, &mapping) {
            Ok(vpad) => vpads.push(vpad),
            Err(error) => warn!("player {player}: {error}"),
        }
    }
    if vpads.is_empty() {
        anyhow::bail!("no pad could be republished");
    }

    if let Err(error) = publish_artefacts(&vpads, assignments::keyboard_seat(&saved)) {
        warn!("could not write the mapping files: {error}");
    }

    let mut republisher = republish::Republisher::new(vpads);
    let mut reactor = reactor::Reactor::new(TICK).context("creating the event loop")?;
    use std::os::fd::AsFd;
    for (index, vpad) in republisher.pads.iter().enumerate() {
        reactor
            .watch(vpad.source.as_fd(), reactor::Watched::Source(index))
            .context("watching a pad")?;
        reactor
            .watch(vpad.clone.as_fd(), reactor::Watched::Clone(index))
            .context("watching a clone")?;
        if let Some(sensor) = vpad.sensor.as_ref() {
            reactor
                .watch(sensor.as_fd(), reactor::Watched::Motion(index))
                .context("watching a motion sensor")?;
        }
    }

    let mut motion = match danstick_daemon::dsu::Motion::bind(danstick_core::dsu::PORT) {
        Ok(motion) => {
            reactor
                .watch(motion.as_fd(), reactor::Watched::Dsu)
                .context("watching the motion socket")?;
            Some(motion)
        }
        Err(error) => {
            warn!("no motion server ({error}); the pads still work, their gyros do not");
            None
        }
    };

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
                    } else {
                        serve_motion(motion.as_mut(), &republisher, false);
                    }
                }
                reactor::Watched::Motion(index) => {
                    if republisher.read_motion(index).gone {
                        if let Some(sensor) = republisher
                            .pads
                            .get(index)
                            .and_then(|vpad| vpad.sensor.as_ref())
                        {
                            let _ = reactor.unwatch(sensor.as_fd());
                        }
                        republisher.drop_sensor(index);
                    } else {
                        serve_motion(motion.as_mut(), &republisher, false);
                    }
                }
                reactor::Watched::Dsu => serve_motion(motion.as_mut(), &republisher, true),
                reactor::Watched::Clone(index) => republisher.feedback(index),
                reactor::Watched::Tick => {
                    let expiries = reactor.take_tick();
                    if expiries > 1 {
                        late_ticks += expiries - 1;
                    }
                    republisher.flush_debounce();
                    serve_motion(motion.as_mut(), &republisher, false);
                }
                reactor::Watched::Listener
                | reactor::Watched::Client(_)
                | reactor::Watched::Session(_)
                | reactor::Watched::Solo
                | reactor::Watched::Seating(_) => {}
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

fn publish_artefacts(vpads: &[danstick_input::VirtualPad], keyboard: Option<u32>) -> Result<()> {
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
        let (_scope, mut mapping) = stored
            .as_ref()
            .map(|profile| profile.resolve("", ""))
            .unwrap_or_default();
        let mut bindings = mapping.resolved();
        let xbox = vpad.translator.is_some();
        if xbox {
            // The clone is a 360 pad whatever is behind it; describe that.
            bindings = danstick_core::xbox::bindings();
            mapping.layout = danstick_core::layout::default_id().to_owned();
        }

        // Unmapped pads must still be usable; d-pad and sticks come from capabilities.
        let line = if bindings.is_empty() {
            let (keys, axis_codes) = vpad.source.capabilities();
            let spans = vpad.source.axis_spans();
            let guessed = danstick_core::guess::guessed_fields(&keys, &axis_codes, Some(&spans));
            emit::sdl_line(
                &emit::virtual_guid(vpad.player, identity),
                &emit::virtual_name(vpad.player),
                &guessed,
            )
        } else {
            emit::sdl_line_for(vpad.player, identity, &bindings, None)
        };
        let (keys, axes) = if xbox {
            (
                danstick_core::xbox::KEYS.to_vec(),
                danstick_core::xbox::axis_codes(),
            )
        } else {
            vpad.source.capabilities()
        };
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

    let fallback = Identity {
        bustype: 0x06,
        vendor: clone::DANSTICK_VID,
        product: clone::DANSTICK_PID,
        version: clone::DANSTICK_VERSION,
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

    let emulators = emulators::publish(&published, &emulators::Destinations::default(), keyboard);
    for path in &emulators.paths {
        info!("emulator config: {}", path.display());
    }
    for (target, why) in &emulators.skipped {
        info!("{target}: not written ({why})");
    }
    Ok(())
}

fn serve_motion(
    motion: Option<&mut danstick_daemon::dsu::Motion>,
    republisher: &republish::Republisher,
    asked: bool,
) {
    let Some(motion) = motion else {
        return;
    };
    let mut ports = Vec::new();
    let mut pads = Vec::new();
    for vpad in republisher.pads.iter().filter(|vpad| !vpad.gone) {
        let has_motion = vpad.sensor.is_some() || vpad.source.motion().is_some();
        ports.push(danstick_daemon::dsu::port_for(vpad.player, has_motion));
        pads.push(*vpad.tracker.pad());
    }
    if asked {
        motion.serve(&ports);
    }
    if motion.has_clients() {
        motion.publish(&ports, &pads);
    }
}

fn drop_pad(reactor: &reactor::Reactor, republisher: &republish::Republisher, index: usize) {
    use std::os::fd::AsFd;
    let Some(vpad) = republisher.pads.get(index) else {
        return;
    };
    let _ = reactor.unwatch(vpad.source.as_fd());
}

fn install_signal_handlers(stop: &Arc<AtomicBool>) -> Result<()> {
    // SIGTERM must release grabs lest pads be held after the process exits.
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(stop))
            .with_context(|| format!("installing a handler for signal {signal}"))?;
    }
    Ok(())
}

const TUNE_USAGE: &str = "\
usage: danstick tune [--pad NAME] [--deadzone F | --deadzone CODE=F]... [--debounce MS]
                   [--ignore-axis CODE]... [--ignore-button CODE]... [--reset] [--show]

For a controller that misbehaves. Settings live with the physical controller
and are applied to everything danstick publishes for it, after calibration.

  --deadzone F         a band of F (0 to 1) of each stick's and trigger's
                       travel that reads as untouched; the rest is stretched
                       so full deflection still reaches the end
  --deadzone CODE=F    the same, for one ABS code (0 = left X, 1 = left Y,
                       2 = left trigger, 3 = right X, 4 = right Y,
                       5 = right trigger)
  --debounce MS        hold every release back MS milliseconds and swallow a
                       press that arrives inside that, for a switch that
                       bounces. Up to 500.
  --ignore-axis CODE   drop that axis entirely
  --ignore-button CODE drop that key code entirely
  --reset              start from nothing before applying the rest
  --show               print what is set and change nothing
";

fn tune_request(args: &[String]) -> Result<danstick_core::tuning::Request> {
    use danstick_core::tuning::Request;
    let mut request = Request {
        reset: args.iter().any(|arg| arg == "--reset"),
        ..Request::default()
    };
    let mut ignore_axes = std::collections::BTreeSet::new();
    let mut ignore_buttons = std::collections::BTreeSet::new();
    let mut saw_ignore_axes = false;
    let mut saw_ignore_buttons = false;
    let mut at = 0;
    while at < args.len() {
        let flag = args[at].as_str();
        let value = || -> Result<&String> {
            args.get(at + 1)
                .filter(|v| !v.starts_with("--"))
                .with_context(|| format!("{flag} needs a value"))
        };
        match flag {
            "--deadzone" => {
                let text = value()?;
                if let Some((code, fraction)) = text.split_once('=') {
                    let code: u16 = code
                        .trim()
                        .parse()
                        .with_context(|| format!("{code:?} is not an ABS code"))?;
                    let fraction: f32 = fraction
                        .trim()
                        .parse()
                        .with_context(|| format!("{fraction:?} is not a number"))?;
                    anyhow::ensure!((0.0..=1.0).contains(&fraction), "a deadzone is from 0 to 1");
                    request.deadzone.insert(code, fraction);
                } else {
                    let fraction: f32 = text
                        .parse()
                        .with_context(|| format!("{text:?} is not a number"))?;
                    anyhow::ensure!((0.0..=1.0).contains(&fraction), "a deadzone is from 0 to 1");
                    request.deadzone_all = Some(fraction);
                }
                at += 2;
            }
            "--debounce" => {
                let text = value()?;
                let ms: u32 = text
                    .trim_end_matches("ms")
                    .parse()
                    .with_context(|| format!("{text:?} is not a number of milliseconds"))?;
                anyhow::ensure!(
                    ms <= danstick_core::tuning::MAX_DEBOUNCE_MS,
                    "a debounce is at most {} ms",
                    danstick_core::tuning::MAX_DEBOUNCE_MS
                );
                request.debounce_ms = Some(ms);
                at += 2;
            }
            "--ignore-axis" => {
                let text = value()?;
                ignore_axes.insert(
                    text.parse::<u16>()
                        .with_context(|| format!("{text:?} is not an ABS code"))?,
                );
                saw_ignore_axes = true;
                at += 2;
            }
            "--ignore-button" => {
                let text = value()?;
                ignore_buttons.insert(
                    text.parse::<u16>()
                        .with_context(|| format!("{text:?} is not a key code"))?,
                );
                saw_ignore_buttons = true;
                at += 2;
            }
            "--pad" => at += 2,
            "--reset" | "--show" => at += 1,
            other => anyhow::bail!("unknown flag {other:?}\n{TUNE_USAGE}"),
        }
    }
    if saw_ignore_axes || request.reset {
        request.ignore_axes = Some(ignore_axes);
    }
    if saw_ignore_buttons || request.reset {
        request.ignore_buttons = Some(ignore_buttons);
    }
    Ok(request)
}
