//! Everything written to disk when the roster changes.

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

/// The 360 identity a player's clone wears under `mode`, which a reserved
/// seat wears before it has a pad; `None` when it depends on the pad.
pub fn xbox_identity(mode: IdentityMode, player: u32) -> Option<Identity> {
    mode.xbox_identity(player).map(|identity| Identity {
        bustype: identity.bustype,
        vendor: identity.vendor,
        product: identity.product,
        version: identity.version,
    })
}

/// The 360 clone's capabilities, the same for every pad behind one.
pub fn xbox_facts() -> PadFacts {
    PadFacts {
        keys: padmap_core::xbox::KEYS.to_vec(),
        axes: padmap_core::xbox::axis_codes(),
        spans: padmap_core::xbox::spans(),
        physical_guid: None,
    }
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
    if let Some(identity) = xbox_identity(mode, player) {
        return identity;
    }
    if triton::owns(pad) {
        return match mode {
            IdentityMode::Padmap => padmap_own,
            IdentityMode::Mirror | IdentityMode::Xbox360 | IdentityMode::Xbox360Numbered => {
                Identity {
                    bustype: 0x03,
                    vendor: pad.vid,
                    product: pad.pid,
                    version: emit::version_for(player),
                }
            }
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

/// The stored mapping's line, or empty.
pub fn stored_sdl_line(player: u32, pad: &Pad, identity: Identity, facts: &PadFacts) -> String {
    let bindings = resolved(pad, "", "").1.resolved();
    if bindings.is_empty() {
        return String::new();
    }
    sdl_line_for(player, identity, &bindings, facts)
}

/// A usable SDL line for an unmapped pad, and why it is what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    pub line: String,
    pub note: String,
    /// A guess made while SDL was still being asked; replaced once it answers.
    pub provisional: bool,
}

/// A usable SDL line for unmapped pads.
pub fn fallback_line_for(player: u32, identity: Identity, facts: &PadFacts) -> Option<Fallback> {
    if facts.keys.is_empty() {
        return None;
    }
    let found = carried(facts.physical_guid.as_deref());
    let provisional = found == Carried::Pending;
    let (fields, source) = match found {
        Carried::Found(fields, from) => (fields, format!("carried over from {from}")),
        Carried::Pending | Carried::Absent => (
            guess::guessed_fields(&facts.keys, &facts.axes, Some(&facts.spans)),
            if provisional {
                "guessed from the controller's own capabilities while SDL is asked".to_owned()
            } else {
                "guessed from the controller's own capabilities".to_owned()
            },
        ),
    };
    let line = emit::sdl_line(
        &emit::virtual_guid(player, identity),
        &emit::virtual_name(player),
        &fields,
    );
    Some(Fallback {
        line,
        note: format!("{source}; run the mapping wizard to replace it"),
        provisional,
    })
}

/// Whether a mapping for this GUID is on disk, in SDL's database, or not yet known.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Carried {
    Found(Fields, String),
    /// SDL is being asked off the event loop and has not answered.
    Pending,
    Absent,
}

/// Mapping already on disk or in SDL's database for this GUID. SDL is never
/// asked here: a probe is a subprocess, and this runs on the event loop.
fn carried(guid: Option<&str>) -> Carried {
    let Some(guid) = guid else {
        return Carried::Absent;
    };
    if let Some((fields, from)) = artefacts::carried_fields(guid) {
        return Carried::Found(fields, from);
    }
    let line = match sdlprobe::shared().lookup(guid) {
        sdlprobe::Lookup::Pending => return Carried::Pending,
        sdlprobe::Lookup::Known(None) => return Carried::Absent,
        sdlprobe::Lookup::Known(Some(line)) => line,
    };
    match sdl::parse_line(&line) {
        Some((_, name, fields)) if !name.starts_with(emit::VIRTUAL_PREFIX) => Carried::Found(
            artefacts::binding_fields(&fields),
            "SDL's built-in database".to_owned(),
        ),
        _ => Carried::Absent,
    }
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

/// Log layout controls a mapping doesn't bind.
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
    /// Some seated player's mapping was guessed while SDL was still being
    /// asked, so these files are worth writing again when it answers.
    pub awaiting_sdl: bool,
}

/// What one player's files were written from, and what they came out as.
#[derive(Debug, Clone)]
struct Derived {
    from: Source,
    /// Whether the pad's capabilities could be read; a guess made without them
    /// is not worth keeping, since the next join would otherwise repeat it.
    sound: bool,
    /// Guessed while SDL was being asked about this pad.
    provisional: bool,
    profile: String,
    line: String,
    note: Option<String>,
    published: emulators::Published,
}

/// Everything a player's files depend on, so a stale one is never reused.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Source {
    signature: String,
    /// The stored profile as it was. A capture or a `forget` rewrites this file
    /// and nothing else tells the daemon, not even when the CLI did it.
    profile: Option<String>,
    identity: Identity,
    xbox: bool,
    scope: (String, String, String),
}

/// Each player's files as last written, so a join never works the rest of the room out again.
#[derive(Debug, Default)]
pub struct Cache {
    players: BTreeMap<u32, Derived>,
}

impl Cache {
    /// Forget every player's cached files; only a join may reuse them unchanged.
    pub fn clear(&mut self) {
        self.players.clear();
    }
}

/// Work one player's files out from scratch: reading the pad's capabilities and
/// asking SDL about its GUID, which is most of what writing them costs.
fn derive(
    slot: &Slot,
    identity: Identity,
    from: Source,
    xbox: bool,
    console: &str,
    game: &str,
    context: &str,
) -> Derived {
    let profile = if xbox {
        emit::retroarch_profile(
            slot.player,
            identity,
            &padmap_core::xbox::bindings(),
            "",
            padmap_core::layout::default_id(),
            "",
            context,
        )
    } else {
        profile_text(&slot.pad, slot.player, identity, console, game, context)
    };
    let facts = if xbox {
        xbox_facts()
    } else {
        pad_facts(&slot.pad)
    };
    let stored = if xbox {
        sdl_line_for(
            slot.player,
            identity,
            &padmap_core::xbox::bindings(),
            &facts,
        )
    } else {
        stored_sdl_line(slot.player, &slot.pad, identity, &facts)
    };
    let (line, note, provisional) = if stored.is_empty() {
        match fallback_line_for(slot.player, identity, &facts) {
            Some(fallback) => {
                info!(
                    "player {}: no capture yet, SDL mapping {}",
                    slot.player, fallback.note
                );
                (fallback.line, Some(fallback.note), fallback.provisional)
            }
            None => (String::new(), None, false),
        }
    } else {
        (stored, None, false)
    };
    if provisional {
        info!(
            "player {}: worked out its files for now; again once SDL answers",
            slot.player
        );
    } else {
        info!("player {}: worked out its files", slot.player);
    }
    let published = emulators::Published {
        player: slot.player,
        guid: emit::virtual_guid(slot.player, identity),
        name: emit::virtual_name(slot.player),
        keys: facts.keys.clone(),
        axes: facts.axes.clone(),
        sdl_line: line.clone(),
    };
    Derived {
        from,
        // A guess made while SDL is asked is not kept: when it answers, the
        // next write works this player out again and gets the real line.
        sound: (xbox || !facts.keys.is_empty()) && !provisional,
        provisional,
        profile,
        line,
        note,
        published,
    }
}

/// Where a launch's two files go, and which seats it has waiting.
#[derive(Debug, Clone, Copy)]
pub struct Launch<'a> {
    pub config: &'a Path,
    pub args: &'a Path,
    /// Seats published for the launch that nobody has taken. They are written
    /// like any other player, so a game can bind ports nobody is sitting at.
    pub reserved: &'a [u32],
}

/// Write every file the roster implies.
pub fn write_all(
    slots: &[Slot],
    virtual_paths: &BTreeMap<u32, String>,
    mode: IdentityMode,
    launch: Launch<'_>,
    last: Option<&runtime::Game>,
    keyboard: Option<u32>,
    cache: &mut Cache,
) -> Written {
    let (launch_config_path, launch_args_path) = (launch.config, launch.args);
    let seated: Vec<u32> = slots.iter().map(|slot| slot.player).collect();
    let reserved: Vec<u32> = launch
        .reserved
        .iter()
        .copied()
        .filter(|player| !seated.contains(player))
        .collect();
    let console = last.map(|game| game.console.as_str()).unwrap_or("");
    let game = last.map(|game| game.key.as_str()).unwrap_or("");
    let context = last.map(|game| game.title.as_str()).unwrap_or("");
    let identities: BTreeMap<u32, Identity> = slots
        .iter()
        .map(|slot| (slot.player, identity_of(&slot.pad, slot.player, mode)))
        .collect();
    let players: Vec<u32> = slots.iter().map(|slot| slot.player).collect();

    // Under the 360 identity every consumer describes the clone's layout,
    // which is the same for every pad, rather than the pad behind it.
    let xbox = mode.is_xbox_layout();
    let scope = (console.to_owned(), game.to_owned(), context.to_owned());
    let mut previous = std::mem::take(&mut cache.players);
    let derived: BTreeMap<u32, Derived> = slots
        .iter()
        .map(|slot| {
            let identity = identities[&slot.player];
            let from = Source {
                signature: format!(
                    "{}|{}|{}",
                    profiles::signature_of(&slot.pad),
                    slot.pad.phys,
                    slot.pad.uniq
                ),
                profile: std::fs::read_to_string(profiles::path_for(&slot.pad, None)).ok(),
                identity,
                xbox,
                scope: scope.clone(),
            };
            if let Some(kept) = previous.remove(&slot.player) {
                if kept.from == from {
                    return (slot.player, kept);
                }
            }
            (
                slot.player,
                derive(slot, identity, from, xbox, console, game, context),
            )
        })
        .collect();
    let profiles_out: BTreeMap<u32, String> = derived
        .iter()
        .map(|(player, one)| (*player, one.profile.clone()))
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
    let mut config =
        retroarch::launch_config(&players, virtual_paths, &order, &facts, emit::virtual_name);
    let managed_sorted: Vec<u32> = managed.keys().copied().collect();
    config.push_str(&retroarch::keyboard_config(&managed_sorted, keyboard));
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
        let Some(one) = derived.get(&slot.player) else {
            continue;
        };
        if !one.line.is_empty() {
            lines.insert(slot.player, one.line.clone());
        }
        if let Some(note) = one.note.as_ref() {
            notes.insert(slot.player, note.clone());
        }
        published.push(one.published.clone());
    }
    for player in &reserved {
        let Some(identity) = xbox_identity(mode, *player) else {
            continue;
        };
        let facts = xbox_facts();
        let line = sdl_line_for(*player, identity, &padmap_core::xbox::bindings(), &facts);
        if !line.is_empty() {
            lines.insert(*player, line.clone());
        }
        published.push(emulators::Published {
            player: *player,
            guid: emit::virtual_guid(*player, identity),
            name: emit::virtual_name(*player),
            keys: facts.keys.clone(),
            axes: facts.axes.clone(),
            sdl_line: line,
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
    let wrote = emulators::publish(&published, &emulators::Destinations::default(), keyboard);
    for (target, why) in &wrote.skipped {
        info!("{target}: not written ({why})");
    }

    // A player whose pad could not be read is left out, so the next join has
    // another go rather than keeping a mapping guessed from nothing.
    let awaiting_sdl = derived.values().any(|one| one.provisional);
    cache.players = derived.into_iter().filter(|(_, one)| one.sound).collect();
    Written {
        sdl_lines: lines.into_values().collect(),
        awaiting_sdl,
    }
}

/// Get or create a profile for a pad, preserving existing identity.
fn profile_for(pad: &Pad) -> Profile {
    let mut profile = profiles::load(pad, None).unwrap_or_default();
    profile.signature = profiles::signature_of(pad);
    profile.name = crate::clean(&pad.name);
    profile
}

/// Save a mapping capture under a scope.
pub fn store_mapping(
    pad: &Pad,
    layout_id: &str,
    bindings: &BTreeMap<Control, Binding>,
    scope: &str,
) -> Vec<String> {
    let mut profile = profile_for(pad);
    if profile.icon.is_empty() && scope.is_empty() && padmap_core::icons::known(layout_id) {
        profile.icon = layout_id.to_owned();
    }
    // A control's second inputs survive a rewrite of its first, unless the new
    // capture put that input on some control as a first: then it is spoken for.
    let buttons: BTreeMap<String, Binding> = bindings
        .iter()
        .map(|(control, binding)| (control.to_string(), *binding))
        .collect();
    let extra: BTreeMap<String, Vec<Binding>> = profile
        .mappings
        .get(scope)
        .filter(|existing| existing.layout == layout_id)
        .map(|existing| existing.extra.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|(control, _)| buttons.contains_key(control))
        .map(|(control, twins)| {
            let kept: Vec<Binding> = twins
                .into_iter()
                .filter(|twin| !buttons.values().any(|primary| primary == twin))
                .collect();
            (control, kept)
        })
        .filter(|(_, twins)| !twins.is_empty())
        .collect();
    profile.record(
        scope,
        padmap_core::profile::Mapping {
            buttons,
            extra,
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

/// The capture already filed under exactly this scope, if it was made for this layout.
///
/// A run seeded from it refines the mapping instead of replacing it; a
/// capture under another layout is about to be replaced wholesale, so there
/// is nothing to carry over.
/// One more input for one control, beside the one it has; the first input a
/// control ever gets becomes its binding.
pub fn add_binding(pad: &Pad, layout_id: &str, control: Control, binding: Binding, scope: &str) {
    let mut profile = profile_for(pad);
    let scope_key = if scope.is_empty() {
        padmap_core::scope::UNIVERSAL.to_owned()
    } else {
        scope.to_owned()
    };
    let mut mapping = profile
        .mappings
        .get(&scope_key)
        .cloned()
        .unwrap_or_default();
    if mapping.layout.is_empty() {
        mapping.layout = layout_id.to_owned();
    }
    mapping.add(&control.to_string(), binding);
    profile.record(&scope_key, mapping);
    if let Err(error) = profiles::save(&profile, None) {
        warn!("could not save the profile for {}: {error}", pad.name);
    }
}

pub fn stored_mapping(pad: &Pad, scope: &str, layout_id: &str) -> BTreeMap<Control, Binding> {
    profiles::load(pad, None)
        .and_then(|profile| profile.mappings.get(scope).cloned())
        .filter(|mapping| mapping.layout == layout_id)
        .map(|mapping| mapping.resolved())
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn source(profile: Option<&str>) -> Source {
        Source {
            signature: "1209:0001:Pad|phys|uniq".to_owned(),
            profile: profile.map(str::to_owned),
            identity: Identity {
                bustype: 0x06,
                vendor: 0x1209,
                product: 0x0001,
                version: 1,
            },
            xbox: false,
            scope: (String::new(), String::new(), String::new()),
        }
    }

    #[test]
    fn a_profile_stored_or_forgotten_is_a_different_answer() {
        // Nothing tells the daemon when the wizard stores one or `forget_pad`
        // removes it, and the CLI writes the same file from another process --
        // so the file's contents are what says whether a cached answer stands.
        let none = source(None);
        let stored = source(Some(r#"{"mappings":{"":{"buttons":{"a":{}}}}}"#));
        let other = source(Some(r#"{"mappings":{"":{"buttons":{"b":{}}}}}"#));
        assert_ne!(none, stored, "a capture was reused past");
        assert_ne!(stored, none, "a forget was reused past");
        assert_ne!(stored, other, "a remap was reused past");
        assert_eq!(
            stored,
            source(Some(r#"{"mappings":{"":{"buttons":{"a":{}}}}}"#))
        );
    }

    #[test]
    fn two_units_of_one_model_are_not_one_answer() {
        let mut one = source(None);
        let mut two = source(None);
        one.signature = "1209:0001:Pad|usb-0000:00:14.0-1|".to_owned();
        two.signature = "1209:0001:Pad|usb-0000:00:14.0-2|".to_owned();
        assert_ne!(one, two, "one pad's files stood in for another's");
    }
}
