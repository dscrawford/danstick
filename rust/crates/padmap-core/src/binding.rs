//! Control locations and consumer-specific spellings (hat vs buttons, axis vs digital).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Button numbering bases: SDL starts 0x120; RetroArch starts 0x100.
pub const BTN_MISC: u16 = 0x100;
pub const BTN_JOYSTICK: u16 = 0x120;

/// Sentinel: buttons below BTN_MISC invisible to RetroArch (not just absent).
pub const RA_INVISIBLE: i32 = -1;

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
    InvisibleToRetroarch,
}

impl fmt::Display for Unspellable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unspellable::HatValue(value) => {
                write!(f, "hat value {value} is not one direction bit")
            }
            Unspellable::InvisibleToRetroarch => f.write_str(
                "RetroArch's udev driver has no number for this button \
                 (its evdev code is below BTN_MISC)",
            ),
        }
    }
}

impl std::error::Error for Unspellable {}

/// Control location: kind, index, value; ra_index for RetroArch offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Binding {
    pub kind: BindingKind,
    pub index: i32,
    #[serde(default)]
    pub value: i32,
    #[serde(default)]
    pub ra_index: Option<i32>,
}

impl Binding {
    pub const fn button(index: i32) -> Self {
        Binding {
            kind: BindingKind::Button,
            index,
            value: 0,
            ra_index: None,
        }
    }

    pub const fn hat(index: i32, value: i32) -> Self {
        Binding {
            kind: BindingKind::Hat,
            index,
            value,
            ra_index: None,
        }
    }

    pub const fn axis(index: i32, value: i32) -> Self {
        Binding {
            kind: BindingKind::Axis,
            index,
            value,
            ra_index: None,
        }
    }

    pub const fn with_ra_index(mut self, ra_index: Option<i32>) -> Self {
        self.ra_index = ra_index;
        self
    }

    /// Whether SDL can be told about this binding at all.
    pub const fn sdl_visible(&self) -> bool {
        match self.kind {
            BindingKind::Hat => hat_direction(self.value).is_some(),
            BindingKind::Button | BindingKind::Axis => true,
        }
    }

    /// Whether RetroArch can be told about this binding at all.
    pub const fn retroarch_visible(&self) -> bool {
        if !self.sdl_visible() {
            return false;
        }
        // Any negative index, not just RA_INVISIBLE itself: a profile off disk can hold whatever it likes, and no real button is negative.
        match (self.kind, self.ra_index) {
            (BindingKind::Button, Some(index)) => index >= 0,
            _ => true,
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

    /// RetroArch's spelling of the same thing: `3`, `h0up`, `+2`.
    pub fn retroarch(&self) -> Result<String, Unspellable> {
        let index = self.ra_index.unwrap_or(self.index);
        match self.kind {
            BindingKind::Button => {
                if index < 0 {
                    Err(Unspellable::InvisibleToRetroarch)
                } else {
                    Ok(index.to_string())
                }
            }
            BindingKind::Hat => match hat_direction(self.value) {
                Some(word) => Ok(format!("h{index}{word}")),
                None => Err(Unspellable::HatValue(self.value)),
            },
            BindingKind::Axis => {
                let sign = if self.value >= 0 { '+' } else { '-' };
                Ok(format!("{sign}{index}"))
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

/// RetroArch button index: ascending from BTN_MISC (0x100).
pub fn retroarch_button_index(keys: &[u16], code: u16) -> Option<i32> {
    if !keys.contains(&code) {
        return None;
    }
    if code < BTN_MISC {
        return Some(RA_INVISIBLE);
    }
    let mut sorted: Vec<u16> = keys.iter().copied().filter(|c| *c >= BTN_MISC).collect();
    sorted.sort_unstable();
    sorted
        .iter()
        .position(|candidate| *candidate == code)
        .map(|index| index as i32)
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
    fn a_button_spells_the_same_number_to_both_when_they_agree() {
        let binding = Binding::button(3);
        assert_eq!(binding.sdl().expect("sdl"), "b3");
        assert_eq!(binding.retroarch().expect("retroarch"), "3");
    }

    #[test]
    fn a_split_index_spells_differently_to_each_consumer() {
        let binding = Binding::button(13).with_ra_index(Some(11));
        assert_eq!(binding.sdl().expect("sdl"), "b13");
        assert_eq!(binding.retroarch().expect("retroarch"), "11");
    }

    #[test]
    fn a_button_retroarch_cannot_see_is_refused_rather_than_numbered() {
        let binding = Binding::button(13).with_ra_index(Some(RA_INVISIBLE));
        assert!(binding.sdl_visible());
        assert!(!binding.retroarch_visible());
        assert_eq!(
            binding.retroarch(),
            Err(Unspellable::InvisibleToRetroarch),
            "a negative index must not become a plausible button number"
        );
    }

    #[test]
    fn any_negative_ra_index_is_refused_not_only_the_sentinel() {
        for index in [-1, -2, -99, i32::MIN] {
            let binding = Binding::button(4).with_ra_index(Some(index));
            assert!(!binding.retroarch_visible(), "index {index} was accepted");
        }
    }

    #[test]
    fn the_four_hat_directions_spell_correctly() {
        for (value, word) in [(1, "up"), (2, "right"), (4, "down"), (8, "left")] {
            let binding = Binding::hat(0, value);
            assert!(binding.sdl_visible());
            assert_eq!(binding.sdl().expect("sdl"), format!("h0.{value}"));
            assert_eq!(binding.retroarch().expect("ra"), format!("h0{word}"));
        }
    }

    #[test]
    fn a_diagonal_or_resting_hat_is_not_a_control() {
        for value in [0, 3, 5, 6, 9, 12, 15, -1] {
            let binding = Binding::hat(0, value);
            assert!(!binding.sdl_visible(), "hat {value} passed sdl_visible");
            assert!(
                !binding.retroarch_visible(),
                "hat {value} passed ra_visible"
            );
            assert_eq!(binding.sdl(), Err(Unspellable::HatValue(value)));
            assert_eq!(binding.retroarch(), Err(Unspellable::HatValue(value)));
        }
    }

    #[test]
    fn an_axis_carries_its_sign_and_zero_counts_as_positive() {
        assert_eq!(Binding::axis(2, 1).sdl().expect("sdl"), "+a2");
        assert_eq!(Binding::axis(2, -1).sdl().expect("sdl"), "-a2");
        assert_eq!(Binding::axis(2, 0).sdl().expect("sdl"), "+a2");
        assert_eq!(Binding::axis(2, 1).retroarch().expect("ra"), "+2");
        assert_eq!(Binding::axis(2, -1).retroarch().expect("ra"), "-2");
        assert_eq!(Binding::axis(2, 0).retroarch().expect("ra"), "+2");
        assert_eq!(Binding::axis(0, -1).retroarch().expect("ra"), "-0");
        assert_eq!(Binding::axis(0, 1).retroarch().expect("ra"), "+0");
    }

    #[test]
    fn an_axis_ra_index_wins_over_the_sdl_one() {
        let binding = Binding::axis(5, -1).with_ra_index(Some(2));
        assert_eq!(binding.sdl().expect("sdl"), "-a5");
        assert_eq!(binding.retroarch().expect("ra"), "-2");
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
    fn retroarch_counts_from_btn_misc_and_skips_nothing_above_it() {
        let keys = [0x100_u16, 0x120, 0x121];
        assert_eq!(retroarch_button_index(&keys, 0x100), Some(0));
        assert_eq!(retroarch_button_index(&keys, 0x120), Some(1));
        assert_eq!(retroarch_button_index(&keys, 0x121), Some(2));
        assert_eq!(sdl_button_index(&keys, 0x100), Some(2));
        assert_eq!(sdl_button_index(&keys, 0x120), Some(0));
    }

    #[test]
    fn retroarch_reports_a_sub_btn_misc_code_as_invisible_not_absent() {
        let keys = [0x1e_u16, 0x120];
        assert_eq!(retroarch_button_index(&keys, 0x1e), Some(RA_INVISIBLE));
        assert_eq!(retroarch_button_index(&keys, 0x99), None);
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
            assert_eq!(
                retroarch_button_index(&ascending, code),
                retroarch_button_index(&shuffled, code)
            );
        }
    }

    #[test]
    fn a_binding_round_trips_through_json_with_the_python_field_names() {
        let binding = Binding::hat(0, 4).with_ra_index(Some(RA_INVISIBLE));
        let json = serde_json::to_value(binding).expect("serialize");
        assert_eq!(json["kind"], "hat");
        assert_eq!(json["index"], 0);
        assert_eq!(json["value"], 4);
        assert_eq!(json["ra_index"], -1);
        let back: Binding = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, binding);
    }

    #[test]
    fn an_absent_ra_index_reads_back_as_absent_not_as_zero() {
        let raw = serde_json::json!({"kind": "button", "index": 7});
        let binding: Binding = serde_json::from_value(raw).expect("deserialize");
        assert_eq!(binding.ra_index, None);
        assert_eq!(binding.value, 0);
        assert_eq!(binding.retroarch().expect("ra"), "7");
    }
}
