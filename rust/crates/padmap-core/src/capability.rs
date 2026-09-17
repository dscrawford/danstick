//! Joypad detection from sysfs bitmaps (not ID_INPUT_JOYSTICK, which padmap hide clears).

/// Button range that, with axes, indicates a joypad (same as udev's input_id).
pub const BTN_JOYSTICK_RANGE: std::ops::Range<u16> = 0x120..0x140;

/// A sysfs capability bitmap: 64-bit words MSB-first (too large for a single integer).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mask {
    /// Most significant first, as read.
    words: Vec<u64>,
}

impl Mask {
    /// Parse a bitmap string, distinguishing empty from missing (important for "0" axes).
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

    /// Is bit `index` set? Words are MSB-first, so last word holds bits 0..64.
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

/// Is this a joypad (axes AND buttons in range)? None means cannot determine.
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
