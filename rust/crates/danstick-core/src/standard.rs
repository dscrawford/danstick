//! Kernel gamepad codes (BTN_SOUTH, BTN_EAST, etc.) for exact button binding.

use std::collections::BTreeMap;

use crate::binding::sdl_button_index;
use crate::fields::Fields;
use crate::sdl::AxisSpan;

/// Kernel gamepad buttons: codes named by position, controls by Xbox lettering.
pub const STANDARD_BUTTONS: [(u16, &str); 13] = [
    (0x130, "a"),             // BTN_SOUTH
    (0x131, "b"),             // BTN_EAST
    (0x133, "y"),             // BTN_NORTH
    (0x134, "x"),             // BTN_WEST
    (0x136, "leftshoulder"),  // BTN_TL
    (0x137, "rightshoulder"), // BTN_TR
    (0x138, "lefttrigger"),   // BTN_TL2 (digital; analogue axes win)
    (0x139, "righttrigger"),  // BTN_TR2
    (0x13A, "back"),          // BTN_SELECT
    (0x13B, "start"),         // BTN_START
    (0x13C, "guide"),         // BTN_MODE
    (0x13D, "leftstick"),     // BTN_THUMBL
    (0x13E, "rightstick"),    // BTN_THUMBR
];

pub const TRIGGER_AXES: [(u16, &str); 2] = [
    (0x02, "lefttrigger"),  // ABS_Z
    (0x05, "righttrigger"), // ABS_RZ
];

/// Has BTN_SOUTH and BTN_EAST (required for convention).
pub fn is_standard(keys: &[u16]) -> bool {
    keys.contains(&0x130) && keys.contains(&0x131)
}

/// Analogue trigger axes vs sticks: determined by rest position.
fn trigger_fields(axis_codes: &[u16], axes: Option<&BTreeMap<u16, AxisSpan>>) -> Fields {
    let mut fields = Fields::new();
    for (code, field) in TRIGGER_AXES {
        if !axis_codes.contains(&code) {
            continue;
        }
        let centred = axes
            .and_then(|spans| spans.get(&code))
            .map(AxisSpan::rests_centred);
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

/// Axis ordinal among non-hat axes in SDL numbering.
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

/// Buttons and triggers this standard pad's codes name.
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
        assert_eq!(fields.get("a"), Some("b0"), "BTN_SOUTH");
        assert_eq!(fields.get("b"), Some("b1"), "BTN_EAST");
        assert_eq!(fields.get("y"), Some("b2"), "BTN_NORTH is y, not x");
        assert_eq!(fields.get("x"), Some("b3"), "BTN_WEST is x, not y");
        assert_eq!(fields.get("back"), Some("b6"), "BTN_SELECT, not BTN_MODE");
        assert_eq!(fields.get("start"), Some("b7"));
        assert_eq!(fields.get("guide"), Some("b8"));
        assert_eq!(fields.get("leftstick"), Some("b9"));
        assert_eq!(fields.get("rightstick"), Some("b10"));
        assert_eq!(fields.get("lefttrigger"), Some("a2"));
        assert_eq!(fields.get("righttrigger"), Some("a5"));
    }

    #[test]
    fn a_gamecube_adapters_c_stick_is_not_a_trigger() {
        let keys = vec![0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x13A, 0x13B];
        let axis_codes = vec![0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x10, 0x11];
        let mut axes = BTreeMap::new();
        axes.insert(0x00, AxisSpan::new(0, 255, 128));
        axes.insert(0x01, AxisSpan::new(0, 255, 128));
        axes.insert(0x02, AxisSpan::new(0, 255, 131));
        axes.insert(0x05, AxisSpan::new(0, 255, 128));
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
        assert!(standard_fields(&[0x120, 0x121, 0x122], &[], None).is_none());
        assert!(!is_standard(&[0x120]));
        // One of the two is not enough: BTN_SOUTH alone could be anything.
        assert!(!is_standard(&[0x130]));
        assert!(is_standard(&[0x130, 0x131]));
    }

    #[test]
    fn a_control_the_pad_does_not_have_is_left_unbound() {
        let keys = vec![0x130, 0x131];
        let fields = standard_fields(&keys, &[], None).expect("standard");
        assert_eq!(fields.get("a"), Some("b0"));
        assert_eq!(fields.get("b"), Some("b1"));
        assert_eq!(fields.get("start"), None);
        assert_eq!(fields.get("guide"), None);
    }
}
