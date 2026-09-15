//! The vocabulary and the numbering, held still.
//!
//! Everything in `control` and `binding` is a name or a small integer, and
//! every one of them ends up in a file some other program reads without ever
//! saying it disagreed. RetroArch binds a button that does not exist and still
//! reports the pad as configured; SDL skips a mapping line it cannot parse and
//! leaves the control dead. So the tests here are less about arithmetic than
//! about pinning the exact spellings and the exact off-by-ones, because the
//! failure mode for all of them is "the mapping did not take" with nothing
//! logged anywhere.
//!
//! The Python this was ported from is `src/padmap/mapping.py`; where a rule
//! looks arbitrary, its comment there says which real controller caused it.

use std::collections::{BTreeMap, BTreeSet};

use padmap_core::binding::{
    axis_index, hat_direction, retroarch_button_index, sdl_button_index, Binding, BindingKind,
    Unspellable, BTN_JOYSTICK, BTN_MISC, HAT_CODES, RA_INVISIBLE,
};
use padmap_core::control::{Control, UnknownControl, CANONICAL_ORDER};

// ---------------------------------------------------------------------------
// Control: the eighteen names
// ---------------------------------------------------------------------------

/// The Python's `list(SDL_FIELDS)`, copied by hand from `mapping.py`.
///
/// Pinned as a literal rather than derived from the enum: these strings sit in
/// every stored profile on every user's disk, so a rename is a silent data
/// migration, not a refactor.
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
    let names: Vec<&str> = CANONICAL_ORDER.iter().map(|c| c.as_str()).collect();
    assert_eq!(
        names, PYTHON_NAMES,
        "a renamed control orphans every stored profile"
    );
}

#[test]
fn every_control_round_trips_through_its_name_display_and_parse() {
    for control in Control::ALL {
        let name = control.as_str();
        assert_eq!(name.parse::<Control>(), Ok(control));
        // Display is what interpolates into log lines and into the wire
        // protocol; if it ever diverges from as_str the daemon and the CLI
        // start naming the same button two different things.
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
        18,
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
    // `sdl::lines` walks CANONICAL_ORDER but the bindings arrive in a
    // BTreeMap, and elsewhere the map's own iteration order is what gets
    // written. If the enum is reordered without the array, the two disagree
    // and a regenerated mapping looks like a change in every diff.
    let sorted: Vec<Control> = Control::ALL
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(sorted.as_slice(), CANONICAL_ORDER.as_slice());
}

#[test]
fn a_name_that_differs_only_in_case_or_whitespace_is_refused() {
    // Refused rather than trimmed: a profile holding " a" was written by
    // something that is not padmap, and guessing at it hides the real fault.
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
    // SDL field names, RetroArch keys and SDL controls padmap does not model
    // all look like control names and are not. Accepting one would store a
    // binding under a name no consumer ever looks up: the button captures
    // fine and does nothing in the game.
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
        "input_b_btn",
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
    // Quoted, so a name that is whitespace or empty is still visible in the
    // message rather than vanishing into the sentence.
    assert_eq!(error.to_string(), "\"guide\" is not a canonical control");
    let padded = " a"
        .parse::<Control>()
        .expect_err("a padded name is not a control");
    assert_eq!(padded.to_string(), "\" a\" is not a canonical control");
    let error: &dyn std::error::Error = &padded;
    assert!(error.source().is_none());
}

// ---------------------------------------------------------------------------
// Control: the two consumers' spellings
// ---------------------------------------------------------------------------

#[test]
fn sdl_field_is_total_and_injective() {
    // Totality is the compiler's job -- the match has no wildcard -- so what
    // is left to check is that two controls never claim one field. They would
    // not collide loudly: the second line overwrites the first in SDL's
    // parser and the pad reads as mapped.
    let mut seen: BTreeMap<&str, Control> = BTreeMap::new();
    for control in Control::ALL {
        let field = control.sdl_field();
        assert!(!field.is_empty(), "{control} has no SDL field");
        if let Some(other) = seen.insert(field, control) {
            panic!("{control} and {other} both write the SDL field {field:?}");
        }
    }
    assert_eq!(seen.len(), 18);
}

#[test]
fn retroarch_key_is_total_and_injective() {
    let mut seen: BTreeMap<&str, Control> = BTreeMap::new();
    for control in Control::ALL {
        let key = control.retroarch_key();
        assert!(
            key.starts_with("input_"),
            "{control} has a key RetroArch will not read: {key:?}"
        );
        assert!(
            key.ends_with("_btn"),
            "{control}'s key is not a button key: {key:?}"
        );
        if let Some(other) = seen.insert(key, control) {
            panic!("{control} and {other} both write the RetroArch key {key:?}");
        }
    }
    assert_eq!(seen.len(), 18);
}

#[test]
fn retroarch_and_sdl_disagree_about_a_and_b_deliberately() {
    // RetroArch's a/b are the Nintendo positions: b is the bottom face
    // button, a is the right one. SDL's a/b are bottom and right. Crossing
    // them swaps confirm and cancel in every game, and nothing reports an
    // error -- it reads as "the mapping did not take".
    assert_eq!(Control::A.retroarch_key(), "input_b_btn");
    assert_eq!(Control::B.retroarch_key(), "input_a_btn");
    assert_eq!(Control::X.retroarch_key(), "input_y_btn");
    assert_eq!(Control::Y.retroarch_key(), "input_x_btn");
    // ...while SDL calls each of the four by its own letter.
    assert_eq!(Control::A.sdl_field(), "a");
    assert_eq!(Control::B.sdl_field(), "b");
    assert_eq!(Control::X.sdl_field(), "x");
    assert_eq!(Control::Y.sdl_field(), "y");
}

#[test]
fn the_face_buttons_are_the_only_controls_whose_two_spellings_cross() {
    // Everything else maps letter-for-letter, so a future control added by
    // copying its neighbour is right by default. Only these four are not.
    let crossed = [Control::A, Control::B, Control::X, Control::Y];
    for control in Control::ALL {
        let key_stem = control
            .retroarch_key()
            .trim_start_matches("input_")
            .trim_end_matches("_btn");
        if crossed.contains(&control) {
            assert_ne!(key_stem, control.sdl_field(), "{control} should cross");
        }
    }
    assert_eq!(Control::Back.retroarch_key(), "input_select_btn");
    assert_eq!(Control::Start.retroarch_key(), "input_start_btn");
    assert_eq!(Control::LeftShoulder.retroarch_key(), "input_l_btn");
    assert_eq!(Control::RightShoulder.retroarch_key(), "input_r_btn");
    assert_eq!(Control::LeftTrigger.retroarch_key(), "input_l2_btn");
    assert_eq!(Control::RightTrigger.retroarch_key(), "input_r2_btn");
    assert_eq!(Control::DpadUp.retroarch_key(), "input_up_btn");
    assert_eq!(Control::DpadDown.retroarch_key(), "input_down_btn");
    assert_eq!(Control::DpadLeft.retroarch_key(), "input_left_btn");
    assert_eq!(Control::DpadRight.retroarch_key(), "input_right_btn");
}

#[test]
fn the_four_right_stick_halves_spell_the_same_direction_to_both_consumers() {
    // An N64 pad's C-buttons are these. SDL has no "C-up", so the button is
    // written as a half-axis target; RetroArch drives the stick axis from a
    // button instead. Y is positive downwards in both, which is the part that
    // is easy to invert: getting it wrong points the C-stick the wrong way
    // and every camera control in the game fights the player.
    let halves = [
        (Control::RightStickUp, "-righty", "input_r_y_minus_btn"),
        (Control::RightStickDown, "+righty", "input_r_y_plus_btn"),
        (Control::RightStickLeft, "-rightx", "input_r_x_minus_btn"),
        (Control::RightStickRight, "+rightx", "input_r_x_plus_btn"),
    ];
    for (control, field, key) in halves {
        assert_eq!(control.sdl_field(), field, "{control}'s SDL half-axis");
        assert_eq!(control.retroarch_key(), key, "{control}'s RetroArch key");
    }
}

#[test]
fn only_the_stick_halves_have_an_sdl_field_unlike_their_own_name() {
    // The invariant that makes the table above readable: every other control
    // is spelled to SDL exactly as padmap spells it internally, so a mismatch
    // anywhere else is a typo rather than a deliberate translation.
    let halves = [
        Control::RightStickUp,
        Control::RightStickDown,
        Control::RightStickLeft,
        Control::RightStickRight,
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
        // SDL reads a leading sign as "only this half of the axis". A stray
        // one on a face button makes the whole line unparseable and SDL drops
        // it without a word.
        assert_eq!(
            signed,
            field.contains("right") && field.ends_with(['x', 'y'])
        );
    }
}

// ---------------------------------------------------------------------------
// Control: serde
// ---------------------------------------------------------------------------

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
    // Defaulting would bind the user's press to whichever control sorted
    // first, which is a working button in the front-end and the wrong one in
    // the game.
    for raw in ["\"guide\"", "\"A\"", "\"\"", "\"dpUp\""] {
        let parsed: Result<Control, _> = serde_json::from_str(raw);
        assert!(parsed.is_err(), "{raw} deserialized");
    }
    let wrong_type: Result<Control, _> = serde_json::from_str("3");
    assert!(wrong_type.is_err(), "a control is a string, not an index");
}

#[test]
fn a_profile_keyed_by_control_round_trips_through_json() {
    // A stored capture is an object keyed by control name, so Control has to
    // work as a serde map key and not only as a value.
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

// ---------------------------------------------------------------------------
// Binding: hats
// ---------------------------------------------------------------------------

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
fn only_the_four_single_hat_bits_are_spellable_and_both_consumers_agree() {
    // The asymmetry is the bug being guarded against: if one consumer renders
    // a diagonal and the other refuses it, the d-pad exists in the front-end
    // and not in the game, or the reverse, and neither says so.
    for value in -8_i32..=16 {
        let binding = Binding::hat(0, value);
        let spellable = matches!(value, 1 | 2 | 4 | 8);
        assert_eq!(
            binding.sdl_visible(),
            spellable,
            "sdl_visible for hat {value}"
        );
        assert_eq!(
            binding.retroarch_visible(),
            spellable,
            "retroarch_visible for hat {value}"
        );
        match (binding.sdl(), binding.retroarch()) {
            (Ok(_), Ok(_)) => assert!(spellable, "hat {value} was spelled by both"),
            (Err(from_sdl), Err(from_retroarch)) => {
                assert!(!spellable, "hat {value} was refused by both");
                assert_eq!(from_sdl, Unspellable::HatValue(value));
                assert_eq!(from_retroarch, Unspellable::HatValue(value));
            }
            (from_sdl, from_retroarch) => panic!(
                "hat {value} split the consumers: sdl {from_sdl:?}, retroarch {from_retroarch:?}"
            ),
        }
    }
}

#[test]
fn the_four_hat_directions_spell_a_bit_to_sdl_and_a_word_to_retroarch() {
    for (value, word) in [(1, "up"), (2, "right"), (4, "down"), (8, "left")] {
        let binding = Binding::hat(2, value);
        assert_eq!(
            binding.sdl().expect("sdl spells a hat"),
            format!("h2.{value}")
        );
        assert_eq!(
            binding.retroarch().expect("retroarch spells a hat"),
            format!("h2{word}")
        );
    }
}

#[test]
fn a_hat_ra_index_renumbers_retroarch_only() {
    let binding = Binding::hat(1, 4).with_ra_index(Some(0));
    assert_eq!(binding.sdl().expect("sdl"), "h1.4");
    assert_eq!(binding.retroarch().expect("retroarch"), "h0down");
}

#[test]
fn a_hat_value_that_names_nothing_reports_the_value_it_was_given() {
    // The message is the only thing a user editing a profile by hand has to
    // go on, so it has to carry the offending number rather than say "bad".
    let error = Binding::hat(0, 3)
        .sdl()
        .expect_err("a diagonal is not a control");
    assert_eq!(error.to_string(), "hat value 3 is not one direction bit");
    assert_eq!(
        Binding::hat(0, 3)
            .retroarch()
            .expect_err("same from retroarch"),
        error
    );
}

// ---------------------------------------------------------------------------
// Binding: buttons and axes
// ---------------------------------------------------------------------------

#[test]
fn a_button_spells_a_bare_number_to_retroarch_and_a_prefixed_one_to_sdl() {
    for index in [0, 1, 7, 13, 127, i32::MAX] {
        let binding = Binding::button(index);
        assert_eq!(binding.sdl().expect("sdl"), format!("b{index}"));
        assert_eq!(binding.retroarch().expect("retroarch"), index.to_string());
    }
}

#[test]
fn a_split_index_spells_differently_to_each_consumer() {
    // The whole reason ra_index exists: a combo adapter's twelve buttons are
    // 0..11 to RetroArch and the thirteenth code shifts SDL's count.
    let binding = Binding::button(13).with_ra_index(Some(11));
    assert_eq!(binding.sdl().expect("sdl"), "b13");
    assert_eq!(binding.retroarch().expect("retroarch"), "11");
}

#[test]
fn an_ra_index_of_zero_is_not_the_same_as_an_absent_one() {
    // Zero is a real button. Treating Some(0) as "no override" would send
    // RetroArch the SDL number, which on the pad that needs the override is
    // always the wrong button.
    let overridden = Binding::button(5).with_ra_index(Some(0));
    assert_eq!(overridden.retroarch().expect("retroarch"), "0");
    let absent = Binding::button(5);
    assert_eq!(absent.retroarch().expect("retroarch"), "5");
}

#[test]
fn a_button_retroarch_cannot_see_is_refused_rather_than_numbered() {
    let binding = Binding::button(13).with_ra_index(Some(RA_INVISIBLE));
    assert!(binding.sdl_visible(), "SDL can still reach it");
    assert_eq!(binding.sdl().expect("sdl"), "b13");
    assert!(!binding.retroarch_visible());
    assert_eq!(
        binding.retroarch(),
        Err(Unspellable::InvisibleToRetroarch),
        "a negative index must never become a plausible button number"
    );
}

#[test]
fn any_negative_ra_index_is_refused_not_only_the_sentinel() {
    // A profile off disk can hold whatever it likes, and no real button index
    // is negative. -13 formatted as a button number is what RetroArch would
    // otherwise bind, silently, to nothing.
    for index in [-1, -2, -13, -99, i32::MIN, i32::MIN + 1] {
        let binding = Binding::button(4).with_ra_index(Some(index));
        assert!(
            !binding.retroarch_visible(),
            "ra_index {index} was accepted"
        );
        assert_eq!(binding.retroarch(), Err(Unspellable::InvisibleToRetroarch));
        // SDL is unaffected -- it never reads ra_index at all.
        assert_eq!(binding.sdl().expect("sdl"), "b4");
    }
}

#[test]
fn the_invisible_sentinel_is_still_minus_one() {
    // Stored in profiles on disk. Changing it reinterprets every existing
    // "RetroArch cannot see this" as an ordinary button number.
    assert_eq!(RA_INVISIBLE, -1);
}

#[test]
fn a_button_with_a_negative_index_and_no_ra_index_is_visible_but_unspellable() {
    // Documented, not endorsed. `retroarch_visible` only inspects ra_index,
    // so a button whose *own* index is negative passes the visibility check
    // and then fails to spell. Ported faithfully from mapping.py, which has
    // the same gap; a caller that trusts retroarch_visible and then unwraps
    // would panic on a hand-edited profile. Every caller in this crate
    // handles the Err, so today it costs a dropped line rather than a crash.
    let binding = Binding::button(-3);
    assert!(binding.retroarch_visible(), "the gap: visibility says yes");
    assert_eq!(
        binding.retroarch(),
        Err(Unspellable::InvisibleToRetroarch),
        "and spelling says no"
    );
    assert_eq!(
        binding.sdl().expect("sdl"),
        "b-3",
        "SDL spells it regardless"
    );
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
        assert_eq!(binding.retroarch().expect("retroarch"), format!("{sign}2"));
    }
}

#[test]
fn axis_zero_keeps_its_sign_even_though_retroarch_will_misparse_it() {
    // RetroArch parses a _btn value with strtoull, so "-0" and "+0" both come
    // out as button 0: the two halves of one stick collapse and pressing
    // either activates both. `retroarch::lines` moves an axis onto the _axis
    // key to dodge that, which only works if the sign survives to here.
    assert_eq!(Binding::axis(0, -1).retroarch().expect("retroarch"), "-0");
    assert_eq!(Binding::axis(0, 1).retroarch().expect("retroarch"), "+0");
    assert_eq!(Binding::axis(0, 0).retroarch().expect("retroarch"), "+0");
    assert_eq!(Binding::axis(0, -1).sdl().expect("sdl"), "-a0");
    assert_eq!(Binding::axis(0, 1).sdl().expect("sdl"), "+a0");
}

#[test]
fn an_axis_ra_index_wins_over_the_sdl_one() {
    let binding = Binding::axis(5, -1).with_ra_index(Some(2));
    assert_eq!(binding.sdl().expect("sdl"), "-a5");
    assert_eq!(binding.retroarch().expect("retroarch"), "-2");
}

#[test]
fn an_axis_is_never_refused_for_a_negative_ra_index_the_way_a_button_is() {
    // Deliberate, and worth pinning because it looks like an oversight: the
    // sub-BTN_MISC problem is about evdev *key* codes, so an axis has no
    // equivalent and the guard would only ever reject a real binding.
    let binding = Binding::axis(3, -1).with_ra_index(Some(RA_INVISIBLE));
    assert!(binding.retroarch_visible());
    assert_eq!(binding.retroarch().expect("retroarch"), "--1");
}

#[test]
fn an_extreme_index_neither_panics_nor_wraps() {
    // A hand-edited or corrupt profile is the source. Formatting must not
    // overflow, and the number that comes out must still be the one that went
    // in rather than a wrapped positive that names a real button.
    for index in [i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX] {
        assert_eq!(
            Binding::axis(index, 1).sdl().expect("sdl"),
            format!("+a{index}")
        );
        assert_eq!(
            Binding::axis(index, 1).retroarch().expect("retroarch"),
            format!("+{index}")
        );
        assert_eq!(
            Binding::hat(index, 8).sdl().expect("sdl"),
            format!("h{index}.8")
        );
        assert_eq!(
            Binding::hat(index, 8).retroarch().expect("retroarch"),
            format!("h{index}left")
        );
        assert_eq!(
            Binding::button(index).sdl().expect("sdl"),
            format!("b{index}")
        );
    }
    assert_eq!(
        Binding::button(i32::MAX).retroarch().expect("retroarch"),
        "2147483647"
    );
    assert_eq!(
        Binding::hat(0, i32::MIN).sdl(),
        Err(Unspellable::HatValue(i32::MIN))
    );
    assert_eq!(
        Binding::hat(0, i32::MAX).retroarch(),
        Err(Unspellable::HatValue(i32::MAX))
    );
}

#[test]
fn the_constructors_leave_the_second_numbering_absent() {
    // Absent means "the two consumers agree", which is the case on most pads
    // and the only safe default: inventing a number here is exactly what
    // RA_INVISIBLE exists to stop.
    assert_eq!(Binding::button(3).ra_index, None);
    assert_eq!(Binding::hat(0, 1).ra_index, None);
    assert_eq!(Binding::axis(2, -1).ra_index, None);
    assert_eq!(
        Binding::button(3).value,
        0,
        "a button has no value to carry"
    );
}

#[test]
fn with_ra_index_returns_a_new_binding_rather_than_editing_the_old_one() {
    let original = Binding::button(13);
    let renumbered = original.with_ra_index(Some(11));
    assert_eq!(original.ra_index, None, "the original was mutated");
    assert_eq!(renumbered.ra_index, Some(11));
    assert_eq!(renumbered.index, original.index);
    assert_eq!(
        renumbered.with_ra_index(None).ra_index,
        None,
        "an override can be cleared"
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

// ---------------------------------------------------------------------------
// Binding: visibility agrees with spellability, over every shape
// ---------------------------------------------------------------------------

/// Every interesting `Binding` shape: kind x index x value x ra_index.
///
/// Built as a cross-product rather than case by case because the property
/// being checked is about *all* of them -- one shape where visibility and
/// spelling disagree is one control that either crashes a caller or goes
/// missing from a file with nothing said.
fn every_shape() -> Vec<Binding> {
    let kinds = [BindingKind::Button, BindingKind::Hat, BindingKind::Axis];
    let indices = [i32::MIN, -7, -1, 0, 1, 2, 13, i32::MAX];
    let values = [i32::MIN, -2, -1, 0, 1, 2, 3, 4, 8, 15, 16, i32::MAX];
    let ra_indices = [
        None,
        Some(i32::MIN),
        Some(RA_INVISIBLE),
        Some(0),
        Some(1),
        Some(11),
        Some(i32::MAX),
    ];
    let mut shapes = Vec::new();
    for kind in kinds {
        for index in indices {
            for value in values {
                for ra_index in ra_indices {
                    shapes.push(Binding {
                        kind,
                        index,
                        value,
                        ra_index,
                    });
                }
            }
        }
    }
    shapes
}

#[test]
fn the_cross_product_is_the_size_it_claims_to_be() {
    // A generator that quietly produces nothing turns the invariant below
    // into a test that passes by doing nothing at all.
    assert_eq!(every_shape().len(), 3 * 8 * 12 * 7);
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

#[test]
fn retroarch_visibility_answers_whether_retroarch_can_spell_it_except_for_one_gap() {
    // The exception is carved out rather than papered over: see
    // `a_button_with_a_negative_index_and_no_ra_index_is_visible_but_unspellable`.
    for binding in every_shape() {
        let gap = binding.kind == BindingKind::Button
            && binding.ra_index.is_none()
            && binding.index < 0
            && binding.sdl_visible();
        if gap {
            assert!(binding.retroarch_visible(), "{binding:?}");
            assert!(binding.retroarch().is_err(), "{binding:?}");
        } else {
            assert_eq!(
                binding.retroarch_visible(),
                binding.retroarch().is_ok(),
                "{binding:?}: asking and doing gave different answers"
            );
        }
    }
}

#[test]
fn a_binding_one_consumer_refuses_for_its_hat_value_is_refused_by_the_other_too() {
    for binding in every_shape() {
        if binding.kind != BindingKind::Hat {
            continue;
        }
        let from_sdl = binding.sdl().is_err();
        let from_retroarch = binding.retroarch().is_err();
        assert_eq!(
            from_sdl, from_retroarch,
            "{binding:?}: a hat rendered by one consumer and refused by the other leaves the \
             d-pad working in one place and dead in the other"
        );
    }
}

#[test]
fn no_negative_number_ever_reaches_a_retroarch_button_value() {
    // The wound: RetroArch binds a button that does not exist without
    // complaining and still reports the pad as configured.
    for binding in every_shape() {
        if binding.kind != BindingKind::Button {
            continue;
        }
        if let Ok(spelled) = binding.retroarch() {
            assert!(!spelled.starts_with('-'), "{binding:?} spelled {spelled:?}");
            let parsed: i32 = spelled.parse().expect("a button is a plain number");
            assert!(parsed >= 0, "{binding:?} spelled {spelled:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// sdl_button_index / retroarch_button_index
// ---------------------------------------------------------------------------

/// The ordinary pad: twelve buttons running from BTN_JOYSTICK.
fn plain_pad() -> Vec<u16> {
    (BTN_JOYSTICK..BTN_JOYSTICK + 12).collect()
}

#[test]
fn the_two_numberings_are_defined_to_start_where_they_say_they_do() {
    assert_eq!(BTN_MISC, 0x100);
    assert_eq!(BTN_JOYSTICK, 0x120);
    // SDL starts counting later than RetroArch, which is the whole problem.
    assert_eq!(BTN_MISC.min(BTN_JOYSTICK), BTN_MISC);
}

#[test]
fn a_pad_whose_codes_all_start_at_btn_joystick_numbers_the_same_for_both() {
    // Most pads. This is why the difference below can go unnoticed for years.
    let keys = plain_pad();
    for (expected, code) in keys.iter().copied().enumerate() {
        let expected = expected as i32;
        assert_eq!(
            sdl_button_index(&keys, code),
            Some(expected),
            "sdl for {code:#x}"
        );
        assert_eq!(
            retroarch_button_index(&keys, code),
            Some(expected),
            "retroarch for {code:#x}"
        );
    }
    // Pinned against a mapping SDL itself wrote on the measured pad.
    assert_eq!(sdl_button_index(&keys, 0x121), Some(1));
    assert_eq!(sdl_button_index(&keys, 0x128), Some(8));
}

#[test]
fn a_pad_carrying_btn_misc_codes_makes_the_two_numberings_disagree() {
    // BTN_0..BTN_2 are 0x100..0x102 -- arcade encoders report them. SDL walks
    // BTN_JOYSTICK..KEY_MAX first and only then 0..BTN_JOYSTICK, so those
    // three sort last for SDL and first for RetroArch. Storing one numbering
    // and recomputing the other shifts every binding on such a pad.
    let keys = [0x100_u16, 0x101, 0x102, 0x120, 0x121, 0x122];
    assert_eq!(retroarch_button_index(&keys, 0x100), Some(0));
    assert_eq!(retroarch_button_index(&keys, 0x101), Some(1));
    assert_eq!(retroarch_button_index(&keys, 0x102), Some(2));
    assert_eq!(retroarch_button_index(&keys, 0x120), Some(3));
    assert_eq!(retroarch_button_index(&keys, 0x122), Some(5));
    assert_eq!(sdl_button_index(&keys, 0x120), Some(0));
    assert_eq!(sdl_button_index(&keys, 0x122), Some(2));
    assert_eq!(sdl_button_index(&keys, 0x100), Some(3));
    assert_eq!(sdl_button_index(&keys, 0x102), Some(5));
    for code in keys {
        assert_ne!(
            sdl_button_index(&keys, code),
            retroarch_button_index(&keys, code),
            "{code:#x} happens to agree, which would weaken this test"
        );
    }
}

#[test]
fn a_keyboard_code_sorts_after_every_joystick_button_for_sdl() {
    // KEY_A (0x1e) from a combo adapter. SDL puts it last; it is still a real
    // SDL button and a mapping may legitimately use it.
    let keys = [0x1e_u16, 0x120, 0x121, 0x122];
    assert_eq!(sdl_button_index(&keys, 0x120), Some(0));
    assert_eq!(sdl_button_index(&keys, 0x122), Some(2));
    assert_eq!(sdl_button_index(&keys, 0x1e), Some(3));
}

#[test]
fn retroarch_answers_invisible_not_absent_for_a_reported_keyboard_code() {
    // The distinction RA_INVISIBLE exists for. None means "the two consumers
    // agree", so answering None here stored index 3 for a pad RetroArch
    // numbers 0..2: RetroArch binds a button that does not exist without
    // complaining and still reports the pad as configured, and the control
    // works in the front-end and is dead in every game.
    let keys = [0x1e_u16, 0x120, 0x121, 0x122];
    assert_eq!(retroarch_button_index(&keys, 0x1e), Some(RA_INVISIBLE));
    assert_ne!(retroarch_button_index(&keys, 0x1e), None);
    assert_eq!(
        Binding::button(3)
            .with_ra_index(retroarch_button_index(&keys, 0x1e))
            .retroarch(),
        Err(Unspellable::InvisibleToRetroarch),
        "the sentinel has to survive all the way into the spelled line"
    );
    // Every KEY_* code below BTN_MISC, not just the one.
    for code in [0x01_u16, 0x1e, 0x2c, 0x9e, 0xff] {
        let keys = [code, 0x120, 0x121];
        assert_eq!(
            retroarch_button_index(&keys, code),
            Some(RA_INVISIBLE),
            "{code:#x}"
        );
    }
    // ...and the boundary itself is visible: BTN_MISC is enumerated.
    let at_boundary = [BTN_MISC, 0x120];
    assert_eq!(retroarch_button_index(&at_boundary, BTN_MISC), Some(0));
    let below_boundary = [BTN_MISC - 1, 0x120];
    assert_eq!(
        retroarch_button_index(&below_boundary, BTN_MISC - 1),
        Some(RA_INVISIBLE)
    );
}

#[test]
fn a_code_the_pad_does_not_report_is_absent_from_both() {
    let keys = plain_pad();
    for code in [0x00_u16, 0x1e, 0xff, 0x100, 0x11f, 0x12c, 0x2c0, u16::MAX] {
        assert_eq!(
            sdl_button_index(&keys, code),
            None,
            "sdl invented a number for {code:#x}"
        );
        assert_eq!(
            retroarch_button_index(&keys, code),
            None,
            "retroarch invented a number for {code:#x}"
        );
    }
}

#[test]
fn an_empty_key_list_numbers_nothing_not_even_an_invisible_code() {
    // A pad that reports no keys at all is a pad still enumerating, or one
    // that is really an accelerometer. Answering RA_INVISIBLE here would say
    // "the code exists but RetroArch cannot see it", which is a different and
    // wrong claim.
    assert_eq!(sdl_button_index(&[], 0x120), None);
    assert_eq!(retroarch_button_index(&[], 0x120), None);
    assert_eq!(sdl_button_index(&[], 0x1e), None);
    assert_eq!(
        retroarch_button_index(&[], 0x1e),
        None,
        "absent beats invisible"
    );
}

#[test]
fn indices_do_not_depend_on_the_order_the_codes_arrive_in() {
    // evdev's EVIOCGBIT walk happens to be ascending, but nothing in the API
    // promises it, and the capture path passes the list straight through.
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
            assert_eq!(
                retroarch_button_index(&sorted, code),
                retroarch_button_index(other, code),
                "retroarch moved {code:#x}"
            );
        }
    }
}

#[test]
fn a_duplicated_code_shifts_every_later_button_for_both_consumers() {
    // Documented, not endorsed. A pad reported twice -- the same code twice
    // in one key list -- is counted twice by both numberings, so everything
    // after it is off by one. Both are wrong the same way, which is the least
    // bad outcome: the two consumers still agree with each other, and the
    // fault stays upstream in whatever built the list.
    let once = [0x120_u16, 0x121, 0x122];
    let twice = [0x120_u16, 0x120, 0x121, 0x122];
    assert_eq!(
        sdl_button_index(&twice, 0x120),
        Some(0),
        "the first copy wins"
    );
    assert_eq!(sdl_button_index(&twice, 0x121), Some(2));
    assert_eq!(sdl_button_index(&once, 0x121), Some(1));
    assert_eq!(retroarch_button_index(&twice, 0x120), Some(0));
    assert_eq!(retroarch_button_index(&twice, 0x121), Some(2));
    assert_eq!(retroarch_button_index(&once, 0x121), Some(1));
    for code in twice {
        assert_eq!(
            sdl_button_index(&twice, code),
            retroarch_button_index(&twice, code)
        );
    }
}

#[test]
fn trigger_happy_codes_are_ordinary_buttons_to_both_consumers() {
    // BTN_TRIGGER_HAPPY1 is 0x2c0; arcade encoders with more than sixteen
    // buttons run up there. They are above BTN_JOYSTICK, so both numberings
    // treat them plainly and the pad is one of the easy ones.
    let mut keys: Vec<u16> = (BTN_JOYSTICK..BTN_JOYSTICK + 4).collect();
    keys.extend(0x2c0_u16..0x2c8);
    for (expected, code) in keys.iter().copied().enumerate() {
        let expected = expected as i32;
        assert_eq!(
            sdl_button_index(&keys, code),
            Some(expected),
            "sdl for {code:#x}"
        );
        assert_eq!(
            retroarch_button_index(&keys, code),
            Some(expected),
            "retroarch for {code:#x}"
        );
    }
    assert_eq!(sdl_button_index(&keys, 0x2c0), Some(4));
    assert_eq!(sdl_button_index(&keys, 0x2c7), Some(11));
}

#[test]
fn sdl_numbers_every_reported_code_exactly_once_from_zero() {
    // A gap or a repeat in the numbering means two controls share a button
    // number, and the second binding written wins for both.
    let keys = [0x1e_u16, 0x2c, 0x100, 0x110, 0x120, 0x13f, 0x2c0];
    let numbers: BTreeSet<i32> = keys
        .iter()
        .map(|code| sdl_button_index(&keys, *code).expect("a reported code has a number"))
        .collect();
    let expected: BTreeSet<i32> = (0..keys.len() as i32).collect();
    assert_eq!(numbers, expected);
}

#[test]
fn retroarch_numbers_the_visible_codes_densely_and_skips_the_rest() {
    let keys = [0x1e_u16, 0x2c, 0x100, 0x110, 0x120, 0x13f, 0x2c0];
    let visible: Vec<u16> = keys.iter().copied().filter(|c| *c >= BTN_MISC).collect();
    let numbers: Vec<i32> = visible
        .iter()
        .map(|code| retroarch_button_index(&keys, *code).expect("a visible code has a number"))
        .collect();
    assert_eq!(numbers, (0..visible.len() as i32).collect::<Vec<_>>());
    // The invisible ones do not consume a number -- that is the correction.
    for code in keys.iter().copied().filter(|c| *c < BTN_MISC) {
        assert_eq!(
            retroarch_button_index(&keys, code),
            Some(RA_INVISIBLE),
            "{code:#x}"
        );
    }
}

#[test]
fn a_pad_of_nothing_but_keyboard_codes_is_invisible_to_retroarch_end_to_end() {
    // The pure-keyboard case an arcade encoder in keyboard mode produces:
    // SDL numbers all of them, RetroArch none.
    let keys = [0x1e_u16, 0x1f, 0x20];
    for (expected, code) in keys.iter().copied().enumerate() {
        assert_eq!(sdl_button_index(&keys, code), Some(expected as i32));
        assert_eq!(retroarch_button_index(&keys, code), Some(RA_INVISIBLE));
        let binding =
            Binding::button(expected as i32).with_ra_index(retroarch_button_index(&keys, code));
        assert!(binding.sdl_visible() && !binding.retroarch_visible());
    }
}

// ---------------------------------------------------------------------------
// axis_index
// ---------------------------------------------------------------------------

#[test]
fn hats_are_skipped_so_abs_rz_is_not_axis_five() {
    // ABS_X, ABS_Y, ABS_Z, ABS_RX, ABS_RY, ABS_RZ and a hat pair. Storing the
    // raw evdev code and hoping it is the index works right up to the first
    // pad whose axes are not 0,1,2,...
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
    // The interleaved pad: the hat codes sit between the sticks and the
    // triggers, so counting them would shift every axis after the d-pad.
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
    // ABS_HAT3Y is 0x17 and ABS_PRESSURE is 0x18. An off-by-one at this edge
    // renumbers every axis on any pad that reports pressure.
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
    // Documented, as for the button numberings: the first copy keeps its
    // number and everything after it moves up one.
    let once = [0x00_u16, 0x01, 0x03];
    let twice = [0x00_u16, 0x01, 0x01, 0x03];
    assert_eq!(axis_index(&once, 0x03), Some(2));
    assert_eq!(axis_index(&twice, 0x01), Some(1), "the first copy wins");
    assert_eq!(axis_index(&twice, 0x03), Some(3));
}

// ---------------------------------------------------------------------------
// JSON: the shape the Python's to_json/from_json used
// ---------------------------------------------------------------------------

#[test]
fn a_binding_writes_the_python_field_names() {
    let binding = Binding::hat(0, 4).with_ra_index(Some(RA_INVISIBLE));
    let json = serde_json::to_value(binding).expect("serialize a binding");
    assert_eq!(json["kind"], "hat");
    assert_eq!(json["index"], 0);
    assert_eq!(json["value"], 4);
    assert_eq!(json["ra_index"], -1);
    let object = json.as_object().expect("a binding is an object");
    assert_eq!(
        object.len(),
        4,
        "an extra field is one the Python's reader would ignore"
    );
    let back: Binding = serde_json::from_value(json).expect("deserialize a binding");
    assert_eq!(back, binding);
}

#[test]
fn an_absent_ra_index_reads_back_as_absent_not_as_zero() {
    // Zero is button zero. Reading an absent override as 0 would point every
    // binding in an old profile at the first button on the pad.
    let raw = serde_json::json!({"kind": "button", "index": 7});
    let binding: Binding = serde_json::from_value(raw).expect("deserialize");
    assert_eq!(binding.ra_index, None);
    assert_eq!(binding.retroarch().expect("retroarch"), "7");
}

#[test]
fn an_explicit_null_ra_index_reads_the_same_as_an_absent_one() {
    // The Python's to_json always wrote the key, with None for "they agree",
    // so every profile written before this port has an explicit null in it.
    let raw = serde_json::json!({"kind": "button", "index": 7, "value": 0, "ra_index": null});
    let binding: Binding = serde_json::from_value(raw).expect("deserialize");
    assert_eq!(binding.ra_index, None);
    assert_eq!(binding, Binding::button(7));
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
fn a_stored_ra_index_of_zero_survives_the_round_trip_as_zero() {
    let binding = Binding::button(5).with_ra_index(Some(0));
    let json = serde_json::to_value(binding).expect("serialize");
    assert_eq!(json["ra_index"], 0);
    let back: Binding = serde_json::from_value(json).expect("deserialize");
    assert_eq!(
        back.ra_index,
        Some(0),
        "Some(0) must not collapse into None"
    );
    assert_eq!(back.retroarch().expect("retroarch"), "0");
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
    // The Python stored `kind` as a bare string and only noticed at spelling
    // time; here a profile naming a kind nobody implements fails to load,
    // which is the difference between a message and a missing control.
    for name in ["Button", "BUTTON", "trigger", "", "buttons"] {
        let raw = serde_json::json!({"kind": name, "index": 1});
        let parsed: Result<Binding, _> = serde_json::from_value(raw);
        assert!(parsed.is_err(), "kind {name:?} was accepted");
    }
}

#[test]
fn a_binding_with_no_index_is_refused_rather_than_defaulted_to_button_zero() {
    // A divergence from the Python, kept deliberately: `from_json` used
    // `raw.get("index", 0)`, so a truncated profile silently bound the
    // control to button 0 -- a real button, usually A.
    let raw = serde_json::json!({"kind": "button"});
    let parsed: Result<Binding, _> = serde_json::from_value(raw);
    assert!(parsed.is_err(), "a binding with no index was accepted");
    let no_kind = serde_json::json!({"index": 3});
    let parsed: Result<Binding, _> = serde_json::from_value(no_kind);
    assert!(parsed.is_err(), "a binding with no kind was accepted");
}

#[test]
fn every_shape_survives_a_json_round_trip_unchanged() {
    // Including the ones no consumer can spell: a profile has to be able to
    // hold a binding, be rewritten, and still refuse it for the same reason.
    for binding in every_shape() {
        let json = serde_json::to_value(binding).expect("serialize");
        let back: Binding = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, binding);
        assert_eq!(back.sdl(), binding.sdl());
        assert_eq!(back.retroarch(), binding.retroarch());
    }
}
