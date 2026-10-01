//! Everything written to disk when the roster changes.

use std::collections::BTreeMap;

use danstick_core::binding::Binding;
use danstick_core::emit::{self, Identity};
use danstick_core::fields::Fields;
use danstick_core::profile::Profile;
use danstick_core::sdl::AxisSpan;
use danstick_core::{guess, sdl, Control};
use danstick_input::clone::{self, IdentityMode};
use danstick_input::pad::Pad;
use danstick_input::{artefacts, emulators, profiles, runtime, sdlprobe, triton};
use evdev::Device;
use log::{info, warn};

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

/// The 360 identity a player's clone wears under `mode`, or `None` when it depends on the pad.
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
        keys: danstick_core::xbox::KEYS.to_vec(),
        axes: danstick_core::xbox::axis_codes(),
        spans: danstick_core::xbox::spans(),
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
    let danstick_own = Identity {
        bustype: 0x06,
        vendor: clone::DANSTICK_VID,
        product: clone::DANSTICK_PID,
        version: emit::version_for(player),
    };
    if let Some(identity) = xbox_identity(mode, player) {
        return identity;
    }
    if triton::owns(pad) {
        return match mode {
            IdentityMode::Danstick => danstick_own,
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
        Err(_) => danstick_own,
    }
}

pub fn resolved(pad: &Pad, console: &str, game: &str) -> (String, danstick_core::profile::Mapping) {
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

/// The stored mapping's line for what is being played, or empty.
pub fn stored_sdl_line(
    player: u32,
    pad: &Pad,
    identity: Identity,
    facts: &PadFacts,
    console: &str,
    game: &str,
) -> String {
    let bindings = resolved(pad, console, game).1.resolved();
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

/// Mapping already on disk or in SDL's database for this GUID.
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

/// What the writers produced.
#[derive(Debug, Default, Clone)]
pub struct Written {
    pub sdl_lines: Vec<String>,
    /// Whether some seated player's mapping was guessed while SDL was still answering.
    pub awaiting_sdl: bool,
}

/// What one player's files were written from, and what they came out as.
#[derive(Debug, Clone)]
struct Derived {
    from: Source,
    /// Whether the pad's capabilities could be read.
    sound: bool,
    /// Guessed while SDL was being asked about this pad.
    provisional: bool,
    line: String,
    note: Option<String>,
    published: emulators::Published,
}

/// Everything a player's files depend on, so a stale one is never reused.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Source {
    signature: String,
    /// The stored profile as it was; nothing else tells the daemon when it changes.
    profile: Option<String>,
    identity: Identity,
    xbox: bool,
    scope: (String, String),
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

/// Works one player's files out from scratch: capabilities and SDL's GUID lookup.
fn derive(
    slot: &Slot,
    identity: Identity,
    from: Source,
    xbox: bool,
    console: &str,
    game: &str,
) -> Derived {
    let facts = if xbox {
        xbox_facts()
    } else {
        pad_facts(&slot.pad)
    };
    let stored = if xbox {
        sdl_line_for(
            slot.player,
            identity,
            &danstick_core::xbox::bindings(),
            &facts,
        )
    } else {
        stored_sdl_line(slot.player, &slot.pad, identity, &facts, console, game)
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
        // A guess made while SDL answers is not kept, so the next write redoes it.
        sound: (xbox || !facts.keys.is_empty()) && !provisional,
        provisional,
        line,
        note,
        published,
    }
}

/// Write every file the roster implies.
pub fn write_all(
    slots: &[Slot],
    mode: IdentityMode,
    reserved: &[u32],
    last: Option<&runtime::Game>,
    keyboard: Option<u32>,
    cache: &mut Cache,
) -> Written {
    let seated: Vec<u32> = slots.iter().map(|slot| slot.player).collect();
    let reserved: Vec<u32> = reserved
        .iter()
        .copied()
        .filter(|player| !seated.contains(player))
        .collect();
    let console = last.map(|game| game.console.as_str()).unwrap_or("");
    let game = last.map(|game| game.key.as_str()).unwrap_or("");
    let identities: BTreeMap<u32, Identity> = slots
        .iter()
        .map(|slot| (slot.player, identity_of(&slot.pad, slot.player, mode)))
        .collect();

    // Under the 360 identity every consumer describes the clone's layout, not the pad's.
    let xbox = mode.is_xbox_layout();
    let scope = (console.to_owned(), game.to_owned());
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
                derive(slot, identity, from, xbox, console, game),
            )
        })
        .collect();
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
        let line = sdl_line_for(*player, identity, &danstick_core::xbox::bindings(), &facts);
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
        vendor: clone::DANSTICK_VID,
        product: clone::DANSTICK_PID,
        version: clone::DANSTICK_VERSION,
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

    // A player whose pad could not be read is left out, so the next join tries again.
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
    if profile.icon.is_empty() && scope.is_empty() && danstick_core::icons::known(layout_id) {
        profile.icon = layout_id.to_owned();
    }
    // A control's second inputs survive unless the new capture claims them as a first.
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
        danstick_core::profile::Mapping {
            buttons,
            extra,
            layout: layout_id.to_owned(),
            name: String::new(),
        },
    );
    if let Err(error) = profiles::save(&profile, None) {
        warn!("could not save the profile for {}: {error}", pad.name);
    }
    danstick_core::layout::get(layout_id)
        .controls
        .iter()
        .filter(|control| !bindings.contains_key(&control.canonical))
        .map(|control| control.canonical.to_string())
        .collect()
}

/// Save axis calibration, preserving everything else.
pub fn store_calibration(
    pad: &Pad,
    axes: BTreeMap<u16, danstick_core::calibration::AxisCalibration>,
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
pub fn store_tuning(pad: &Pad, tuning: danstick_core::tuning::Tuning) -> std::io::Result<()> {
    let mut profile = profile_for(pad);
    profile.tuning = tuning;
    profiles::save(&profile, None).map(|_| ())
}

/// Get the user's tuning settings for a pad, or defaults.
pub fn tuning_for(pad: &Pad) -> danstick_core::tuning::Tuning {
    profiles::load(pad, None)
        .map(|profile| profile.tuning)
        .unwrap_or_default()
}

/// The icon a pad should display.
pub fn icon_for(pad: &Pad, overrides: &BTreeMap<String, String>) -> &'static str {
    let stored = profiles::load(pad, None).map(|profile| profile.icon);
    danstick_core::icons::for_pad(pad.vid, pad.pid, &pad.name, stored.as_deref(), overrides)
}

/// One more input for a control, beside the one it has; the first input it gets is its binding.
pub fn add_binding(pad: &Pad, layout_id: &str, control: Control, binding: Binding, scope: &str) {
    let mut profile = profile_for(pad);
    let scope_key = if scope.is_empty() {
        danstick_core::scope::UNIVERSAL.to_owned()
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
            scope: (String::new(), String::new()),
        }
    }

    #[test]
    fn a_profile_stored_or_forgotten_is_a_different_answer() {
        // Nothing tells the daemon of a store, forget, or another process's rewrite.
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
