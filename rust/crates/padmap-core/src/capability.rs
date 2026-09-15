//! Deciding what a joypad is, from the bitmaps udev itself reads.
//!
//! padmap must not use `ID_INPUT_JOYSTICK` for its own discovery, because
//! `padmap hide` deliberately clears it. Sharing the filter would mean
//! installing the hide rules made every controller invisible to padmap too, so
//! `padmap setup` could never be run again -- unrecoverable without removing
//! the rules by hand.
//!
//! So it asks the device what it can do. From **sysfs**, not by opening it:
//! opening every input node to ask two questions means closing every input
//! node afterwards, and releasing a USB HID descriptor takes about 11ms while
//! the driver tears down its URB. Measured at 390ms of a 400ms scan, in 36
//! calls to close. These are the same bitmaps udev's own `input_id` builtin
//! reads to decide `ID_INPUT_JOYSTICK`, and reading two small files costs
//! microseconds.

/// BTN_JOYSTICK (0x120) through BTN_THUMBR (0x13f): the range udev's
/// `input_id` builtin uses, together with absolute axes, to decide
/// ID_INPUT_JOYSTICK.
pub const BTN_JOYSTICK_RANGE: std::ops::Range<u16> = 0x120..0x140;

/// A sysfs capability bitmap.
///
/// Stored as the 64-bit words the kernel wrote, most significant first, rather
/// than as a number: a key bitmap is 768 bits and there is no integer that
/// wide. Testing a bit is an index, which is all anything here needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mask {
    /// Most significant first, as read.
    words: Vec<u64>,
}

impl Mask {
    /// Parse one, or `None` if it is not a bitmap.
    ///
    /// `None` and an all-zero mask are different answers and the distinction
    /// is load-bearing: a device with no absolute axes has an `abs` file
    /// containing "0", and reading that as "could not be read" sends it down
    /// the fallback that opens the device -- the expensive path this exists to
    /// avoid. Twelve of the 33 input devices on the machine this was written
    /// on are exactly that shape.
    pub fn parse(raw: &str) -> Option<Mask> {
        if raw.trim().is_empty() {
            return None;
        }
        let mut words = Vec::new();
        for word in raw.split_whitespace() {
            words.push(u64::from_str_radix(word, 16).ok()?);
        }
        Some(Mask { words })
    }

    /// Is every bit clear?
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    /// Is bit `index` set?
    ///
    /// The words are most significant first, so the *last* one holds bits
    /// 0..64. Reading them in the other order is a bitmap that looks
    /// plausible and describes a different device.
    pub fn bit(&self, index: usize) -> bool {
        let Some(from_end) = index.checked_div(64) else {
            return false;
        };
        if from_end >= self.words.len() {
            return false;
        }
        let word = self.words[self.words.len() - 1 - from_end];
        (word >> (index % 64)) & 1 == 1
    }

    /// Every set bit below `limit`, for tests and diagnostics.
    pub fn bits(&self, limit: usize) -> Vec<usize> {
        (0..limit).filter(|index| self.bit(*index)).collect()
    }
}

/// Is this a joypad, given its two bitmaps? `None` means "cannot tell".
///
/// A joypad is a device with at least one absolute axis and at least one
/// button in the joypad range. Both halves matter: a touchpad has axes and no
/// such buttons, and a keyboard has buttons and no axes.
pub fn joypad(absolute: Option<&Mask>, keys: Option<&Mask>) -> Option<bool> {
    let (absolute, keys) = (absolute?, keys?);
    if absolute.is_empty() {
        return Some(false);
    }
    Some(
        BTN_JOYSTICK_RANGE
            .map(usize::from)
            .any(|code| keys.bit(code)),
    )
}
