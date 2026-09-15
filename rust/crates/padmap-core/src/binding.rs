//! Where one control lives on a pad, and how each consumer spells that.
//!
//! A d-pad is a hat on most pads and four ordinary buttons on some, and
//! triggers are an axis on anything with analogue ones, so a capture has to be
//! able to say which rather than assuming everything is a button.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Where each consumer starts counting buttons. They are not the same, and the
/// difference is invisible on most pads.
pub const BTN_MISC: u16 = 0x100;
pub const BTN_JOYSTICK: u16 = 0x120;

/// `ra_index` for a button RetroArch cannot see at all.
///
/// `retroarch_button_index` used to answer `None` for two different reasons --
/// the two consumers agree (the usual case), or the code is below `BTN_MISC`
/// and RetroArch's udev driver never enumerates it -- and the binding read
/// that `None` as "they agree". A combo adapter reporting KEY_A alongside its
/// twelve buttons therefore stored index 13 on a pad RetroArch numbers 0..11.
/// RetroArch binds a button that does not exist without complaining, and still
/// reports the pad as configured: the control works in the front-end and is
/// dead in every game.
pub const RA_INVISIBLE: i32 = -1;

/// Hats are absolute axes too, but neither consumer counts them as axes.
pub const HAT_CODES: std::ops::Range<u16> = 0x10..0x18;

/// SDL hat bit -> RetroArch's direction word.
///
/// Only the four single bits. A hat reads 3 ("up and right") on a diagonal and
/// 0 at rest, and neither is a control: RetroArch's config has one direction
/// word per key, and an SDL mask of two bits only matches while both are held.
/// Both are refused rather than letting one consumer render what the other
/// cannot.
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

/// A binding that one of the two consumers cannot be told about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unspellable {
    /// A hat mask naming two directions or none. Not a control anyone can press.
    HatValue(i32),
    /// An evdev code below `BTN_MISC`: RetroArch's udev driver never enumerates
    /// those, so there is no number that names it.
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

/// Where one control lives on a pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Binding {
    pub kind: BindingKind,
    pub index: i32,
    #[serde(default)]
    pub value: i32,
    /// RetroArch numbers buttons from a lower base than SDL, so the same
    /// physical button can be b2 to one and 0 to the other. Carrying both is
    /// the only way a stored binding stays right for both consumers; `None`
    /// means they agree, which is the case on most pads, and [`RA_INVISIBLE`]
    /// means RetroArch has no number for this button at all.
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
        // Any negative index, not just RA_INVISIBLE itself: a profile off disk
        // can hold whatever it likes, and no real button is negative.
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
    ///
    /// Both the shapes that fail came out of real files: a hat value that is
    /// not a single direction (a hand-edited or half-written profile) used to
    /// raise from the middle of writing the launch profiles, and a button
    /// below `BTN_MISC` used to come out as a plausible-looking number naming
    /// a different button, or none.
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

/// Which button number SDL will give an evdev key code.
///
/// SDL walks `BTN_JOYSTICK..KEY_MAX` first and only then `0..BTN_JOYSTICK`, so
/// the numbering is *not* simply ascending for a pad carrying any button below
/// 0x120 -- some arcade sticks report BTN_MISC-range codes.
///
/// `keys` need not be sorted; it is sorted here, as the Python did.
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

/// Which button number RetroArch's udev driver will give an evdev key code.
///
/// Plain ascending order from `BTN_MISC`, a lower starting point than SDL's.
/// On a pad whose buttons all sit at 0x120 or above -- most of them -- the two
/// agree exactly, which is why this difference can go unnoticed until it
/// silently shifts every binding on the one pad that does not.
///
/// Three answers, not two, because a caller storing this in a [`Binding`] has
/// to be able to tell them apart. See [`RA_INVISIBLE`].
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

/// Which axis number an evdev ABS code will be given.
///
/// Both SDL and RetroArch number axes by ascending code among the real axes,
/// skipping the hat codes, which they treat as hats instead. Storing the raw
/// evdev code and hoping it is the index works right up to the first pad whose
/// axes are not 0,1,2,... -- ABS_RZ is 5.
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
        // A profile off disk can hold whatever it likes.
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
        // 3 is up+right on a diagonal, 0 is at rest. Neither is pressable, and
        // both are refused by both consumers rather than by one.
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
        // Axis 0 is where RetroArch's strtoull misparse bites: "-0" and "+0"
        // are both button 0 under the _btn key, which is why retroarch::lines
        // moves an axis binding onto _axis. The sign still has to survive here.
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
        // Pinned against a mapping SDL wrote: on the measured pad key 0x121 is
        // b1 and 0x128 is b8.
        let keys: Vec<u16> = (0x120..=0x12b).collect();
        assert_eq!(sdl_button_index(&keys, 0x120), Some(0));
        assert_eq!(sdl_button_index(&keys, 0x121), Some(1));
        assert_eq!(sdl_button_index(&keys, 0x128), Some(8));
    }

    #[test]
    fn a_keyboard_code_sorts_after_every_joystick_button_for_sdl() {
        // KEY_A (0x1e) is below BTN_JOYSTICK, so SDL puts it last.
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
        // BTN_0 (0x100) is enumerated by RetroArch but sorts last for SDL.
        let keys = [0x100_u16, 0x120, 0x121];
        assert_eq!(retroarch_button_index(&keys, 0x100), Some(0));
        assert_eq!(retroarch_button_index(&keys, 0x120), Some(1));
        assert_eq!(retroarch_button_index(&keys, 0x121), Some(2));
        // ...and SDL disagrees about every one of them.
        assert_eq!(sdl_button_index(&keys, 0x100), Some(2));
        assert_eq!(sdl_button_index(&keys, 0x120), Some(0));
    }

    #[test]
    fn retroarch_reports_a_sub_btn_misc_code_as_invisible_not_absent() {
        // The distinction that RA_INVISIBLE exists for: the pad reports the
        // code, RetroArch cannot see it, and "absent" would be read as "the
        // two consumers agree".
        let keys = [0x1e_u16, 0x120];
        assert_eq!(retroarch_button_index(&keys, 0x1e), Some(RA_INVISIBLE));
        assert_eq!(retroarch_button_index(&keys, 0x99), None);
    }

    #[test]
    fn axes_are_numbered_among_real_axes_with_hats_skipped() {
        // ABS_X, ABS_Y, ABS_RX, ABS_RY, ABS_RZ, and a hat pair in between.
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
