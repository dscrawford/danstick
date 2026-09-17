//! ares controller bindings: raw SDL joystick (not gamepad layer).

use std::collections::BTreeMap;

/// ares allows five ports.
pub const MAX_PLAYERS: u32 = 5;

/// ares' HID group IDs.
const GROUP_AXIS: u8 = 0;
const GROUP_HAT: u8 = 1;
const GROUP_BUTTON: u8 = 3;

/// Which half of an axis: ares treats <-16384 as Lo, >+16384 as Hi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    Lo,
    Hi,
}

/// One control on ares' virtual pad, and what it should read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// An evdev `KEY_`/`BTN_` code.
    Button(u16),
    /// An evdev `ABS_` code, and which half if the control is digital.
    Axis(u16, Option<Half>),
    /// The d-pad, as hat 0's X or Y.
    Hat { vertical: bool, half: Half },
}

/// ares' control names: display names with `" "` and `"("` → `"."`, `")"` dropped.
pub const CONTROLS: [(&str, Source); 24] = [
    (
        "Pad.Up",
        Source::Hat {
            vertical: true,
            half: Half::Lo,
        },
    ),
    (
        "Pad.Down",
        Source::Hat {
            vertical: true,
            half: Half::Hi,
        },
    ),
    (
        "Pad.Left",
        Source::Hat {
            vertical: false,
            half: Half::Lo,
        },
    ),
    (
        "Pad.Right",
        Source::Hat {
            vertical: false,
            half: Half::Hi,
        },
    ),
    ("Select", Source::Button(0x13A)), // BTN_SELECT
    ("Start", Source::Button(0x13B)),  // BTN_START
    ("A..South", Source::Button(0x130)),
    ("B..East", Source::Button(0x131)),
    ("X..West", Source::Button(0x134)),
    ("Y..North", Source::Button(0x133)),
    ("L-Bumper", Source::Button(0x136)),               // BTN_TL
    ("R-Bumper", Source::Button(0x137)),               // BTN_TR
    ("L-Trigger", Source::Axis(0x02, Some(Half::Hi))), // ABS_Z
    ("R-Trigger", Source::Axis(0x05, Some(Half::Hi))), // ABS_RZ
    ("L-Stick..Click", Source::Button(0x13D)),         // BTN_THUMBL
    ("R-Stick..Click", Source::Button(0x13E)),         // BTN_THUMBR
    ("L-Up", Source::Axis(0x01, Some(Half::Lo))),      // ABS_Y
    ("L-Down", Source::Axis(0x01, Some(Half::Hi))),
    ("L-Left", Source::Axis(0x00, Some(Half::Lo))), // ABS_X
    ("L-Right", Source::Axis(0x00, Some(Half::Hi))),
    ("R-Up", Source::Axis(0x04, Some(Half::Lo))), // ABS_RY
    ("R-Down", Source::Axis(0x04, Some(Half::Hi))),
    ("R-Left", Source::Axis(0x03, Some(Half::Lo))), // ABS_RX
    ("R-Right", Source::Axis(0x03, Some(Half::Hi))),
];

/// Hat axes, which SDL reports as hats rather than counting among the axes.
const HAT_X: u16 = 0x10;
const HAT_Y: u16 = 0x11;

/// Where each evdev code sits in SDL's raw joystick numbering (ordinal in ascending order).
#[derive(Debug, Clone, Default)]
pub struct Indices {
    buttons: BTreeMap<u16, u8>,
    axes: BTreeMap<u16, u8>,
    has_hat: bool,
}

impl Indices {
    pub fn of(keys: &[u16], abs: &[u16]) -> Indices {
        let mut sorted_keys: Vec<u16> = keys.to_vec();
        sorted_keys.sort_unstable();
        sorted_keys.dedup();

        let mut sorted_axes: Vec<u16> = abs
            .iter()
            .copied()
            .filter(|code| *code != HAT_X && *code != HAT_Y)
            .collect();
        sorted_axes.sort_unstable();
        sorted_axes.dedup();

        Indices {
            buttons: sorted_keys
                .into_iter()
                .enumerate()
                .map(|(at, code)| (code, at as u8))
                .collect(),
            axes: sorted_axes
                .into_iter()
                .enumerate()
                .map(|(at, code)| (code, at as u8))
                .collect(),
            has_hat: abs.contains(&HAT_X) || abs.contains(&HAT_Y),
        }
    }
}

/// One assignment string, or `None` if the pad lacks this control (better than binding wrong).
pub fn assignment(guid: &str, source: Source, indices: &Indices) -> Option<String> {
    let (group, input, half) = match source {
        Source::Button(code) => (GROUP_BUTTON, *indices.buttons.get(&code)?, None),
        Source::Axis(code, half) => (GROUP_AXIS, *indices.axes.get(&code)?, half),
        Source::Hat { vertical, half } => {
            if !indices.has_hat {
                return None;
            }
            (GROUP_HAT, u8::from(vertical), Some(half))
        }
    };
    let mut out = format!("{guid}/0/{group}/{input}");
    if let Some(half) = half {
        out.push_str(match half {
            Half::Lo => "/Lo",
            Half::Hi => "/Hi",
        });
    }
    Some(out)
}

/// The `VirtualPadN` block for one player (every control, bound or not).
pub fn virtual_pad(player: u32, guid: &str, indices: &Indices) -> String {
    let mut out = format!("VirtualPad{player}\n");
    for (name, source) in CONTROLS {
        let value = assignment(guid, source, indices).unwrap_or_default();
        out.push_str(&format!("  {name}: {value};;\n"));
    }
    out.push_str("  Rumble: ;;\n");
    out
}
