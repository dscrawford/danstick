//! What padmap writes into a RetroArch autoconfig, and what it refuses to.
//!
//! The one fact behind every test here: **RetroArch will happily bind a button
//! that does not exist and still report the pad as configured.** There is no
//! error, no log line, and the front-end keeps working -- the control is simply
//! dead in every game. So the module under test has no way to fail loudly, and
//! a plausible-looking wrong answer is indistinguishable from a right one until
//! someone tries to play something. These tests are the only place that
//! difference is visible.
//!
//! The expectations were taken from the Python original (`retroarch_lines`,
//! `drop_shadowed_axis_halves` in `src/padmap/mapping.py`) by running it, not by
//! reading it.

use std::collections::BTreeMap;

use padmap_core::binding::{Binding, RA_INVISIBLE};
use padmap_core::control::{Control, CANONICAL_ORDER};
use padmap_core::layout;
use padmap_core::retroarch::{drop_shadowed_axis_halves, lines, ANALOG_STEMS};

fn bindings(entries: &[(Control, Binding)]) -> BTreeMap<Control, Binding> {
    entries.iter().copied().collect()
}

fn overrides(entries: &[(Control, &str)]) -> BTreeMap<Control, String> {
    entries
        .iter()
        .map(|(control, key)| (*control, (*key).to_owned()))
        .collect()
}

fn owned(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

fn key_of(line: &str) -> &str {
    line.split(" = ")
        .next()
        .expect("split always yields one part")
}

// ---------------------------------------------------------------------------
// lines(): what gets written at all
// ---------------------------------------------------------------------------

#[test]
fn an_empty_capture_writes_no_lines_at_all() {
    // Not even a skeleton of empty keys. A key bound to nothing is worse than
    // an absent one: RetroArch binds the non-existent button, calls the pad
    // configured, and the user is told the mapping took when it did not.
    assert!(lines(&BTreeMap::new(), &BTreeMap::new()).is_empty());
}

#[test]
fn a_single_captured_control_writes_exactly_one_line() {
    let out = lines(
        &bindings(&[(Control::Start, Binding::button(9))]),
        &BTreeMap::new(),
    );
    assert_eq!(out, ["input_start_btn = \"9\""]);
}

#[test]
fn a_control_that_was_not_captured_leaves_no_trace_in_the_output() {
    // The capture holds A only; every other key must be missing rather than
    // present-and-empty.
    let out = lines(
        &bindings(&[(Control::A, Binding::button(1))]),
        &BTreeMap::new(),
    );
    assert_eq!(out.len(), 1);
    for control in CANONICAL_ORDER {
        if control == Control::A {
            continue;
        }
        let key = control.retroarch_key();
        assert!(
            !out.iter().any(|line| key_of(line) == key),
            "{key} was written for a control nobody pressed"
        );
    }
}

#[test]
fn all_eighteen_controls_each_write_one_line() {
    let captured: Vec<(Control, Binding)> = CANONICAL_ORDER
        .iter()
        .enumerate()
        .map(|(index, control)| (*control, Binding::button(index as i32)))
        .collect();
    let out = lines(&bindings(&captured), &BTreeMap::new());
    assert_eq!(out.len(), CANONICAL_ORDER.len(), "a control went missing");
    let expected: Vec<String> = CANONICAL_ORDER
        .iter()
        .enumerate()
        .map(|(index, control)| format!("{} = \"{index}\"", control.retroarch_key()))
        .collect();
    assert_eq!(out, expected);
}

#[test]
fn output_order_is_canonical_whatever_order_the_capture_was_built_in() {
    // Stable output order is what keeps a regenerated mapping from producing a
    // diff that looks like a change. The capture must not get a vote.
    let mut reversed: BTreeMap<Control, Binding> = BTreeMap::new();
    for (index, control) in CANONICAL_ORDER.iter().rev().enumerate() {
        reversed.insert(*control, Binding::button(index as i32));
    }
    let out = lines(&reversed, &BTreeMap::new());
    let written: Vec<&str> = out.iter().map(|line| key_of(line)).collect();
    let expected: Vec<&str> = CANONICAL_ORDER
        .iter()
        .map(|control| control.retroarch_key())
        .collect();
    assert_eq!(written, expected);
}

#[test]
fn a_capture_of_nothing_but_unwritable_bindings_produces_an_empty_file() {
    // Two separate refusals, and between them they must not leave a half-file:
    // an autoconfig with a header and no binds still reports as configured.
    let out = lines(
        &bindings(&[
            (
                Control::A,
                Binding::button(3).with_ra_index(Some(RA_INVISIBLE)),
            ),
            (Control::DpadUp, Binding::hat(0, 3)),
        ]),
        &BTreeMap::new(),
    );
    assert!(out.is_empty());
}

// ---------------------------------------------------------------------------
// The a/b inversion
// ---------------------------------------------------------------------------

#[test]
fn canonical_a_and_b_swap_because_retroarch_uses_the_nintendo_positions() {
    // RetroArch's input_b_btn is the *bottom* face button and input_a_btn the
    // right one; SDL's a and b are the other way round. Getting this backwards
    // swaps confirm and cancel in every game and nothing reports an error --
    // it reads to the user as "the mapping did not take".
    let out = lines(
        &bindings(&[
            (Control::A, Binding::button(1)),
            (Control::B, Binding::button(2)),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(out, ["input_b_btn = \"1\"", "input_a_btn = \"2\""]);
}

#[test]
fn canonical_x_and_y_swap_the_same_way() {
    let out = lines(
        &bindings(&[
            (Control::X, Binding::button(0)),
            (Control::Y, Binding::button(3)),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(out, ["input_y_btn = \"0\"", "input_x_btn = \"3\""]);
}

#[test]
fn the_four_face_buttons_come_out_crossed_in_pairs_and_never_collide() {
    let out = lines(
        &bindings(&[
            (Control::A, Binding::button(1)),
            (Control::B, Binding::button(2)),
            (Control::X, Binding::button(0)),
            (Control::Y, Binding::button(3)),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(
        out,
        [
            "input_b_btn = \"1\"",
            "input_a_btn = \"2\"",
            "input_y_btn = \"0\"",
            "input_x_btn = \"3\"",
        ]
    );
    let mut keys: Vec<&str> = out.iter().map(|line| key_of(line)).collect();
    keys.sort_unstable();
    keys.dedup();
    // Two controls under one key means the later one silently wins and the
    // earlier button is dead, with the pad still reporting as configured.
    assert_eq!(keys.len(), 4, "two face buttons landed on one key");
}

// ---------------------------------------------------------------------------
// Console overrides
// ---------------------------------------------------------------------------

#[test]
fn a_console_override_replaces_the_canonical_key() {
    // Cores map the abstract RetroPad onto real console buttons themselves, and
    // not identically: mupen64plus-next reads N64 B from RetroPad Y, so the
    // canonical key would bind the physical B to something the core never asks
    // about -- a dead button on a pad that reports as configured.
    let out = lines(
        &bindings(&[(Control::B, Binding::button(2))]),
        &overrides(&[(Control::B, "input_y_btn")]),
    );
    assert_eq!(out, ["input_y_btn = \"2\""]);
}

#[test]
fn an_empty_override_falls_back_to_the_canonical_key() {
    // The Python was `overrides.get(control) or RETROARCH_KEYS.get(control)`,
    // so a falsy override means fall back. In Rust `Some("")` is perfectly
    // truthy, and taking it at its word writes ` = "2"` -- a nameless key that
    // RetroArch parses as nothing and complains about not at all.
    let out = lines(
        &bindings(&[(Control::B, Binding::button(2))]),
        &overrides(&[(Control::B, "")]),
    );
    assert_eq!(out, ["input_a_btn = \"2\""]);
}

#[test]
fn an_empty_override_falls_back_for_every_control_not_just_the_face_buttons() {
    for control in CANONICAL_ORDER {
        let out = lines(
            &bindings(&[(control, Binding::button(4))]),
            &overrides(&[(control, "")]),
        );
        assert_eq!(
            out,
            [format!("{} = \"4\"", control.retroarch_key())],
            "{control} lost its key to an empty override"
        );
    }
}

#[test]
fn an_override_for_a_control_that_was_not_captured_changes_nothing() {
    // A layout carries overrides for controls the user never pressed. Those
    // must not conjure a line: see the whole file's premise.
    let out = lines(
        &bindings(&[(Control::A, Binding::button(1))]),
        &overrides(&[
            (Control::B, "input_y_btn"),
            (Control::RightTrigger, "input_r_btn"),
        ]),
    );
    assert_eq!(out, ["input_b_btn = \"1\""]);
}

#[test]
fn overrides_for_every_control_at_once_replace_every_key() {
    let captured: Vec<(Control, Binding)> = CANONICAL_ORDER
        .iter()
        .enumerate()
        .map(|(index, control)| (*control, Binding::button(index as i32)))
        .collect();
    let all: Vec<(Control, String)> = CANONICAL_ORDER
        .iter()
        .enumerate()
        .map(|(index, control)| (*control, format!("input_custom{index}_btn")))
        .collect();
    let out = lines(&bindings(&captured), &all.into_iter().collect());
    let expected: Vec<String> = (0..CANONICAL_ORDER.len())
        .map(|index| format!("input_custom{index}_btn = \"{index}\""))
        .collect();
    assert_eq!(out, expected, "one canonical key survived an override");
}

#[test]
fn an_override_equal_to_the_canonical_key_is_indistinguishable_from_none() {
    let out = lines(
        &bindings(&[(Control::Start, Binding::button(9))]),
        &overrides(&[(Control::Start, "input_start_btn")]),
    );
    assert_eq!(out, ["input_start_btn = \"9\""]);
}

#[test]
fn an_override_ending_in_btn_is_rewritten_to_axis_when_the_binding_is_an_axis() {
    // GameCube L is an analogue trigger and the dolphin core reads it from
    // RetroPad L2, so both rules fire at once: the override picks the key and
    // the axis rewrite renames it. Applying only the first writes
    // `input_l2_btn = "+4"`, which strtoull reads as button 4.
    let out = lines(
        &bindings(&[(Control::LeftShoulder, Binding::axis(4, 1))]),
        &overrides(&[(Control::LeftShoulder, "input_l2_btn")]),
    );
    assert_eq!(out, ["input_l2_axis = \"+4\""]);
}

#[test]
fn an_override_that_does_not_mention_btn_is_written_exactly_as_given() {
    let out = lines(
        &bindings(&[(Control::RightStickUp, Binding::axis(3, -1))]),
        &overrides(&[(Control::RightStickUp, "input_r_y_minus_axis")]),
    );
    assert_eq!(out, ["input_r_y_minus_axis = \"-3\""]);
}

#[test]
fn the_axis_rewrite_replaces_every_btn_in_the_key_as_pythons_replace_did() {
    // `str.replace` is replace-all in both languages. Nothing in the real key
    // tables contains `_btn` twice, so this pins parity rather than a
    // requirement -- if the Rust ever switched to a suffix strip, this is where
    // the two implementations would start disagreeing.
    let out = lines(
        &bindings(&[(Control::A, Binding::axis(1, 1))]),
        &overrides(&[(Control::A, "input_btn_btn")]),
    );
    assert_eq!(out, ["input_axis_axis = \"+1\""]);
}

#[test]
fn the_real_n64_layout_override_moves_b_onto_retropad_y() {
    let out = lines(
        &bindings(&[(Control::B, Binding::button(2))]),
        &layout::get("n64").retroarch_keys(),
    );
    assert_eq!(out, ["input_y_btn = \"2\""]);
}

#[test]
fn the_real_gamecube_layout_overrides_uncross_the_face_buttons() {
    // The dolphin core binds GC A to RetroPad A, an identity mapping -- which
    // is exactly what the global table does not do. Without these four the
    // GameCube pad's A and B come out swapped.
    let out = lines(
        &bindings(&[
            (Control::A, Binding::button(0)),
            (Control::B, Binding::button(1)),
            (Control::X, Binding::button(2)),
            (Control::Y, Binding::button(3)),
        ]),
        &layout::get("gamecube").retroarch_keys(),
    );
    assert_eq!(
        out,
        [
            "input_a_btn = \"0\"",
            "input_b_btn = \"1\"",
            "input_x_btn = \"2\"",
            "input_y_btn = \"3\"",
        ]
    );
}

#[test]
fn a_layout_with_no_overrides_writes_the_canonical_keys() {
    // SNES was read against snes9x's own source and needs none.
    let snes = layout::get("snes").retroarch_keys();
    assert!(
        snes.is_empty(),
        "snes grew an override that wants justifying"
    );
    let out = lines(&bindings(&[(Control::A, Binding::button(1))]), &snes);
    assert_eq!(out, ["input_b_btn = \"1\""]);
}

// ---------------------------------------------------------------------------
// Axis bindings move to the _axis key
// ---------------------------------------------------------------------------

#[test]
fn an_axis_moves_to_the_axis_key_because_btn_misparses_the_sign() {
    // RetroArch parses a _btn value with strtoull. "-2" under a _btn key is
    // button 2, silently.
    let out = lines(
        &bindings(&[(Control::LeftTrigger, Binding::axis(2, -1))]),
        &BTreeMap::new(),
    );
    assert_eq!(out, ["input_l2_axis = \"-2\""]);
}

#[test]
fn axis_zero_is_where_the_strtoull_misparse_actually_bites() {
    // "-0" and "+0" both come out as button 0, so the two directions of one
    // stick collapse onto the same button and pressing either activates both.
    let out = lines(
        &bindings(&[
            (Control::RightStickLeft, Binding::axis(0, -1)),
            (Control::RightStickRight, Binding::axis(0, 1)),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(
        out,
        [
            "input_r_x_minus_axis = \"-0\"",
            "input_r_x_plus_axis = \"+0\""
        ]
    );
    for line in &out {
        assert!(!line.contains("_btn"), "{line} would parse as button 0");
    }
}

#[test]
fn every_control_puts_an_axis_binding_under_an_axis_key() {
    // Any control can be an axis on some pad: triggers usually, a d-pad on an
    // adapter that reports it as a hat-shaped pair of axes, C-buttons always.
    for control in CANONICAL_ORDER {
        let out = lines(
            &bindings(&[(control, Binding::axis(1, -1))]),
            &BTreeMap::new(),
        );
        let key = control.retroarch_key().replace("_btn", "_axis");
        assert_eq!(out, [format!("{key} = \"-1\"")], "{control} kept _btn");
        assert!(
            !out[0].contains("_btn"),
            "{control} wrote an axis under a _btn key"
        );
    }
}

#[test]
fn a_positive_axis_keeps_its_sign_under_the_axis_key() {
    let out = lines(
        &bindings(&[(Control::RightTrigger, Binding::axis(5, 1))]),
        &BTreeMap::new(),
    );
    assert_eq!(out, ["input_r2_axis = \"+5\""]);
}

#[test]
fn a_button_binding_stays_on_the_btn_key() {
    let out = lines(
        &bindings(&[(Control::LeftTrigger, Binding::button(6))]),
        &BTreeMap::new(),
    );
    assert_eq!(
        out,
        ["input_l2_btn = \"6\""],
        "a digital trigger is a button"
    );
}

#[test]
fn a_hat_binding_stays_on_the_btn_key_with_a_direction_word() {
    // A hat is not an axis to either consumer; `h0up` under the _btn key is
    // what RetroArch expects.
    let out = lines(
        &bindings(&[
            (Control::DpadUp, Binding::hat(0, 1)),
            (Control::DpadDown, Binding::hat(0, 4)),
            (Control::DpadLeft, Binding::hat(0, 8)),
            (Control::DpadRight, Binding::hat(0, 2)),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(
        out,
        [
            "input_up_btn = \"h0up\"",
            "input_down_btn = \"h0down\"",
            "input_left_btn = \"h0left\"",
            "input_right_btn = \"h0right\"",
        ]
    );
}

#[test]
fn an_axis_writes_the_retroarch_index_not_the_sdl_one() {
    // The two consumers number differently; writing SDL's number names a
    // different axis, which moves but is the wrong stick.
    let out = lines(
        &bindings(&[(
            Control::RightStickDown,
            Binding::axis(5, 1).with_ra_index(Some(2)),
        )]),
        &BTreeMap::new(),
    );
    assert_eq!(out, ["input_r_y_plus_axis = \"+2\""]);
}

#[test]
fn an_axis_with_a_negative_ra_index_is_written_as_the_python_wrote_it() {
    // A quirk, pinned deliberately: retroarch_visible only screens buttons, so
    // an axis carrying a negative index falls through and the sign is prefixed
    // to a negative number. The Python answers `input_l2_axis = "--1"` for this
    // input too, so the port is faithful -- but nothing produces this shape
    // today and the output is nonsense if anything ever does.
    let out = lines(
        &bindings(&[(
            Control::LeftTrigger,
            Binding::axis(2, -1).with_ra_index(Some(-1)),
        )]),
        &BTreeMap::new(),
    );
    assert_eq!(out, ["input_l2_axis = \"--1\""]);
}

// ---------------------------------------------------------------------------
// Bindings RetroArch cannot name are dropped, not guessed at
// ---------------------------------------------------------------------------

#[test]
fn a_button_retroarch_cannot_see_writes_no_line() {
    // RA_INVISIBLE: the pad reports the code, RetroArch's udev driver never
    // enumerates it, so any number written here names a different button or
    // none -- and the pad still reports as configured either way.
    let out = lines(
        &bindings(&[
            (Control::A, Binding::button(1)),
            (
                Control::B,
                Binding::button(13).with_ra_index(Some(RA_INVISIBLE)),
            ),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(out, ["input_b_btn = \"1\""]);
}

#[test]
fn any_negative_ra_index_is_dropped_not_only_the_sentinel() {
    // A profile off disk can hold whatever it likes, and no real button is
    // negative.
    for index in [-1, -2, -99, i32::MIN] {
        let out = lines(
            &bindings(&[
                (Control::A, Binding::button(1)),
                (Control::B, Binding::button(13).with_ra_index(Some(index))),
            ]),
            &BTreeMap::new(),
        );
        assert_eq!(out, ["input_b_btn = \"1\""], "ra_index {index} was written");
    }
}

#[test]
fn a_diagonal_hat_writes_no_line_instead_of_aborting_the_whole_file() {
    // A hat reads 3 on a diagonal, which is not a control anyone can press.
    // This used to raise from the middle of writing a launch profile and leave
    // the game to start with no controller config at all -- one unpressable
    // direction cost the user every other binding.
    let out = lines(
        &bindings(&[
            (Control::A, Binding::button(1)),
            (Control::DpadUp, Binding::hat(0, 3)),
            (Control::Start, Binding::button(9)),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(out, ["input_b_btn = \"1\"", "input_start_btn = \"9\""]);
}

#[test]
fn every_unpressable_hat_value_is_refused_and_the_rest_survive() {
    for value in [0, 3, 5, 6, 9, 12, 15, -1] {
        let out = lines(
            &bindings(&[
                (Control::A, Binding::button(1)),
                (Control::DpadUp, Binding::hat(0, value)),
                (Control::DpadDown, Binding::hat(0, 4)),
            ]),
            &BTreeMap::new(),
        );
        assert_eq!(
            out,
            ["input_b_btn = \"1\"", "input_down_btn = \"h0down\""],
            "hat value {value} was written or took its neighbours with it"
        );
    }
}

#[test]
fn a_dropped_control_does_not_disturb_the_order_of_the_ones_around_it() {
    let out = lines(
        &bindings(&[
            (Control::A, Binding::button(1)),
            (
                Control::B,
                Binding::button(2).with_ra_index(Some(RA_INVISIBLE)),
            ),
            (Control::X, Binding::button(0)),
            (Control::Y, Binding::button(3)),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(
        out,
        [
            "input_b_btn = \"1\"",
            "input_y_btn = \"0\"",
            "input_x_btn = \"3\""
        ]
    );
}

// ---------------------------------------------------------------------------
// drop_shadowed_axis_halves: the reason this module exists
// ---------------------------------------------------------------------------
//
// The incident, from FINDINGS.md: "Y mapped to C-up did nothing, and the
// C-stick looked fine". Everything padmap owned was right -- the capture, the
// button number, the autoconfig, the log line saying the profile was applied.
// One layer below, `input_joypad_analog_axis` only consults the button binds
// when abs(plus) - abs(minus) is exactly zero, and on the 0..255 range these
// adapters report there is no value that normalises to zero. A C-stick resting
// at 131 comes out at +900: 2.7% of full scale, inside the core's deadzone, so
// the stick behaved perfectly while the button on the other half was dead.
//
// The rule below is the fix: the captured button is the deliberate instruction,
// so the opposing axis is dropped. It costs that stick's other direction, which
// RetroArch gives no way to keep.

#[test]
fn the_four_analog_stems_are_both_sticks_on_both_axes() {
    assert_eq!(
        ANALOG_STEMS,
        ["input_l_x", "input_l_y", "input_r_x", "input_r_y"]
    );
}

#[test]
fn a_minus_button_kills_the_plus_axis_on_every_stem() {
    for stem in ANALOG_STEMS {
        let out = drop_shadowed_axis_halves(vec![
            format!("{stem}_minus_btn = \"11\""),
            format!("{stem}_plus_axis = \"+3\""),
        ]);
        assert_eq!(
            out,
            [format!("{stem}_minus_btn = \"11\"")],
            "{stem}: the surviving axis leaves the button unreachable"
        );
    }
}

#[test]
fn a_plus_button_kills_the_minus_axis_on_every_stem() {
    // The rule is symmetric even though the hardware is not: FINDINGS notes a
    // button on the plus half cannot be rescued by calibration the way a minus
    // one can, which makes dropping the opposing axis the only option here.
    for stem in ANALOG_STEMS {
        let out = drop_shadowed_axis_halves(vec![
            format!("{stem}_plus_btn = \"12\""),
            format!("{stem}_minus_axis = \"-3\""),
        ]);
        assert_eq!(out, [format!("{stem}_plus_btn = \"12\"")], "{stem}");
    }
}

#[test]
fn a_stick_with_axes_on_both_halves_is_left_alone_on_every_stem() {
    // An ordinary analogue stick. There is no button to protect, and dropping
    // half of it would cost the user a working direction for nothing.
    for stem in ANALOG_STEMS {
        let input = vec![
            format!("{stem}_minus_axis = \"-3\""),
            format!("{stem}_plus_axis = \"+3\""),
        ];
        assert_eq!(drop_shadowed_axis_halves(input.clone()), input, "{stem}");
    }
}

#[test]
fn a_stick_with_buttons_on_both_halves_is_left_alone_on_every_stem() {
    // The healthy N64 C-cluster shape: nothing to shadow, so both survive.
    for stem in ANALOG_STEMS {
        let input = vec![
            format!("{stem}_minus_btn = \"11\""),
            format!("{stem}_plus_btn = \"12\""),
        ];
        assert_eq!(drop_shadowed_axis_halves(input.clone()), input, "{stem}");
    }
}

#[test]
fn one_half_on_its_own_is_left_alone_whichever_half_it_is() {
    for stem in ANALOG_STEMS {
        for half in ["minus", "plus"] {
            for suffix in ["btn", "axis"] {
                let input = vec![format!("{stem}_{half}_{suffix} = \"7\"")];
                assert_eq!(
                    drop_shadowed_axis_halves(input.clone()),
                    input,
                    "{stem}_{half}_{suffix} was dropped with nothing shadowing it"
                );
            }
        }
    }
}

#[test]
fn two_shadowed_stems_are_handled_independently() {
    let out = drop_shadowed_axis_halves(owned(&[
        "input_r_y_minus_btn = \"11\"",
        "input_r_y_plus_axis = \"+3\"",
        "input_r_x_plus_btn = \"14\"",
        "input_r_x_minus_axis = \"-2\"",
        "input_l_y_minus_axis = \"-1\"",
        "input_l_y_plus_axis = \"+1\"",
    ]));
    assert_eq!(
        out,
        [
            "input_r_y_minus_btn = \"11\"",
            "input_r_x_plus_btn = \"14\"",
            "input_l_y_minus_axis = \"-1\"",
            "input_l_y_plus_axis = \"+1\"",
        ],
        "each stem decides for itself; the untouched left stick keeps both halves"
    );
}

#[test]
fn a_button_on_one_stem_leaves_another_stems_axis_alone() {
    let out = drop_shadowed_axis_halves(owned(&[
        "input_r_y_minus_btn = \"11\"",
        "input_l_y_plus_axis = \"+1\"",
    ]));
    assert_eq!(
        out,
        [
            "input_r_y_minus_btn = \"11\"",
            "input_l_y_plus_axis = \"+1\""
        ],
        "the rule matched across two different sticks"
    );
}

#[test]
fn a_line_with_no_separator_is_not_mistaken_for_a_key_and_is_preserved() {
    // Autoconfigs carry comments and a header. Reading one as a key name would
    // at best do nothing and at worst drop the line.
    let input = owned(&[
        "# padmap Player 1",
        "input_r_y_minus_btn = \"11\"",
        "input_r_y_plus_axis = \"+3\"",
        "",
    ]);
    let out = drop_shadowed_axis_halves(input);
    assert_eq!(
        out,
        ["# padmap Player 1", "input_r_y_minus_btn = \"11\"", ""]
    );
}

#[test]
fn line_order_is_otherwise_preserved_exactly() {
    // The output is a file. Reordering it makes every regeneration look like a
    // change in review, which is how real changes stop being noticed.
    let input = owned(&[
        "input_start_btn = \"9\"",
        "input_b_btn = \"1\"",
        "input_r_y_minus_btn = \"11\"",
        "input_up_btn = \"h0up\"",
        "input_r_y_plus_axis = \"+3\"",
        "input_a_btn = \"2\"",
    ]);
    let out = drop_shadowed_axis_halves(input);
    assert_eq!(
        out,
        [
            "input_start_btn = \"9\"",
            "input_b_btn = \"1\"",
            "input_r_y_minus_btn = \"11\"",
            "input_up_btn = \"h0up\"",
            "input_a_btn = \"2\"",
        ]
    );
}

#[test]
fn an_empty_list_comes_back_empty() {
    assert!(drop_shadowed_axis_halves(Vec::new()).is_empty());
}

#[test]
fn a_key_that_merely_starts_with_a_stem_does_not_trigger_the_rule() {
    // Matching on a prefix rather than on the eight exact key names would drop
    // binds nobody asked about, and the user would see a control vanish with no
    // explanation anywhere.
    let input = owned(&[
        "input_l_x_minus_btn_extra = \"5\"",
        "input_l_x_plus_axis = \"+0\"",
    ]);
    assert_eq!(drop_shadowed_axis_halves(input.clone()), input);
}

#[test]
fn a_longer_axis_key_is_not_shadowed_by_a_real_button_either() {
    let input = owned(&[
        "input_l_x_minus_btn = \"5\"",
        "input_l_x_plus_axis_extra = \"+0\"",
    ]);
    assert_eq!(
        drop_shadowed_axis_halves(input.clone()),
        input,
        "only the eight exact half-axis keys take part"
    );
}

#[test]
fn unrelated_keys_are_never_dropped() {
    let input = owned(&[
        "input_b_btn = \"1\"",
        "input_a_btn = \"2\"",
        "input_l2_axis = \"-4\"",
        "input_up_btn = \"h0up\"",
    ]);
    assert_eq!(drop_shadowed_axis_halves(input.clone()), input);
}

#[test]
fn an_unrelated_axis_survives_beside_a_shadowed_one() {
    let out = drop_shadowed_axis_halves(owned(&[
        "input_r_y_minus_btn = \"11\"",
        "input_r_y_plus_axis = \"+3\"",
        "input_l2_axis = \"-4\"",
    ]));
    assert_eq!(
        out,
        ["input_r_y_minus_btn = \"11\"", "input_l2_axis = \"-4\""],
        "an analogue trigger is not half of a stick"
    );
}

#[test]
fn the_rule_is_idempotent() {
    // It runs at the end of `lines`, and a caller that post-processes and
    // re-runs it must not get a second bite at a different answer.
    let input = owned(&[
        "input_r_y_minus_btn = \"11\"",
        "input_r_y_plus_axis = \"+3\"",
        "input_l_x_minus_axis = \"-0\"",
    ]);
    let once = drop_shadowed_axis_halves(input);
    let twice = drop_shadowed_axis_halves(once.clone());
    assert_eq!(once, twice);
}

#[test]
fn only_the_text_before_the_first_separator_counts_as_the_key() {
    // A value containing " = " must not shift what the key is read as. Python
    // split with maxsplit=1; the Rust takes the first element of the split.
    let out = drop_shadowed_axis_halves(owned(&[
        "input_r_y_minus_btn = \"11\"",
        "input_r_y_plus_axis = \"+3 = spare\"",
    ]));
    assert_eq!(out, ["input_r_y_minus_btn = \"11\""]);
}

// ---------------------------------------------------------------------------
// End to end through lines()
// ---------------------------------------------------------------------------

#[test]
fn the_c_up_button_kills_the_c_down_axis_end_to_end() {
    // The reported capture: Y pressed for C-up (a button on the right stick's
    // minus half) while C-down was still captured as the stick's own axis. The
    // axis then decided the answer and Y was never consulted -- in Smash Bros,
    // with a C-stick that behaved and no error anywhere.
    let out = lines(
        &bindings(&[
            (Control::A, Binding::button(1)),
            (Control::RightStickUp, Binding::button(3)),
            (Control::RightStickDown, Binding::axis(2, 1)),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(
        out,
        ["input_b_btn = \"1\"", "input_r_y_minus_btn = \"3\""],
        "the opposing axis must go, and nothing else"
    );
    assert!(
        !out.iter().any(|line| line.contains("input_r_y_plus")),
        "C-down surviving as an axis is exactly what made C-up dead"
    );
}

#[test]
fn the_healthy_c_cluster_of_four_buttons_drops_nothing() {
    let out = lines(
        &bindings(&[
            (Control::RightStickUp, Binding::button(11)),
            (Control::RightStickDown, Binding::button(12)),
            (Control::RightStickLeft, Binding::button(13)),
            (Control::RightStickRight, Binding::button(14)),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(
        out,
        [
            "input_r_y_minus_btn = \"11\"",
            "input_r_y_plus_btn = \"12\"",
            "input_r_x_minus_btn = \"13\"",
            "input_r_x_plus_btn = \"14\"",
        ],
        "all four C-buttons captured as buttons shadow nothing"
    );
}

#[test]
fn a_whole_c_stick_captured_as_axes_survives_end_to_end() {
    let out = lines(
        &bindings(&[
            (Control::RightStickUp, Binding::axis(3, -1)),
            (Control::RightStickDown, Binding::axis(3, 1)),
            (Control::RightStickLeft, Binding::axis(2, -1)),
            (Control::RightStickRight, Binding::axis(2, 1)),
        ]),
        &BTreeMap::new(),
    );
    assert_eq!(
        out,
        [
            "input_r_y_minus_axis = \"-3\"",
            "input_r_y_plus_axis = \"+3\"",
            "input_r_x_minus_axis = \"-2\"",
            "input_r_x_plus_axis = \"+2\"",
        ]
    );
}

#[test]
fn the_shadow_rule_reaches_keys_that_only_an_override_could_produce() {
    // Nothing canonical names the *left* stick's halves, so the only way
    // input_l_y_* appears is through a console override. The rule has to see it
    // there too -- it runs on the finished lines, not on the controls.
    let out = lines(
        &bindings(&[
            (Control::DpadUp, Binding::button(7)),
            (Control::DpadDown, Binding::axis(1, 1)),
        ]),
        &overrides(&[
            (Control::DpadUp, "input_l_y_minus_btn"),
            (Control::DpadDown, "input_l_y_plus_btn"),
        ]),
    );
    assert_eq!(out, ["input_l_y_minus_btn = \"7\""]);
}

// ---------------------------------------------------------------------------
// Whole-capture regression anchors
//
// Three plausible pads, written out in full. These exist so that a change to a
// key table moves exactly one of them: if all three move, something global
// changed; if none does, the change was inert.
// ---------------------------------------------------------------------------

#[test]
fn a_plausible_snes_capture_writes_the_whole_expected_autoconfig() {
    // No analogue anything, d-pad on a hat, two shoulders. Nothing overridden:
    // snes9x reads the plain RetroPad.
    let out = lines(
        &bindings(&[
            (Control::A, Binding::button(1)),
            (Control::B, Binding::button(2)),
            (Control::X, Binding::button(0)),
            (Control::Y, Binding::button(3)),
            (Control::Back, Binding::button(8)),
            (Control::Start, Binding::button(9)),
            (Control::LeftShoulder, Binding::button(4)),
            (Control::RightShoulder, Binding::button(5)),
            (Control::DpadUp, Binding::hat(0, 1)),
            (Control::DpadDown, Binding::hat(0, 4)),
            (Control::DpadLeft, Binding::hat(0, 8)),
            (Control::DpadRight, Binding::hat(0, 2)),
        ]),
        &layout::get("snes").retroarch_keys(),
    );
    assert_eq!(
        out,
        [
            "input_b_btn = \"1\"",
            "input_a_btn = \"2\"",
            "input_y_btn = \"0\"",
            "input_x_btn = \"3\"",
            "input_select_btn = \"8\"",
            "input_start_btn = \"9\"",
            "input_l_btn = \"4\"",
            "input_r_btn = \"5\"",
            "input_up_btn = \"h0up\"",
            "input_down_btn = \"h0down\"",
            "input_left_btn = \"h0left\"",
            "input_right_btn = \"h0right\"",
        ]
    );
}

#[test]
fn a_plausible_gamecube_capture_writes_the_whole_expected_autoconfig() {
    // Analogue triggers on axes (so L/R land on _axis under an overridden key),
    // Z on RetroPad R, face buttons uncrossed by the layout, and the C-stick as
    // a real stick -- both halves axes, so nothing is shadowed.
    let out = lines(
        &bindings(&[
            (Control::A, Binding::button(0)),
            (Control::B, Binding::button(1)),
            (Control::X, Binding::button(2)),
            (Control::Y, Binding::button(3)),
            (Control::Start, Binding::button(9)),
            (Control::LeftShoulder, Binding::axis(4, 1)),
            (Control::RightShoulder, Binding::axis(5, 1)),
            (Control::RightTrigger, Binding::button(7)),
            (Control::DpadUp, Binding::hat(0, 1)),
            (Control::DpadDown, Binding::hat(0, 4)),
            (Control::DpadLeft, Binding::hat(0, 8)),
            (Control::DpadRight, Binding::hat(0, 2)),
            (Control::RightStickUp, Binding::axis(3, -1)),
            (Control::RightStickDown, Binding::axis(3, 1)),
            (Control::RightStickLeft, Binding::axis(2, -1)),
            (Control::RightStickRight, Binding::axis(2, 1)),
        ]),
        &layout::get("gamecube").retroarch_keys(),
    );
    assert_eq!(
        out,
        [
            "input_a_btn = \"0\"",
            "input_b_btn = \"1\"",
            "input_x_btn = \"2\"",
            "input_y_btn = \"3\"",
            "input_start_btn = \"9\"",
            "input_l2_axis = \"+4\"",
            "input_r2_axis = \"+5\"",
            "input_r_btn = \"7\"",
            "input_up_btn = \"h0up\"",
            "input_down_btn = \"h0down\"",
            "input_left_btn = \"h0left\"",
            "input_right_btn = \"h0right\"",
            "input_r_y_minus_axis = \"-3\"",
            "input_r_y_plus_axis = \"+3\"",
            "input_r_x_minus_axis = \"-2\"",
            "input_r_x_plus_axis = \"+2\"",
        ]
    );
}

#[test]
fn a_plausible_n64_capture_writes_the_whole_expected_autoconfig() {
    // B moves to RetroPad Y for mupen64plus-next, Z is the left trigger, and
    // the C-cluster is four buttons on the right stick's halves.
    let out = lines(
        &bindings(&[
            (Control::A, Binding::button(1)),
            (Control::B, Binding::button(2)),
            (Control::Start, Binding::button(9)),
            (Control::LeftShoulder, Binding::button(4)),
            (Control::RightShoulder, Binding::button(5)),
            (Control::LeftTrigger, Binding::button(6)),
            (Control::DpadUp, Binding::hat(0, 1)),
            (Control::DpadDown, Binding::hat(0, 4)),
            (Control::DpadLeft, Binding::hat(0, 8)),
            (Control::DpadRight, Binding::hat(0, 2)),
            (Control::RightStickUp, Binding::button(11)),
            (Control::RightStickDown, Binding::button(12)),
            (Control::RightStickLeft, Binding::button(13)),
            (Control::RightStickRight, Binding::button(14)),
        ]),
        &layout::get("n64").retroarch_keys(),
    );
    assert_eq!(
        out,
        [
            "input_b_btn = \"1\"",
            "input_y_btn = \"2\"",
            "input_start_btn = \"9\"",
            "input_l_btn = \"4\"",
            "input_r_btn = \"5\"",
            "input_l2_btn = \"6\"",
            "input_up_btn = \"h0up\"",
            "input_down_btn = \"h0down\"",
            "input_left_btn = \"h0left\"",
            "input_right_btn = \"h0right\"",
            "input_r_y_minus_btn = \"11\"",
            "input_r_y_plus_btn = \"12\"",
            "input_r_x_minus_btn = \"13\"",
            "input_r_x_plus_btn = \"14\"",
        ]
    );
}
