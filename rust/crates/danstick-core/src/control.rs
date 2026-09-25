//! Canonical control vocabulary: a single name danstick uses internally.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A control on the canonical pad (multiple physical buttons can share one name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Control {
    A,
    B,
    X,
    Y,
    Back,
    Start,
    LeftShoulder,
    RightShoulder,
    LeftTrigger,
    RightTrigger,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    /// N64 C-buttons are modeled as right stick halves (SDL has no C-button concept).
    RightStickUp,
    RightStickDown,
    RightStickLeft,
    RightStickRight,
    /// The stick a console's own manual calls the control stick, as four halves.
    LeftStickUp,
    LeftStickDown,
    LeftStickLeft,
    LeftStickRight,
}

/// Every control in mapping order: appended to, never reordered.
pub const CANONICAL_ORDER: [Control; 22] = [
    Control::A,
    Control::B,
    Control::X,
    Control::Y,
    Control::Back,
    Control::Start,
    Control::LeftShoulder,
    Control::RightShoulder,
    Control::LeftTrigger,
    Control::RightTrigger,
    Control::DpadUp,
    Control::DpadDown,
    Control::DpadLeft,
    Control::DpadRight,
    Control::RightStickUp,
    Control::RightStickDown,
    Control::RightStickLeft,
    Control::RightStickRight,
    Control::LeftStickUp,
    Control::LeftStickDown,
    Control::LeftStickLeft,
    Control::LeftStickRight,
];

impl Control {
    /// Every control, in mapping order.
    pub const ALL: [Control; 22] = CANONICAL_ORDER;

    /// danstick's name for this control (used in profiles, layouts, wire protocol).
    pub const fn as_str(self) -> &'static str {
        match self {
            Control::A => "a",
            Control::B => "b",
            Control::X => "x",
            Control::Y => "y",
            Control::Back => "back",
            Control::Start => "start",
            Control::LeftShoulder => "leftshoulder",
            Control::RightShoulder => "rightshoulder",
            Control::LeftTrigger => "lefttrigger",
            Control::RightTrigger => "righttrigger",
            Control::DpadUp => "dpup",
            Control::DpadDown => "dpdown",
            Control::DpadLeft => "dpleft",
            Control::DpadRight => "dpright",
            Control::RightStickUp => "rightstick_up",
            Control::RightStickDown => "rightstick_down",
            Control::RightStickLeft => "rightstick_left",
            Control::RightStickRight => "rightstick_right",
            Control::LeftStickUp => "leftstick_up",
            Control::LeftStickDown => "leftstick_down",
            Control::LeftStickLeft => "leftstick_left",
            Control::LeftStickRight => "leftstick_right",
        }
    }

    /// SDL's spelling in gamecontrollerdb (stick halves as +/- axis, not C-buttons).
    pub const fn sdl_field(self) -> &'static str {
        match self {
            Control::A => "a",
            Control::B => "b",
            Control::X => "x",
            Control::Y => "y",
            Control::Back => "back",
            Control::Start => "start",
            Control::LeftShoulder => "leftshoulder",
            Control::RightShoulder => "rightshoulder",
            Control::LeftTrigger => "lefttrigger",
            Control::RightTrigger => "righttrigger",
            Control::DpadUp => "dpup",
            Control::DpadDown => "dpdown",
            Control::DpadLeft => "dpleft",
            Control::DpadRight => "dpright",
            Control::RightStickUp => "-righty",
            Control::RightStickDown => "+righty",
            Control::RightStickLeft => "-rightx",
            Control::RightStickRight => "+rightx",
            Control::LeftStickUp => "-lefty",
            Control::LeftStickDown => "+lefty",
            Control::LeftStickLeft => "-leftx",
            Control::LeftStickRight => "+leftx",
        }
    }

    /// RetroArch autoconfig key: a/b are swapped vs SDL (Nintendo button positions).
    pub const fn retroarch_key(self) -> &'static str {
        match self {
            Control::A => "input_b_btn",
            Control::B => "input_a_btn",
            Control::X => "input_y_btn",
            Control::Y => "input_x_btn",
            Control::Back => "input_select_btn",
            Control::Start => "input_start_btn",
            Control::LeftShoulder => "input_l_btn",
            Control::RightShoulder => "input_r_btn",
            Control::LeftTrigger => "input_l2_btn",
            Control::RightTrigger => "input_r2_btn",
            Control::DpadUp => "input_up_btn",
            Control::DpadDown => "input_down_btn",
            Control::DpadLeft => "input_left_btn",
            Control::DpadRight => "input_right_btn",
            Control::RightStickUp => "input_r_y_minus_btn",
            Control::RightStickDown => "input_r_y_plus_btn",
            Control::RightStickLeft => "input_r_x_minus_btn",
            Control::RightStickRight => "input_r_x_plus_btn",
            Control::LeftStickUp => "input_l_y_minus_btn",
            Control::LeftStickDown => "input_l_y_plus_btn",
            Control::LeftStickLeft => "input_l_x_minus_btn",
            Control::LeftStickRight => "input_l_x_plus_btn",
        }
    }
}

/// A string that names no canonical control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownControl(pub String);

impl fmt::Display for UnknownControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a canonical control", self.0)
    }
}

impl std::error::Error for UnknownControl {}

impl FromStr for Control {
    type Err = UnknownControl;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        CANONICAL_ORDER
            .iter()
            .copied()
            .find(|control| control.as_str() == name)
            .ok_or_else(|| UnknownControl(name.to_owned()))
    }
}

impl fmt::Display for Control {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Control {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Control {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        name.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_round_trips_through_its_name() {
        for control in Control::ALL {
            assert_eq!(control.as_str().parse::<Control>(), Ok(control));
        }
    }

    #[test]
    fn names_are_unique() {
        let mut seen: Vec<&str> = Control::ALL.iter().map(|c| c.as_str()).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before, "two controls share a name");
    }

    #[test]
    fn retroarch_keys_are_unique() {
        let mut keys: Vec<&str> = Control::ALL.iter().map(|c| c.retroarch_key()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "two controls share a RetroArch key");
    }

    #[test]
    fn sdl_fields_are_unique() {
        let mut fields: Vec<&str> = Control::ALL.iter().map(|c| c.sdl_field()).collect();
        fields.sort_unstable();
        let before = fields.len();
        fields.dedup();
        assert_eq!(fields.len(), before, "two controls share an SDL field");
    }

    #[test]
    fn unknown_names_are_rejected_rather_than_guessed() {
        assert!("guide".parse::<Control>().is_err());
        assert!("".parse::<Control>().is_err());
        assert!(
            "A".parse::<Control>().is_err(),
            "matching is case-sensitive"
        );
    }

    #[test]
    fn retroarch_and_sdl_disagree_about_a_and_b_deliberately() {
        assert_eq!(Control::A.retroarch_key(), "input_b_btn");
        assert_eq!(Control::B.retroarch_key(), "input_a_btn");
        assert_eq!(Control::X.retroarch_key(), "input_y_btn");
        assert_eq!(Control::Y.retroarch_key(), "input_x_btn");
    }

    #[test]
    fn c_buttons_are_right_stick_halves_to_both_consumers() {
        assert_eq!(Control::RightStickUp.sdl_field(), "-righty");
        assert_eq!(Control::RightStickDown.sdl_field(), "+righty");
        assert_eq!(Control::RightStickUp.retroarch_key(), "input_r_y_minus_btn");
        assert_eq!(
            Control::RightStickDown.retroarch_key(),
            "input_r_y_plus_btn"
        );
    }

    #[test]
    fn serde_uses_the_on_disk_spelling() {
        let json = serde_json::to_string(&Control::RightStickUp).expect("serialize");
        assert_eq!(json, "\"rightstick_up\"");
        let back: Control = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, Control::RightStickUp);
    }

    #[test]
    fn canonical_order_holds_every_control_once() {
        assert_eq!(CANONICAL_ORDER.len(), Control::ALL.len());
        for control in Control::ALL {
            assert_eq!(CANONICAL_ORDER.iter().filter(|c| **c == control).count(), 1);
        }
    }
}
