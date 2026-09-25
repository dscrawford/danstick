//! Per-device profiles: measured and stored per-controller calibration, icons, and button mappings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::binding::Binding;
use crate::calibration::AxisCalibration;
use crate::scope;
use crate::tuning::Tuning;

/// One capture: where every control of one layout lives on this pad.
///
/// On disk a control's entry is one binding, or a list whose first entry is
/// what everything downstream reads and whose rest are second inputs for the
/// same control. A file with no lists is byte-for-byte what it always was.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mapping {
    /// Canonical control name -> where it lives on this pad.
    pub buttons: BTreeMap<String, Binding>,
    /// Canonical control name -> its second (third, ...) inputs, if any.
    pub extra: BTreeMap<String, Vec<Binding>>,
    pub layout: String,
    /// Empty means describe it from the scope.
    pub name: String,
}

impl Mapping {
    /// The bindings, as the enum the rest of the crate uses.
    pub fn resolved(&self) -> BTreeMap<crate::Control, Binding> {
        self.buttons
            .iter()
            .filter_map(|(name, binding)| name.parse().ok().map(|c| (c, *binding)))
            .collect()
    }

    /// The second inputs, as the enum the rest of the crate uses.
    pub fn resolved_extra(&self) -> BTreeMap<crate::Control, Vec<Binding>> {
        self.extra
            .iter()
            .filter(|(_, twins)| !twins.is_empty())
            .filter_map(|(name, twins)| name.parse().ok().map(|c| (c, twins.clone())))
            .collect()
    }

    /// Every input a control has, primary first.
    pub fn all(&self, control: &str) -> Vec<Binding> {
        let mut out: Vec<Binding> = self.buttons.get(control).copied().into_iter().collect();
        out.extend(self.extra.get(control).cloned().unwrap_or_default());
        out
    }

    /// Add an input to a control: the first becomes its binding, the rest its twins.
    pub fn add(&mut self, control: &str, binding: Binding) {
        match self.buttons.get(control) {
            None => {
                self.buttons.insert(control.to_owned(), binding);
            }
            Some(primary) if *primary == binding => {}
            Some(_) => {
                let twins = self.extra.entry(control.to_owned()).or_default();
                if !twins.contains(&binding) {
                    twins.push(binding);
                }
            }
        }
    }

    /// The `buttons` object as it is written: one binding, or a list with the twins.
    pub fn buttons_value(&self) -> Value {
        let mut out = serde_json::Map::new();
        for (control, binding) in &self.buttons {
            let twins = self.extra.get(control).filter(|t| !t.is_empty());
            let value = match twins {
                None => serde_json::to_value(binding).unwrap_or(Value::Null),
                Some(twins) => Value::Array(
                    std::iter::once(binding)
                        .chain(twins.iter())
                        .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
                        .collect(),
                ),
            };
            out.insert(control.clone(), value);
        }
        Value::Object(out)
    }

    /// Never fails; junk in one binding slot costs that binding only.
    pub fn from_value(raw: &Value) -> Mapping {
        let Some(object) = raw.as_object() else {
            return Mapping::default();
        };
        let mut buttons = BTreeMap::new();
        let mut extra: BTreeMap<String, Vec<Binding>> = BTreeMap::new();
        if let Some(stored) = object.get("buttons").and_then(Value::as_object) {
            for (control, values) in stored {
                let listed: Vec<Binding> = match values {
                    Value::Array(items) => items
                        .iter()
                        .filter_map(|item| serde_json::from_value::<Binding>(item.clone()).ok())
                        .collect(),
                    other => serde_json::from_value::<Binding>(other.clone())
                        .ok()
                        .into_iter()
                        .collect(),
                };
                let mut listed = listed.into_iter();
                if let Some(first) = listed.next() {
                    buttons.insert(control.clone(), first);
                    let twins: Vec<Binding> = listed.filter(|b| *b != first).collect();
                    if !twins.is_empty() {
                        extra.insert(control.clone(), twins);
                    }
                }
            }
        }
        Mapping {
            buttons,
            extra,
            layout: string_at(object.get("layout")),
            name: string_at(object.get("name")),
        }
    }
}

impl Serialize for Mapping {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = serde_json::json!({
            "buttons": self.buttons_value(),
            "layout": self.layout,
            "name": self.name,
        });
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Mapping {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(Mapping::from_value(&value))
    }
}

/// Everything learned about one model of controller.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    pub signature: String,
    pub name: String,
    pub icon: String,
    /// evdev ABS code -> calibration.
    pub axes: BTreeMap<u16, AxisCalibration>,
    /// Scope -> capture.
    pub mappings: BTreeMap<String, Mapping>,
    /// User tuning: deadzones, debounce, ignored axes/buttons.
    pub tuning: Tuning,
}

/// What went wrong reading one axis, for a caller that wants to say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedAxis {
    pub code: String,
    pub why: String,
}

impl Profile {
    pub fn buttons(&self) -> BTreeMap<String, Binding> {
        self.mappings
            .get(scope::UNIVERSAL)
            .map(|m| m.buttons.clone())
            .unwrap_or_default()
    }

    pub fn layout(&self) -> &str {
        self.mappings
            .get(scope::UNIVERSAL)
            .map(|m| m.layout.as_str())
            .unwrap_or("")
    }

    /// Whether any scope has been captured.
    pub fn has_bindings(&self) -> bool {
        self.mappings.values().any(|m| !m.buttons.is_empty())
    }

    /// Most specific scope wins; empty scopes skipped.
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

    /// File a capture under a scope, seeding universal from the first.
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
        let mut out = serde_json::json!({
            "signature": self.signature,
            "name": self.name,
            "icon": self.icon,
            "axes": axes,
            "mappings": mappings,
            "layout": universal.layout,
            "buttons": universal.buttons_value(),
        });
        if !self.tuning.is_default() {
            if let Ok(tuning) = serde_json::to_value(&self.tuning) {
                out["tuning"] = tuning;
            }
        }
        out
    }

    /// Never fails: damaged files cost settings, not existence.
    pub fn from_value(raw: &Value) -> (Profile, Vec<RejectedAxis>) {
        let empty = serde_json::Map::new();
        let object = raw.as_object().unwrap_or(&empty);

        let mut axes = BTreeMap::new();
        let mut rejected = Vec::new();
        if let Some(stored) = object.get("axes").and_then(Value::as_object) {
            for (code, values) in stored {
                let Ok(number) = code.parse::<u16>() else {
                    continue;
                };
                let Ok(cal) = serde_json::from_value::<AxisCalibration>(values.clone()) else {
                    continue;
                };
                if !cal.fits() {
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

        if !mappings.contains_key(scope::UNIVERSAL) {
            let legacy = Mapping::from_value(&serde_json::json!({
                "buttons": object.get("buttons").cloned().unwrap_or(Value::Null),
                "layout": object.get("layout").cloned().unwrap_or(Value::Null),
            }));
            if !legacy.buttons.is_empty() || !legacy.layout.is_empty() {
                mappings.insert(scope::UNIVERSAL.to_owned(), legacy);
            }
        }

        // Junk costs tuning, not profile.
        let tuning = object
            .get("tuning")
            .and_then(|raw| serde_json::from_value(raw.clone()).ok())
            .unwrap_or_default();

        let profile = Profile {
            signature: string_at(object.get("signature")),
            name: string_at(object.get("name")),
            icon: string_at(object.get("icon")),
            axes,
            mappings,
            tuning,
        };
        (profile, rejected)
    }
}

fn string_at(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// Python's `str.isprintable`; exact match since result is profile filename.
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

/// Stable identity for a controller model, across replugs.
pub fn signature(vid: u16, pid: u16, name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| printable(*c)).collect();
    format!("{vid:04x}:{pid:04x}:{}", cleaned.trim())
}

/// Filename for a signature, truncated to 120 chars + .json suffix.
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
            extra: BTreeMap::new(),
            layout: "n64".to_owned(),
            name: String::new(),
        }
    }

    #[test]
    fn a_control_with_one_input_is_written_as_it_always_was() {
        let mapping = capture("a", 3);
        let value = mapping.buttons_value();
        assert_eq!(value["a"]["kind"], "button");
        assert!(
            !value["a"].is_array(),
            "no list where there is nothing to list"
        );
        assert_eq!(
            Mapping::from_value(&serde_json::to_value(&mapping).expect("json")),
            mapping
        );
    }

    #[test]
    fn a_second_input_is_a_list_whose_first_entry_is_the_binding() {
        let mut mapping = capture("rightshoulder", 5);
        mapping.add("rightshoulder", Binding::axis(5, 1));
        assert_eq!(
            mapping.buttons["rightshoulder"],
            Binding::button(5),
            "the first is unmoved"
        );
        assert_eq!(mapping.extra["rightshoulder"], vec![Binding::axis(5, 1)]);
        let value = mapping.buttons_value();
        let listed = value["rightshoulder"].as_array().expect("a list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["kind"], "button");
        assert_eq!(listed[1]["kind"], "axis");
        // An old reader takes the first entry; a new one round-trips the whole list.
        assert_eq!(
            Mapping::from_value(&serde_json::to_value(&mapping).expect("json")),
            mapping
        );
        assert_eq!(
            mapping.all("rightshoulder"),
            vec![Binding::button(5), Binding::axis(5, 1)]
        );
    }

    #[test]
    fn the_first_input_a_control_gets_is_its_binding_and_the_same_one_twice_is_once() {
        let mut mapping = Mapping::default();
        mapping.add("a", Binding::button(1));
        assert_eq!(mapping.buttons["a"], Binding::button(1));
        assert!(mapping.extra.is_empty(), "the first is not a twin");
        mapping.add("a", Binding::button(1));
        assert!(
            mapping.extra.is_empty(),
            "rebinding the same input changes nothing"
        );
        mapping.add("a", Binding::button(2));
        mapping.add("a", Binding::button(2));
        assert_eq!(
            mapping.extra["a"],
            vec![Binding::button(2)],
            "and neither does twice"
        );
    }

    #[test]
    fn a_list_read_back_keeps_only_what_a_binding_can_be() {
        let raw = serde_json::json!({
            "buttons": {"a": [{"kind": "button", "index": 1}, "nonsense", {"kind": "hat", "index": 0, "value": 2}]},
            "layout": "n64"
        });
        let mapping = Mapping::from_value(&raw);
        assert_eq!(mapping.buttons["a"], Binding::button(1));
        assert_eq!(mapping.extra["a"], vec![Binding::hat(0, 2)]);
    }

    #[test]
    fn an_empty_list_leaves_the_control_unbound_rather_than_half_bound() {
        let raw = serde_json::json!({"buttons": {"a": []}, "layout": "n64"});
        let mapping = Mapping::from_value(&raw);
        assert!(mapping.buttons.is_empty() && mapping.extra.is_empty());
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
        assert_eq!(signature(1, 2, "\u{18} Pad"), "0001:0002:Pad");
        assert_eq!(signature(1, 2, "  Pad  "), "0001:0002:Pad");
    }

    #[test]
    fn a_space_inside_a_name_is_printable_and_kept() {
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
        let mut profile = Profile::default();
        assert!(!profile.has_bindings());
        profile.record("console:n64", capture("a", 1));
        assert!(profile.has_bindings());
    }

    #[test]
    fn a_profile_that_is_not_an_object_reads_as_an_empty_one() {
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
            tuning: Tuning {
                deadzone: BTreeMap::from([(0, 0.15)]),
                debounce_ms: 25,
                ..Tuning::default()
            },
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
    fn a_control_name_danstick_no_longer_knows_costs_that_binding_only() {
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
    //! Intentional differences from Python implementation.
    use super::*;

    #[test]
    fn a_field_that_is_not_a_string_reads_as_empty_rather_than_as_its_repr() {
        let raw = serde_json::json!({"signature": 5, "name": null, "icon": ["x"]});
        let (profile, _) = Profile::from_value(&raw);
        assert_eq!(profile.signature, "");
        assert_eq!(profile.name, "");
        assert_eq!(profile.icon, "");
    }

    #[test]
    fn an_untuned_profile_writes_no_tuning_key() {
        let value = Profile::default().to_value();
        assert!(value.get("tuning").is_none());
    }

    #[test]
    fn junk_under_tuning_costs_the_tuning_and_not_the_profile() {
        let (profile, _) = Profile::from_value(&serde_json::json!({
            "signature": "s",
            "icon": "n64",
            "tuning": {"deadzone": "lots", "debounce_ms": -3}
        }));
        assert_eq!(profile.icon, "n64");
        assert!(profile.tuning.is_default());
        let (profile, _) = Profile::from_value(&serde_json::json!({"tuning": 7}));
        assert!(profile.tuning.is_default());
    }
}
