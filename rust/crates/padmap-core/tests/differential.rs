//! Hold the port to what the Python actually answered.
//!
//! `tools/gen_corpus.py` calls the real Python functions and records every
//! answer under `tests/corpus/`. This replays them. The distinction from an
//! ordinary unit test matters: a hand-written expectation encodes what the
//! porter *believed* the Python did, and that belief is the thing most likely
//! to be wrong. Two of these files exist specifically because the languages
//! disagree silently -- Python's `round` is ties-to-even where Rust's rounds
//! away from zero, and `//` floors where `/` truncates -- and both of those run
//! on the per-event path.
//!
//! A failure here is not automatically a bug in the Rust. It is a divergence,
//! and the question it asks is which side is right. Where the Python's answer
//! was judged wrong, the case is moved to an explicit test that says so.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use padmap_core::binding::{axis_index, retroarch_button_index, sdl_button_index, Binding};
use padmap_core::calibration::AxisCalibration;
use padmap_core::control::Control;
use padmap_core::fields::Fields;
use padmap_core::sdl::AxisSpan;
use padmap_core::{layout, retroarch, scope, sdl};
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error} -- run tools/gen_corpus.py", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn binding_from(raw: &Value) -> Binding {
    serde_json::from_value(raw.clone()).expect("a recorded binding must parse")
}

fn bindings_from(raw: &Value) -> BTreeMap<Control, Binding> {
    raw.as_object()
        .expect("a recorded capture is an object")
        .iter()
        .map(|(name, value)| {
            (
                name.parse::<Control>().expect("a recorded control name"),
                binding_from(value),
            )
        })
        .collect()
}

fn u16s(raw: &Value) -> Vec<u16> {
    raw.as_array()
        .expect("an array of codes")
        .iter()
        .map(|value| value.as_u64().expect("a code") as u16)
        .collect()
}

/// What the Python recorded for a call that may raise: `{"ok": bool, ...}`.
fn expect_fallible(recorded: &Value, actual: Result<String, impl std::fmt::Display>, what: &str) {
    let ok = recorded["ok"].as_bool().expect("ok is a bool");
    match (ok, actual) {
        (true, Ok(value)) => {
            assert_eq!(
                value,
                recorded["value"].as_str().expect("a value"),
                "{what}"
            );
        }
        (false, Err(_)) => {}
        (true, Err(error)) => {
            panic!(
                "{what}: the Python answered {:?}, this refused ({error})",
                recorded["value"]
            )
        }
        (false, Ok(value)) => panic!(
            "{what}: the Python refused ({}), this answered {value:?}",
            recorded["error"]
        ),
    }
}

#[test]
fn bindings_spell_the_same_thing_to_both_consumers() {
    let cases = corpus("bindings");
    assert!(!cases.is_empty());
    for case in &cases {
        let binding = binding_from(&case["in"]);
        let out = &case["out"];
        let what = format!("{binding:?}");
        assert_eq!(
            binding.sdl_visible(),
            out["sdl_visible"],
            "sdl_visible for {what}"
        );
        assert_eq!(
            binding.retroarch_visible(),
            out["retroarch_visible"],
            "retroarch_visible for {what}"
        );
        expect_fallible(&out["sdl"], binding.sdl(), &format!("sdl() for {what}"));
        expect_fallible(
            &out["retroarch"],
            binding.retroarch(),
            &format!("retroarch() for {what}"),
        );
    }
}

#[test]
fn both_consumers_number_buttons_the_way_the_python_did() {
    for case in corpus("button_indices") {
        let keys = u16s(&case["in"]["keys"]);
        let code = case["in"]["code"].as_u64().expect("a code") as u16;
        let expected_sdl = case["out"]["sdl"].as_i64().map(|value| value as i32);
        let expected_ra = case["out"]["retroarch"].as_i64().map(|value| value as i32);
        assert_eq!(
            sdl_button_index(&keys, code),
            expected_sdl,
            "sdl {code:#x} in {keys:x?}"
        );
        assert_eq!(
            retroarch_button_index(&keys, code),
            expected_ra,
            "retroarch {code:#x} in {keys:x?}"
        );
    }
}

#[test]
fn axes_are_numbered_the_way_the_python_did() {
    for case in corpus("axis_indices") {
        let codes = u16s(&case["in"]["codes"]);
        let code = case["in"]["code"].as_u64().expect("a code") as u16;
        let expected = case["out"].as_i64().map(|value| value as i32);
        assert_eq!(
            axis_index(&codes, code),
            expected,
            "{code:#x} in {codes:x?}"
        );
    }
}

#[test]
fn every_recorded_guid_is_reproduced_exactly() {
    // A GUID that differs by one hex digit is never matched, and SDL does not
    // complain -- the mapping simply does nothing.
    let cases = corpus("guids");
    assert!(cases.len() > 100, "the corpus is meant to be broad");
    for case in &cases {
        let input = &case["in"];
        let computed = sdl::guid(
            input["bus"].as_u64().expect("bus") as u16,
            input["vendor"].as_u64().expect("vendor") as u16,
            input["product"].as_u64().expect("product") as u16,
            input["version"].as_u64().expect("version") as u16,
            input["name"].as_str().expect("name"),
        );
        assert_eq!(
            computed,
            case["out"].as_str().expect("a guid"),
            "for {input}"
        );
    }
}

#[test]
fn a_hostile_device_name_is_cleaned_the_same_way() {
    for case in corpus("sdl_lines") {
        let input = &case["in"];
        let fields: Fields = input["fields"]
            .as_object()
            .expect("fields")
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str().expect("a target").to_owned()))
            .collect();
        let built = sdl::line(
            input["guid"].as_str().expect("guid"),
            input["name"].as_str().expect("name"),
            &fields,
            input["platform"].as_str().expect("platform"),
        );
        assert_eq!(
            built,
            case["out"].as_str().expect("a line"),
            "for {:?}",
            input["name"]
        );
    }
}

#[test]
fn a_capture_becomes_the_same_sdl_line() {
    for case in corpus("sdl_mappings") {
        let input = &case["in"];
        let bindings = bindings_from(&input["bindings"]);
        let sticks: Option<Fields> = input["sticks"].as_object().map(|entries| {
            entries
                .iter()
                .map(|(name, value)| (name.clone(), value.as_str().expect("a target").to_owned()))
                .collect()
        });
        let built = sdl::mapping_line(
            input["guid"].as_str().expect("guid"),
            input["name"].as_str().expect("name"),
            &bindings,
            input["platform"].as_str().expect("platform"),
            sticks.as_ref(),
        );
        assert_eq!(built, case["out"].as_str().expect("a line"), "for {input}");
    }
}

#[test]
fn a_database_line_is_parsed_the_same_way() {
    for case in corpus("parsed_sdl_lines") {
        let raw = case["in"].as_str().expect("a line");
        match (sdl::parse_line(raw), case["out"].as_object()) {
            (None, None) => {}
            (Some((guid, name, fields)), Some(expected)) => {
                assert_eq!(guid, expected["guid"].as_str().expect("guid"), "{raw:?}");
                assert_eq!(name, expected["name"].as_str().expect("name"), "{raw:?}");
                let actual: Vec<(String, String)> =
                    fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let wanted: Vec<(String, String)> = expected["fields"]
                    .as_array()
                    .expect("fields")
                    .iter()
                    .map(|pair| {
                        (
                            pair[0].as_str().expect("field").to_owned(),
                            pair[1].as_str().expect("target").to_owned(),
                        )
                    })
                    .collect();
                assert_eq!(
                    actual, wanted,
                    "{raw:?} -- field order is part of the answer"
                );
            }
            (parsed, expected) => {
                panic!("{raw:?}: the Python said {expected:?}, this said {parsed:?}")
            }
        }
    }
}

#[test]
fn the_same_axes_are_called_sticks() {
    for case in corpus("stick_fields") {
        let input = &case["in"];
        let codes = u16s(&input["axis_codes"]);
        let bindings = bindings_from(&input["bindings"]);
        let axes: Option<BTreeMap<u16, AxisSpan>> = input["axes"].as_array().map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let span = entry[1].as_array().expect("a span");
                    (
                        entry[0].as_u64().expect("a code") as u16,
                        AxisSpan::new(
                            span[0].as_i64().expect("min") as i32,
                            span[1].as_i64().expect("max") as i32,
                            span[2].as_i64().expect("rest") as i32,
                        ),
                    )
                })
                .collect()
        });
        let fields = sdl::stick_fields(&codes, &bindings, axes.as_ref());
        let actual: Vec<(String, String)> =
            fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let wanted: Vec<(String, String)> = case["out"]
            .as_array()
            .expect("pairs")
            .iter()
            .map(|pair| {
                (
                    pair[0].as_str().expect("field").to_owned(),
                    pair[1].as_str().expect("target").to_owned(),
                )
            })
            .collect();
        assert_eq!(actual, wanted, "for {input}");
    }
}

#[test]
fn a_capture_becomes_the_same_autoconfig() {
    for case in corpus("retroarch_lines") {
        let input = &case["in"];
        let bindings = bindings_from(&input["bindings"]);
        let overrides: BTreeMap<Control, String> = input["overrides"]
            .as_object()
            .expect("overrides")
            .iter()
            .map(|(name, value)| {
                (
                    name.parse::<Control>().expect("a control name"),
                    value.as_str().expect("a key").to_owned(),
                )
            })
            .collect();
        let built = retroarch::lines(&bindings, &overrides);
        let wanted: Vec<String> = case["out"]
            .as_array()
            .expect("lines")
            .iter()
            .map(|line| line.as_str().expect("a line").to_owned())
            .collect();
        assert_eq!(built, wanted, "for {input}");
    }
}

#[test]
fn the_shadowed_axis_rule_drops_the_same_lines() {
    for case in corpus("shadowed_axis_halves") {
        let input: Vec<String> = case["in"]
            .as_array()
            .expect("lines")
            .iter()
            .map(|line| line.as_str().expect("a line").to_owned())
            .collect();
        let wanted: Vec<String> = case["out"]
            .as_array()
            .expect("lines")
            .iter()
            .map(|line| line.as_str().expect("a line").to_owned())
            .collect();
        assert_eq!(
            retroarch::drop_shadowed_axis_halves(input.clone()),
            wanted,
            "for {input:?}"
        );
    }
}

#[test]
fn every_recorded_axis_reading_rescales_to_the_same_count() {
    // The one function on the per-event path, swept across its whole input
    // range rather than sampled. A disagreement of one count at one value is
    // exactly what survives a spot check and then reads as a stick that drifts.
    let cases = corpus("calibration");
    let mut checked = 0usize;
    for case in &cases {
        let input = &case["in"];
        let cal = AxisCalibration {
            center: input["center"].as_i64().expect("center") as i32,
            minimum: input["min"].as_i64().expect("min") as i32,
            maximum: input["max"].as_i64().expect("max") as i32,
            flat: input["flat"].as_i64().expect("flat") as i32,
            reach_min: input["reach_min"].as_i64().map(|value| value as i32),
            reach_max: input["reach_max"].as_i64().map(|value| value as i32),
        };
        let out = &case["out"];
        assert_eq!(
            cal.low(),
            out["low"].as_i64().expect("low"),
            "low for {input}"
        );
        assert_eq!(
            cal.high(),
            out["high"].as_i64().expect("high"),
            "high for {input}"
        );
        assert_eq!(cal.fits(), out["fits_evdev"], "fits for {input}");
        assert_eq!(
            cal.midpoint(),
            out["midpoint"].as_i64().expect("midpoint"),
            "mid for {input}"
        );
        for pair in out["applied"].as_array().expect("readings") {
            let value = pair[0].as_i64().expect("a reading") as i32;
            let wanted = pair[1].as_i64().expect("a result") as i32;
            assert_eq!(cal.apply(value), wanted, "apply({value}) for {input}");
            checked += 1;
        }
    }
    assert!(checked > 2000, "only {checked} readings were swept");
}

#[test]
fn a_rom_path_becomes_the_same_game_key() {
    for case in corpus("game_keys") {
        let input = &case["in"];
        let key = scope::game_key(
            input["console"].as_str().expect("console"),
            input["rom"].as_str().expect("rom"),
        );
        assert_eq!(key, case["out"].as_str().expect("a key"), "for {input}");
    }
}

#[test]
fn scope_precedence_is_unchanged() {
    for case in corpus("scope_order") {
        let input = &case["in"];
        let order = scope::order(
            input["console"].as_str().expect("console"),
            input["game"].as_str().expect("game"),
        );
        let wanted: Vec<String> = case["out"]
            .as_array()
            .expect("scopes")
            .iter()
            .map(|value| value.as_str().expect("a scope").to_owned())
            .collect();
        assert_eq!(order, wanted, "for {input}");
    }
}

#[test]
fn every_core_resolves_to_the_same_console() {
    let cases = corpus("cores");
    assert!(cases.len() > 50);
    for case in &cases {
        let core = case["in"].as_str().expect("a core");
        assert_eq!(
            layout::for_core(core),
            case["out"].as_str().expect("a layout id"),
            "{core:?}"
        );
    }
}

#[test]
fn every_layout_carries_the_same_coordinates_labels_and_overrides() {
    // Compared field by field rather than as JSON: the Rust omits empty
    // optionals where the Python always wrote them, which is a serialisation
    // difference and not a disagreement about the layout.
    for case in corpus("layouts") {
        let id = case["in"].as_str().expect("a layout id");
        let wanted = &case["out"];
        let layout = layout::get(id);
        assert_eq!(layout.id, wanted["id"].as_str().expect("id"), "{id}: id");
        assert_eq!(
            layout.label,
            wanted["label"].as_str().expect("label"),
            "{id}: label"
        );
        assert_eq!(
            layout.image,
            wanted["image"].as_str().expect("image"),
            "{id}: image"
        );

        let shapes = wanted["shapes"].as_array().expect("shapes");
        assert_eq!(layout.shapes.len(), shapes.len(), "{id}: shape count");
        for (shape, wanted) in layout.shapes.iter().zip(shapes) {
            assert_eq!(
                shape.kind,
                wanted["kind"].as_str().expect("kind"),
                "{id}: shape kind"
            );
            let points: Vec<f64> = wanted["points"]
                .as_array()
                .expect("points")
                .iter()
                .map(|value| value.as_f64().expect("a coordinate"))
                .collect();
            assert_eq!(shape.points, points, "{id}: shape points");
            assert_eq!(
                shape.radius,
                wanted["radius"].as_f64().expect("radius"),
                "{id}: radius"
            );
        }

        let controls = wanted["controls"].as_array().expect("controls");
        assert_eq!(layout.controls.len(), controls.len(), "{id}: control count");
        for (control, wanted) in layout.controls.iter().zip(controls) {
            let name = wanted["canonical"].as_str().expect("canonical");
            assert_eq!(
                control.canonical.as_str(),
                name,
                "{id}: canonical, in order"
            );
            assert_eq!(
                control.label,
                wanted["label"].as_str().expect("label"),
                "{id}/{name}"
            );
            assert_eq!(
                control.x,
                wanted["x"].as_f64().expect("x"),
                "{id}/{name}: x"
            );
            assert_eq!(
                control.y,
                wanted["y"].as_f64().expect("y"),
                "{id}/{name}: y"
            );
            assert_eq!(
                control.kind,
                wanted["kind"].as_str().expect("kind"),
                "{id}/{name}: kind"
            );
            assert_eq!(
                control.radius,
                wanted["radius"].as_f64().expect("radius"),
                "{id}/{name}"
            );
            assert_eq!(
                control.retroarch,
                wanted["retroarch"].as_str().expect("retroarch"),
                "{id}/{name}: the console's own key wiring"
            );
        }
    }
}

#[test]
fn the_corpus_covers_every_layout_the_port_ships() {
    // A layout added on one side and not the other is a silent gap in every
    // test above.
    let recorded: BTreeSet<String> = corpus("layouts")
        .iter()
        .map(|case| case["in"].as_str().expect("an id").to_owned())
        .collect();
    let shipped: BTreeSet<String> = layout::all()
        .iter()
        .map(|layout| layout.id.clone())
        .collect();
    assert_eq!(recorded, shipped, "run tools/gen_corpus.py");
}

#[test]
fn every_codepoint_is_printable_to_the_same_answer_as_python() {
    // The one that decides a profile's filename. A single codepoint's
    // disagreement orphans every profile whose controller name contains it,
    // silently, because a profile that cannot be found reads as "never
    // configured" and the wizard simply opens again.
    let cases = corpus("printable");
    assert!(cases.len() > 2000, "the sweep is meant to be broad");
    for case in &cases {
        let point = case["in"].as_u64().expect("a codepoint") as u32;
        let character = char::from_u32(point).expect("a recorded codepoint is a char");
        assert_eq!(
            padmap_core::profile::printable(character),
            case["out"].as_bool().expect("a verdict"),
            "U+{point:04X}"
        );
    }
}

#[test]
fn a_controller_name_becomes_the_same_signature_and_the_same_filename() {
    for case in corpus("signatures") {
        let input = &case["in"];
        let signature = padmap_core::profile::signature(
            input["vid"].as_u64().expect("vid") as u16,
            input["pid"].as_u64().expect("pid") as u16,
            input["name"].as_str().expect("name"),
        );
        assert_eq!(
            signature,
            case["out"]["signature"].as_str().expect("a signature"),
            "for {input}"
        );
        assert_eq!(
            padmap_core::profile::filename(&signature),
            case["out"]["filename"].as_str().expect("a filename"),
            "for {input}"
        );
    }
}

#[test]
fn a_stored_profile_reads_back_and_writes_out_the_way_the_python_did() {
    use padmap_core::profile::Profile;
    for case in corpus("stored_profiles") {
        let input = &case["in"];
        let (profile, _rejected) = Profile::from_value(input);
        let expected = &case["out"];

        assert_eq!(
            profile.has_bindings(),
            expected["has_bindings"],
            "for {input}"
        );
        assert_eq!(
            profile.layout(),
            expected["layout"].as_str().expect("layout"),
            "for {input}"
        );

        for (key, wanted) in expected["resolved"].as_object().expect("resolved") {
            let (console, game) = key.split_once('|').expect("console|game");
            assert_eq!(
                profile.resolve(console, game).0,
                wanted.as_str().expect("a scope"),
                "resolve({console:?}, {game:?}) for {input}"
            );
        }

        assert_eq!(
            profile.to_value(),
            expected["json"],
            "written form, for {input}"
        );
    }
}

#[test]
fn every_controller_name_gets_the_same_icon_as_the_python() {
    // An ordered list of regexes, where the order is the rule. A wrong icon
    // looks like a design choice rather than a defect, so nobody reports it.
    use padmap_core::icons;
    let cases = corpus("icons");
    assert!(cases.len() > 200, "the sweep is meant to be broad");
    for case in &cases {
        let input = &case["in"];
        let got = icons::for_pad(
            input["vid"].as_u64().expect("vid") as u16,
            input["pid"].as_u64().expect("pid") as u16,
            input["name"].as_str().expect("name"),
            None,
            &BTreeMap::new(),
        );
        assert_eq!(got, case["out"].as_str().expect("an icon"), "for {input}");
    }
}

#[test]
fn the_udev_rules_match_the_python_rule_for_rule() {
    // The *rules*, not the file. udev ignores comments and blank lines, and
    // the two implementations differ in both deliberately: the Python header
    // names RetroArch, which is no longer the only consumer, and it emits a
    // blank line after the header that this does not. What udev actually acts
    // on has to be identical, and a stray space or a lowercase hex digit where
    // the kernel writes uppercase makes a rule that matches nothing at all --
    // legally, and with no error anywhere.
    use padmap_core::hide::{self, Hideable};

    fn effective(rules: &str) -> Vec<&str> {
        rules
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect()
    }

    for case in corpus("hide_rules") {
        let pads: Vec<Hideable> = case["in"]
            .as_array()
            .expect("pads")
            .iter()
            .map(|raw| Hideable {
                name: raw["name"].as_str().expect("name").to_owned(),
                vid: raw["vid"].as_u64().expect("vid") as u16,
                pid: raw["pid"].as_u64().expect("pid") as u16,
            })
            .collect();
        let out = &case["out"];

        let mine = hide::generate_rules(&pads);
        let theirs = out["rules"].as_str().expect("rules");
        assert_eq!(
            effective(&mine),
            effective(theirs),
            "rules for {:?}",
            case["in"]
        );
        // And what each file says it covers has to agree, which is the part
        // `unhidden` reads back.
        assert_eq!(hide::covered(&mine), hide::covered(theirs), "coverage");

        assert_eq!(
            hide::nix_module_snippet(&pads),
            out["nix"].as_str().expect("nix"),
            "nix snippet for {:?}",
            case["in"]
        );
        let wanted: Vec<(u16, u16)> = out["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .map(|raw| {
                (
                    raw["vid"].as_u64().expect("vid") as u16,
                    raw["pid"].as_u64().expect("pid") as u16,
                )
            })
            .collect();
        let got: Vec<(u16, u16)> = hide::targets(&pads, &[])
            .iter()
            .map(|p| (p.vid, p.pid))
            .collect();
        assert_eq!(got, wanted, "targets for {:?}", case["in"]);
    }
}
