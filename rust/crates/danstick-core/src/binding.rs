//! Control locations and their SDL spellings (hat vs buttons, axis vs digital).

use std::fmt;

use serde::{Deserialize, Serialize};

/// SDL numbers joystick buttons from here before the codes below it.
pub const BTN_JOYSTICK: u16 = 0x120;

pub const HAT_CODES: std::ops::Range<u16> = 0x10..0x18;

/// Hat bit to direction word: only single bits (diagonals refused).
pub const fn hat_direction(value: i32) -> Option<&'static str> {
    match value {
        1 => Some("up"),
        2 => Some("right"),
        4 => Some("down"),
        8 => Some("left"),
        _ => None,
    }
}

/// What kind of input a control is wired to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindingKind {
    Button,
    Hat,
    Axis,
}

impl BindingKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            BindingKind::Button => "button",
            BindingKind::Hat => "hat",
            BindingKind::Axis => "axis",
        }
    }
}

impl fmt::Display for BindingKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Binding unexpressible to a consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unspellable {
    HatValue(i32),
}

impl fmt::Display for Unspellable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unspellable::HatValue(value) => {
                write!(f, "hat value {value} is not one direction bit")
            }
        }
    }
}

impl std::error::Error for Unspellable {}

/// Control location: kind, index, value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Binding {
    pub kind: BindingKind,
    pub index: i32,
    #[serde(default)]
    pub value: i32,
}

impl Binding {
    pub const fn button(index: i32) -> Self {
        Binding {
            kind: BindingKind::Button,
            index,
            value: 0,
        }
    }

    pub const fn hat(index: i32, value: i32) -> Self {
        Binding {
            kind: BindingKind::Hat,
            index,
            value,
        }
    }

    pub const fn axis(index: i32, value: i32) -> Self {
        Binding {
            kind: BindingKind::Axis,
            index,
            value,
        }
    }

    /// Whether SDL can be told about this binding at all.
    pub const fn sdl_visible(&self) -> bool {
        match self.kind {
            BindingKind::Hat => hat_direction(self.value).is_some(),
            BindingKind::Button | BindingKind::Axis => true,
        }
    }

    /// SDL's spelling: `b3`, `h0.1`, `+a2`.
    pub fn sdl(&self) -> Result<String, Unspellable> {
        match self.kind {
            BindingKind::Button => Ok(format!("b{}", self.index)),
            BindingKind::Hat => match hat_direction(self.value) {
                Some(_) => Ok(format!("h{}.{}", self.index, self.value)),
                None => Err(Unspellable::HatValue(self.value)),
            },
            BindingKind::Axis => {
                let sign = if self.value >= 0 { '+' } else { '-' };
                Ok(format!("{sign}a{}", self.index))
            }
        }
    }
}

/// SDL button index: walks 0x120..KEY_MAX first, then 0..0x120.
pub fn sdl_button_index(keys: &[u16], code: u16) -> Option<i32> {
    let mut sorted: Vec<u16> = keys.to_vec();
    sorted.sort_unstable();
    let ordered = sorted
        .iter()
        .copied()
        .filter(|c| *c >= BTN_JOYSTICK)
        .chain(sorted.iter().copied().filter(|c| *c < BTN_JOYSTICK));
    ordered
        .enumerate()
        .find(|(_, candidate)| *candidate == code)
        .map(|(index, _)| index as i32)
}

/// Axis index: ascending ABS codes, skipping hats.
pub fn axis_index(codes: &[u16], code: u16) -> Option<i32> {
    let mut sorted: Vec<u16> = codes
        .iter()
        .copied()
        .filter(|c| !HAT_CODES.contains(c))
        .collect();
    sorted.sort_unstable();
    sorted
        .iter()
        .position(|candidate| *candidate == code)
        .map(|index| index as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_spells_its_number() {
        assert_eq!(Binding::button(3).sdl().expect("sdl"), "b3");
        assert_eq!(Binding::button(13).sdl().expect("sdl"), "b13");
    }

    #[test]
    fn the_four_hat_directions_spell_correctly() {
        for value in [1, 2, 4, 8] {
            let binding = Binding::hat(0, value);
            assert!(binding.sdl_visible());
            assert_eq!(binding.sdl().expect("sdl"), format!("h0.{value}"));
        }
    }

    #[test]
    fn a_diagonal_or_resting_hat_is_not_a_control() {
        for value in [0, 3, 5, 6, 9, 12, 15, -1] {
            let binding = Binding::hat(0, value);
            assert!(!binding.sdl_visible(), "hat {value} passed sdl_visible");
            assert_eq!(binding.sdl(), Err(Unspellable::HatValue(value)));
        }
    }

    #[test]
    fn an_axis_carries_its_sign_and_zero_counts_as_positive() {
        assert_eq!(Binding::axis(2, 1).sdl().expect("sdl"), "+a2");
        assert_eq!(Binding::axis(2, -1).sdl().expect("sdl"), "-a2");
        assert_eq!(Binding::axis(2, 0).sdl().expect("sdl"), "+a2");
        assert_eq!(Binding::axis(0, -1).sdl().expect("sdl"), "-a0");
    }

    #[test]
    fn sdl_numbers_joystick_buttons_before_the_low_ones() {
        let keys: Vec<u16> = (0x120..=0x12b).collect();
        assert_eq!(sdl_button_index(&keys, 0x120), Some(0));
        assert_eq!(sdl_button_index(&keys, 0x121), Some(1));
        assert_eq!(sdl_button_index(&keys, 0x128), Some(8));
    }

    #[test]
    fn a_keyboard_code_sorts_after_every_joystick_button_for_sdl() {
        let keys = [0x1e_u16, 0x120, 0x121, 0x122];
        assert_eq!(sdl_button_index(&keys, 0x120), Some(0));
        assert_eq!(sdl_button_index(&keys, 0x122), Some(2));
        assert_eq!(sdl_button_index(&keys, 0x1e), Some(3));
    }

    #[test]
    fn sdl_does_not_number_a_code_the_pad_does_not_report() {
        assert_eq!(sdl_button_index(&[0x120, 0x121], 0x130), None);
        assert_eq!(sdl_button_index(&[], 0x120), None);
    }

    #[test]
    fn a_btn_misc_code_sorts_after_the_joystick_buttons_for_sdl() {
        let keys = [0x100_u16, 0x120, 0x121];
        assert_eq!(sdl_button_index(&keys, 0x120), Some(0));
        assert_eq!(sdl_button_index(&keys, 0x121), Some(1));
        assert_eq!(sdl_button_index(&keys, 0x100), Some(2));
    }

    #[test]
    fn axes_are_numbered_among_real_axes_with_hats_skipped() {
        let codes = [0x00_u16, 0x01, 0x03, 0x04, 0x05, 0x10, 0x11];
        assert_eq!(axis_index(&codes, 0x00), Some(0));
        assert_eq!(axis_index(&codes, 0x01), Some(1));
        assert_eq!(axis_index(&codes, 0x03), Some(2));
        assert_eq!(axis_index(&codes, 0x04), Some(3));
        assert_eq!(axis_index(&codes, 0x05), Some(4), "ABS_RZ is not axis 5");
        assert_eq!(axis_index(&codes, 0x10), None, "a hat is not an axis");
    }

    #[test]
    fn indices_do_not_depend_on_the_order_the_codes_arrive_in() {
        let ascending = [0x120_u16, 0x121, 0x122];
        let shuffled = [0x122_u16, 0x120, 0x121];
        for code in ascending {
            assert_eq!(
                sdl_button_index(&ascending, code),
                sdl_button_index(&shuffled, code)
            );
        }
    }

    #[test]
    fn a_binding_round_trips_through_json_with_the_python_field_names() {
        let binding = Binding::hat(0, 4);
        let json = serde_json::to_value(binding).expect("serialize");
        assert_eq!(json["kind"], "hat");
        assert_eq!(json["index"], 0);
        assert_eq!(json["value"], 4);
        let back: Binding = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, binding);
    }

    #[test]
    fn a_stored_binding_from_before_still_reads_and_drops_its_ra_index() {
        let raw = serde_json::json!({"kind": "button", "index": 7, "ra_index": -1});
        let binding: Binding = serde_json::from_value(raw).expect("deserialize");
        assert_eq!(binding, Binding::button(7));
        let json = serde_json::to_value(binding).expect("serialize");
        assert!(json.get("ra_index").is_none(), "{json}");
    }
}
