//! Everything written to disk when the roster changes.
//! Regenerated whole on each accept/republish to keep in step.

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub player: u32,
    pub pad: Pad,
}

#[derive(Debug, Clone, Default)]
pub struct PadFacts {
    pub keys: Vec<u16>,
    pub axes: Vec<u16>,
    pub spans: BTreeMap<u16, AxisSpan>,
    pub physical_guid: Option<String>,
}

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

pub fn resolved(pad: &Pad, console: &str, game: &str) -> (String, padmap_core::profile::Mapping) {
    profiles::load(pad, None)
        .map(|profile| profile.resolve(console, game))
        .unwrap_or_default()
}

pub fn has_mapping(pad: &Pad) -> bool {
    profiles::load(pad, None).is_some_and(|profile| profile.has_bindings())
}

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

/// The SDL database line for a player's clone.
pub fn sdl_line_for(
    player: u32,
    identity: Identity,
    bindings: &BTreeMap<Control, Binding>,
    facts: &PadFacts,
) -> String {
    let sticks = sdl::stick_fields(&facts.axes, bindings, Some(&facts.spans));
    emit::sdl_line_for(player, identity, bindings, Some(&sticks))
}

/// The stored mapping's line, or empty. Empty is better than a line from no bindings.
pub fn stored_sdl_line(player: u32, pad: &Pad, identity: Identity, facts: &PadFacts) -> String {
    let bindings = resolved(pad, "", "").1.resolved();
    if bindings.is_empty() {
        return String::new();
    }
    sdl_line_for(player, identity, &bindings, facts)
}

/// A usable SDL line for unmapped pads. None if the pad reports no buttons.
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

/// Mapping already on disk or in SDL's database for this GUID.
fn carried(guid: Option<&str>) -> Option<(Fields, String)> {
    let guid = guid?;
    if let Some(found) = artefacts::carried_fields(guid) {
        return Some(found);
    }
    // Skip virtual pads named by padmap itself.
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

/// Log layout controls a mapping doesn't bind. User must remap to use them.
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

/// Pad index to device path, in RetroArch's enumeration order.
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

/// What the writers produced.
#[derive(Debug, Default, Clone)]
pub struct Written {
    pub sdl_lines: Vec<String>,
}

/// Write every file the roster implies. Resolves configs for the last-launched game.
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

/// Get or create a profile for a pad, preserving existing identity.
fn profile_for(pad: &Pad) -> Profile {
    let mut profile = profiles::load(pad, None).unwrap_or_default();
    profile.signature = profiles::signature_of(pad);
    profile.name = crate::clean(&pad.name);
    profile
}

/// Save a mapping capture under a scope. Returns controls it didn't bind.
pub fn store_mapping(
    pad: &Pad,
    layout_id: &str,
    bindings: &BTreeMap<Control, Binding>,
    scope: &str,
) -> Vec<String> {
    let mut profile = profile_for(pad);
    // Only set icon from default-scope captures with layout id matching an icon.
    if profile.icon.is_empty() && scope.is_empty() && padmap_core::icons::known(layout_id) {
        profile.icon = layout_id.to_owned();
    }
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

/// Save axis calibration, preserving everything else.
pub fn store_calibration(
    pad: &Pad,
    axes: BTreeMap<u16, padmap_core::calibration::AxisCalibration>,
) {
    let mut profile = profile_for(pad);
    profile.axes = axes;
    match profiles::save(&profile, None) {
        Ok(_) => info!(
            "calibrated {}: {} axis/axes",
            crate::clean(&pad.name),
            profile.axes.len()
        ),
        Err(error) => warn!("could not save the profile for {}: {error}", pad.name),
    }
}

/// Save the user's icon choice.
pub fn store_icon(pad: &Pad, icon: &str) {
    let mut profile = profile_for(pad);
    profile.icon = icon.to_owned();
    if let Err(error) = profiles::save(&profile, None) {
        warn!("could not save the profile for {}: {error}", pad.name);
    }
}

/// Save tuning adjustments for a misbehaving pad.
pub fn store_tuning(pad: &Pad, tuning: padmap_core::tuning::Tuning) -> std::io::Result<()> {
    let mut profile = profile_for(pad);
    profile.tuning = tuning;
    profiles::save(&profile, None).map(|_| ())
}

/// Get the user's tuning settings for a pad, or defaults.
pub fn tuning_for(pad: &Pad) -> padmap_core::tuning::Tuning {
    profiles::load(pad, None)
        .map(|profile| profile.tuning)
        .unwrap_or_default()
}

/// The icon a pad should display.
pub fn icon_for(pad: &Pad, overrides: &BTreeMap<String, String>) -> &'static str {
    let stored = profiles::load(pad, None).map(|profile| profile.icon);
    padmap_core::icons::for_pad(pad.vid, pad.pid, &pad.name, stored.as_deref(), overrides)
}

/// Layout of the pad's default (no-scope) capture.
pub fn stored_layout(pad: &Pad) -> String {
    resolved(pad, "", "").1.layout
}

/// Layout ids the pad has captures under.
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
