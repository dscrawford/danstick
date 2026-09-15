//! Per-device profiles: what was measured and captured for one controller.
//!
//! Everything device-specific padmap needs -- which icon to show, where a stick
//! actually rests, which button is which -- is learned once and stored under
//! the user's data directory. Nothing here ships a table of known vendor ids,
//! and the hardware is why: vendor 0x0079 is resold in a great many unrelated
//! adapters, so `0079:1879` is an N64 adapter on one machine and a generic pad
//! on the next; and a worn N64 stick rests at 174 on a 0-255 axis, which is a
//! property of one physical controller rather than of a product line.
//!
//! The on-disk shape is the Python's, exactly, including the two keys it writes
//! only for the benefit of a rollback. A profile store is user data that
//! outlives any one version.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::binding::Binding;
use crate::calibration::AxisCalibration;
use crate::scope;

/// One capture: where every control of one layout lives on this pad.
///
/// The unit a scope points at. The layout id is carried *inside* the mapping
/// rather than beside it, because a binding set is only interpretable together
/// with the layout it was captured under -- the whole reason a GameCube pad
/// needs a separate N64 mapping is that the N64 layout asks for different
/// controls and emits different RetroArch keys.
///
/// The id is stored rather than the resolved keys, so correcting a console's
/// key table fixes every profile already captured under it. A wrong key is a
/// button that silently does nothing, which is not something a user will think
/// to re-capture for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mapping {
    /// Canonical control name -> where it lives on this pad.
    ///
    /// Keyed by `String` and not by `Control`, deliberately: this is read off
    /// disk, and a name padmap no longer knows must cost that binding rather
    /// than the whole profile.
    #[serde(default)]
    pub buttons: BTreeMap<String, Binding>,
    #[serde(default)]
    pub layout: String,
    /// What to call this mapping in a list. Empty means "describe it from the
    /// scope", which is what almost every one of them is.
    #[serde(default)]
    pub name: String,
}

impl Mapping {
    /// The bindings, as the enum the rest of the crate uses.
    ///
    /// Names that are not canonical controls are dropped here rather than at
    /// load, so a profile written by a newer padmap still loads on an older one
    /// and keeps everything it does understand.
    pub fn resolved(&self) -> BTreeMap<crate::Control, Binding> {
        self.buttons
            .iter()
            .filter_map(|(name, binding)| name.parse().ok().map(|c| (c, *binding)))
            .collect()
    }

    /// Read one out of whatever a file happens to contain.
    ///
    /// Never fails. Junk in one binding slot costs that binding.
    pub fn from_value(raw: &Value) -> Mapping {
        let Some(object) = raw.as_object() else {
            return Mapping::default();
        };
        let mut buttons = BTreeMap::new();
        if let Some(stored) = object.get("buttons").and_then(Value::as_object) {
            for (control, values) in stored {
                if let Ok(binding) = serde_json::from_value::<Binding>(values.clone()) {
                    buttons.insert(control.clone(), binding);
                }
            }
        }
        Mapping {
            buttons,
            layout: string_at(object.get("layout")),
            name: string_at(object.get("name")),
        }
    }
}

/// Everything learned about one model of controller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub signature: String,
    pub name: String,
    pub icon: String,
    /// evdev ABS code -> calibration.
    pub axes: BTreeMap<u16, AxisCalibration>,
    /// Scope -> capture, keyed by the *physical* controller's signature, so a
    /// mapping follows the controller rather than the player slot it happened
    /// to claim -- which is how SDL's own database loses it, since that is
    /// keyed on "padmap Player N".
    pub mappings: BTreeMap<String, Mapping>,
}

/// What went wrong reading one axis, for a caller that wants to say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedAxis {
    pub code: String,
    pub why: String,
}

impl Profile {
    /// The universal mapping's bindings.
    pub fn buttons(&self) -> BTreeMap<String, Binding> {
        self.mappings
            .get(scope::UNIVERSAL)
            .map(|m| m.buttons.clone())
            .unwrap_or_default()
    }

    /// The universal mapping's layout id.
    pub fn layout(&self) -> &str {
        self.mappings
            .get(scope::UNIVERSAL)
            .map(|m| m.layout.as_str())
            .unwrap_or("")
    }

    /// Whether *any* scope has been captured.
    ///
    /// Not just the universal one: a pad mapped only for N64 has been through
    /// the wizard, and reporting it as unconfigured would offer the wizard
    /// again on every session.
    pub fn has_bindings(&self) -> bool {
        self.mappings.values().any(|m| !m.buttons.is_empty())
    }

    /// The mapping that applies, and the scope it came from.
    ///
    /// Most specific wins. A scope holding an *empty* capture is skipped rather
    /// than matched -- it would otherwise shadow the more general mapping that
    /// does have bindings, which is the one case where "most specific wins" is
    /// not what anybody means.
    pub fn resolve(&self, console: &str, game: &str) -> (String, Mapping) {
        for candidate in scope::order(console, game) {
            if let Some(found) = self.mappings.get(&candidate) {
                if !found.buttons.is_empty() {
                    return (candidate, found.clone());
                }
            }
        }
        (scope::UNIVERSAL.to_owned(), Mapping::default())
    }

    /// File a capture under a scope, seeding the default from the first.
    ///
    /// Someone whose first act is "map this pad for N64 games" would otherwise
    /// end up with no universal mapping at all -- no SDL line for the front-end
    /// to navigate with, and nothing for any other console, so the pad they
    /// just configured would still be driven by a guess everywhere else. A
    /// capture the user performed beats a guess, so the first one becomes the
    /// default too.
    ///
    /// Later captures do not disturb it: once a default exists, saying "and for
    /// N64, this instead" must not silently change every other console.
    pub fn record(&mut self, scope_key: &str, captured: Mapping) {
        self.mappings.insert(scope_key.to_owned(), captured.clone());
        let universal_is_empty = self
            .mappings
            .get(scope::UNIVERSAL)
            .map(|m| m.buttons.is_empty())
            .unwrap_or(true);
        if scope_key != scope::UNIVERSAL && universal_is_empty {
            self.mappings.insert(scope::UNIVERSAL.to_owned(), captured);
        }
    }

    /// The JSON a profile is stored as.
    pub fn to_value(&self) -> Value {
        let universal = self
            .mappings
            .get(scope::UNIVERSAL)
            .cloned()
            .unwrap_or_default();
        let axes: serde_json::Map<String, Value> = self
            .axes
            .iter()
            .map(|(code, cal)| {
                (
                    code.to_string(),
                    serde_json::to_value(cal).unwrap_or(Value::Null),
                )
            })
            .collect();
        let mappings: serde_json::Map<String, Value> = self
            .mappings
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    serde_json::to_value(value).unwrap_or(Value::Null),
                )
            })
            .collect();
        serde_json::json!({
            "signature": self.signature,
            "name": self.name,
            "icon": self.icon,
            "axes": axes,
            "mappings": mappings,
            // The universal mapping is *also* written where it has always
            // been. Nothing padmap ships reads these two keys any more, but a
            // rollback to a build predating scopes then still finds the
            // controller mapped instead of offering the wizard again.
            "layout": universal.layout,
            "buttons": universal.buttons,
        })
    }

    /// Read a profile out of whatever a file happens to contain.
    ///
    /// Never fails, and that is the point. `is_known` is "a profile loaded",
    /// and discovery calls it for every pad, so one damaged file must cost that
    /// controller its settings and not its existence.
    pub fn from_value(raw: &Value) -> (Profile, Vec<RejectedAxis>) {
        // A file can hold valid JSON that is not a profile at all -- null, a
        // list, a bare string -- and every read below assumes an object.
        let empty = serde_json::Map::new();
        let object = raw.as_object().unwrap_or(&empty);

        let mut axes = BTreeMap::new();
        let mut rejected = Vec::new();
        if let Some(stored) = object.get("axes").and_then(Value::as_object) {
            for (code, values) in stored {
                let Ok(number) = code.parse::<u16>() else {
                    continue;
                };
                // Junk in one axis slot costs that axis, not the profile.
                let Ok(cal) = serde_json::from_value::<AxisCalibration>(values.clone()) else {
                    continue;
                };
                if !cal.fits() {
                    // Rejected, not clamped, and deliberately. A range wider
                    // than an evdev value can hold is not a measurement that
                    // overshot -- no stick reports 2^40 -- it is a hand-edit or
                    // a save cut short, and the numbers beside it are worth
                    // nothing either. Clamping keeps scaling every reading
                    // against nonsense, so the user trades a dead daemon for a
                    // stick that reads permanently slammed into a corner, which
                    // a front-end acts on immediately. Dropping the axis falls
                    // back to forwarding it verbatim, which is what an
                    // uncalibrated pad already does and is the one behaviour
                    // here known to work.
                    rejected.push(RejectedAxis {
                        code: code.clone(),
                        why: format!(
                            "centre {}, range {}..{} is outside what an evdev value can carry",
                            cal.center, cal.minimum, cal.maximum
                        ),
                    });
                    continue;
                }
                axes.insert(number, cal);
            }
        }

        let mut mappings: BTreeMap<String, Mapping> = BTreeMap::new();
        if let Some(stored) = object.get("mappings").and_then(Value::as_object) {
            for (key, values) in stored {
                if values.is_object() {
                    mappings.insert(key.clone(), Mapping::from_value(values));
                }
            }
        }

        // Migration, in place and without a version number. Every profile
        // written before scopes carries flat `buttons`/`layout` and no
        // `mappings`, and that pair is exactly the universal scope -- it was
        // the only scope there was. Doing it here rather than in a one-shot
        // upgrade pass means a profile is migrated the first time it is looked
        // at, including one restored from a backup years later, and there is no
        // separate code path that can be forgotten.
        if !mappings.contains_key(scope::UNIVERSAL) {
            let legacy = Mapping::from_value(&serde_json::json!({
                "buttons": object.get("buttons").cloned().unwrap_or(Value::Null),
                "layout": object.get("layout").cloned().unwrap_or(Value::Null),
            }));
            if !legacy.buttons.is_empty() || !legacy.layout.is_empty() {
                mappings.insert(scope::UNIVERSAL.to_owned(), legacy);
            }
        }

        let profile = Profile {
            signature: string_at(object.get("signature")),
            name: string_at(object.get("name")),
            icon: string_at(object.get("icon")),
            axes,
            mappings,
        };
        (profile, rejected)
    }
}

fn string_at(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        // `str(raw.get(...))` in the Python, which stringifies whatever it
        // finds. Anything that is not a string here is a damaged file, and an
        // empty answer is the one that keeps the rest of the profile usable.
        _ => String::new(),
    }
}

/// Python's `str.isprintable`.
///
/// "Nonprintable characters are those characters defined in the Unicode
/// character database as 'Other' or 'Separator', excepting the ASCII space."
/// Reproduced rather than approximated because the result is a profile's
/// *filename*: get it wrong and every profile a user already has is orphaned,
/// silently, since a missing profile reads as "never configured" and the wizard
/// simply opens again.
pub fn printable(character: char) -> bool {
    use unicode_general_category::{get_general_category, GeneralCategory::*};
    if character == ' ' {
        return true;
    }
    !matches!(
        get_general_category(character),
        Control
            | Format
            | Surrogate
            | PrivateUse
            | Unassigned
            | LineSeparator
            | ParagraphSeparator
            | SpaceSeparator
    )
}

/// Stable identity for a *model* of controller, across replugs.
///
/// Deliberately coarser than a single physical pad: two identical controllers
/// should share a profile, and nothing padmap can read distinguishes them
/// anyway.
pub fn signature(vid: u16, pid: u16, name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| printable(*c)).collect();
    format!("{vid:04x}:{pid:04x}:{}", cleaned.trim())
}

/// The file a signature is stored in.
///
/// Truncated to 120 characters *before* the suffix, as the Python did, so a
/// pad with an absurd name cannot produce a filename the filesystem refuses.
pub fn filename(signature: &str) -> String {
    let mut out = String::with_capacity(signature.len());
    let mut in_run = false;
    for character in signature.chars() {
        let keep = character.is_ascii_alphanumeric()
            || character == '.'
            || character == '_'
            || character == '-';
        if keep {
            out.push(character);
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    let truncated: String = out.chars().take(120).collect();
    format!("{truncated}.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::Binding;

    fn capture(control: &str, index: i32) -> Mapping {
        Mapping {
            buttons: [(control.to_owned(), Binding::button(index))]
                .into_iter()
                .collect(),
            layout: "n64".to_owned(),
            name: String::new(),
        }
    }

    #[test]
    fn a_signature_is_the_ids_and_the_printable_name() {
        assert_eq!(
            signature(0x0079, 0x1879, "N64 Adapter"),
            "0079:1879:N64 Adapter"
        );
    }

    #[test]
    fn a_control_character_in_a_name_is_stripped_before_the_name_is_trimmed() {
        // This machine reports an adapter whose name begins 0x18. If the strip
        // and the trim happen in the wrong order the leading space survives
        // into the signature, and therefore into the filename.
        assert_eq!(signature(1, 2, "\u{18} Pad"), "0001:0002:Pad");
        assert_eq!(signature(1, 2, "  Pad  "), "0001:0002:Pad");
    }

    #[test]
    fn a_space_inside_a_name_is_printable_and_kept() {
        // The one exception in Python's definition: ASCII space is printable
        // although its category is a separator.
        assert!(printable(' '));
        assert_eq!(
            signature(1, 2, "Pro Controller"),
            "0001:0002:Pro Controller"
        );
    }

    #[test]
    fn the_unprintable_categories_are_the_ones_python_names() {
        for character in [
            '\u{0}', '\u{1b}', '\u{7f}', '\u{200b}', '\u{feff}', '\u{2028}', '\u{2029}', '\u{a0}',
            '\u{e000}',
        ] {
            assert!(!printable(character), "{character:?} must not be printable");
        }
        for character in ['a', 'Z', '0', '-', 'é', '日', '🎮'] {
            assert!(printable(character), "{character:?} must be printable");
        }
    }

    #[test]
    fn a_filename_keeps_only_what_a_filesystem_is_happy_with() {
        assert_eq!(
            filename("0079:1879:N64 Adapter"),
            "0079_1879_N64_Adapter.json"
        );
        assert_eq!(filename("a/b\\c"), "a_b_c.json");
    }

    #[test]
    fn runs_of_forbidden_characters_collapse_to_one_underscore() {
        assert_eq!(filename("a:::b"), "a_b.json");
        assert_eq!(filename("a   b"), "a_b.json");
    }

    #[test]
    fn an_absurd_name_cannot_produce_a_filename_the_filesystem_refuses() {
        let long = filename(&"x".repeat(500));
        assert_eq!(long.len(), 125, "120 characters plus .json");
        assert!(long.ends_with(".json"));
    }

    #[test]
    fn resolution_prefers_the_most_specific_scope_that_has_bindings() {
        let mut profile = Profile::default();
        profile.record(scope::UNIVERSAL, capture("a", 1));
        profile.record("console:n64", capture("a", 2));
        profile.record("game:n64/mario", capture("a", 3));

        assert_eq!(profile.resolve("n64", "n64/mario").0, "game:n64/mario");
        assert_eq!(profile.resolve("n64", "").0, "console:n64");
        assert_eq!(profile.resolve("", "").0, scope::UNIVERSAL);
        assert_eq!(profile.resolve("snes", "").0, scope::UNIVERSAL);
    }

    #[test]
    fn an_empty_capture_does_not_shadow_a_general_one_that_has_bindings() {
        // The one case where "most specific wins" is not what anybody means.
        let mut profile = Profile::default();
        profile.record(scope::UNIVERSAL, capture("a", 1));
        profile
            .mappings
            .insert("console:n64".to_owned(), Mapping::default());
        let (found, mapping) = profile.resolve("n64", "");
        assert_eq!(found, scope::UNIVERSAL);
        assert_eq!(mapping.buttons["a"], Binding::button(1));
    }

    #[test]
    fn nothing_captured_resolves_to_an_empty_universal_mapping() {
        let profile = Profile::default();
        let (found, mapping) = profile.resolve("n64", "n64/mario");
        assert_eq!(found, scope::UNIVERSAL);
        assert!(mapping.buttons.is_empty());
    }

    #[test]
    fn the_first_capture_becomes_the_default_whatever_scope_it_was_for() {
        // Otherwise someone whose first act is "map this for N64" has no SDL
        // line for the front-end to navigate with.
        let mut profile = Profile::default();
        profile.record("console:n64", capture("a", 7));
        assert_eq!(profile.buttons()["a"], Binding::button(7));
        assert_eq!(profile.resolve("", "").0, scope::UNIVERSAL);
    }

    #[test]
    fn a_later_capture_does_not_disturb_an_existing_default() {
        let mut profile = Profile::default();
        profile.record(scope::UNIVERSAL, capture("a", 1));
        profile.record("console:n64", capture("a", 9));
        assert_eq!(
            profile.buttons()["a"],
            Binding::button(1),
            "the default moved"
        );
        assert_eq!(
            profile.resolve("n64", "").1.buttons["a"],
            Binding::button(9)
        );
    }

    #[test]
    fn a_pad_mapped_only_for_one_console_counts_as_configured() {
        // Reporting it unconfigured would offer the wizard again every session.
        let mut profile = Profile::default();
        assert!(!profile.has_bindings());
        profile.record("console:n64", capture("a", 1));
        assert!(profile.has_bindings());
    }

    #[test]
    fn a_profile_that_is_not_an_object_reads_as_an_empty_one() {
        // is_known() is "a profile loaded", and discovery calls it for every
        // pad, so one damaged file must not cost the controller its existence.
        for raw in [
            Value::Null,
            serde_json::json!([1, 2]),
            serde_json::json!("text"),
        ] {
            let (profile, rejected) = Profile::from_value(&raw);
            assert_eq!(profile, Profile::default());
            assert!(rejected.is_empty());
        }
    }

    #[test]
    fn junk_in_one_axis_slot_costs_that_axis_and_nothing_else() {
        let raw = serde_json::json!({
            "signature": "s",
            "axes": {
                "0": {"center": 128, "min": 0, "max": 255},
                "1": "not an object",
                "2": {"center": 1},
                "notanumber": {"center": 128, "min": 0, "max": 255},
            }
        });
        let (profile, _) = Profile::from_value(&raw);
        assert_eq!(profile.axes.len(), 1);
        assert!(profile.axes.contains_key(&0));
    }

    #[test]
    fn an_axis_no_evdev_value_can_carry_is_reported_and_dropped() {
        let raw = serde_json::json!({
            "axes": {"0": {"center": 0, "min": -2147483648, "max": 2147483647}}
        });
        let (profile, rejected) = Profile::from_value(&raw);
        assert_eq!(profile.axes.len(), 1, "the i32 extremes are carryable");
        assert!(rejected.is_empty());
    }

    #[test]
    fn a_profile_written_before_scopes_is_migrated_on_the_way_in() {
        // Flat buttons/layout and no mappings: that pair *is* the universal
        // scope, because it was the only scope there was.
        let raw = serde_json::json!({
            "signature": "0079:1879:Pad",
            "layout": "n64",
            "buttons": {"a": {"kind": "button", "index": 1}},
        });
        let (profile, _) = Profile::from_value(&raw);
        assert_eq!(profile.layout(), "n64");
        assert_eq!(profile.buttons()["a"], Binding::button(1));
        assert_eq!(profile.resolve("", "").0, scope::UNIVERSAL);
    }

    #[test]
    fn a_legacy_profile_with_nothing_in_it_gains_no_empty_universal_mapping() {
        let (profile, _) = Profile::from_value(&serde_json::json!({"signature": "s"}));
        assert!(profile.mappings.is_empty());
        assert!(!profile.has_bindings());
    }

    #[test]
    fn a_stored_profile_round_trips_through_its_json() {
        let mut profile = Profile {
            signature: "0079:1879:Pad".to_owned(),
            name: "Pad".to_owned(),
            icon: "n64".to_owned(),
            axes: [(0u16, AxisCalibration::new(174, 0, 255).with_reach(20, 250))]
                .into_iter()
                .collect(),
            mappings: BTreeMap::new(),
        };
        profile.record(scope::UNIVERSAL, capture("a", 1));
        profile.record("console:n64", capture("b", 2));

        let value = profile.to_value();
        let (back, rejected) = Profile::from_value(&value);
        assert!(rejected.is_empty());
        assert_eq!(back, profile);
    }

    #[test]
    fn the_stored_axes_use_the_key_names_a_users_file_already_has() {
        let profile = Profile {
            axes: [(3u16, AxisCalibration::new(128, 0, 255))]
                .into_iter()
                .collect(),
            ..Profile::default()
        };
        let value = profile.to_value();
        let axis = &value["axes"]["3"];
        assert!(
            axis.get("min").is_some(),
            "renaming this orphans every stored axis"
        );
        assert!(axis.get("max").is_some());
        assert_eq!(axis["center"], 128);
    }

    #[test]
    fn the_rollback_keys_are_written_beside_the_scoped_ones() {
        let mut profile = Profile::default();
        profile.record(scope::UNIVERSAL, capture("a", 1));
        let value = profile.to_value();
        assert_eq!(value["layout"], "n64");
        assert_eq!(value["buttons"]["a"]["index"], 1);
        assert!(value["mappings"][""]["buttons"]["a"].is_object());
    }

    #[test]
    fn a_control_name_padmap_no_longer_knows_costs_that_binding_only() {
        let mapping = Mapping::from_value(&serde_json::json!({
            "buttons": {
                "a": {"kind": "button", "index": 1},
                "guide": {"kind": "button", "index": 2},
            }
        }));
        assert_eq!(mapping.buttons.len(), 2, "kept on disk");
        assert_eq!(mapping.resolved().len(), 1, "but only one is a control");
    }
}

#[cfg(test)]
mod divergences {
    //! Where this deliberately does not do what the Python did.

    use super::*;

    #[test]
    fn a_field_that_is_not_a_string_reads_as_empty_rather_than_as_its_repr() {
        // The Python wrote `str(raw.get("name"))`, which turns None into the
        // literal "None" and ["x"] into "['x']" -- and then writes that back to
        // the user's file on the next save, where it is indistinguishable from
        // a controller actually called None. Only a hand-edited or truncated
        // file gets here, and for all three fields an empty answer is honest
        // about not knowing: the filename comes from the pad rather than from
        // `signature`, an empty `icon` falls back to the guess, and an empty
        // `name` is display-only.
        let raw = serde_json::json!({"signature": 5, "name": null, "icon": ["x"]});
        let (profile, _) = Profile::from_value(&raw);
        assert_eq!(profile.signature, "");
        assert_eq!(profile.name, "");
        assert_eq!(profile.icon, "");
    }
}
