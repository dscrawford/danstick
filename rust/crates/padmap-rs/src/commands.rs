//! The offline commands: everything that is not the daemon.
//!
//! Each of these runs, does one thing and exits. They share the daemon's
//! libraries and none of its lifetime, so the rule that governs the daemon --
//! never die, never drop a pad -- does not apply; here, failing loudly with a
//! message a user can act on is the right answer.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use padmap_core::capture::{self, MappingRun};
use padmap_core::emit;
use padmap_core::launch;
use padmap_daemon::publish;
use padmap_input::clone::{self, IdentityMode};
use padmap_input::pad::{self, Pad};
use padmap_input::{artefacts, assignments, profiles, runtime};

/// Discover the pads a command should act on.
pub fn discover() -> Result<Vec<Pad>> {
    pad::discover(pad::Filter::default()).context("enumerating input devices")
}

/// What is plugged in, as JSON.
///
/// For a launcher: it runs once, needs one answer, has `jq`, and should not
/// have to start a daemon to ask. The socket is the right answer for a
/// front-end that is already connected; this is for the script that runs
/// before the game does.
///
/// The vocabulary is deliberately the `controller` event's -- same keys, same
/// meanings -- so a caller that reads both learns one shape rather than two.
///
/// Entries come out in the order a consumer enumerates pads, with assigned
/// players first, and an unassigned controller has `"player": null`.
pub fn cmd_list_json() -> Result<()> {
    use serde_json::json;

    let pads = pad::discover(pad::Filter::default())?;
    // Clones too, so a player's node can be reported: they are what a game
    // actually opens, and a launcher binds those rather than the hardware.
    let clones = pad::clone_nodes();
    let saved = assignments::load(&runtime::assignments_path()).unwrap_or_default();
    let order = publish::visible_order();
    let mode = IdentityMode::from_env();

    let mut entries = Vec::new();
    for pad in &pads {
        // Whether padmap can bind this pad correctly with no capture at all,
        // so a picker can say "this already works; remap only if you want to"
        // rather than sending everybody through a wizard they do not need.
        let facts = publish::pad_facts(pad);
        let autobound = padmap_core::standard::is_standard(&facts.keys);
        let player = saved
            .iter()
            .find(|entry| entry.path == pad.path)
            .map(|entry| entry.player);
        let clone = player.and_then(|player| clones.get(&emit::virtual_name(player)));
        let virtual_pad = player.map(|player| {
            let identity = publish::identity_of(pad, player, mode);
            let node = clone.map(|path| path.display().to_string());
            json!({
                "name": emit::virtual_name(player),
                "node": node,
                "guid": emit::virtual_guid(player, identity),
                "vid": format!("{:04x}", identity.vendor),
                "pid": format!("{:04x}", identity.product),
                "bustype": identity.bustype,
                // Where a consumer enumerating pads will find it, or null:
                // a clone that is not running yet has no index, and guessing
                // one would point a port at somebody else's controller.
                "index": node
                    .as_ref()
                    .and_then(|node| {
                        order.iter().find(|(_, path)| *path == node).map(|(index, _)| *index)
                    }),
            })
        });
        entries.push(json!({
            "player": player,
            "controller": {
                "name": pad.name,
                "path": pad.path.display().to_string(),
                "vid": format!("{:04x}", pad.vid),
                "pid": format!("{:04x}", pad.pid),
                "phys": pad.phys,
                "uniq": pad.uniq,
                "signature": profiles::signature_of(pad),
                "configured": publish::has_mapping(pad),
                "autobound": autobound,
                // False once `padmap hide` has cleared ID_INPUT_JOYSTICK.
                "retroarch_visible": pad.retroarch_visible,
                // The controller's own motion sensor, and **not** something
                // padmap republishes: a clone carries the axes its source's
                // joypad node declares, and a gyro is never among them. Open
                // this to read motion; the clone will never carry it.
                "motion": pad.motion.is_some(),
                "motion_node": pad.motion.as_ref().map(|path| path.display().to_string()),
                // What the user set for a misbehaving controller. `{}` is
                // nothing; see `padmap tune`.
                "tuning": serde_json::to_value(publish::tuning_for(pad)).unwrap_or(serde_json::json!({})),
            },
            "virtual": virtual_pad,
        }));
    }
    // Assigned players first and in seat order, so `.[0]` is player 1.
    entries.sort_by_key(|entry| {
        entry["player"]
            .as_u64()
            .map(|player| (0, player))
            .unwrap_or((1, 0))
    });
    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}

/// Resolve each controller's mapping for the game that is about to start.
///
/// The launcher's half: `padmap-play` hands over RetroArch's own command line,
/// and this rewrites the autoconfig directory the launch override already
/// points at with whichever of each controller's mappings applies -- this
/// game, else this console, else the default.
///
/// Never fails a launch. The profiles the daemon wrote are already in place
/// and are a working, if less specific, answer; a game that refuses to start
/// because a mapping could not be narrowed is a far worse outcome than one
/// played on the default mapping.
pub fn cmd_play(argv: Vec<String>) -> Result<()> {
    // `--` is how the wrapper separates its own arguments from RetroArch's.
    let argv: Vec<String> = argv.into_iter().skip_while(|arg| arg == "--").collect();
    let (core, rom) = launch::split_args(&argv, |path| Path::new(path).exists());
    let console = padmap_core::layout::for_core(&core);
    let key = if rom.is_empty() {
        String::new()
    } else {
        padmap_core::scope::game_key(console, &rom)
    };
    let title = if rom.is_empty() {
        String::new()
    } else {
        launch::title_for(&rom)
    };

    // Recorded even when nothing about this launch is recognised: the scope
    // picker offers "...for the game you just played", and a launch with an
    // unknown core is exactly the one whose controls felt wrong.
    if !key.is_empty() {
        let game = runtime::Game {
            console: console.to_owned(),
            key: key.clone(),
            title: title.clone(),
        };
        if let Err(error) = runtime::write_last_game(&game) {
            eprintln!("padmap: could not record the launch ({error})");
        }
    }

    let saved = assignments::load(&runtime::assignments_path()).unwrap_or_default();
    let pads = discover().unwrap_or_default();
    let (found, _missing) = assignments::resolve(&saved, &pads);
    if found.is_empty() {
        // Not an error: running a game with no padmap assignment is what
        // happens before anyone has been through the setup screen.
        eprintln!("padmap: no assigned controllers; leaving autoconfig alone");
        return Ok(());
    }

    let context = if title.is_empty() {
        Path::new(&rom)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("an unidentified game")
            .to_owned()
    } else {
        title
    };
    let mode = IdentityMode::from_env();
    let mut profiles_out = BTreeMap::new();
    for (player, pad) in &found {
        let identity = publish::identity_of(pad, *player, mode);
        profiles_out.insert(
            *player,
            publish::profile_text(pad, *player, identity, console, &key, &context),
        );
        let (scope, _) = publish::resolved(pad, console, &key);
        eprintln!(
            "padmap: player {player} using the {} mapping{}",
            if scope.is_empty() { "default" } else { &scope },
            if console.is_empty() {
                " (unknown console)".to_owned()
            } else {
                format!(" (console {console})")
            }
        );
    }
    match artefacts::write_autoconfig(&profiles_out, None) {
        Ok(written) => eprintln!("padmap: wrote {} autoconfig profile(s)", written.len()),
        Err(error) => {
            eprintln!("padmap: could not write the profiles ({error}); using the default")
        }
    }
    Ok(())
}

/// Assign player order by pressing and holding a button.
pub fn cmd_setup(wanted: Option<u32>) -> Result<()> {
    use padmap_core::assign::HOLD_SECONDS;
    use padmap_daemon::session::Session;

    let pads = discover()?;
    if pads.is_empty() {
        println!("No joypads found.");
        std::process::exit(1);
    }
    println!("{} pad(s) detected.", pads.len());
    println!("Hold a button on each controller, in the order you want them");
    println!("assigned -- hold for about {HOLD_SECONDS:.2}s, a tap will not register.");
    match wanted {
        Some(count) => println!("Waiting for {count} controller(s); Ctrl-C to stop early.\n"),
        None => println!("Press Ctrl-C when you are done.\n"),
    }

    let mut session = Session::open(pads).context("opening the controllers")?;
    let target = wanted.unwrap_or(u32::MAX);
    let started = std::time::Instant::now();
    let mut announced = 0usize;
    while (session.claims().len() as u32) < target {
        let now = started.elapsed().as_secs_f64();
        for index in 0..session.len() {
            for event in session.read(index) {
                session
                    .assigner
                    .feed(index, event.kind, event.code, event.value, now);
            }
        }
        let ticked = session.assigner.tick(started.elapsed().as_secs_f64());
        for claim in &ticked.claimed {
            if let Some(pad) = session.pads.get(claim.pad) {
                println!(
                    "  Player {}: {} [{}]",
                    claim.player,
                    padmap_daemon::clean(&pad.name),
                    pad.event()
                );
            }
        }
        announced += ticked.claimed.len();
        let _ = announced;
        std::thread::sleep(std::time::Duration::from_millis(20));
        if wanted.is_none() && session.claims().len() == session.len() {
            break;
        }
    }

    let claimed: Vec<assignments::Assignment> = session
        .claimed_pads()
        .into_iter()
        .map(|(player, pad)| assignments::Assignment {
            player,
            path: pad.path.clone(),
            name: pad.name.clone(),
            phys: pad.phys.clone(),
            vid: pad.vid,
            pid: pad.pid,
        })
        .collect();
    drop(session);
    if claimed.is_empty() {
        println!("Nothing assigned.");
        std::process::exit(1);
    }
    let path = runtime::assignments_path();
    assignments::save(&path, &claimed).context("saving the controller order")?;
    println!(
        "\nSaved {} assignment(s) to {}",
        claimed.len(),
        path.display()
    );
    println!("Start the daemon with: padmap run");
    Ok(())
}

/// Delete stored profiles so controllers are treated as new again.
pub fn cmd_forget(all: bool) -> Result<()> {
    let directory = profiles::dir();
    let connected: BTreeSet<String> = discover()
        .unwrap_or_default()
        .iter()
        .map(profiles::signature_of)
        .collect();

    let stored: Vec<PathBuf> = std::fs::read_dir(&directory)
        .map(|entries| {
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect();
            found.sort();
            found
        })
        .unwrap_or_default();

    let mut targets = Vec::new();
    for path in &stored {
        if all {
            targets.push(path.clone());
            continue;
        }
        // isinstance-style care: a profile that is not an object, or is not
        // JSON at all, must not stop the command that exists to clear up a
        // broken profile store.
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(serde_json::Value::Object(raw)) = serde_json::from_str::<serde_json::Value>(&text)
        else {
            continue;
        };
        if raw
            .get("signature")
            .and_then(|value| value.as_str())
            .is_some_and(|signature| connected.contains(signature))
        {
            targets.push(path.clone());
        }
    }

    let mut removed = 0;
    for path in &targets {
        match std::fs::remove_file(path) {
            Ok(()) => {
                removed += 1;
                println!(
                    "  removed {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
            // A directory named *.json, or one that is not ours to remove:
            // --all unlinks everything it globbed, so without this the
            // "remove everything and start again" command is the only one
            // that dies on the mess it is meant to clear up.
            Err(error) => println!(
                "  could not remove {}: {error}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
        }
    }

    // Clear the 'already asked' record for every controller in scope, not
    // only the ones that had a profile. A controller stuck in the second
    // memory with nothing in the first -- never configured, so it reports as
    // new, but already asked about, so the daemon stays silent -- is exactly
    // what someone runs this to fix.
    let prompted_path = runtime::prompted_path();
    let prompted = runtime::read_prompted(&prompted_path);
    let kept: BTreeSet<String> = if all {
        BTreeSet::new()
    } else {
        prompted
            .iter()
            .filter(|signature| !connected.contains(*signature))
            .cloned()
            .collect()
    };
    let cleared = prompted.len() - kept.len();
    if cleared > 0 {
        let _ = if kept.is_empty() {
            std::fs::remove_file(&prompted_path).map_err(|_| ())
        } else {
            runtime::write_prompted(&prompted_path, &kept).map_err(|_| ())
        };
    }

    if targets.is_empty() && cleared == 0 {
        println!(
            "Nothing to forget for the connected controllers (profiles in {}).",
            directory.display()
        );
        if !stored.is_empty() && !all {
            println!("Use --all to remove every stored profile.");
        }
        return Ok(());
    }
    if !targets.is_empty() {
        // What was actually removed, not what was attempted: reporting five
        // when one is still on disk sends someone away believing a controller
        // was reset when it will come back configured.
        println!("\nForgot {removed} profile(s). They will be set up again");
        println!("on the next controller assignment.");
    }
    if cleared > 0 {
        println!("Cleared {cleared} 'already asked' record(s), so setup is");
        println!("offered again without waiting for a reboot.");
    }
    // The SDL database is the other half of "reset this controller", and
    // padmap has no business deleting it silently -- but say where it is.
    let database = artefacts::sdl_database_path();
    if database.is_file() {
        println!("\nThe SDL mappings padmap wrote are separate and still present:");
        println!("  {}", database.display());
        println!("Delete that file to reset those too.");
    }
    Ok(())
}

/// Strip padmap values that `config_save_on_exit` persisted into retroarch.cfg.
pub fn cmd_clean_config(config: Option<String>, dry_run: bool) -> Result<()> {
    let target = config
        .map(PathBuf::from)
        .unwrap_or_else(|| artefacts::retroarch_config_dir().join("retroarch.cfg"));
    if !target.is_file() {
        println!("No RetroArch config at {}", target.display());
        std::process::exit(1);
    }
    // Bytes in, bytes out: the only lines this may alter are the ones it
    // reports, and the backup has to be the file that was there rather than
    // padmap's reading of it.
    let raw = std::fs::read(&target).with_context(|| format!("reading {}", target.display()))?;
    let (changes, body) = padmap_core::userconfig::clean_bytes(&raw);
    if changes.is_empty() {
        println!("{} has no padmap leftovers.", target.display());
        return Ok(());
    }
    println!(
        "{} {} setting(s) in {}:\n",
        if dry_run { "Would change" } else { "Changed" },
        changes.len(),
        target.display()
    );
    for change in &changes {
        println!("  {change}");
    }
    if dry_run {
        println!("\nDry run; nothing written. Re-run without --dry-run to apply.");
        return Ok(());
    }
    // RetroArch's config is thousands of lines the user did not write by
    // hand, but it is still theirs and this rewrite is not reconstructible
    // from padmap state. Back it up before touching it.
    let backup = target.with_extension(format!(
        "{}.padmap-backup",
        target.extension().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&backup, &raw).with_context(|| format!("writing {}", backup.display()))?;
    std::fs::write(&target, body).with_context(|| format!("writing {}", target.display()))?;
    println!("\nOriginal saved to {}", backup.display());
    Ok(())
}

/// Record which button is which, from a terminal.
///
/// The same capture state machine the daemon drives for a front-end. It
/// exists because a front-end was once the only thing that could reach it,
/// and padmap is a virtual gamepad before it is any one front-end's
/// component -- a controller you cannot map is a virtual gamepad that emits
/// the wrong buttons.
pub fn cmd_map(layout_id: Option<String>, which: Option<String>, scope: String) -> Result<()> {
    use padmap_daemon::session::Session;

    let pads = discover()?;
    if pads.is_empty() {
        println!("No joypads found.");
        std::process::exit(1);
    }
    let pad = match &which {
        Some(name) => match pads
            .iter()
            .find(|pad| pad.name.contains(name.as_str()) || pad.event() == name)
        {
            Some(pad) => pad.clone(),
            None => {
                println!("No pad matches {name:?}. Connected:");
                for pad in &pads {
                    println!("  {}  {}", pad.event(), pad.name);
                }
                std::process::exit(1);
            }
        },
        None if pads.len() == 1 => pads[0].clone(),
        None => {
            println!("Several pads are connected; name one with --pad:");
            for pad in &pads {
                println!("  {}  {}", pad.event(), pad.name);
            }
            std::process::exit(1);
        }
    };

    // A stored layout is the better default than the generic pad: it is what
    // this controller was mapped as last time, and re-running the wizard is
    // usually correcting a control rather than changing the console.
    let stored = publish::stored_layout(&pad);
    let chosen = layout_id.unwrap_or(stored);
    if chosen.is_empty() {
        println!("Which controller is this? Pass one with --layout:");
        for layout in padmap_core::layout::all() {
            println!("  {:<10} {}", layout.id, layout.label);
        }
        std::process::exit(1);
    }
    if !padmap_core::layout::exists(&chosen) {
        let known: Vec<&str> = padmap_core::layout::all()
            .iter()
            .map(|layout| layout.id.as_str())
            .collect();
        println!("Unknown layout {chosen:?}. Known: {}", known.join(", "));
        std::process::exit(1);
    }
    let layout = padmap_core::layout::get(&chosen);

    let mut session = Session::open(vec![pad.clone()]).context("opening the controller")?;
    if !session.grab_failures.is_empty() {
        println!(
            "warning: could not grab {} exclusively; presses will also reach \
             whatever else is listening",
            pad.event()
        );
    }
    let (mut keys, _) = session
        .source_mut(0)
        .map(|source| source.capabilities())
        .unwrap_or_default();
    keys.sort_unstable();
    let axes = session
        .source_mut(0)
        .map(|source| source.axis_spans())
        .unwrap_or_default();
    let held: BTreeSet<u16> = session
        .source_mut(0)
        .map(|source| source.held_keys().into_iter().collect())
        .unwrap_or_default();

    let name = padmap_daemon::clean(&pad.name);
    println!(
        "\nMapping {name} [{}] as {}{}.",
        pad.event(),
        layout.label,
        if scope.is_empty() {
            String::new()
        } else {
            format!(" for {scope}")
        }
    );
    println!(
        "Press each control as it is named. Hold any button for {:.1}s to skip \
         one this pad does not have.",
        capture::SKIP_HOLD_SECONDS
    );
    println!("Ctrl-C to abandon without saving.\n");

    let mut run = MappingRun::new(1, layout, keys, scope.clone(), axes, held);
    let started = std::time::Instant::now();
    let mut shown = usize::MAX;
    while !run.finished() {
        if run.index() != shown {
            shown = run.index();
            if let Some(control) = run.current() {
                let label = layout
                    .controls
                    .iter()
                    .find(|entry| entry.canonical == control)
                    .map(|entry| entry.label.as_str())
                    .unwrap_or("");
                print!("  [{}/{}] {label} ... ", run.index() + 1, run.total());
                let _ = std::io::stdout().flush();
            }
        }
        let now = started.elapsed().as_secs_f64();
        for event in session.read(0) {
            if event.kind != capture::EV_KEY && event.kind != capture::EV_ABS {
                continue;
            }
            let outcome = run.feed(
                capture::Event {
                    kind: event.kind,
                    code: event.code,
                    value: event.value,
                },
                now,
            );
            match outcome {
                capture::Outcome::Recorded { binding, .. } => {
                    println!("{}", binding.sdl().unwrap_or_else(|_| "?".to_owned()));
                }
                capture::Outcome::Skipped { .. } => println!("skipped"),
                capture::Outcome::Refused { held_by, .. } => {
                    println!(
                        "\n      (already bound to {held_by:?}; restart to give that \
                         control a different input)"
                    );
                }
                _ => {}
            }
        }
        // The skip hold ends on the clock, not on an event.
        let now = started.elapsed().as_secs_f64();
        let _ = run.feed(capture::Event::key(0, 2), now);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(session);

    let missing = publish::store_mapping(&pad, &layout.id, run.bindings(), &scope);
    println!(
        "\nSaved {} control(s) for {name}{}.",
        run.bindings().len(),
        if scope.is_empty() {
            String::new()
        } else {
            format!(" under {scope}")
        }
    );
    if !missing.is_empty() {
        println!("  {} not mapped: {}", missing.len(), missing.join(", "));
        println!("  Those emit no binding at all, so they do nothing in game.");
    }
    println!("\nRun `padmap run` or `padmap launch` to apply it.");
    Ok(())
}

/// Walk each controller through centre and range measurement.
///
/// Two phases, because measuring only the resting position is not enough: an
/// adapter can declare a 0-255 axis while the stick physically reaches far
/// less, and scaling against the declared range then leaves one direction
/// with no usable travel.
pub fn cmd_calibrate(force: bool) -> Result<()> {
    use padmap_daemon::calibration::{CalibrationRun, Phase, Step};
    use padmap_daemon::session::Session;

    let pads = discover()?;
    if pads.is_empty() {
        println!("No joypads found.");
        std::process::exit(1);
    }
    let todo: Vec<Pad> = pads
        .iter()
        .filter(|pad| force || !profiles::is_known(pad, None))
        .cloned()
        .collect();
    let skipped = pads.len() - todo.len();
    if skipped > 0 {
        println!("{skipped} controller(s) already configured (--force to redo).\n");
    }
    if todo.is_empty() {
        println!("Nothing to do.");
        return Ok(());
    }

    for pad in todo {
        let name = padmap_daemon::clean(&pad.name);
        println!("--- {name} [{}] ---", pad.event());
        let mut session = Session::open(vec![pad.clone()]).context("opening the controller")?;
        let declared = session
            .source_mut(0)
            .map(|source| source.declared_axes())
            .unwrap_or_default();
        let axes: BTreeMap<u16, padmap_core::calibration::Declared> = declared
            .into_iter()
            .filter(|(code, axis)| axis.calibratable(*code))
            .collect();
        if axes.is_empty() {
            println!("  no centring axes on this device, skipping\n");
            continue;
        }
        println!("  1. Let go of the sticks. Press a button when steady.");
        let started = std::time::Instant::now();
        let mut run = CalibrationRun::new(1, pad.path.display().to_string(), axes, 0.0);
        let mut measured = None;
        while measured.is_none() {
            let now = started.elapsed().as_secs_f64();
            for event in session.read(0) {
                run.feed(event.kind, event.code, event.value);
            }
            match run.tick(now) {
                Step::Measured(axes) => measured = Some(axes),
                Step::Advanced => match run.phase {
                    Phase::Rest => println!("     measuring the centre..."),
                    Phase::AwaitReach => println!(
                        "  2. Now rotate every stick through its full range. Press a \
                         button when the circles reach the edges."
                    ),
                    Phase::Reach => println!("     measuring the reach..."),
                    _ => {}
                },
                _ => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        drop(session);
        let axes = measured.unwrap_or_default();
        for (code, calibration) in &axes {
            let span = format!("{}..{}", calibration.low(), calibration.high());
            let declared = format!("{}..{}", calibration.minimum, calibration.maximum);
            let note = if span == declared {
                String::new()
            } else {
                format!("  (declared {declared})")
            };
            println!(
                "      axis {code:<5} centre={:<5} reach={span:<12} deadband=+/-{}{note}",
                calibration.center, calibration.flat
            );
        }
        publish::store_calibration(&pad, axes);
        println!();
    }
    println!("Restart the daemon to apply:  padmap ensure-daemon");
    Ok(())
}

/// Guarantee a daemon is running *this* code, then return.
///
/// Meant to be run just before a front-end starts. A daemon keeps the code it
/// was started with, so after a rebuild it goes on answering and goes on
/// writing plausible files from the previous version -- which is not visible
/// in anything it produces.
pub fn cmd_ensure_daemon(check: bool, timeout: f64) -> Result<()> {
    // Rules are generated once from whatever was plugged in at the time, so a
    // controller added later is not in them -- and then a consumer sees the
    // physical adapter as well as the clone. Silent until someone wonders why
    // there are extra controllers.
    let pads = discover().unwrap_or_default();
    let hideable: Vec<padmap_core::hide::Hideable> = pads
        .iter()
        .map(|pad| padmap_core::hide::Hideable {
            name: pad.name.clone(),
            vid: pad.vid,
            pid: pad.pid,
        })
        .collect();
    let rules = installed_rules();
    let missing = padmap_core::hide::unhidden(&hideable, rules.as_deref());
    if !missing.is_empty() {
        println!(
            "warning: {} adapter(s) padmap republishes are still visible:",
            missing.len()
        );
        for target in &missing {
            println!("  {:04x}:{:04x}  {}", target.vid, target.pid, target.name);
        }
        println!("  regenerate the udev rules with:  padmap hide");
    }

    let ours = runtime::build_id_of_binary();
    let our_identity = IdentityMode::from_env();
    let Some(state) = daemon_state(1.0) else {
        if check {
            println!("no daemon running");
            std::process::exit(1);
        }
        println!("no daemon running; starting one");
        spawn_daemon()?;
        let Some(state) = wait_for_daemon(timeout) else {
            println!("daemon did not come up within {timeout:.0}s");
            std::process::exit(1);
        };
        println!(
            "daemon up, build {}",
            state["build"].as_str().unwrap_or("?")
        );
        return Ok(());
    };

    let theirs = state["build"].as_str().unwrap_or("");
    let their_identity = state["identity"].as_str();
    // Identity as well as build id: flipping PADMAP_PAD_IDENTITY changes what
    // the clones advertise, and so the GUID every mapping is written under,
    // but it changes no source -- a build-id comparison alone reports a stale
    // daemon as current and leaves it publishing pads nothing has a mapping
    // for.
    if theirs == ours && their_identity.is_none_or(|mode| mode == our_identity.as_str()) {
        println!(
            "daemon is current (build {ours}, identity {})",
            our_identity.as_str()
        );
        return Ok(());
    }
    if theirs == ours {
        println!("daemon is running with a different pad identity:");
        println!("  daemon: {}", their_identity.unwrap_or("?"));
        println!("  ours:   {}", our_identity.as_str());
    } else {
        println!("daemon is running older code:");
        println!("  daemon: {theirs}");
        println!("  ours:   {ours}");
    }
    if check {
        std::process::exit(1);
    }
    if !stop_daemon(&state, timeout) {
        println!("could not stop the running daemon");
        std::process::exit(1);
    }
    spawn_daemon()?;
    let Some(state) = wait_for_daemon(timeout) else {
        println!("replacement daemon did not come up within {timeout:.0}s");
        std::process::exit(1);
    };
    println!(
        "restarted; build {}, {} assignment(s) restored",
        state["build"].as_str().unwrap_or("?"),
        state["players"].as_array().map(Vec::len).unwrap_or(0)
    );
    Ok(())
}

fn installed_rules() -> Option<String> {
    for path in [
        "/run/udev/rules.d/99-padmap.rules",
        "/etc/udev/rules.d/99-padmap.rules",
    ] {
        if let Ok(raw) = std::fs::read(path) {
            return Some(String::from_utf8_lossy(&raw).into_owned());
        }
    }
    None
}

/// Send one command to the running daemon and wait for one event by name,
/// or `None` if nothing answers in time.
///
/// An `error` event ends the wait too, and is returned, so a caller can say
/// what the daemon said rather than "timed out".
pub fn daemon_ask(
    command: &serde_json::Value,
    want: &str,
    timeout: f64,
) -> Option<serde_json::Value> {
    use std::io::Read;
    use std::os::unix::net::UnixStream;

    let mut sock = UnixStream::connect(runtime::socket_path()).ok()?;
    let window = std::time::Duration::from_secs_f64(timeout);
    sock.set_read_timeout(Some(window)).ok()?;
    let mut line = serde_json::to_string(command).ok()?;
    line.push('\n');
    sock.write_all(line.as_bytes()).ok()?;
    let mut reader = padmap_core::wire::LineReader::new();
    let deadline = std::time::Instant::now() + window;
    let mut chunk = [0u8; 65536];
    while std::time::Instant::now() < deadline {
        let count = match sock.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => break,
        };
        for message in reader.feed(&chunk[..count]) {
            let event = message.get("event").and_then(|e| e.as_str());
            if event == Some(want) || event == Some("error") {
                return Some(serde_json::Value::Object(message));
            }
        }
    }
    None
}

/// Ask the running daemon for its state, or `None` if nothing answers.
pub fn daemon_state(timeout: f64) -> Option<serde_json::Value> {
    use std::io::Read;
    use std::os::unix::net::UnixStream;

    let mut sock = UnixStream::connect(runtime::socket_path()).ok()?;
    let window = std::time::Duration::from_secs_f64(timeout);
    sock.set_read_timeout(Some(window)).ok()?;
    // The daemon greets on connect, so no command is needed; asking anyway
    // makes this work regardless of that greeting.
    sock.write_all(b"{\"cmd\":\"status\"}\n").ok()?;
    let mut reader = padmap_core::wire::LineReader::new();
    let deadline = std::time::Instant::now() + window;
    let mut chunk = [0u8; 65536];
    while std::time::Instant::now() < deadline {
        let count = match sock.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => break,
        };
        for message in reader.feed(&chunk[..count]) {
            if message.get("event").and_then(|e| e.as_str()) == Some("state") {
                return Some(serde_json::Value::Object(message));
            }
        }
    }
    None
}

/// Start a daemon detached from this process, so it outlives whatever asked
/// for it -- a front-end launcher, typically, which would otherwise take the
/// controllers down with it when it exits.
fn spawn_daemon() -> Result<()> {
    let exe = std::env::current_exe().context("finding this binary")?;
    // Into a file rather than /dev/null: the daemon is the only thing that
    // sees a controller being claimed or a mapping captured, and discarding
    // all of it means the answer to "what happened" is simply gone.
    let log_path = runtime::daemon_log_path();
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let log = std::fs::File::create(&log_path).ok();
    let mut command = std::process::Command::new(exe);
    command.arg("serve");
    if let Some(log) = log {
        let errors = log.try_clone().ok();
        command.stdout(log);
        if let Some(errors) = errors {
            command.stderr(errors);
        }
    }
    // Its own process group, so a front-end exiting -- or the shell that
    // started it being killed -- does not take the daemon's controllers with
    // it. `process_group` rather than a `pre_exec` closure, because that
    // would be the only unsafe block in this binary for something the
    // standard library already does safely.
    command.process_group(0);
    command.spawn().context("starting the daemon")?;
    Ok(())
}

fn wait_for_daemon(timeout: f64) -> Option<serde_json::Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(timeout);
    while std::time::Instant::now() < deadline {
        if let Some(state) = daemon_state(0.5) {
            return Some(state);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    None
}

/// Ask the daemon that answered us to exit, and wait for it to go.
///
/// Signals exactly the pid it reported. Matching on the command line instead
/// hits *every* padmap daemon the user is running, including one on a
/// different `XDG_RUNTIME_DIR` -- an earlier version of this, run from a test
/// with its own runtime dir, took down the real daemon with it.
///
/// SIGTERM rather than SIGKILL: the handler releases every grab on the way
/// out, and a killed daemon leaves the machine with no working controllers.
fn stop_daemon(state: &serde_json::Value, timeout: f64) -> bool {
    let pid = match state["pid"].as_i64() {
        Some(pid) => pid as i32,
        None => {
            let candidates = runtime::daemon_pids(None);
            if candidates.len() != 1 {
                println!(
                    "  cannot identify the running daemon ({} candidates); stop it by hand",
                    candidates.len()
                );
                return false;
            }
            candidates[0] as i32
        }
    };
    let Ok(pid) = rustix::process::Pid::from_raw(pid).ok_or(()) else {
        return false;
    };
    match rustix::process::kill_process(pid, rustix::process::Signal::TERM) {
        Ok(()) => {}
        Err(rustix::io::Errno::SRCH) => return true,
        Err(error) => {
            println!("  could not signal pid {pid:?}: {error}");
            return false;
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(timeout);
    while std::time::Instant::now() < deadline {
        if daemon_state(0.3).is_none() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    false
}

/// Where the udev rules go: tmpfs, so a reboot undoes it, and on NixOS
/// `/etc/udev/rules.d` is a store symlink that cannot be written at all.
pub const RUNTIME_RULES_PATH: &str = "/run/udev/rules.d/99-padmap.rules";

/// Install the rules and make udev apply them.
///
/// Reloading is not optional: udev keeps its rules in memory, so a file
/// written without the reload changes nothing until the next boot -- and a
/// controller still visible after padmap said it had hidden it is precisely
/// the silent gap this project keeps tripping over.
pub fn install_rules(rules: &str) -> (bool, Vec<String>) {
    let target = Path::new(RUNTIME_RULES_PATH);
    let existing = std::fs::read(target)
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
        .unwrap_or_default();
    if existing == rules {
        return (
            false,
            vec![format!("{} already up to date", target.display())],
        );
    }
    let mut messages = Vec::new();
    if let Some(parent) = target.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return (
                false,
                vec![format!("could not write {}: {error}", target.display())],
            );
        }
    }
    if let Err(error) = std::fs::write(target, rules) {
        return (
            false,
            vec![format!("could not write {}: {error}", target.display())],
        );
    }
    messages.push(format!("wrote {}", target.display()));
    for args in [
        ["control", "--reload-rules"].as_slice(),
        ["trigger", "--subsystem-match=input"].as_slice(),
    ] {
        messages.push(match udevadm(args) {
            Ok(()) => format!("udevadm {} ok", args[0]),
            Err(why) => why,
        });
    }
    (true, messages)
}

fn udevadm(args: &[&str]) -> Result<(), String> {
    // Located rather than assumed: on NixOS it lives under
    // /run/current-system/sw/bin, which is not always on a root shell's PATH.
    let binary = ["udevadm", "/run/current-system/sw/bin/udevadm"]
        .into_iter()
        .find(|candidate| {
            candidate.starts_with('/') && Path::new(candidate).exists()
                || !candidate.starts_with('/')
        })
        .unwrap_or("udevadm");
    match std::process::Command::new(binary).args(args).output() {
        Ok(done) if done.status.success() => Ok(()),
        Ok(done) => Err(format!(
            "udevadm {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&done.stderr).trim()
        )),
        Err(error) => Err(format!("could not run udevadm: {error}")),
    }
}

/// Republish the assigned pads, then start RetroArch bound to that order.
pub fn cmd_launch(rest: Vec<String>, log: Option<Option<String>>) -> Result<i32> {
    let saved = assignments::load(&runtime::assignments_path())?;
    if saved.is_empty() {
        println!("No assignments. Run `padmap setup` first.");
        std::process::exit(1);
    }
    // A running daemon already holds the controllers, and this republishes
    // them itself; the two cannot coexist. Said plainly, because what came
    // out before was `OSError: [Errno 16] Device or resource busy` from
    // inside a library, which names neither the daemon nor the way out.
    let running = runtime::daemon_pids(None);
    if let Some(pid) = running.first() {
        println!("the padmap daemon (pid {pid}) already holds the controllers.");
        println!("This command republishes them itself, so the two cannot run at once.\n");
        println!("To launch a game while the daemon runs, use the launcher that");
        println!("resolves mappings against it:");
        println!("  padmap-play -L <core.so> <rom>\n");
        println!("To use this command instead, stop the daemon first:");
        println!("  kill {pid}");
        std::process::exit(1);
    }

    let log_path = log.map(|explicit| {
        explicit
            .map(PathBuf::from)
            .unwrap_or_else(|| runtime::dir().join("retroarch.log"))
    });
    let argv: Vec<String> = rest.into_iter().skip_while(|arg| arg == "--").collect();
    let config = runtime::dir().join("launch.cfg");
    let mut command = std::process::Command::new("retroarch");
    command.arg("--appendconfig").arg(&config);
    if let Some(path) = &log_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        command.arg("--verbose").arg("--log-file").arg(path);
        println!("logging RetroArch output to {}", path.display());
    }
    // The --nodevice flags go before the caller's own, so a port can still be
    // overridden by hand: RetroArch's parser takes the last one it sees.
    let args_file = runtime::dir().join("launch.args");
    if let Ok(text) = std::fs::read_to_string(&args_file) {
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            command.arg(line);
        }
    }
    command.args(&argv);
    println!("launching retroarch\n");
    let status = command.status().context("running retroarch")?;
    Ok(status.code().unwrap_or(0))
}

/// The pad `--pad` names, or the only one, or a list and an exit.
fn pick_pad(pads: &[Pad], which: Option<&str>) -> Pad {
    match which {
        Some(name) => match pads
            .iter()
            .find(|pad| pad.name.contains(name) || pad.event() == name)
        {
            Some(pad) => pad.clone(),
            None => {
                println!("No pad matches {name:?}. Connected:");
                for pad in pads {
                    println!("  {}  {}", pad.event(), pad.name);
                }
                std::process::exit(1);
            }
        },
        None if pads.len() == 1 => pads[0].clone(),
        None => {
            println!("Several pads are connected; name one with --pad:");
            for pad in pads {
                println!("  {}  {}", pad.event(), pad.name);
            }
            std::process::exit(1);
        }
    }
}

/// Set a deadzone, a debounce, or an axis or button to ignore, for a
/// controller that misbehaves.
///
/// Through the daemon when one is running, so a seated pad is rebuilt with
/// the new setting at once; straight into the profile store otherwise, where
/// the next `run` or `serve` reads it. Either way the setting lives with the
/// physical controller and follows it to whatever seat it takes.
pub fn cmd_tune(
    which: Option<String>,
    request: padmap_core::tuning::Request,
    show: bool,
) -> Result<()> {
    let pads = discover()?;
    if pads.is_empty() {
        println!("No joypads found.");
        std::process::exit(1);
    }
    let pad = pick_pad(&pads, which.as_deref());
    let signature = profiles::signature_of(&pad);

    if show || request.is_empty() {
        let tuning = publish::tuning_for(&pad);
        println!("{}  {}", pad.event(), pad.name);
        println!("{}", serde_json::to_string_pretty(&tuning)?);
        if request.is_empty() && !show {
            println!("\nNothing to change. See `padmap tune --help`.");
        }
        return Ok(());
    }

    let mut message = request.to_json();
    message["cmd"] = "tune".into();
    message["signature"] = signature.clone().into();
    let tuning = match daemon_ask(&message, "tuned", 5.0) {
        Some(reply) if reply.get("event").and_then(|e| e.as_str()) == Some("tuned") => reply
            .get("tuning")
            .cloned()
            .unwrap_or(serde_json::json!({})),
        Some(reply) => anyhow::bail!(
            "the daemon refused: {}",
            reply
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("no reason given")
        ),
        None => {
            // No daemon. Read the axes ourselves and write the profile; the
            // next republish picks it up.
            let declared: Vec<u16> = clone::open_source(&pad, false)
                .map(|source| source.declared_axes().keys().copied().collect())
                .unwrap_or_default();
            if request.deadzone_all.is_some() && declared.is_empty() {
                anyhow::bail!(
                    "could not read {}'s axes to set a deadzone on them",
                    pad.name
                );
            }
            let tuning = request.apply(&publish::tuning_for(&pad), &declared);
            publish::store_tuning(&pad, tuning.clone())
                .with_context(|| format!("saving the profile for {}", pad.name))?;
            serde_json::to_value(&tuning)?
        }
    };
    println!("{}  {}", pad.event(), pad.name);
    println!("{}", serde_json::to_string_pretty(&tuning)?);
    Ok(())
}
