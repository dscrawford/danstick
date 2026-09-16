//! A pad that follows the kernel's gamepad convention binds itself.
//!
//! [`crate::guess`] is for a pad nothing knows anything about, and says so:
//! its face buttons are a genuine guess, assigned by *index* in the order SDL
//! assumes. That is the right answer for a device padmap cannot recognise, and
//! the wrong one for almost every controller made this decade.
//!
//! `Documentation/input/gamepad.rst` defines the codes: `BTN_SOUTH` is the
//! bottom face button, `BTN_EAST` the right one, and so on round. xpad,
//! hid-nintendo, hid-playstation and every hid-generic gamepad emit them. For
//! such a pad the binding is not a guess at all -- it is written down, and
//! reading it off the codes is exact.
//!
//! What that is worth, measured on a wired Xbox 360 pad -- the most common
//! controller on Linux, and one SDL's own database has no entry for, since SDL
//! recognises xpad devices by heuristic rather than by GUID:
//!
//! | control | by index | by code |
//! | --- | --- | --- |
//! | x / y | swapped | right |
//! | back | `BTN_MODE` | `BTN_SELECT` |
//! | start | `BTN_THUMBL` | `BTN_START` |
//! | lefttrigger | `BTN_SELECT` | `ABS_Z` |
//!
//! Seven of its eleven buttons landed somewhere else. A user plugging one in
//! found Select opened a menu bound to the left trigger, and the only way out
//! was the mapping wizard -- which they had to navigate with those bindings.

use std::collections::BTreeMap;

use crate::binding::sdl_button_index;
use crate::fields::Fields;
use crate::sdl::AxisSpan;

/// The kernel's gamepad buttons, and the control each one *is*.
///
/// `BTN_NORTH` is `y` and `BTN_WEST` is `x`, which reads backwards until you
/// remember the codes are named for position and the controls for an Xbox
/// pad's lettering: north is where Y sits, west is where X sits.
pub const STANDARD_BUTTONS: [(u16, &str); 13] = [
    (0x130, "a"),             // BTN_SOUTH
    (0x131, "b"),             // BTN_EAST
    (0x133, "y"),             // BTN_NORTH
    (0x134, "x"),             // BTN_WEST
    (0x136, "leftshoulder"),  // BTN_TL
    (0x137, "rightshoulder"), // BTN_TR
    // Digital triggers. A pad with analogue ones reports those instead, and
    // the axis wins where both exist -- see `trigger_fields`.
    (0x138, "lefttrigger"),  // BTN_TL2
    (0x139, "righttrigger"), // BTN_TR2
    (0x13A, "back"),         // BTN_SELECT
    (0x13B, "start"),        // BTN_START
    (0x13C, "guide"),        // BTN_MODE
    (0x13D, "leftstick"),    // BTN_THUMBL
    (0x13E, "rightstick"),   // BTN_THUMBR
];

/// Analogue triggers, where a standard pad puts them.
pub const TRIGGER_AXES: [(u16, &str); 2] = [
    (0x02, "lefttrigger"),  // ABS_Z
    (0x05, "righttrigger"), // ABS_RZ
];

/// Does this pad speak the convention?
///
/// The two buttons every gamepad has. A device with neither -- an arcade
/// stick reporting `BTN_TRIGGER`, a wheel, something exotic -- is not one of
/// these and falls back to the positional guess, which is what it was always
/// getting.
pub fn is_standard(keys: &[u16]) -> bool {
    keys.contains(&0x130) && keys.contains(&0x131)
}

/// Which axes are analogue triggers rather than sticks.
///
/// `ABS_Z` is a trigger on an Xbox pad and the C-stick's Y on a Mayflash
/// GameCube adapter, and both declare `BTN_SOUTH`, so the code cannot decide
/// it. Where the axis *rests* can: a trigger sits at its minimum untouched and
/// a stick centres. The same rule keeps padmap from publishing that adapter's
/// triggers as a right stick.
fn trigger_fields(axis_codes: &[u16], axes: Option<&BTreeMap<u16, AxisSpan>>) -> Fields {
    let mut fields = Fields::new();
    for (code, field) in TRIGGER_AXES {
        if !axis_codes.contains(&code) {
            continue;
        }
        let centred = axes
            .and_then(|spans| spans.get(&code))
            .map(AxisSpan::rests_centred);
        // With no absinfo to consult, the code's conventional meaning stands:
        // a caller that could not read the spans is no worse off than one that
        // never looked.
        if centred == Some(true) {
            continue;
        }
        let index = axis_index(axis_codes, code);
        if let Some(index) = index {
            fields.insert(field, format!("a{index}"));
        }
    }
    fields
}

/// Where an axis sits in SDL's numbering: its ordinal among the axes that are
/// not hats.
fn axis_index(axis_codes: &[u16], wanted: u16) -> Option<usize> {
    let mut sorted: Vec<u16> = axis_codes
        .iter()
        .copied()
        .filter(|code| *code != crate::guess::ABS_HAT0X && *code != crate::guess::ABS_HAT0Y)
        .collect();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.iter().position(|code| *code == wanted)
}

/// Every control this pad's codes name, or `None` if it is not a standard pad.
///
/// Only the buttons and triggers: directions and sticks are the same question
/// for every pad and [`crate::guess::stick_and_dpad_fields`] already answers
/// it.
pub fn standard_fields(
    keys: &[u16],
    axis_codes: &[u16],
    axes: Option<&BTreeMap<u16, AxisSpan>>,
) -> Option<Fields> {
    if !is_standard(keys) {
        return None;
    }
    let triggers = trigger_fields(axis_codes, axes);
    let mut fields = Fields::new();
    for (code, field) in STANDARD_BUTTONS {
        // An analogue trigger beats the digital button of the same name: a
        // pad with both reports the button only at full pull, and a game
        // reading the axis gets nothing from it.
        if triggers.contains_key(field) {
            continue;
        }
        if let Some(index) = sdl_button_index(keys, code) {
            fields.insert(field, format!("b{index}"));
        }
    }
    fields.extend(&triggers);
    Some(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wired Xbox 360 pad, exactly as xpad publishes it.
    fn xbox() -> (Vec<u16>, Vec<u16>, BTreeMap<u16, AxisSpan>) {
        let keys = vec![
            0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x13A, 0x13B, 0x13C, 0x13D, 0x13E,
        ];
        let axis_codes = vec![0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x10, 0x11];
        let mut axes = BTreeMap::new();
        for code in [0x00, 0x01, 0x03, 0x04] {
            axes.insert(code, AxisSpan::new(-32768, 32767, 0));
        }
        for code in [0x02, 0x05] {
            axes.insert(code, AxisSpan::new(0, 255, 0));
        }
        (keys, axis_codes, axes)
    }

    #[test]
    fn an_xbox_pad_binds_itself_correctly() {
        let (keys, axis_codes, axes) = xbox();
        let fields = standard_fields(&keys, &axis_codes, Some(&axes)).expect("a standard pad");
        // The four the positional guess got wrong, and the two it got right.
        assert_eq!(fields.get("a"), Some("b0"), "BTN_SOUTH");
        assert_eq!(fields.get("b"), Some("b1"), "BTN_EAST");
        assert_eq!(fields.get("y"), Some("b2"), "BTN_NORTH is y, not x");
        assert_eq!(fields.get("x"), Some("b3"), "BTN_WEST is x, not y");
        assert_eq!(fields.get("back"), Some("b6"), "BTN_SELECT, not BTN_MODE");
        assert_eq!(fields.get("start"), Some("b7"));
        assert_eq!(fields.get("guide"), Some("b8"));
        assert_eq!(fields.get("leftstick"), Some("b9"));
        assert_eq!(fields.get("rightstick"), Some("b10"));
        // And the triggers are axes, which the positional guess never bound
        // at all -- it had them as two of the face buttons.
        assert_eq!(fields.get("lefttrigger"), Some("a2"));
        assert_eq!(fields.get("righttrigger"), Some("a5"));
    }

    #[test]
    fn a_gamecube_adapters_c_stick_is_not_a_trigger() {
        // The case the code alone cannot decide: ABS_Z is a trigger on an Xbox
        // pad and the C-stick's Y here, and both declare BTN_SOUTH. Where it
        // rests is what tells them apart.
        let keys = vec![0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x13A, 0x13B];
        let axis_codes = vec![0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x10, 0x11];
        let mut axes = BTreeMap::new();
        axes.insert(0x00, AxisSpan::new(0, 255, 128));
        axes.insert(0x01, AxisSpan::new(0, 255, 128));
        // The C-stick, centred.
        axes.insert(0x02, AxisSpan::new(0, 255, 131));
        axes.insert(0x05, AxisSpan::new(0, 255, 128));
        // The real triggers, on stick codes and resting at their minimum.
        axes.insert(0x03, AxisSpan::new(0, 255, 24));
        axes.insert(0x04, AxisSpan::new(0, 255, 25));

        let fields = standard_fields(&keys, &axis_codes, Some(&axes)).expect("standard buttons");
        assert_eq!(
            fields.get("lefttrigger"),
            None,
            "the C-stick was bound as a trigger"
        );
        assert_eq!(fields.get("righttrigger"), None);
    }

    #[test]
    fn an_analogue_trigger_beats_the_digital_button_of_the_same_name() {
        // A DualSense reports both. The button only fires at full pull, so a
        // game reading the axis would get nothing from it.
        let keys = vec![0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x138, 0x139];
        let axis_codes = vec![0x00, 0x01, 0x02, 0x05];
        let mut axes = BTreeMap::new();
        axes.insert(0x02, AxisSpan::new(0, 255, 0));
        axes.insert(0x05, AxisSpan::new(0, 255, 0));
        let fields = standard_fields(&keys, &axis_codes, Some(&axes)).expect("standard");
        assert_eq!(fields.get("lefttrigger"), Some("a2"));
        assert_eq!(fields.get("righttrigger"), Some("a3"));
    }

    #[test]
    fn a_digital_only_trigger_is_still_bound() {
        let keys = vec![0x130, 0x131, 0x138, 0x139];
        let fields = standard_fields(&keys, &[], None).expect("standard");
        assert_eq!(fields.get("lefttrigger"), Some("b2"));
        assert_eq!(fields.get("righttrigger"), Some("b3"));
    }

    #[test]
    fn a_pad_that_does_not_speak_the_convention_is_left_to_the_guess() {
        // An arcade stick reporting BTN_TRIGGER and friends, which is exactly
        // the device whose face buttons genuinely are a guess -- SDL's own
        // database has the measured Fightstick as `a:b1,x:b0`.
        assert!(standard_fields(&[0x120, 0x121, 0x122], &[], None).is_none());
        assert!(!is_standard(&[0x120]));
        // One of the two is not enough: BTN_SOUTH alone could be anything.
        assert!(!is_standard(&[0x130]));
        assert!(is_standard(&[0x130, 0x131]));
    }

    #[test]
    fn a_control_the_pad_does_not_have_is_left_unbound() {
        // Binding it anyway would claim a button that is not there, and SDL
        // would report presses for a control nobody can reach.
        let keys = vec![0x130, 0x131];
        let fields = standard_fields(&keys, &[], None).expect("standard");
        assert_eq!(fields.get("a"), Some("b0"));
        assert_eq!(fields.get("b"), Some("b1"));
        assert_eq!(fields.get("start"), None);
        assert_eq!(fields.get("guide"), None);
    }
}
