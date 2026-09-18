//! padmap's republisher, in Rust.

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
    artefacts, assignments, clone, emulators, isolate, lizard, pad, profiles, reactor, republish,
    runtime, triton,
};

const TICK: Duration = Duration::from_millis(20);

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

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
        "usage: padmap list [--json] | setup | map | calibrate | tune | forget | run | serve | \
         launch | play | hide | ensure-daemon | clean-config | \
         emit [--cemu-dir D] [--dolphin-dir D] [--ares-settings F] \
         [--ryujinx-config F] \
         [--env-file F] | \
         exec -- <program> [args...] | sdl-mapping <guid>"
    );
}

/// SIGTERM must release grabs lest the machine be left with no working controllers.
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
    let written = emulators::publish(&pads, &destinations);
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

fn cmd_exec(args: Vec<String>) -> Result<()> {
    let args: Vec<String> = args.into_iter().skip_while(|arg| arg == "--").collect();
    let Some((program, rest)) = args.split_first() else {
        eprintln!("usage: padmap-rs exec -- <program> [args...]");
        std::process::exit(2);
    };

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

    // padmap's pads and nothing else. Every consumer is meant to read the
    // clones -- the mapping, the player order, the motion and the remapping
    // all live there -- and nothing stopped a game from opening the physical
    // pad as well and binding whichever SDL saw first. See `isolate`.
    //
    // PADMAP_NO_ISOLATE=1 turns it off, for somebody who has to reach a
    // controller padmap has not republished.
    let mut argv: Vec<String> = std::iter::once(program.clone())
        .chain(rest.iter().cloned())
        .collect();
    if std::env::var("PADMAP_NO_ISOLATE").unwrap_or_default() != "1" {
        let raw: Vec<std::path::PathBuf> = pad::discover(pad::Filter::default())
            .map(|pads| pads.into_iter().map(|pad| pad.path).collect())
            .unwrap_or_default();
        let plan = isolate::plan(&isolate::event_nodes(), &raw, &isolate::hidraw_nodes());
        if !plan.worth_it() {
            // No clone published: hiding this game's controllers would leave
            // it with none at all, which is worse than the problem.
            info!("no virtual pads published; {program} will see the controllers as they are");
        } else if which_bwrap().is_none() {
            warn!("bwrap is not here; {program} will see the physical pads as well as padmap's");
        } else {
            info!(
                "{program} sees {} input node(s): padmap's pads, the keyboard and the mouse",
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
    let status = command
        .status()
        .with_context(|| format!("running {program}"))?;
    std::process::exit(status.code().unwrap_or(1));
}

fn cmd_list() -> Result<()> {
    let pads = pad::discover(pad::Filter::default()).context("enumerating input devices")?;
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
        let tuning = padmap_daemon::publish::tuning_for(pad);
        match clone::create(pad, player, mode, &axes, tuning, true) {
            Ok(vpad) => vpads.push(vpad),
            Err(error) => warn!("player {player}: {error}"),
        }
    }
    if vpads.is_empty() {
        anyhow::bail!("no pad could be republished");
    }

    if let Err(error) = publish_artefacts(&vpads) {
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

    let mut motion = match padmap_daemon::dsu::Motion::bind(padmap_core::dsu::PORT) {
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

        // Unmapped pads must still be usable; d-pad and sticks come from capabilities.
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

    let emulators = emulators::publish(&published, &emulators::Destinations::default());
    for path in &emulators.paths {
        info!("emulator config: {}", path.display());
    }
    for (target, why) in &emulators.skipped {
        info!("{target}: not written ({why})");
    }
    Ok(())
}

fn serve_motion(
    motion: Option<&mut padmap_daemon::dsu::Motion>,
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
        ports.push(padmap_daemon::dsu::port_for(vpad.player, has_motion));
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
usage: padmap tune [--pad NAME] [--deadzone F | --deadzone CODE=F]... [--debounce MS]
                   [--ignore-axis CODE]... [--ignore-button CODE]... [--reset] [--show]

For a controller that misbehaves. Settings live with the physical controller
and are applied to everything padmap publishes for it, after calibration.

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

fn tune_request(args: &[String]) -> Result<padmap_core::tuning::Request> {
    use padmap_core::tuning::Request;
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
                    ms <= padmap_core::tuning::MAX_DEBOUNCE_MS,
                    "a debounce is at most {} ms",
                    padmap_core::tuning::MAX_DEBOUNCE_MS
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
