//! RetroArch's side of a mapping: the autoconfig lines it reads.

use std::collections::BTreeMap;

use crate::binding::{Binding, BindingKind};
use crate::control::{Control, CANONICAL_ORDER};

/// The four analog stick half-axis pairs, by the stem of their RetroArch keys.
/// Each stem gets both a `_minus` and a `_plus` bind, and RetroArch reads the
/// two together -- see [`drop_shadowed_axis_halves`].
pub const ANALOG_STEMS: [&str; 4] = ["input_l_x", "input_l_y", "input_r_x", "input_r_y"];

/// Remove an `_axis` bind that would stop the other half's `_btn` working.
///
/// RetroArch reads a stick axis in `input_joypad_analog_axis`, and it reads
/// both halves before it will look at a button:
///
/// ```text
/// res  = abs(input_joypad_axis(..., axis_plus,  ...));
/// res -= abs(input_joypad_axis(..., axis_minus, ...));
/// if (res == 0) { ... consult bind_minus->joykey / bind_plus->joykey ... }
/// ```
///
/// So a mapping that puts a button on one half of an axis and leaves an axis on
/// the other half only works while that axis reads *exactly* zero, and it never
/// does: `udev_compute_axis` is `(value - min) * 0xffff / range - 0x7fff`, and
/// on the 0..255 range these adapters report there is no value that normalises
/// to zero. An uncalibrated C-stick resting at 131 comes out at +900 -- under
/// 3% of full scale, so it sits inside the core's deadzone and the stick looks
/// perfectly normal, while `res` is 900 and the button on the other half is
/// dead. That was the reported bug: Y mapped to C-up did nothing in Smash Bros.
///
/// The captured button is the deliberate instruction, so it wins. Dropping the
/// opposing axis makes both halves AXIS_NONE, `res` is then always 0, and the
/// button is read every time. It costs the stick's other direction, which is
/// not padmap's to fix: RetroArch cannot express "this button, and also that
/// axis" on one analog axis.
pub fn drop_shadowed_axis_halves(lines: Vec<String>) -> Vec<String> {
    let key_of = |line: &str| line.split(" = ").next().unwrap_or("").to_owned();
    let keys: Vec<String> = lines
        .iter()
        .filter(|line| line.contains(" = "))
        .map(|line| key_of(line))
        .collect();

    let mut doomed: Vec<String> = Vec::new();
    for stem in ANALOG_STEMS {
        for (half, other) in [("minus", "plus"), ("plus", "minus")] {
            let button = format!("{stem}_{half}_btn");
            let axis = format!("{stem}_{other}_axis");
            if keys.contains(&button) && keys.contains(&axis) {
                doomed.push(axis);
            }
        }
    }
    if doomed.is_empty() {
        return lines;
    }
    lines
        .into_iter()
        .filter(|line| !doomed.contains(&key_of(line)))
        .collect()
}

/// Autoconfig entries for the controls that were captured.
///
/// Only what the user actually pressed: a key bound to nothing is worse than an
/// absent one, because RetroArch will happily bind a button that does not exist
/// and report the pad as configured.
///
/// Which is also why a binding RetroArch cannot name is dropped here instead of
/// guessed at -- a button below `BTN_MISC`, or a hat value that is not one
/// direction bit, which used to fail out of the middle of writing a launch
/// profile and leave the game to start with no controller config at all.
///
/// `overrides` carries the console's own wiring: cores map the abstract
/// RetroPad onto real console buttons themselves and not identically, so
/// mupen64plus-next reads N64 B from RetroPad **Y**. An empty override string
/// falls back to the canonical key, as the Python's `or` did.
pub fn lines(
    bindings: &BTreeMap<Control, Binding>,
    overrides: &BTreeMap<Control, String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for control in CANONICAL_ORDER {
        let Some(binding) = bindings.get(&control) else {
            continue;
        };
        if !binding.retroarch_visible() {
            continue;
        }
        let Ok(target) = binding.retroarch() else {
            continue;
        };
        let key = match overrides.get(&control) {
            Some(override_key) if !override_key.is_empty() => override_key.clone(),
            _ => control.retroarch_key().to_owned(),
        };
        // An axis has to go under the _axis key, not _btn. RetroArch parses a
        // _btn value with strtoull, so "-0" and "+0" both come out as button 0
        // -- the two directions of one stick collapse onto the same button and
        // pressing either activates both. A silent misparse: nothing warns, and
        // the pad looks configured.
        let key = if binding.kind == BindingKind::Axis {
            key.replace("_btn", "_axis")
        } else {
            key
        };
        out.push(format!("{key} = \"{target}\""));
    }
    drop_shadowed_axis_halves(out)
}

/// RetroArch's player slots. Every one is written by a launch override,
/// assigned or not.
pub const MAX_PLAYERS: u32 = 16;

/// `input_playerN_device_reservation_type` values.
pub const RESERVATION_NONE: i32 = 0;
pub const RESERVATION_RESERVED: i32 = 2;

/// Pad indices with no device behind them, for the unmanaged slots.
///
/// RetroArch enumerates `pad_count` pads into 0..pad_count-1, so everything
/// from there to MAX_PLAYERS-1 is vacant. Handing each unmanaged player a
/// *distinct* vacant index is what stops two slots sharing one pad; reusing
/// RetroArch's own N-1 default would not, because with the physical pads
/// unhidden index N-1 is a real controller.
///
/// Degrades rather than collides if every index is occupied: the unmanaged
/// slots are set to RETRO_DEVICE_NONE regardless, so a repeated index there
/// reaches no core port.
pub fn empty_indices(pad_count: usize, wanted: usize) -> Vec<usize> {
    let ceiling = MAX_PLAYERS as usize - 1;
    (0..wanted).map(|n| (pad_count + n).min(ceiling)).collect()
}

/// Player -> the pad index RetroArch's udev driver will give its clone.
///
/// `order` is the live enumeration: index -> device path.
pub fn compute_pad_indices(
    virtual_paths: &BTreeMap<u32, String>,
    order: &BTreeMap<usize, String>,
) -> BTreeMap<u32, usize> {
    let by_path: BTreeMap<&str, usize> = order
        .iter()
        .map(|(index, path)| (path.as_str(), *index))
        .collect();
    virtual_paths
        .iter()
        .filter_map(|(player, path)| by_path.get(path.as_str()).map(|index| (*player, *index)))
        .collect()
}

/// Player -> pad index, for the players padmap is actually binding.
///
/// An assignment whose clone is missing from the enumeration is *not*
/// managed: padmap cannot bind a pad RetroArch will not see, and pretending
/// otherwise leaves that slot on whatever retroarch.cfg holds.
pub fn managed_players(
    players: &[u32],
    virtual_paths: &BTreeMap<u32, String>,
    order: &BTreeMap<usize, String>,
) -> BTreeMap<u32, usize> {
    let indices = compute_pad_indices(virtual_paths, order);
    players
        .iter()
        .filter_map(|player| indices.get(player).map(|index| (*player, *index)))
        .collect()
}

/// Reservation settings for all sixteen slots.
///
/// Managed slots are reserved for their clone by name; every other slot is
/// cleared, because an uncleared reservation naming a pad that is no longer
/// republished still occupies the slot.
pub fn reservation_lines(managed: &[u32], virtual_name: impl Fn(u32) -> String) -> String {
    let mut lines = Vec::new();
    for player in 1..=MAX_PLAYERS {
        let reserved = managed.contains(&player);
        let name = if reserved {
            virtual_name(player)
        } else {
            String::new()
        };
        let kind = if reserved {
            RESERVATION_RESERVED
        } else {
            RESERVATION_NONE
        };
        lines.push(format!("input_player{player}_reserved_device = \"{name}\""));
        lines.push(format!(
            "input_player{player}_device_reservation_type = \"{kind}\""
        ));
    }
    lines.join("\n")
}

/// Config text pinning each player to its clone by name.
///
/// Deliberately does **not** set `input_max_users`.
/// `reallocate_port_if_needed` computes the first free player slot while
/// skipping RESERVED ones, then early-returns if that index is at or above
/// `input_max_users` -- before it ever reaches the reservation matching loop.
/// Setting it to the number of reserved players makes every slot below it
/// reserved, the early return fires, and no reservation is ever matched.
/// There has to be at least one free, unreserved slot below it.
pub fn reservation_config(players: &[u32], virtual_name: impl Fn(u32) -> String) -> String {
    let mut sorted: Vec<u32> = players.to_vec();
    sorted.sort_unstable();
    let lines = [
        "# Generated by padmap. Pass to RetroArch with --appendconfig.",
        "# Players are pinned by device name, so pad enumeration order",
        "# no longer matters.",
        "",
        &reservation_lines(&sorted, virtual_name),
        "",
    ];
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::RA_INVISIBLE;

    fn bindings(entries: &[(Control, Binding)]) -> BTreeMap<Control, Binding> {
        entries.iter().copied().collect()
    }

    #[test]
    fn a_captured_button_is_written_under_its_retroarch_key() {
        let out = lines(
            &bindings(&[(Control::A, Binding::button(1))]),
            &BTreeMap::new(),
        );
        assert_eq!(out, ["input_b_btn = \"1\""]);
    }

    #[test]
    fn an_uncaptured_control_gets_no_line_at_all() {
        // A key bound to nothing is worse than an absent one: RetroArch binds a
        // button that does not exist and still reports the pad as configured.
        let out = lines(&BTreeMap::new(), &BTreeMap::new());
        assert!(out.is_empty());
    }

    #[test]
    fn lines_come_out_in_canonical_order_whatever_order_they_went_in() {
        let out = lines(
            &bindings(&[
                (Control::Start, Binding::button(9)),
                (Control::A, Binding::button(1)),
                (Control::B, Binding::button(2)),
            ]),
            &BTreeMap::new(),
        );
        assert_eq!(
            out,
            [
                "input_b_btn = \"1\"",
                "input_a_btn = \"2\"",
                "input_start_btn = \"9\"",
            ]
        );
    }

    #[test]
    fn a_console_override_replaces_the_canonical_key() {
        // mupen64plus-next reads N64 B from RetroPad Y, so binding the physical
        // B to the key the canonical name suggests produces a dead button.
        let overrides: BTreeMap<Control, String> = [(Control::B, "input_y_btn".to_owned())]
            .into_iter()
            .collect();
        let out = lines(&bindings(&[(Control::B, Binding::button(2))]), &overrides);
        assert_eq!(out, ["input_y_btn = \"2\""]);
    }

    #[test]
    fn an_empty_override_falls_back_rather_than_writing_a_nameless_key() {
        let overrides: BTreeMap<Control, String> =
            [(Control::B, String::new())].into_iter().collect();
        let out = lines(&bindings(&[(Control::B, Binding::button(2))]), &overrides);
        assert_eq!(out, ["input_a_btn = \"2\""]);
    }

    #[test]
    fn an_axis_moves_to_the_axis_key_because_btn_misparses_the_sign() {
        let out = lines(
            &bindings(&[(Control::LeftTrigger, Binding::axis(2, -1))]),
            &BTreeMap::new(),
        );
        assert_eq!(out, ["input_l2_axis = \"-2\""]);
        assert!(
            !out[0].contains("_btn"),
            "strtoull would read -2 as button 2"
        );
    }

    #[test]
    fn a_button_retroarch_cannot_see_is_dropped_not_renumbered() {
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
    fn a_hat_value_that_is_not_one_direction_is_dropped_not_fatal() {
        // This used to fail out of the middle of writing a launch profile and
        // leave the game to start with no controller config at all.
        let out = lines(
            &bindings(&[
                (Control::A, Binding::button(1)),
                (Control::DpadUp, Binding::hat(0, 3)),
            ]),
            &BTreeMap::new(),
        );
        assert_eq!(out, ["input_b_btn = \"1\""]);
    }

    #[test]
    fn a_c_button_on_one_half_kills_the_axis_on_the_other() {
        // The reported bug: Y mapped to C-up did nothing in Smash Bros, with a
        // C-stick that behaved and no error anywhere.
        let out = drop_shadowed_axis_halves(vec![
            "input_r_y_minus_btn = \"11\"".to_owned(),
            "input_r_y_plus_axis = \"+3\"".to_owned(),
            "input_l_x_minus_axis = \"-0\"".to_owned(),
        ]);
        assert_eq!(
            out,
            [
                "input_r_y_minus_btn = \"11\"",
                "input_l_x_minus_axis = \"-0\"",
            ],
            "the opposing axis must go, and nothing else"
        );
    }

    #[test]
    fn the_shadow_rule_works_in_both_directions() {
        let out = drop_shadowed_axis_halves(vec![
            "input_r_x_plus_btn = \"12\"".to_owned(),
            "input_r_x_minus_axis = \"-2\"".to_owned(),
        ]);
        assert_eq!(out, ["input_r_x_plus_btn = \"12\""]);
    }

    #[test]
    fn a_stick_with_axes_on_both_halves_is_left_alone() {
        let input = vec![
            "input_r_y_minus_axis = \"-3\"".to_owned(),
            "input_r_y_plus_axis = \"+3\"".to_owned(),
        ];
        assert_eq!(drop_shadowed_axis_halves(input.clone()), input);
    }

    #[test]
    fn a_stick_with_buttons_on_both_halves_is_left_alone() {
        let input = vec![
            "input_r_y_minus_btn = \"11\"".to_owned(),
            "input_r_y_plus_btn = \"12\"".to_owned(),
        ];
        assert_eq!(drop_shadowed_axis_halves(input.clone()), input);
    }

    #[test]
    fn a_line_with_no_separator_is_not_mistaken_for_a_key() {
        let input = vec!["# a comment".to_owned(), "input_b_btn = \"1\"".to_owned()];
        assert_eq!(drop_shadowed_axis_halves(input.clone()), input);
    }

    #[test]
    fn the_full_n64_c_button_capture_survives_end_to_end() {
        // Four C-buttons as right-stick halves, which is the case the shadow
        // rule exists for, plus the face buttons around them.
        let out = lines(
            &bindings(&[
                (Control::A, Binding::button(1)),
                (Control::B, Binding::button(2)),
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
                "input_b_btn = \"1\"",
                "input_a_btn = \"2\"",
                "input_r_y_minus_btn = \"11\"",
                "input_r_y_plus_btn = \"12\"",
                "input_r_x_minus_btn = \"13\"",
                "input_r_x_plus_btn = \"14\"",
            ]
        );
    }
}
