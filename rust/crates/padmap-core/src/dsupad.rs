//! evdev events in, a [`dsu::Pad`] out.
//!
//! padmap already forwards every event from a physical pad to its clone. This
//! watches the same stream go past and keeps a DSU-shaped picture of it, so a
//! consumer that reads padmap over UDP sees the buttons as well as the gyro.
//!
//! The names on the wire are a DualShock's, because the format is one. The
//! translation is by *position*, the same rule padmap uses everywhere else:
//! `BTN_SOUTH` is the bottom face button, and on a DualShock that is Cross.

use std::collections::BTreeMap;

use crate::dsu::{analog, button, Pad, CENTRE};
use crate::motion::Motion;

/// `EV_KEY`.
pub const EV_KEY: u16 = 0x01;
/// `EV_ABS`.
pub const EV_ABS: u16 = 0x03;

/// Kernel button code -> the bit it sets in `digital_button`.
///
/// `BTN_MODE` is deliberately absent: the guide button has a byte of its own
/// in the packet rather than a bit in the field.
pub const BUTTON_BITS: [(u16, u16); 12] = [
    (0x130, button::CROSS),    // BTN_SOUTH
    (0x131, button::CIRCLE),   // BTN_EAST
    (0x133, button::TRIANGLE), // BTN_NORTH
    (0x134, button::SQUARE),   // BTN_WEST
    (0x136, button::L1),       // BTN_TL
    (0x137, button::R1),       // BTN_TR
    (0x138, button::L2),       // BTN_TL2
    (0x139, button::R2),       // BTN_TR2
    (0x13A, button::SHARE),    // BTN_SELECT
    (0x13B, button::OPTIONS),  // BTN_START
    (0x13D, button::L3),       // BTN_THUMBL
    (0x13E, button::R3),       // BTN_THUMBR
];

/// `BTN_MODE`, which the packet carries as a byte of its own.
pub const BTN_MODE: u16 = 0x13C;

/// Kernel button code -> the byte it fills in `AnalogButton`.
///
/// A digital button reports 0 or 255 here. The field exists for pads with
/// pressure-sensitive faces, which none of padmap's are.
const BUTTON_ANALOG: [(u16, usize); 8] = [
    (0x130, analog::CROSS),
    (0x131, analog::CIRCLE),
    (0x133, analog::TRIANGLE),
    (0x134, analog::SQUARE),
    (0x136, analog::L1),
    (0x137, analog::R1),
    (0x138, analog::L2),
    (0x139, analog::R2),
];

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

/// An axis's declared range, for scaling into DSU's `u8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub min: i32,
    pub max: i32,
}

impl Range {
    /// `value` mapped onto 0..=255, clamped.
    ///
    /// A degenerate range -- a driver that declares min == max -- yields the
    /// centre rather than a division by zero, because the alternative reaches
    /// an emulator as a stick jammed in a corner.
    pub fn to_u8(&self, value: i32) -> u8 {
        if self.max <= self.min {
            return CENTRE;
        }
        let span = (self.max - self.min) as i64;
        let offset = (value.clamp(self.min, self.max) - self.min) as i64;
        ((offset * 255 + span / 2) / span) as u8
    }

    /// `value` mapped onto 0..=255 with the minimum at the top.
    ///
    /// evdev's vertical axes grow downwards and DSU's grow upwards. Missing
    /// this inverts every stick, which reads as a controller that works and is
    /// fighting you.
    pub fn to_u8_inverted(&self, value: i32) -> u8 {
        255 - self.to_u8(value)
    }

    /// `value` mapped onto 0..=255 from the minimum, for a trigger, which
    /// rests at its minimum rather than its middle.
    pub fn to_trigger(&self, value: i32) -> u8 {
        self.to_u8(value)
    }
}

/// A DSU picture of one controller, updated event by event.
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    pad: Pad,
    /// Declared ranges, by ABS code. An axis with no entry is ignored rather
    /// than guessed: a pad that does not have it will never send it, and one
    /// that does always declares it.
    ranges: BTreeMap<u16, Range>,
}

impl Tracker {
    pub fn new(ranges: BTreeMap<u16, Range>) -> Tracker {
        Tracker {
            pad: Pad::default(),
            ranges,
        }
    }

    /// The picture as it stands.
    pub fn pad(&self) -> &Pad {
        &self.pad
    }

    /// Replace the motion sample without disturbing the buttons.
    ///
    /// Motion arrives on a different descriptor from the buttons -- a separate
    /// IMU node, or a hidraw report -- so the two are folded together here
    /// rather than at the source.
    pub fn set_motion(&mut self, motion: Motion) {
        self.pad.motion = motion;
    }

    /// Fold one event in. Anything unrecognised is ignored.
    pub fn apply(&mut self, event_type: u16, code: u16, value: i32) {
        match event_type {
            EV_KEY => self.apply_key(code, value),
            EV_ABS => self.apply_abs(code, value),
            _ => {}
        }
    }

    fn apply_key(&mut self, code: u16, value: i32) {
        // Key autorepeat sends 2, which is still held.
        let down = value != 0;
        if code == BTN_MODE {
            self.pad.home = u8::from(down);
            return;
        }
        for (button, bit) in BUTTON_BITS {
            if button == code {
                if down {
                    self.pad.buttons |= bit;
                } else {
                    self.pad.buttons &= !bit;
                }
            }
        }
        for (button, index) in BUTTON_ANALOG {
            if button == code {
                self.pad.analog[index] = if down { 255 } else { 0 };
            }
        }
    }

    fn apply_abs(&mut self, code: u16, value: i32) {
        match code {
            ABS_HAT0X => {
                self.set_dpad(button::LEFT, analog::DPAD_LEFT, value < 0);
                self.set_dpad(button::RIGHT, analog::DPAD_RIGHT, value > 0);
                return;
            }
            ABS_HAT0Y => {
                // A hat's Y grows downwards too.
                self.set_dpad(button::UP, analog::DPAD_UP, value < 0);
                self.set_dpad(button::DOWN, analog::DPAD_DOWN, value > 0);
                return;
            }
            _ => {}
        }
        let Some(range) = self.ranges.get(&code).copied() else {
            return;
        };
        match code {
            ABS_X => self.pad.left_x = range.to_u8(value),
            ABS_Y => self.pad.left_y = range.to_u8_inverted(value),
            ABS_RX => self.pad.right_x = range.to_u8(value),
            ABS_RY => self.pad.right_y = range.to_u8_inverted(value),
            ABS_Z => {
                let pressed = range.to_trigger(value);
                self.pad.analog[analog::L2] = pressed;
                // A pad with an analog trigger usually has no BTN_TL2 to go
                // with it, so the bit is derived. Half way is the threshold
                // every emulator uses for a digital read of an analog trigger.
                self.set_bit(button::L2, pressed >= CENTRE);
            }
            ABS_RZ => {
                let pressed = range.to_trigger(value);
                self.pad.analog[analog::R2] = pressed;
                self.set_bit(button::R2, pressed >= CENTRE);
            }
            _ => {}
        }
    }

    fn set_bit(&mut self, bit: u16, on: bool) {
        if on {
            self.pad.buttons |= bit;
        } else {
            self.pad.buttons &= !bit;
        }
    }

    fn set_dpad(&mut self, bit: u16, index: usize, on: bool) {
        self.set_bit(bit, on);
        self.pad.analog[index] = if on { 255 } else { 0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stick_ranges() -> BTreeMap<u16, Range> {
        let full = Range {
            min: -32768,
            max: 32767,
        };
        let trigger = Range { min: 0, max: 255 };
        BTreeMap::from([
            (ABS_X, full),
            (ABS_Y, full),
            (ABS_RX, full),
            (ABS_RY, full),
            (ABS_Z, trigger),
            (ABS_RZ, trigger),
        ])
    }

    #[test]
    fn a_fresh_tracker_is_a_pad_nobody_is_touching() {
        let tracker = Tracker::new(stick_ranges());
        assert_eq!(tracker.pad().buttons, 0);
        assert_eq!(tracker.pad().left_x, CENTRE);
        assert_eq!(tracker.pad().left_y, CENTRE);
    }

    #[test]
    fn the_bottom_face_button_is_cross() {
        // By position, not by the letter printed on it -- the rule padmap uses
        // everywhere. BTN_SOUTH on a Nintendo pad says B, and it is still the
        // bottom one, and on a DualShock that is Cross.
        let mut tracker = Tracker::new(stick_ranges());
        tracker.apply(EV_KEY, 0x130, 1);
        assert_eq!(tracker.pad().buttons, button::CROSS);
        assert_eq!(tracker.pad().analog[analog::CROSS], 255);
        tracker.apply(EV_KEY, 0x130, 0);
        assert_eq!(tracker.pad().buttons, 0);
        assert_eq!(tracker.pad().analog[analog::CROSS], 0);
    }

    #[test]
    fn north_is_triangle_and_west_is_square() {
        // BTN_NORTH is 0x133 and BTN_WEST is 0x134, which is the pair whose
        // numbering does not follow their names. Swapping them binds the top
        // face button to the left one in every DSU consumer at once.
        let mut tracker = Tracker::new(stick_ranges());
        tracker.apply(EV_KEY, 0x133, 1);
        tracker.apply(EV_KEY, 0x134, 1);
        assert_eq!(tracker.pad().buttons, button::TRIANGLE | button::SQUARE);
    }

    #[test]
    fn the_guide_button_is_a_byte_and_not_a_bit() {
        let mut tracker = Tracker::new(stick_ranges());
        tracker.apply(EV_KEY, BTN_MODE, 1);
        assert_eq!(tracker.pad().home, 1);
        assert_eq!(tracker.pad().buttons, 0);
    }

    #[test]
    fn autorepeat_is_still_held() {
        let mut tracker = Tracker::new(stick_ranges());
        tracker.apply(EV_KEY, 0x130, 2);
        assert_eq!(tracker.pad().buttons, button::CROSS);
    }

    #[test]
    fn a_stick_pushed_up_reads_high() {
        // evdev's Y grows downwards and DSU's grows upwards. This is the test
        // that catches the inversion, which otherwise reads as a controller
        // that works and fights you.
        let mut tracker = Tracker::new(stick_ranges());
        tracker.apply(EV_ABS, ABS_Y, -32768);
        assert_eq!(tracker.pad().left_y, 255);
        tracker.apply(EV_ABS, ABS_Y, 32767);
        assert_eq!(tracker.pad().left_y, 0);
        tracker.apply(EV_ABS, ABS_X, 32767);
        assert_eq!(tracker.pad().left_x, 255);
    }

    #[test]
    fn a_centred_stick_lands_on_the_centre() {
        let mut tracker = Tracker::new(stick_ranges());
        tracker.apply(EV_ABS, ABS_X, 0);
        tracker.apply(EV_ABS, ABS_Y, 0);
        // -32768..32767 has no exact midpoint in 0..255; either neighbour of
        // the centre is correct and a corner is not.
        assert!((127..=128).contains(&tracker.pad().left_x));
        assert!((127..=128).contains(&tracker.pad().left_y));
    }

    #[test]
    fn a_trigger_sets_its_byte_and_its_bit_past_half() {
        let mut tracker = Tracker::new(stick_ranges());
        tracker.apply(EV_ABS, ABS_Z, 0);
        assert_eq!(tracker.pad().analog[analog::L2], 0);
        assert_eq!(tracker.pad().buttons & button::L2, 0);
        tracker.apply(EV_ABS, ABS_Z, 255);
        assert_eq!(tracker.pad().analog[analog::L2], 255);
        assert_eq!(tracker.pad().buttons & button::L2, button::L2);
    }

    #[test]
    fn a_hat_sets_opposite_bits_and_clears_them() {
        let mut tracker = Tracker::new(stick_ranges());
        tracker.apply(EV_ABS, ABS_HAT0Y, -1);
        assert_eq!(tracker.pad().buttons, button::UP);
        assert_eq!(tracker.pad().analog[analog::DPAD_UP], 255);
        tracker.apply(EV_ABS, ABS_HAT0Y, 1);
        assert_eq!(tracker.pad().buttons, button::DOWN);
        assert_eq!(tracker.pad().analog[analog::DPAD_UP], 0);
        tracker.apply(EV_ABS, ABS_HAT0Y, 0);
        assert_eq!(tracker.pad().buttons, 0);
    }

    #[test]
    fn an_axis_the_pad_never_declared_is_ignored() {
        // Rather than scaled against a guessed range. An arcade stick declares
        // no ABS_Z and never sends one; a pad that does always declares it.
        let mut tracker = Tracker::new(BTreeMap::new());
        tracker.apply(EV_ABS, ABS_X, 32767);
        assert_eq!(tracker.pad().left_x, CENTRE);
    }

    #[test]
    fn a_driver_declaring_a_degenerate_range_gives_the_centre() {
        let ranges = BTreeMap::from([(ABS_X, Range { min: 5, max: 5 })]);
        let mut tracker = Tracker::new(ranges);
        tracker.apply(EV_ABS, ABS_X, 5);
        assert_eq!(tracker.pad().left_x, CENTRE);
    }

    #[test]
    fn a_value_outside_the_declared_range_is_clamped() {
        // A worn stick over-travels. Wrapping would read as the opposite
        // direction at full deflection.
        let mut tracker = Tracker::new(stick_ranges());
        tracker.apply(EV_ABS, ABS_X, 99_999);
        assert_eq!(tracker.pad().left_x, 255);
        tracker.apply(EV_ABS, ABS_X, -99_999);
        assert_eq!(tracker.pad().left_x, 0);
    }

    #[test]
    fn motion_folds_in_without_disturbing_the_buttons() {
        let mut tracker = Tracker::new(stick_ranges());
        tracker.apply(EV_KEY, 0x130, 1);
        tracker.set_motion(Motion {
            accel: [0.0, -1.0, 0.0],
            gyro: [0.0; 3],
            timestamp_us: 7,
        });
        assert_eq!(tracker.pad().buttons, button::CROSS);
        assert_eq!(tracker.pad().motion.timestamp_us, 7);
    }
}
