//! The vocabulary and the numbering, held still.

use std::collections::{BTreeMap, BTreeSet};

use danstick_core::binding::{
    axis_index, hat_direction, sdl_button_index, Binding, BindingKind, Unspellable, BTN_JOYSTICK,
    HAT_CODES,
};
use danstick_core::control::{Control, UnknownControl, CANONICAL_ORDER};

const PYTHON_NAMES: [&str; 18] = [
    "a",
    "b",
    "x",
    "y",
    "back",
    "start",
    "leftshoulder",
    "rightshoulder",
    "lefttrigger",
    "righttrigger",
    "dpup",
    "dpdown",
    "dpleft",
    "dpright",
    "rightstick_up",
    "rightstick_down",
    "rightstick_left",
    "rightstick_right",
];

#[test]
fn the_canonical_names_are_still_the_ones_the_python_wrote_to_disk() {
    // An ordered prefix, like COMMANDS: append only, never reorder.
    let names: Vec<&str> = CANONICAL_ORDER.iter().map(|c| c.as_str()).collect();
    assert_eq!(
        &names[..PYTHON_NAMES.len()],
        PYTHON_NAMES,
        "a renamed or reordered control orphans every stored profile"
    );
}

#[test]
fn the_four_left_stick_halves_are_what_was_added_after_the_python() {
    let names: Vec<&str> = CANONICAL_ORDER
        .iter()
        .skip(PYTHON_NAMES.len())
        .map(|c| c.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "leftstick_up",
            "leftstick_down",
            "leftstick_left",
            "leftstick_right"
        ]
    );
}

#[test]
fn every_control_round_trips_through_its_name_display_and_parse() {
    for control in Control::ALL {
        let name = control.as_str();
        assert_eq!(name.parse::<Control>(), Ok(control));
        assert_eq!(control.to_string(), name);
    }
}

#[test]
fn canonical_order_is_a_permutation_of_every_control() {
    assert_eq!(CANONICAL_ORDER.len(), Control::ALL.len());
    let ordered: BTreeSet<Control> = CANONICAL_ORDER.into_iter().collect();
    let all: BTreeSet<Control> = Control::ALL.into_iter().collect();
    assert_eq!(ordered, all);
    assert_eq!(
        ordered.len(),
        22,
        "a control missing from the order is never written out"
    );
    for control in Control::ALL {
        assert_eq!(
            CANONICAL_ORDER.iter().filter(|c| **c == control).count(),
            1,
            "{control} appears twice, so its line would be written twice"
        );
    }
}

#[test]
fn the_derived_ordering_matches_the_canonical_order() {
    let sorted: Vec<Control> = Control::ALL
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(sorted.as_slice(), CANONICAL_ORDER.as_slice());
}

#[test]
fn a_name_that_differs_only_in_case_or_whitespace_is_refused() {
    for name in [
        "A",
        "B",
        "Start",
        "DpUp",
        "dpUp",
        " a",
        "a ",
        "\ta",
        "a\n",
        "RIGHTSTICK_UP",
    ] {
        assert!(
            name.parse::<Control>().is_err(),
            "{name:?} was accepted as a control"
        );
    }
}

#[test]
fn a_name_from_a_neighbouring_vocabulary_is_refused() {
    // SDL field names and SDL controls danstick does not model.
    for name in [
        "guide",
        "leftstick",
        "rightstick",
        "misc1",
        "paddle1",
        "dpadup",
        "dpad_up",
        "rightstick-up",
        "rightstickup",
        "-righty",
        "+rightx",
        "",
    ] {
        assert!(
            name.parse::<Control>().is_err(),
            "{name:?} was accepted as a control"
        );
    }
}

#[test]
fn the_refusal_names_the_string_it_refused() {
    let error = "guide"
        .parse::<Control>()
        .expect_err("guide is not a control");
    assert_eq!(error, UnknownControl("guide".to_owned()));
    assert_eq!(error.to_string(), "\"guide\" is not a canonical control");
    let padded = " a"
        .parse::<Control>()
        .expect_err("a padded name is not a control");
    assert_eq!(padded.to_string(), "\" a\" is not a canonical control");
    let error: &dyn std::error::Error = &padded;
    assert!(error.source().is_none());
}

#[test]
fn sdl_field_is_total_and_injective() {
    let mut seen: BTreeMap<&str, Control> = BTreeMap::new();
    for control in Control::ALL {
        let field = control.sdl_field();
        assert!(!field.is_empty(), "{control} has no SDL field");
        if let Some(other) = seen.insert(field, control) {
            panic!("{control} and {other} both write the SDL field {field:?}");
        }
    }
    assert_eq!(seen.len(), 22);
}

#[test]
fn the_four_right_stick_halves_spell_a_signed_sdl_half_axis() {
    let halves = [
        (Control::RightStickUp, "-righty"),
        (Control::RightStickDown, "+righty"),
        (Control::RightStickLeft, "-rightx"),
        (Control::RightStickRight, "+rightx"),
    ];
    for (control, field) in halves {
        assert_eq!(control.sdl_field(), field, "{control}'s SDL half-axis");
    }
}

#[test]
fn the_four_left_stick_halves_spell_a_signed_sdl_half_axis() {
    let halves = [
        (Control::LeftStickUp, "-lefty"),
        (Control::LeftStickDown, "+lefty"),
        (Control::LeftStickLeft, "-leftx"),
        (Control::LeftStickRight, "+leftx"),
    ];
    for (control, field) in halves {
        assert_eq!(control.sdl_field(), field, "{control}'s SDL half-axis");
    }
}

#[test]
fn only_the_stick_halves_have_an_sdl_field_unlike_their_own_name() {
    let halves = [
        Control::RightStickUp,
        Control::RightStickDown,
        Control::RightStickLeft,
        Control::RightStickRight,
        Control::LeftStickUp,
        Control::LeftStickDown,
        Control::LeftStickLeft,
        Control::LeftStickRight,
    ];
    for control in Control::ALL {
        let differs = control.sdl_field() != control.as_str();
        assert_eq!(
            differs,
            halves.contains(&control),
            "{control} translated unexpectedly"
        );
    }
}

#[test]
fn an_sdl_half_axis_target_carries_a_sign_and_nothing_else_does() {
    for control in Control::ALL {
        let field = control.sdl_field();
        let signed = field.starts_with('+') || field.starts_with('-');
        assert_eq!(
            signed,
            (field.contains("right") || field.contains("left")) && field.ends_with(['x', 'y']),
            "{control} carries a sign it should not, or lacks one it should"
        );
    }
}

#[test]
fn every_control_serialises_as_its_own_on_disk_name() {
    for control in Control::ALL {
        let json = serde_json::to_string(&control).expect("serialize a control");
        assert_eq!(json, format!("\"{}\"", control.as_str()));
        let back: Control = serde_json::from_str(&json).expect("deserialize a control");
        assert_eq!(back, control);
    }
}

#[test]
fn deserialising_an_unknown_name_fails_rather_than_defaulting_to_a_button() {
    // Defaulting would bind the user's press to whichever control sorted.
    for raw in ["\"guide\"", "\"A\"", "\"\"", "\"dpUp\""] {
        let parsed: Result<Control, _> = serde_json::from_str(raw);
        assert!(parsed.is_err(), "{raw} deserialized");
    }
    let wrong_type: Result<Control, _> = serde_json::from_str("3");
    assert!(wrong_type.is_err(), "a control is a string, not an index");
}

#[test]
fn a_profile_keyed_by_control_round_trips_through_json() {
    let profile: BTreeMap<Control, Binding> = Control::ALL
        .into_iter()
        .enumerate()
        .map(|(n, control)| (control, Binding::button(n as i32)))
        .collect();
    let text = serde_json::to_string(&profile).expect("serialize a profile");
    assert!(
        text.contains("\"rightstick_up\":"),
        "the key is the canonical name"
    );
    let back: BTreeMap<Control, Binding> = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(back, profile);
}

#[test]
fn hat_direction_names_the_four_single_bits_and_nothing_else() {
    assert_eq!(hat_direction(1), Some("up"));
    assert_eq!(hat_direction(2), Some("right"));
    assert_eq!(hat_direction(4), Some("down"));
    assert_eq!(hat_direction(8), Some("left"));
    for value in [0, 3, 5, 6, 7, 9, 10, 12, 16, -1, -8, i32::MIN, i32::MAX] {
        assert_eq!(hat_direction(value), None, "hat {value} named a direction");
    }
}

#[test]
fn only_the_four_single_hat_bits_are_spellable() {
    for value in -8_i32..=16 {
        let binding = Binding::hat(0, value);
        let spellable = matches!(value, 1 | 2 | 4 | 8);
        assert_eq!(
            binding.sdl_visible(),
            spellable,
            "sdl_visible for hat {value}"
        );
        match binding.sdl() {
            Ok(_) => assert!(spellable, "hat {value} was spelled"),
            Err(error) => {
                assert!(!spellable, "hat {value} was refused");
                assert_eq!(error, Unspellable::HatValue(value));
            }
        }
    }
}

#[test]
fn the_four_hat_directions_spell_their_bit_to_sdl() {
    for value in [1, 2, 4, 8] {
        let binding = Binding::hat(2, value);
        assert_eq!(
            binding.sdl().expect("sdl spells a hat"),
            format!("h2.{value}")
        );
    }
}

#[test]
fn a_hat_value_that_names_nothing_reports_the_value_it_was_given() {
    let error = Binding::hat(0, 3)
        .sdl()
        .expect_err("a diagonal is not a control");
    assert_eq!(error, Unspellable::HatValue(3));
    assert_eq!(error.to_string(), "hat value 3 is not one direction bit");
}

#[test]
fn a_button_spells_a_prefixed_number_to_sdl() {
    for index in [0, 1, 7, 13, 127, i32::MAX] {
        let binding = Binding::button(index);
        assert!(binding.sdl_visible());
        assert_eq!(binding.sdl().expect("sdl"), format!("b{index}"));
    }
}

#[test]
fn a_button_with_a_negative_index_is_still_spelled_to_sdl() {
    // Documented, not endorsed: a corrupt profile is the only source.
    let binding = Binding::button(-3);
    assert!(binding.sdl_visible());
    assert_eq!(binding.sdl().expect("sdl"), "b-3");
}

#[test]
fn an_axis_carries_its_sign_and_zero_counts_as_positive() {
    for (value, sign) in [(1, '+'), (32767, '+'), (0, '+'), (-1, '-'), (i32::MIN, '-')] {
        let binding = Binding::axis(2, value);
        assert_eq!(
            binding.sdl().expect("sdl"),
            format!("{sign}a2"),
            "axis value {value}"
        );
    }
}

#[test]
fn axis_zero_keeps_its_sign() {
    assert_eq!(Binding::axis(0, -1).sdl().expect("sdl"), "-a0");
    assert_eq!(Binding::axis(0, 1).sdl().expect("sdl"), "+a0");
    assert_eq!(Binding::axis(0, 0).sdl().expect("sdl"), "+a0");
}

#[test]
fn an_extreme_index_neither_panics_nor_wraps() {
    // A hand-edited or corrupt profile is the source.
    for index in [i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX] {
        assert_eq!(
            Binding::axis(index, 1).sdl().expect("sdl"),
            format!("+a{index}")
        );
        assert_eq!(
            Binding::hat(index, 8).sdl().expect("sdl"),
            format!("h{index}.8")
        );
        assert_eq!(
            Binding::button(index).sdl().expect("sdl"),
            format!("b{index}")
        );
    }
    assert_eq!(
        Binding::hat(0, i32::MIN).sdl(),
        Err(Unspellable::HatValue(i32::MIN))
    );
    assert_eq!(
        Binding::hat(0, i32::MAX).sdl(),
        Err(Unspellable::HatValue(i32::MAX))
    );
}

#[test]
fn the_constructors_build_the_kind_they_name() {
    assert_eq!(Binding::button(3).kind, BindingKind::Button);
    assert_eq!(Binding::hat(0, 1).kind, BindingKind::Hat);
    assert_eq!(Binding::axis(2, -1).kind, BindingKind::Axis);
    assert_eq!(Binding::axis(2, -1).value, -1);
    assert_eq!(
        Binding::button(3).value,
        0,
        "a button has no value to carry"
    );
}

#[test]
fn binding_kind_spells_itself_the_way_the_python_stored_it() {
    assert_eq!(BindingKind::Button.as_str(), "button");
    assert_eq!(BindingKind::Hat.as_str(), "hat");
    assert_eq!(BindingKind::Axis.as_str(), "axis");
    for kind in [BindingKind::Button, BindingKind::Hat, BindingKind::Axis] {
        assert_eq!(kind.to_string(), kind.as_str());
        let json = serde_json::to_string(&kind).expect("serialize a kind");
        assert_eq!(
            json,
            format!("\"{}\"", kind.as_str()),
            "the on-disk spelling is lowercase"
        );
    }
}

/// Every interesting `Binding` shape: kind x index x value.
fn every_shape() -> Vec<Binding> {
    let kinds = [BindingKind::Button, BindingKind::Hat, BindingKind::Axis];
    let indices = [i32::MIN, -7, -1, 0, 1, 2, 13, i32::MAX];
    let values = [i32::MIN, -2, -1, 0, 1, 2, 3, 4, 8, 15, 16, i32::MAX];
    let mut shapes = Vec::new();
    for kind in kinds {
        for index in indices {
            for value in values {
                shapes.push(Binding { kind, index, value });
            }
        }
    }
    shapes
}

#[test]
fn the_cross_product_is_the_size_it_claims_to_be() {
    assert_eq!(every_shape().len(), 3 * 8 * 12);
}

#[test]
fn sdl_visibility_answers_exactly_whether_sdl_can_spell_it() {
    for binding in every_shape() {
        assert_eq!(
            binding.sdl_visible(),
            binding.sdl().is_ok(),
            "{binding:?}: asking and doing gave different answers"
        );
    }
}

fn plain_pad() -> Vec<u16> {
    (BTN_JOYSTICK..BTN_JOYSTICK + 12).collect()
}

#[test]
fn sdl_numbers_buttons_from_btn_joystick() {
    assert_eq!(BTN_JOYSTICK, 0x120);
}

#[test]
fn a_pad_whose_codes_all_start_at_btn_joystick_numbers_them_in_order() {
    let keys = plain_pad();
    for (expected, code) in keys.iter().copied().enumerate() {
        assert_eq!(
            sdl_button_index(&keys, code),
            Some(expected as i32),
            "sdl for {code:#x}"
        );
    }
    assert_eq!(sdl_button_index(&keys, 0x121), Some(1));
    assert_eq!(sdl_button_index(&keys, 0x128), Some(8));
}

#[test]
fn a_pad_carrying_btn_misc_codes_numbers_them_after_the_joystick_buttons() {
    // BTN_0..BTN_2 are 0x100..0x102 -- arcade encoders report them.
    let keys = [0x100_u16, 0x101, 0x102, 0x120, 0x121, 0x122];
    assert_eq!(sdl_button_index(&keys, 0x120), Some(0));
    assert_eq!(sdl_button_index(&keys, 0x122), Some(2));
    assert_eq!(sdl_button_index(&keys, 0x100), Some(3));
    assert_eq!(sdl_button_index(&keys, 0x101), Some(4));
    assert_eq!(sdl_button_index(&keys, 0x102), Some(5));
}

#[test]
fn a_keyboard_code_sorts_after_every_joystick_button_for_sdl() {
    // KEY_A (0x1e) from a combo adapter.
    let keys = [0x1e_u16, 0x120, 0x121, 0x122];
    assert_eq!(sdl_button_index(&keys, 0x120), Some(0));
    assert_eq!(sdl_button_index(&keys, 0x122), Some(2));
    assert_eq!(sdl_button_index(&keys, 0x1e), Some(3));
}

#[test]
fn a_code_the_pad_does_not_report_is_absent() {
    let keys = plain_pad();
    for code in [0x00_u16, 0x1e, 0xff, 0x100, 0x11f, 0x12c, 0x2c0, u16::MAX] {
        assert_eq!(
            sdl_button_index(&keys, code),
            None,
            "sdl invented a number for {code:#x}"
        );
    }
}

#[test]
fn an_empty_key_list_numbers_nothing() {
    assert_eq!(sdl_button_index(&[], 0x120), None);
    assert_eq!(sdl_button_index(&[], 0x1e), None);
}

#[test]
fn indices_do_not_depend_on_the_order_the_codes_arrive_in() {
    let sorted = [0x1e_u16, 0x100, 0x101, 0x120, 0x121, 0x122, 0x2c0];
    let shuffled = [0x2c0_u16, 0x121, 0x1e, 0x122, 0x100, 0x120, 0x101];
    let reversed: Vec<u16> = sorted.iter().rev().copied().collect();
    for code in sorted {
        for other in [shuffled.as_slice(), reversed.as_slice()] {
            assert_eq!(
                sdl_button_index(&sorted, code),
                sdl_button_index(other, code),
                "sdl moved {code:#x}"
            );
        }
    }
}

#[test]
fn a_duplicated_code_shifts_every_later_button() {
    let once = [0x120_u16, 0x121, 0x122];
    let twice = [0x120_u16, 0x120, 0x121, 0x122];
    assert_eq!(
        sdl_button_index(&twice, 0x120),
        Some(0),
        "the first copy wins"
    );
    assert_eq!(sdl_button_index(&twice, 0x121), Some(2));
    assert_eq!(sdl_button_index(&once, 0x121), Some(1));
}

#[test]
fn trigger_happy_codes_are_ordinary_buttons() {
    // BTN_TRIGGER_HAPPY1 is 0x2c0; arcade encoders with more than sixteen.
    let mut keys: Vec<u16> = (BTN_JOYSTICK..BTN_JOYSTICK + 4).collect();
    keys.extend(0x2c0_u16..0x2c8);
    for (expected, code) in keys.iter().copied().enumerate() {
        assert_eq!(
            sdl_button_index(&keys, code),
            Some(expected as i32),
            "sdl for {code:#x}"
        );
    }
    assert_eq!(sdl_button_index(&keys, 0x2c0), Some(4));
    assert_eq!(sdl_button_index(&keys, 0x2c7), Some(11));
}

#[test]
fn sdl_numbers_every_reported_code_exactly_once_from_zero() {
    let keys = [0x1e_u16, 0x2c, 0x100, 0x110, 0x120, 0x13f, 0x2c0];
    let numbers: BTreeSet<i32> = keys
        .iter()
        .map(|code| sdl_button_index(&keys, *code).expect("a reported code has a number"))
        .collect();
    let expected: BTreeSet<i32> = (0..keys.len() as i32).collect();
    assert_eq!(numbers, expected);
}

#[test]
fn a_pad_of_nothing_but_keyboard_codes_is_numbered_from_zero_end_to_end() {
    let keys = [0x1e_u16, 0x1f, 0x20];
    for (expected, code) in keys.iter().copied().enumerate() {
        let index = sdl_button_index(&keys, code).expect("a reported code has a number");
        assert_eq!(index, expected as i32);
        let binding = Binding::button(index);
        assert_eq!(binding.sdl().expect("sdl"), format!("b{expected}"));
    }
}

#[test]
fn hats_are_skipped_so_abs_rz_is_not_axis_five() {
    let codes = [0x00_u16, 0x01, 0x02, 0x03, 0x04, 0x05, 0x10, 0x11];
    assert_eq!(axis_index(&codes, 0x00), Some(0));
    assert_eq!(
        axis_index(&codes, 0x05),
        Some(5),
        "contiguous here, by luck"
    );
    let sparse = [0x00_u16, 0x01, 0x03, 0x04, 0x05, 0x10, 0x11];
    assert_eq!(axis_index(&sparse, 0x03), Some(2));
    assert_eq!(
        axis_index(&sparse, 0x05),
        Some(4),
        "ABS_RZ is code 5 and axis 4"
    );
    assert_eq!(axis_index(&sparse, 0x10), None, "a hat is not an axis");
    assert_eq!(axis_index(&sparse, 0x11), None);
}

#[test]
fn a_hat_in_the_middle_of_the_axis_list_does_not_consume_a_number() {
    let codes = [
        0x00_u16, 0x01, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x28, 0x29,
    ];
    assert_eq!(axis_index(&codes, 0x00), Some(0));
    assert_eq!(axis_index(&codes, 0x01), Some(1));
    assert_eq!(
        axis_index(&codes, 0x28),
        Some(2),
        "the hats in between count for nothing"
    );
    assert_eq!(axis_index(&codes, 0x29), Some(3));
    for hat in HAT_CODES {
        assert_eq!(
            axis_index(&codes, hat),
            None,
            "hat {hat:#x} was numbered as an axis"
        );
    }
}

#[test]
fn the_hat_range_is_half_open_so_the_code_just_past_it_is_an_axis() {
    // ABS_HAT3Y is 0x17 and ABS_PRESSURE is 0x18. An off-by-one at this edge.
    assert_eq!(HAT_CODES, 0x10..0x18);
    let codes = [0x0f_u16, 0x10, 0x17, 0x18];
    assert_eq!(axis_index(&codes, 0x0f), Some(0), "0x0f is below the hats");
    assert_eq!(axis_index(&codes, 0x10), None);
    assert_eq!(axis_index(&codes, 0x17), None);
    assert_eq!(axis_index(&codes, 0x18), Some(1), "0x18 is past the hats");
}

#[test]
fn a_pad_of_nothing_but_hats_has_no_axes_at_all() {
    let codes: Vec<u16> = HAT_CODES.collect();
    for code in &codes {
        assert_eq!(axis_index(&codes, *code), None, "{code:#x}");
    }
    assert_eq!(
        axis_index(&codes, 0x00),
        None,
        "an absent code is still absent"
    );
}

#[test]
fn an_empty_or_absent_axis_list_numbers_nothing() {
    assert_eq!(axis_index(&[], 0x00), None);
    assert_eq!(
        axis_index(&[0x00, 0x01], 0x05),
        None,
        "an unreported axis has no number"
    );
    assert_eq!(axis_index(&[0x00, 0x01], u16::MAX), None);
}

#[test]
fn axis_numbering_ignores_the_order_the_codes_arrive_in() {
    let sorted = [0x00_u16, 0x01, 0x03, 0x04, 0x05, 0x10, 0x11, 0x18];
    let shuffled = [0x11_u16, 0x05, 0x00, 0x18, 0x04, 0x10, 0x03, 0x01];
    for code in sorted {
        assert_eq!(
            axis_index(&sorted, code),
            axis_index(&shuffled, code),
            "{code:#x} moved"
        );
    }
}

#[test]
fn a_duplicated_axis_code_shifts_every_later_axis() {
    let once = [0x00_u16, 0x01, 0x03];
    let twice = [0x00_u16, 0x01, 0x01, 0x03];
    assert_eq!(axis_index(&once, 0x03), Some(2));
    assert_eq!(axis_index(&twice, 0x01), Some(1), "the first copy wins");
    assert_eq!(axis_index(&twice, 0x03), Some(3));
}

#[test]
fn a_binding_writes_the_python_field_names() {
    let binding = Binding::hat(0, 4);
    let json = serde_json::to_value(binding).expect("serialize a binding");
    assert_eq!(json["kind"], "hat");
    assert_eq!(json["index"], 0);
    assert_eq!(json["value"], 4);
    let object = json.as_object().expect("a binding is an object");
    assert_eq!(
        object.len(),
        3,
        "an extra field is one the Python's reader would ignore"
    );
    let back: Binding = serde_json::from_value(json).expect("deserialize a binding");
    assert_eq!(back, binding);
}

#[test]
fn a_stored_binding_carrying_an_ra_index_still_reads_and_drops_it_on_write() {
    for ra_index in [
        serde_json::Value::Null,
        serde_json::json!(-1),
        serde_json::json!(0),
        serde_json::json!(11),
    ] {
        let raw =
            serde_json::json!({"kind": "button", "index": 7, "value": 0, "ra_index": ra_index});
        let binding: Binding = serde_json::from_value(raw).expect("deserialize");
        assert_eq!(binding, Binding::button(7), "ra_index {ra_index}");
        let json = serde_json::to_value(binding).expect("serialize");
        assert!(json.get("ra_index").is_none(), "{json}");
    }
}

#[test]
fn an_absent_value_reads_as_zero() {
    let raw = serde_json::json!({"kind": "axis", "index": 2});
    let binding: Binding = serde_json::from_value(raw).expect("deserialize");
    assert_eq!(binding.value, 0);
    assert_eq!(
        binding.sdl().expect("sdl"),
        "+a2",
        "zero is the positive half"
    );
}

#[test]
fn every_kind_round_trips_under_its_lowercase_name() {
    for (kind, name) in [
        (BindingKind::Button, "button"),
        (BindingKind::Hat, "hat"),
        (BindingKind::Axis, "axis"),
    ] {
        let raw = serde_json::json!({"kind": name, "index": 1, "value": 1});
        let binding: Binding = serde_json::from_value(raw).expect("deserialize");
        assert_eq!(binding.kind, kind);
    }
}

#[test]
fn an_unknown_kind_is_refused_rather_than_read_as_a_button() {
    for name in ["Button", "BUTTON", "trigger", "", "buttons"] {
        let raw = serde_json::json!({"kind": name, "index": 1});
        let parsed: Result<Binding, _> = serde_json::from_value(raw);
        assert!(parsed.is_err(), "kind {name:?} was accepted");
    }
}

#[test]
fn a_binding_with_no_index_is_refused_rather_than_defaulted_to_button_zero() {
    let raw = serde_json::json!({"kind": "button"});
    let parsed: Result<Binding, _> = serde_json::from_value(raw);
    assert!(parsed.is_err(), "a binding with no index was accepted");
    let no_kind = serde_json::json!({"index": 3});
    let parsed: Result<Binding, _> = serde_json::from_value(no_kind);
    assert!(parsed.is_err(), "a binding with no kind was accepted");
}

#[test]
fn every_shape_survives_a_json_round_trip_unchanged() {
    for binding in every_shape() {
        let json = serde_json::to_value(binding).expect("serialize");
        let back: Binding = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, binding);
        assert_eq!(back.sdl(), binding.sdl());
    }
}
