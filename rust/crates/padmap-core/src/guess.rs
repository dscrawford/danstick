//! A mapping for a pad nothing knows anything about.
//!
//! Two very different halves, and the split is the point.
//!
//! The face buttons are a genuine guess -- which physical button is A is
//! exactly what the wizard exists to find out, and pads disagree (SDL's own
//! database has the measured Fightstick as `a:b1,x:b0`). The order used here is
//! the one SDL assumes when it has nothing better, so this can only be as wrong
//! as doing nothing, and no more.
//!
//! Everything below the face buttons is *not* a guess, and is where the value
//! is: a hat is a hat and ABS_X is the left stick's X on every pad ever made.
//! Those are the bindings that decide whether a menu can be navigated at all,
//! which is what a user needs before they can reach the wizard that fixes the
//! rest.

use std::collections::BTreeMap;

use crate::binding::sdl_button_index;
use crate::fields::Fields;
use crate::sdl::{self, AxisSpan};

/// The order SDL assumes for a pad it has never seen.
pub const GUESS_BUTTON_ORDER: [&str; 12] = [
    "a",
    "b",
    "x",
    "y",
    "leftshoulder",
    "rightshoulder",
    "lefttrigger",
    "righttrigger",
    "back",
    "start",
    "leftstick",
    "rightstick",
];

/// BTN_DPAD_UP..BTN_DPAD_RIGHT, for pads that report directions as keys.
pub const DPAD_KEYS: [(u16, &str); 4] = [
    (0x220, "dpup"),
    (0x221, "dpdown"),
    (0x222, "dpleft"),
    (0x223, "dpright"),
];

pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

/// Directions and sticks, from what the pad actually reports.
pub fn stick_and_dpad_fields(
    axis_codes: &[u16],
    keys: &[u16],
    axes: Option<&BTreeMap<u16, AxisSpan>>,
) -> Fields {
    let mut fields = Fields::new();
    if axis_codes.contains(&ABS_HAT0X) && axis_codes.contains(&ABS_HAT0Y) {
        // Hats are numbered separately from axes; hat 0 is the only one any
        // measured pad has.
        fields.insert("dpup", "h0.1");
        fields.insert("dpright", "h0.2");
        fields.insert("dpdown", "h0.4");
        fields.insert("dpleft", "h0.8");
    } else {
        // No hat: some pads report the d-pad as four ordinary keys instead.
        for (code, field) in DPAD_KEYS {
            if let Some(index) = sdl_button_index(keys, code) {
                fields.insert(field, format!("b{index}"));
            }
        }
    }
    fields.extend(&sdl::stick_fields(axis_codes, &BTreeMap::new(), axes));
    fields
}

/// A whole guessed mapping for an unmapped pad.
pub fn guessed_fields(
    keys: &[u16],
    axis_codes: &[u16],
    axes: Option<&BTreeMap<u16, AxisSpan>>,
) -> Fields {
    let mut sorted: Vec<u16> = keys.to_vec();
    sorted.sort_unstable();
    // SDL's own ordering: the joystick range first, then anything below it.
    let ordered = sorted.len();
    let mut fields = Fields::new();
    for (index, field) in GUESS_BUTTON_ORDER.iter().enumerate() {
        if index < ordered {
            fields.insert(*field, format!("b{index}"));
        }
    }
    fields.extend(&stick_and_dpad_fields(axis_codes, keys, axes));
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(fields: &Fields) -> Vec<(String, String)> {
        fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    #[test]
    fn a_pad_with_a_hat_gets_the_hat_and_not_four_buttons() {
        let fields = stick_and_dpad_fields(&[0x00, 0x01, ABS_HAT0X, ABS_HAT0Y], &[], None);
        assert_eq!(fields.get("dpup"), Some("h0.1"));
        assert_eq!(fields.get("dpright"), Some("h0.2"));
        assert_eq!(fields.get("dpdown"), Some("h0.4"));
        assert_eq!(fields.get("dpleft"), Some("h0.8"));
    }

    #[test]
    fn a_pad_reporting_directions_as_keys_gets_them_as_buttons() {
        // Some pads have no hat at all; without this they navigate nothing.
        let keys: Vec<u16> = vec![0x130, 0x220, 0x221, 0x222, 0x223];
        let fields = stick_and_dpad_fields(&[0x00, 0x01], &keys, None);
        assert_eq!(fields.get("dpup"), Some("b1"));
        assert_eq!(fields.get("dpdown"), Some("b2"));
        assert_eq!(fields.get("dpleft"), Some("b3"));
        assert_eq!(fields.get("dpright"), Some("b4"));
    }

    #[test]
    fn the_hat_wins_when_a_pad_somehow_reports_both() {
        let keys: Vec<u16> = vec![0x220, 0x221, 0x222, 0x223];
        let fields = stick_and_dpad_fields(&[ABS_HAT0X, ABS_HAT0Y], &keys, None);
        assert_eq!(fields.get("dpup"), Some("h0.1"), "the hat is the real one");
    }

    #[test]
    fn a_pad_with_half_a_hat_falls_back_to_keys() {
        // Both codes or neither: half a hat cannot express four directions.
        let keys: Vec<u16> = vec![0x220];
        let fields = stick_and_dpad_fields(&[ABS_HAT0X], &keys, None);
        assert_eq!(fields.get("dpup"), Some("b0"));
    }

    #[test]
    fn sticks_come_from_the_axes_the_pad_reports() {
        let fields = stick_and_dpad_fields(&[0x00, 0x01, 0x03, 0x04], &[], None);
        assert_eq!(fields.get("leftx"), Some("a0"));
        assert_eq!(fields.get("lefty"), Some("a1"));
        assert_eq!(fields.get("rightx"), Some("a2"));
        assert_eq!(fields.get("righty"), Some("a3"));
    }

    #[test]
    fn a_trigger_masquerading_as_a_stick_axis_is_refused() {
        // The Mayflash GameCube adapter reports its analogue triggers as
        // ABS_RX and ABS_RY; calling them the right stick jams it to a corner.
        let axes: BTreeMap<u16, AxisSpan> = [
            (0x00, AxisSpan::new(0, 255, 128)),
            (0x01, AxisSpan::new(0, 255, 128)),
            (0x03, AxisSpan::new(0, 255, 20)),
            (0x04, AxisSpan::new(0, 255, 235)),
        ]
        .into_iter()
        .collect();
        let fields = stick_and_dpad_fields(&[0x00, 0x01, 0x03, 0x04], &[], Some(&axes));
        assert_eq!(fields.get("leftx"), Some("a0"));
        assert_eq!(fields.get("rightx"), None);
        assert_eq!(fields.get("righty"), None);
    }

    #[test]
    fn a_pad_with_fewer_buttons_than_the_order_gets_only_what_it_has() {
        // Naming a button the pad does not have is a binding that does
        // nothing, on a pad reported as configured.
        let fields = guessed_fields(&[0x130, 0x131, 0x132], &[], None);
        assert_eq!(fields.get("a"), Some("b0"));
        assert_eq!(fields.get("x"), Some("b2"));
        assert_eq!(fields.get("y"), None);
        assert_eq!(fields.get("start"), None);
    }

    #[test]
    fn a_pad_with_more_buttons_than_the_order_stops_at_the_order() {
        let keys: Vec<u16> = (0x130..0x150).collect();
        let fields = guessed_fields(&keys, &[], None);
        assert_eq!(fields.get("rightstick"), Some("b11"));
        assert_eq!(fields.len(), GUESS_BUTTON_ORDER.len());
    }

    #[test]
    fn the_guess_is_face_buttons_in_order_then_everything_measured() {
        let fields = guessed_fields(
            &(0x130..0x13c).collect::<Vec<u16>>(),
            &[0x00, 0x01, ABS_HAT0X, ABS_HAT0Y],
            None,
        );
        let names: Vec<String> = pairs(&fields).into_iter().map(|(k, _)| k).collect();
        assert_eq!(&names[..GUESS_BUTTON_ORDER.len()], &GUESS_BUTTON_ORDER[..]);
        assert!(names.contains(&"dpup".to_owned()));
        assert!(names.contains(&"leftx".to_owned()));
    }

    #[test]
    fn a_pad_with_nothing_at_all_guesses_nothing() {
        assert!(guessed_fields(&[], &[], None).is_empty());
    }
}
