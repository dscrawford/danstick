//! Everything written to disk when the roster changes.
//!
//! The SDL database, the RetroArch autoconfig profiles, the launch override
//! and its flags, and the emulator files -- regenerated whole on every
//! accept and every republish rather than migrated. SDL keys its database on
//! the device name, and padmap's virtual pads are named after the player
//! slot, so a stored line describes "whatever was in slot 1 last time".
//! Rewriting from the current assignment means the question of keeping it in
//! step never arises.

use std::collections::BTreeMap;
use std::path::Path;

use evdev::Device;
use log::{info, warn};
use padmap_core::binding::Binding;
use padmap_core::emit::{self, Identity};
use padmap_core::fields::Fields;
use padmap_core::profile::Profile;
use padmap_core::retroarch::{self, LaunchFacts};
use padmap_core::sdl::AxisSpan;
use padmap_core::{guess, sdl, Control};
use padmap_input::clone::{self, IdentityMode};
use padmap_input::pad::{self, Pad};
use padmap_input::{artefacts, emulators, profiles, runtime, sdlprobe, triton};

/// One player and the pad holding the slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub player: u32,
    pub pad: Pad,
}

/// What a pad reports about itself, read once per write.
#[derive(Debug, Clone, Default)]
pub struct PadFacts {
    pub keys: Vec<u16>,
    pub axes: Vec<u16>,
    pub spans: BTreeMap<u16, AxisSpan>,
    /// The GUID SDL computes for the physical controller, for carrying a
    /// mapping over. `None` when the pad could not be opened, or has no evdev
    /// node for SDL to have seen.
    pub physical_guid: Option<String>,
}

/// Open a pad without grabbing it and read what it declares.
///
/// Empty on failure, deliberately: a pad that cannot be opened right now gets
/// no line rather than the daemon getting no further, and the republisher
/// will say why when it tries.
pub fn pad_facts(pad: &Pad) -> PadFacts {
    match clone::open_source(pad, false) {
        Ok(source) => {
            let (keys, axes) = source.capabilities();
            PadFacts {
                keys,
                axes,
                spans: source.axis_spans(),
                physical_guid: source.physical_guid(),
            }
        }
        Err(error) => {
            warn!("{}: could not read its capabilities ({error})", pad.name);
            PadFacts::default()
        }
    }
}

/// What a clone for this pad advertises, in this identity mode.
///
/// Opens the device to read its ids, which is what mirroring means; the
/// republisher does the same when it creates the clone, so the two answers
/// agree. A pad that cannot be opened falls back to padmap's own identity --
/// the daemon has to write *something* for a slot it is still holding.
pub fn identity_of(pad: &Pad, player: u32, mode: IdentityMode) -> Identity {
    let padmap_own = Identity {
        bustype: 0x06,
        vendor: clone::PADMAP_VID,
        product: clone::PADMAP_PID,
        version: emit::version_for(player),
    };
    if triton::owns(pad) {
        return match mode {
            IdentityMode::Padmap => padmap_own,
            IdentityMode::Mirror => Identity {
                bustype: 0x03,
                vendor: pad.vid,
                product: pad.pid,
                version: emit::version_for(player),
            },
        };
    }
    match Device::open(&pad.path) {
        Ok(device) => {
            let identity = clone::Identity::for_source(mode, &device, player);
            Identity {
                bustype: identity.bustype,
                vendor: identity.vendor,
                product: identity.product,
                version: identity.version,
            }
        }
        Err(_) => padmap_own,
    }
}

/// The capture that applies, and the scope it came from.
pub fn resolved(pad: &Pad, console: &str, game: &str) -> (String, padmap_core::profile::Mapping) {
    profiles::load(pad, None)
        .map(|profile| profile.resolve(console, game))
        .unwrap_or_default()
}

/// Whether this controller has been through the mapping wizard, under any
/// scope. A profile can exist with no buttons -- calibration writes one -- so
/// the presence of a profile is not the question.
pub fn has_mapping(pad: &Pad) -> bool {
    profiles::load(pad, None).is_some_and(|profile| profile.has_bindings())
}

/// Which scopes this controller has a capture under.
pub fn mapping_scopes(pad: &Pad) -> Vec<String> {
    profiles::load(pad, None)
        .map(|profile| {
            profile
                .mappings
                .iter()
                .filter(|(_, mapping)| !mapping.buttons.is_empty())
                .map(|(scope, _)| scope.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The SDL database line for a player's clone, from what the user pressed.
pub fn sdl_line_for(
    player: u32,
    identity: Identity,
    bindings: &BTreeMap<Control, Binding>,
    facts: &PadFacts,
) -> String {
    let sticks = sdl::stick_fields(&facts.axes, bindings, Some(&facts.spans));
    emit::sdl_line_for(player, identity, bindings, Some(&sticks))
}

/// The stored mapping's line, or empty when there is none -- a line built
/// from no bindings claims a pad with no buttons, and SDL believing that is
/// worse than SDL falling back to its own database.
pub fn stored_sdl_line(player: u32, pad: &Pad, identity: Identity, facts: &PadFacts) -> String {
    let bindings = resolved(pad, "", "").1.resolved();
    if bindings.is_empty() {
        return String::new();
    }
    sdl_line_for(player, identity, &bindings, facts)
}

/// A usable line for a pad that has never been mapped, plus where it came
/// from. `None` when the pad reports no buttons at all, which is not a
/// controller anything could navigate with.
///
/// Two sources, in order of how much they are worth trusting: a line the
/// user already has on disk for the physical controller, then SDL's own
/// database, then a guess from the pad's capabilities. Under padmap's own
/// identity the line is the *only* thing between the user and a controller
/// with no buttons, since SDL knows nothing about 1209:0001.
pub fn fallback_line_for(
    player: u32,
    identity: Identity,
    facts: &PadFacts,
) -> Option<(String, String)> {
    if facts.keys.is_empty() {
        return None;
    }
    let (fields, source) = carried(facts.physical_guid.as_deref())
        .map(|(fields, from)| (fields, format!("carried over from {from}")))
        .unwrap_or_else(|| {
            (
                guess::guessed_fields(&facts.keys, &facts.axes, Some(&facts.spans)),
                "guessed from the controller's own capabilities".to_owned(),
            )
        });
    let line = emit::sdl_line(
        &emit::virtual_guid(player, identity),
        &emit::virtual_name(player),
        &fields,
    );
    Some((
        line,
        format!("{source}; run the mapping wizard to replace it"),
    ))
}

/// A mapping already on disk, or in SDL's own database, for this GUID.
fn carried(guid: Option<&str>) -> Option<(Fields, String)> {
    let guid = guid?;
    if let Some(found) = artefacts::carried_fields(guid) {
        return Some(found);
    }
    // SDL's compiled-in database, asked through a process of its own -- see
    // `sdlprobe`. A line naming one of our own pads describes a previous
    // generation rather than a controller.
    let line = sdlprobe::isolated(guid)?;
    let (_, name, fields) = sdl::parse_line(&line)?;
    if name.starts_with(emit::VIRTUAL_PREFIX) {
        return None;
    }
    Some((
        artefacts::binding_fields(&fields),
        "SDL's built-in database".to_owned(),
    ))
}

/// The autoconfig profile for one clone, as text.
///
/// The same bytes the `controller` event reports, so a consumer applying
/// binds live and RetroArch reading the file cannot disagree.
pub fn profile_text(
    pad: &Pad,
    player: u32,
    identity: Identity,
    console: &str,
    game: &str,
    context: &str,
) -> String {
    let source = artefacts::find_profile(&pad.name, pad.vid, pad.pid);
    let (scope, mapping) = resolved(pad, console, game);
    let bindings = mapping.resolved();
    if !bindings.is_empty() {
        // The user pressed these buttons themselves. Copying libretro's
        // entry instead would give two sets of bindings for one controller,
        // differing in ways nobody is told about.
        log_unmapped(pad, &scope, &mapping.layout, &bindings);
        return emit::retroarch_profile(
            player,
            identity,
            &bindings,
            source.as_ref().map(|(name, _)| name.as_str()).unwrap_or(""),
            &mapping.layout,
            &scope,
            context,
        );
    }
    retroarch::derive_profile(
        source
            .as_ref()
            .map(|(name, values)| (name.as_str(), values.as_slice())),
        player,
        identity.vendor,
        identity.product,
        emit::virtual_name,
    )
}

/// Name the layout's controls a mapping has no binding for.
///
/// A mapping captured before its layout gained a control keeps working and
/// keeps being chosen, so the new control is simply dead in game. Logged
/// rather than repaired: the repair is a question for the user.
fn log_unmapped(pad: &Pad, scope: &str, layout_id: &str, bindings: &BTreeMap<Control, Binding>) {
    let layout = padmap_core::layout::get(layout_id);
    let missing: Vec<&str> = layout
        .controls
        .iter()
        .filter(|control| !bindings.contains_key(&control.canonical))
        .map(|control| control.canonical.as_str())
        .collect();
    if !missing.is_empty() {
        info!(
            "{}: mapping for scope {:?} ({layout_id}) is missing {} of the layout's controls: {} -- remap this pad for that console to bind them",
            pad.name,
            if scope.is_empty() { "default" } else { scope },
            missing.len(),
            missing.join(", ")
        );
    }
}

/// Pad index -> device path, exactly as RetroArch's udev driver will see it.
pub fn visible_order() -> BTreeMap<usize, String> {
    pad::discover(pad::Filter {
        include_virtual: true,
        retroarch_only: true,
        include_undriven: false,
    })
    .map(|pads| {
        pads.into_iter()
            .enumerate()
            .map(|(index, pad)| (index, pad.path.display().to_string()))
            .collect()
    })
    .unwrap_or_default()
}

/// What the writers produced, for the events that follow.
#[derive(Debug, Default, Clone)]
pub struct Written {
    /// The SDL lines, in player order.
    pub sdl_lines: Vec<String>,
}

/// Write every file the roster implies.
///
/// `virtual_paths` are the clones' device nodes, which decide the pad
/// indices; `last` is the game most recently launched, whose scope the
/// autoconfig is resolved for -- a consumer applying these during a game must
/// be given that game's mapping, not the context-free one.
pub fn write_all(
    slots: &[Slot],
    virtual_paths: &BTreeMap<u32, String>,
    mode: IdentityMode,
    launch_config_path: &Path,
    launch_args_path: &Path,
    last: Option<&runtime::Game>,
) -> Written {
    let console = last.map(|game| game.console.as_str()).unwrap_or("");
    let game = last.map(|game| game.key.as_str()).unwrap_or("");
    let context = last.map(|game| game.title.as_str()).unwrap_or("");
    let identities: BTreeMap<u32, Identity> = slots
        .iter()
        .map(|slot| (slot.player, identity_of(&slot.pad, slot.player, mode)))
        .collect();
    let players: Vec<u32> = slots.iter().map(|slot| slot.player).collect();

    // The autoconfig profiles, resolved for the last game.
    let profiles_out: BTreeMap<u32, String> = slots
        .iter()
        .map(|slot| {
            (
                slot.player,
                profile_text(
                    &slot.pad,
                    slot.player,
                    identities[&slot.player],
                    console,
                    game,
                    context,
                ),
            )
        })
        .collect();
    match artefacts::write_autoconfig(&profiles_out, None) {
        Ok(written) => info!("wrote {} autoconfig profile(s)", written.len()),
        Err(error) => warn!("could not write the autoconfig profiles: {error}"),
    }

    // The launch override and its flags.
    let order = visible_order();
    let managed = retroarch::managed_players(&players, virtual_paths, &order);
    let all_calibrated = slots
        .iter()
        .filter(|slot| managed.contains_key(&slot.player))
        .all(|slot| {
            profiles::load(&slot.pad, None).is_some_and(|profile| !profile.axes.is_empty())
        });
    let facts = LaunchFacts {
        all_calibrated,
        autoconfig_dir: runtime::dir().join("autoconfig").display().to_string(),
        verbose: false,
    };
    let config =
        retroarch::launch_config(&players, virtual_paths, &order, &facts, emit::virtual_name);
    if let Err(error) = artefacts::write_launch_config(launch_config_path, &config) {
        warn!("could not write the launch config: {error}");
    }
    let args = retroarch::launch_args(&players, virtual_paths, &order);
    if let Err(error) = artefacts::write_launch_args(launch_args_path, &args) {
        warn!("could not write the launch flags: {error}");
    }

    // The SDL database, and the emulators that cannot read it.
    let mut lines: BTreeMap<u32, String> = BTreeMap::new();
    let mut notes: BTreeMap<u32, String> = BTreeMap::new();
    let mut published: Vec<emulators::Published> = Vec::new();
    for slot in slots {
        let facts = pad_facts(&slot.pad);
        let identity = identities[&slot.player];
        let line = stored_sdl_line(slot.player, &slot.pad, identity, &facts);
        if !line.is_empty() {
            lines.insert(slot.player, line);
        } else if let Some((line, note)) = fallback_line_for(slot.player, identity, &facts) {
            info!("player {}: no capture yet, SDL mapping {note}", slot.player);
            lines.insert(slot.player, line);
            notes.insert(slot.player, note);
        }
        published.push(emulators::Published {
            player: slot.player,
            guid: emit::virtual_guid(slot.player, identity),
            name: emit::virtual_name(slot.player),
            keys: facts.keys.clone(),
            axes: facts.axes.clone(),
            sdl_line: lines.get(&slot.player).cloned().unwrap_or_default(),
        });
    }
    let fallback = Identity {
        bustype: 0x06,
        vendor: clone::PADMAP_VID,
        product: clone::PADMAP_PID,
        version: clone::PADMAP_VERSION,
    };
    match artefacts::write_sdl_database(
        &lines,
        &notes,
        |player| identities.get(&player).copied().unwrap_or(fallback),
        None,
    ) {
        Ok(path) => info!("wrote {} SDL mapping(s) to {}", lines.len(), path.display()),
        Err(error) => warn!("could not write SDL mappings: {error}"),
    }
    let wrote = emulators::publish(&published, &emulators::Destinations::default());
    for (target, why) in &wrote.skipped {
        info!("{target}: not written ({why})");
    }

    Written {
        sdl_lines: lines.into_values().collect(),
    }
}

/// Keep a capture against the *controller*, under one scope.
///
/// Everything else on the profile is carried over rather than rebuilt:
/// recording an N64 mapping is not a reason to forget the calibration, the
/// icon, or the mapping for every other console. Returns the controls the
/// layout asked for and did not get -- a skipped control is stored as an
/// absence, which emits no key at all, and naming them is what tells that
/// apart from a control the wizard never offered.
pub fn store_mapping(
    pad: &Pad,
    layout_id: &str,
    bindings: &BTreeMap<Control, Binding>,
    scope: &str,
) -> Vec<String> {
    let existing = profiles::load(pad, None);
    // Only from a capture with no scope, and only layout ids that are also
    // icon names. A GameCube controller mapped *for N64 games* is captured
    // against the N64 layout, and taking the icon from it would relabel the
    // pad as an N64 controller forever after.
    let mut icon = existing
        .as_ref()
        .map(|p| p.icon.clone())
        .unwrap_or_default();
    if icon.is_empty() && scope.is_empty() && padmap_core::icons::known(layout_id) {
        icon = layout_id.to_owned();
    }
    let mut profile = Profile {
        signature: profiles::signature_of(pad),
        name: crate::clean(&pad.name),
        icon,
        axes: existing
            .as_ref()
            .map(|p| p.axes.clone())
            .unwrap_or_default(),
        mappings: existing.map(|p| p.mappings).unwrap_or_default(),
    };
    profile.record(
        scope,
        padmap_core::profile::Mapping {
            buttons: bindings
                .iter()
                .map(|(control, binding)| (control.to_string(), *binding))
                .collect(),
            layout: layout_id.to_owned(),
            name: String::new(),
        },
    );
    if let Err(error) = profiles::save(&profile, None) {
        warn!("could not save the profile for {}: {error}", pad.name);
    }
    padmap_core::layout::get(layout_id)
        .controls
        .iter()
        .filter(|control| !bindings.contains_key(&control.canonical))
        .map(|control| control.canonical.to_string())
        .collect()
}

/// Write a profile with these axes, preserving everything else already
/// chosen for the pad.
pub fn store_calibration(
    pad: &Pad,
    axes: BTreeMap<u16, padmap_core::calibration::AxisCalibration>,
) {
    let existing = profiles::load(pad, None);
    let profile = Profile {
        signature: profiles::signature_of(pad),
        name: crate::clean(&pad.name),
        icon: existing
            .as_ref()
            .map(|p| p.icon.clone())
            .unwrap_or_default(),
        mappings: existing.map(|p| p.mappings).unwrap_or_default(),
        axes,
    };
    match profiles::save(&profile, None) {
        Ok(_) => info!(
            "calibrated {}: {} axis/axes",
            crate::clean(&pad.name),
            profile.axes.len()
        ),
        Err(error) => warn!("could not save the profile for {}: {error}", pad.name),
    }
}

/// Record the user's choice of icon. This is what retires the built-in
/// vid/pid table: once a pad has been through setup, its icon comes from the
/// person who owns it.
pub fn store_icon(pad: &Pad, icon: &str) {
    let existing = profiles::load(pad, None);
    let profile = Profile {
        signature: profiles::signature_of(pad),
        name: crate::clean(&pad.name),
        icon: icon.to_owned(),
        // Carried over, not rebuilt: picking a picture is not a reason to
        // forget where every button is.
        mappings: existing
            .as_ref()
            .map(|p| p.mappings.clone())
            .unwrap_or_default(),
        axes: existing.map(|p| p.axes).unwrap_or_default(),
    };
    if let Err(error) = profiles::save(&profile, None) {
        warn!("could not save the profile for {}: {error}", pad.name);
    }
}

/// The icon a pad should be drawn with.
pub fn icon_for(pad: &Pad, overrides: &BTreeMap<String, String>) -> &'static str {
    let stored = profiles::load(pad, None).map(|profile| profile.icon);
    padmap_core::icons::for_pad(pad.vid, pad.pid, &pad.name, stored.as_deref(), overrides)
}

/// Which layout a pad's default capture was taken under, or "".
pub fn stored_layout(pad: &Pad) -> String {
    resolved(pad, "", "").1.layout
}

/// Layout ids this controller already has a capture under.
pub fn mapped_layouts(pad: &Pad) -> std::collections::BTreeSet<String> {
    profiles::load(pad, None)
        .map(|profile| {
            profile
                .mappings
                .values()
                .filter(|mapping| !mapping.buttons.is_empty())
                .map(|mapping| mapping.layout.clone())
                .collect()
        })
        .unwrap_or_default()
}
