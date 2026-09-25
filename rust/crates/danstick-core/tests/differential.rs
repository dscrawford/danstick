//! Replay what the Python actually answered (`tools/gen_corpus.py` -> `tests/corpus/`).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use danstick_core::binding::{axis_index, retroarch_button_index, sdl_button_index, Binding};
use danstick_core::calibration::AxisCalibration;
use danstick_core::control::Control;
use danstick_core::emit::{self, Identity};
use danstick_core::fields::Fields;
use danstick_core::hide::{self, Hideable};
use danstick_core::sdl::AxisSpan;
use danstick_core::{guess, icons, layout, profile, retroarch, scope, sdl, standard};
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/corpus/{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error} -- run tools/gen_corpus.py", path.display()));
    serde_json::from_str(&common::renamed(&text))
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
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
        .map(|v| v.as_u64().expect("a code") as u16)
        .collect()
}

fn u16_at(raw: &Value, key: &str) -> u16 {
    raw[key].as_u64().unwrap_or_else(|| panic!("{key}")) as u16
}

fn str_at<'a>(raw: &'a Value, key: &str) -> &'a str {
    raw[key].as_str().unwrap_or_else(|| panic!("{key}"))
}

fn strings(raw: &Value) -> Vec<String> {
    raw.as_array()
        .expect("an array")
        .iter()
        .map(|v| v.as_str().expect("a string").to_owned())
        .collect()
}

/// `[[field, target], ...]` as the Python recorded ordered pairs.
fn pairs(raw: &Value) -> Vec<(String, String)> {
    raw.as_array()
        .expect("pairs")
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().expect("field").to_owned(),
                pair[1].as_str().expect("target").to_owned(),
            )
        })
        .collect()
}

fn fields_from(raw: &Value) -> Fields {
    raw.as_object()
        .expect("fields")
        .iter()
        .map(|(name, value)| (name.clone(), value.as_str().expect("a target").to_owned()))
        .collect()
}

fn ordered(fields: &Fields) -> Vec<(String, String)> {
    fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// What the Python recorded for a call that may raise: `{"ok": bool, ...}`.
fn expect_fallible(recorded: &Value, actual: Result<String, impl std::fmt::Display>, what: &str) {
    let ok = recorded["ok"].as_bool().expect("ok is a bool");
    match (ok, actual) {
        (true, Ok(value)) => assert_eq!(
            value,
            recorded["value"].as_str().expect("a value"),
            "{what}"
        ),
        (false, Err(_)) => {}
        (true, Err(error)) => panic!(
            "{what}: the Python answered {:?}, this refused ({error})",
            recorded["value"]
        ),
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
        let code = u16_at(&case["in"], "code");
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
        let code = u16_at(&case["in"], "code");
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
    let cases = corpus("guids");
    assert!(cases.len() > 100, "the corpus is meant to be broad");
    for case in &cases {
        let input = &case["in"];
        let computed = sdl::guid(
            u16_at(input, "bus"),
            u16_at(input, "vendor"),
            u16_at(input, "product"),
            u16_at(input, "version"),
            str_at(input, "name"),
        );
        assert_eq!(computed, str_at(case, "out"), "for {input}");
    }
}

#[test]
fn a_hostile_device_name_is_cleaned_the_same_way() {
    for case in corpus("sdl_lines") {
        let input = &case["in"];
        let fields = fields_from(&input["fields"]);
        let built = sdl::line(
            str_at(input, "guid"),
            str_at(input, "name"),
            &fields,
            str_at(input, "platform"),
        );
        assert_eq!(built, str_at(&case, "out"), "for {:?}", input["name"]);
    }
}

#[test]
fn a_capture_becomes_the_same_sdl_line() {
    for case in corpus("sdl_mappings") {
        let input = &case["in"];
        let bindings = bindings_from(&input["bindings"]);
        let sticks = input["sticks"]
            .is_object()
            .then(|| fields_from(&input["sticks"]));
        let built = sdl::mapping_line(
            str_at(input, "guid"),
            str_at(input, "name"),
            &bindings,
            str_at(input, "platform"),
            sticks.as_ref(),
        );
        assert_eq!(built, str_at(&case, "out"), "for {input}");
    }
}

#[test]
fn a_database_line_is_parsed_the_same_way() {
    for case in corpus("parsed_sdl_lines") {
        let raw = str_at(&case, "in");
        match (sdl::parse_line(raw), case["out"].as_object()) {
            (None, None) => {}
            (Some((guid, name, fields)), Some(expected)) => {
                assert_eq!(guid, expected["guid"].as_str().expect("guid"), "{raw:?}");
                assert_eq!(name, expected["name"].as_str().expect("name"), "{raw:?}");
                assert_eq!(
                    ordered(&fields),
                    pairs(&expected["fields"]),
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
                    let at = |i: usize| span[i].as_i64().expect("a span bound") as i32;
                    (
                        entry[0].as_u64().expect("a code") as u16,
                        AxisSpan::new(at(0), at(1), at(2)),
                    )
                })
                .collect()
        });
        let fields = sdl::stick_fields(&codes, &bindings, axes.as_ref());
        assert_eq!(ordered(&fields), pairs(&case["out"]), "for {input}");
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
        assert_eq!(
            retroarch::lines(&bindings, &overrides),
            strings(&case["out"]),
            "for {input}"
        );
    }
}

#[test]
fn the_shadowed_axis_rule_drops_the_same_lines() {
    for case in corpus("shadowed_axis_halves") {
        let input = strings(&case["in"]);
        assert_eq!(
            retroarch::drop_shadowed_axis_halves(input.clone()),
            strings(&case["out"]),
            "for {input:?}"
        );
    }
}

#[test]
fn every_recorded_axis_reading_rescales_to_the_same_count() {
    // Swept, not sampled: a one-count disagreement at one value reads as a stick that drifts.
    let cases = corpus("calibration");
    let mut checked = 0usize;
    for case in &cases {
        let input = &case["in"];
        let int = |key: &str| input[key].as_i64().unwrap_or_else(|| panic!("{key}")) as i32;
        let cal = AxisCalibration {
            center: int("center"),
            minimum: int("min"),
            maximum: int("max"),
            flat: int("flat"),
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
        let key = scope::game_key(str_at(input, "console"), str_at(input, "rom"));
        assert_eq!(key, str_at(&case, "out"), "for {input}");
    }
}

#[test]
fn scope_precedence_is_unchanged() {
    for case in corpus("scope_order") {
        let input = &case["in"];
        let order = scope::order(str_at(input, "console"), str_at(input, "game"));
        assert_eq!(order, strings(&case["out"]), "for {input}");
    }
}

#[test]
fn every_core_resolves_to_the_same_console() {
    let cases = corpus("cores");
    assert!(cases.len() > 50);
    for case in &cases {
        let core = str_at(case, "in");
        assert_eq!(layout::for_core(core), str_at(case, "out"), "{core:?}");
    }
}

#[test]
fn every_layout_carries_the_same_coordinates_labels_and_overrides() {
    for case in corpus("layouts") {
        let id = str_at(&case, "in");
        let wanted = &case["out"];
        let layout = layout::get(id);
        assert_eq!(layout.id, str_at(wanted, "id"), "{id}: id");
        assert_eq!(layout.label, str_at(wanted, "label"), "{id}: label");
        assert_eq!(layout.image, str_at(wanted, "image"), "{id}: image");

        let shapes = wanted["shapes"].as_array().expect("shapes");
        assert_eq!(layout.shapes.len(), shapes.len(), "{id}: shape count");
        for (shape, wanted) in layout.shapes.iter().zip(shapes) {
            assert_eq!(shape.kind, str_at(wanted, "kind"), "{id}: shape kind");
            let points: Vec<f64> = wanted["points"]
                .as_array()
                .expect("points")
                .iter()
                .map(|v| v.as_f64().expect("a coordinate"))
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
            let name = str_at(wanted, "canonical");
            assert_eq!(
                control.canonical.as_str(),
                name,
                "{id}: canonical, in order"
            );
            assert_eq!(control.label, str_at(wanted, "label"), "{id}/{name}");
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
            assert_eq!(control.kind, str_at(wanted, "kind"), "{id}/{name}: kind");
            assert_eq!(
                control.radius,
                wanted["radius"].as_f64().expect("radius"),
                "{id}/{name}"
            );
            assert_eq!(
                control.retroarch,
                str_at(wanted, "retroarch"),
                "{id}/{name}: the console's own key wiring"
            );
        }
    }
}

#[test]
fn the_corpus_covers_every_layout_the_port_ships() {
    let recorded: BTreeSet<String> = corpus("layouts")
        .iter()
        .map(|case| str_at(case, "in").to_owned())
        .collect();
    let shipped: BTreeSet<String> = layout::all()
        .iter()
        .map(|layout| layout.id.clone())
        .collect();
    assert_eq!(recorded, shipped, "run tools/gen_corpus.py");
}

#[test]
fn every_codepoint_is_printable_to_the_same_answer_as_python() {
    let cases = corpus("printable");
    assert!(cases.len() > 2000, "the sweep is meant to be broad");
    for case in &cases {
        let point = case["in"].as_u64().expect("a codepoint") as u32;
        let character = char::from_u32(point).expect("a recorded codepoint is a char");
        assert_eq!(
            profile::printable(character),
            case["out"].as_bool().expect("a verdict"),
            "U+{point:04X}"
        );
    }
}

#[test]
fn a_controller_name_becomes_the_same_signature_and_the_same_filename() {
    for case in corpus("signatures") {
        let input = &case["in"];
        let signature = profile::signature(
            u16_at(input, "vid"),
            u16_at(input, "pid"),
            str_at(input, "name"),
        );
        assert_eq!(signature, str_at(&case["out"], "signature"), "for {input}");
        assert_eq!(
            profile::filename(&signature),
            str_at(&case["out"], "filename"),
            "for {input}"
        );
    }
}

#[test]
fn a_stored_profile_reads_back_and_writes_out_the_way_the_python_did() {
    for case in corpus("stored_profiles") {
        let input = &case["in"];
        let (profile, _rejected) = profile::Profile::from_value(input);
        let expected = &case["out"];
        assert_eq!(
            profile.has_bindings(),
            expected["has_bindings"],
            "for {input}"
        );
        assert_eq!(profile.layout(), str_at(expected, "layout"), "for {input}");
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
    let cases = corpus("icons");
    assert!(cases.len() > 200, "the sweep is meant to be broad");
    for case in &cases {
        let input = &case["in"];
        let got = icons::for_pad(
            u16_at(input, "vid"),
            u16_at(input, "pid"),
            str_at(input, "name"),
            None,
            &BTreeMap::new(),
        );
        assert_eq!(got, str_at(case, "out"), "for {input}");
    }
}

#[test]
fn the_udev_rules_match_the_python_rule_for_rule() {
    // The rules, not the file: udev ignores comments and blank lines, and the headers differ deliberately.
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
                name: str_at(raw, "name").to_owned(),
                vid: u16_at(raw, "vid"),
                pid: u16_at(raw, "pid"),
            })
            .collect();
        let out = &case["out"];
        let mine = hide::generate_rules(&pads);
        let theirs = str_at(out, "rules");
        assert_eq!(
            effective(&mine),
            effective(theirs),
            "rules for {:?}",
            case["in"]
        );
        assert_eq!(hide::covered(&mine), hide::covered(theirs), "coverage");
        assert_eq!(
            hide::nix_module_snippet(&pads),
            str_at(out, "nix"),
            "nix snippet for {:?}",
            case["in"]
        );
        let wanted: Vec<(u16, u16)> = out["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .map(|raw| (u16_at(raw, "vid"), u16_at(raw, "pid")))
            .collect();
        let got: Vec<(u16, u16)> = hide::targets(&pads, &[])
            .iter()
            .map(|p| (p.vid, p.pid))
            .collect();
        assert_eq!(got, wanted, "targets for {:?}", case["in"]);
    }
}

/// danstick's own identity, which is what the corpus was recorded under.
const DANSTICK_IDENTITY: Identity = Identity {
    bustype: 0x06,
    vendor: 0x1209,
    product: 0x0001,
    version: 0x0001,
};

/// The version carries the player so consumers that blank the name CRC (Ryujinx) still see N pads.
fn identity_for(player: u32) -> Identity {
    Identity {
        version: emit::version_for(player),
        ..DANSTICK_IDENTITY
    }
}

#[test]
fn a_virtual_pads_guid_is_the_one_the_python_computed() {
    for case in corpus("virtual_guids") {
        let player = case["in"]["player"].as_u64().expect("player") as u32;
        assert_eq!(
            emit::virtual_guid(player, identity_for(player)),
            str_at(&case, "out"),
            "player {player}"
        );
    }
}

#[test]
fn every_autoconfig_profile_is_what_the_python_wrote() {
    let cases = corpus("retroarch_profiles");
    assert!(cases.len() > 50);
    for case in &cases {
        let input = &case["in"];
        let built = emit::retroarch_profile(
            input["player"].as_u64().expect("player") as u32,
            DANSTICK_IDENTITY,
            &bindings_from(&input["bindings"]),
            str_at(input, "source"),
            str_at(input, "layout"),
            str_at(input, "scope"),
            str_at(input, "context"),
        );
        assert_eq!(built, str_at(case, "out"), "for {input}");
    }
}

#[test]
fn an_unmapped_pad_is_guessed_at_identically() {
    for case in corpus("guessed_fields") {
        let input = &case["in"];
        let keys = u16s(&input["keys"]);
        if standard::is_standard(&keys) {
            continue;
        }
        let fields = guess::guessed_fields(&keys, &u16s(&input["axis_codes"]), None);
        assert_eq!(ordered(&fields), pairs(&case["out"]), "for {input}");
    }
}

#[test]
fn the_python_guessed_a_standard_pad_wrongly_and_danstick_does_not() {
    let keys: Vec<u16> = vec![
        0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x13A, 0x13B, 0x13C, 0x13D, 0x13E,
    ];
    let ours = guess::guessed_fields(&keys, &[], None);
    let theirs = [
        ("a", "b0"),
        ("b", "b1"),
        ("x", "b2"),
        ("y", "b3"),
        ("leftshoulder", "b4"),
        ("rightshoulder", "b5"),
        ("lefttrigger", "b6"),
        ("righttrigger", "b7"),
        ("back", "b8"),
        ("start", "b9"),
        ("leftstick", "b10"),
    ];
    assert_eq!(theirs[2], ("x", "b2"), "BTN_NORTH is at index 2 and is y");
    assert_eq!(ours.get("y"), Some("b2"));
    assert_eq!(ours.get("x"), Some("b3"));
    assert_eq!(
        theirs[6],
        ("lefttrigger", "b6"),
        "BTN_SELECT is at index 6 and is back"
    );
    assert_eq!(ours.get("back"), Some("b6"));
    assert_eq!(ours.get("lefttrigger"), None, "no ABS_Z was declared here");
}
