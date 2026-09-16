//! ares controller bindings.
//!
//! ares is the odd one of the three. It reads **raw SDL joystick** state --
//! `SDL_OpenJoystick`, `SDL_GetNumJoystickAxes|Hats|Buttons` -- and never
//! touches SDL's gamepad layer, so padmap's mapping line does nothing for it
//! and the numbers in a binding are positions on the device rather than
//! standard gamepad ids.
//!
//! Those positions are computable, and measured rather than assumed: a uinput
//! device declaring eleven keys and six non-hat axes was reported by SDL 3 as
//! `axes=6 hats=1 buttons=11`. So an input's index is its **ordinal in
//! ascending evdev code order** among its own kind, with `ABS_HAT0X`/`Y`
//! pulled out as a hat rather than counted as axes.
//!
//! Bindings live in `~/.local/share/ares/settings.bml` under
//! `VirtualPad1`..`VirtualPad5`, which are ports rather than systems: every
//! emulated console wires its player 1 to `VirtualPad1`, so one set of
//! bindings covers all of them.
//!
//! ```text
//! VirtualPad1
//!   A..South: 03002854de2800000413000002006800/0/3/0;;
//!   L-Left:   03002854de2800000413000002006800/0/0/0/Lo;;
//! ```
//!
//! The value is three alternative assignments joined by `;`, so an unbound
//! control is `;;` and one binding is `x;;`. An assignment is
//! `<guid>/<slot>/<group>/<input>[/Lo|Hi]`, where `slot` distinguishes devices
//! sharing a GUID -- always `0` for padmap, whose GUIDs embed a CRC of each
//! pad's own name.

use std::collections::BTreeMap;

/// ares allows five ports.
pub const MAX_PLAYERS: u32 = 5;

/// `HID::Joypad::GroupID`, from ares' `nall/hid.hpp`.
///
/// The SDL backend fills Axis, Hat and Button; Trigger is left empty, which is
/// why a trigger binds as an *axis* here.
const GROUP_AXIS: u8 = 0;
const GROUP_HAT: u8 = 1;
const GROUP_BUTTON: u8 = 3;

/// Which half of an axis a digital control is bound to.
///
/// ares treats a reading below -16384 as `Lo` and above +16384 as `Hi`; a
/// binding with no qualifier is the analogue value itself.
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

/// ares' control names, exactly as it writes them into settings.bml.
///
/// The key is the input's display name with `" "` and `"("` turned into `"."`
/// and `")"` dropped, which is why `A (South)` is written `A..South`. Spelling
/// one differently does not fail -- ares simply keeps its own unbound entry
/// beside padmap's, and the control stays dead.
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
    ("L-Bumper", Source::Button(0x136)), // BTN_TL
    ("R-Bumper", Source::Button(0x137)), // BTN_TR
    // Analogue triggers are axes, because the SDL backend leaves ares' own
    // Trigger group empty.
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

/// Where each evdev code sits in SDL's raw joystick numbering.
///
/// Ordinal in ascending code order among its own kind. Measured against SDL 3
/// with a uinput device of known capabilities; see the module note.
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

/// One assignment string, or `None` if the pad has no such control.
///
/// A control the device does not have is left unbound rather than pointed at
/// index zero. Pointing it somewhere would bind a real button to a control the
/// user never pressed, which is worse than a dead entry they can see.
pub fn assignment(guid: &str, source: Source, indices: &Indices) -> Option<String> {
    let (group, input, half) = match source {
        Source::Button(code) => (GROUP_BUTTON, *indices.buttons.get(&code)?, None),
        Source::Axis(code, half) => (GROUP_AXIS, *indices.axes.get(&code)?, half),
        Source::Hat { vertical, half } => {
            if !indices.has_hat {
                return None;
            }
            // Hats are two inputs each, X then Y.
            (GROUP_HAT, u8::from(vertical), Some(half))
        }
    };
    // slot is always 0: each padmap pad's GUID embeds a CRC of its own name,
    // so no two share one and none is ever the second holder.
    let mut out = format!("{guid}/0/{group}/{input}");
    if let Some(half) = half {
        out.push_str(match half {
            Half::Lo => "/Lo",
            Half::Hi => "/Hi",
        });
    }
    Some(out)
}

/// The `VirtualPadN` block for one player.
///
/// Every control is written, bound or not: ares keeps its own entry for
/// anything absent, and a half-written block leaves the file with padmap's
/// lines and ares' interleaved, which is harder to read than an explicit `;;`.
pub fn virtual_pad(player: u32, guid: &str, indices: &Indices) -> String {
    let mut out = format!("VirtualPad{player}\n");
    for (name, source) in CONTROLS {
        let value = assignment(guid, source, indices).unwrap_or_default();
        // Three alternative assignments joined by ';', with the trailing one
        // trimmed -- so an unbound control is ";;" and one binding is "x;;".
        out.push_str(&format!("  {name}: {value};;\n"));
    }
    out.push_str("  Rumble: ;;\n");
    out
}
