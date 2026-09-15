//! The Switch Pro Controller, over hidraw.
//!
//! A port of the decode in `src/padmap/hidraw.py`, established against the
//! hardware with `tools/switchprobe.py` -- pressing A set byte 3 to 0x08,
//! which is what the table below says.
//!
//! The pad powers up sending report **0x3f**, a cut-down report with no
//! analogue data at all, and has to be asked for **0x30**. Every 0x3f is
//! discarded by the report-id filter, so a pad stuck in simple mode delivers
//! no input while looking perfectly healthy from every other angle: the node
//! exists, the descriptor is live, reports are flowing, nothing raises and
//! nothing is logged. The only visible symptom is that no button works.
//!
//! The mode request is an **output report**, written with `write`. That is the
//! opposite of the Steam Controller next door, whose request is a *feature*
//! report and needs an ioctl -- sending either one the other way succeeds and
//! does nothing.

use std::collections::BTreeMap;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};

use evdev::{AbsInfo, AbsoluteAxisCode, EventType, InputEvent, KeyCode};
use log::warn;

/// Drivers whose pads speak this protocol.
///
/// A driver name names a protocol; it is not a statement about one product.
/// The previous version of this was `{(0x057E, 0x2009)}`, an allowlist of
/// exactly one controller, and every other pad that needs hidraw -- a
/// Joy-Con, an Online pad, a second Nintendo model bought later -- silently
/// took the evdev path instead, where the node opens, grabs and watches
/// without ever emitting an event.
pub const HID_DRIVERS: [&str; 1] = ["nintendo"];

/// INPUT: the standard full report, with sticks and buttons.
pub const REPORT_FULL: u8 = 0x30;
/// INPUT: the cut-down report the pad powers up in.
pub const REPORT_SIMPLE: u8 = 0x3F;
/// OUTPUT subcommand: set the input report mode.
const SUBCMD_REPORT_MODE: u8 = 0x03;

/// How many 0x3f reports to tolerate before asking again.
///
/// The request at open is not reliable: a pad that has just finished
/// associating over Bluetooth can drop it -- the write succeeds, the
/// controller never acts on it, and the pad sends 0x3f forever. At the pad's
/// ~67 reports a second this waits about a second and a half between
/// attempts, long enough not to spam a controller mid-handshake and short
/// enough that nobody gets as far as unpairing it.
pub const SIMPLE_REPORTS_BEFORE_RETRY: u32 = 100;

/// Every subcommand carries a rumble frame whether or not it rumbles; some
/// firmware ignores a request whose rumble bytes are all zero.
const RUMBLE_NEUTRAL: [u8; 8] = [0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40];

/// Byte 3 of a full report. Nintendo's labels are mirrored against everyone
/// else's, and `hid-nintendo` publishes by position -- so the button *labelled*
/// A is BTN_EAST, which is what every stored mapping keys on.
const BUTTONS_RIGHT: [(u8, KeyCode); 6] = [
    (0x01, KeyCode::BTN_WEST),  // Y, the left face button
    (0x02, KeyCode::BTN_NORTH), // X, the top
    (0x04, KeyCode::BTN_SOUTH), // B, the bottom
    (0x08, KeyCode::BTN_EAST),  // A, the right
    (0x40, KeyCode::BTN_TR),    // R
    (0x80, KeyCode::BTN_TR2),   // ZR
];
/// Byte 4.
const BUTTONS_SHARED: [(u8, KeyCode); 6] = [
    (0x01, KeyCode::BTN_SELECT), // Minus
    (0x02, KeyCode::BTN_START),  // Plus
    (0x04, KeyCode::BTN_THUMBR),
    (0x08, KeyCode::BTN_THUMBL),
    (0x10, KeyCode::BTN_MODE), // Home
    (0x20, KeyCode::BTN_Z),    // Capture
];
/// Byte 5. Its low nibble is the d-pad, published as a hat.
const BUTTONS_LEFT: [(u8, KeyCode); 2] = [
    (0x40, KeyCode::BTN_TL),  // L
    (0x80, KeyCode::BTN_TL2), // ZL
];

const DPAD_DOWN: u8 = 0x01;
const DPAD_UP: u8 = 0x02;
const DPAD_RIGHT: u8 = 0x04;
const DPAD_LEFT: u8 = 0x08;

/// Sticks are 12-bit. Published raw so a stored calibration stays meaningful.
pub const STICK_MIN: i32 = 0;
pub const STICK_MAX: i32 = 4095;
const STICK_FUZZ: i32 = 16;
const STICK_FLAT: i32 = 128;

/// The gamepad fields of one full report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct State {
    pub right: u8,
    pub shared: u8,
    pub left: u8,
    pub left_x: i32,
    pub left_y: i32,
    pub right_x: i32,
    pub right_y: i32,
}

/// Decode one 0x30 report, or `None` if it is too short.
///
/// Y is inverted here rather than downstream: the controller reports it
/// increasing *upwards*, evdev is the other way round like screen
/// coordinates, and `hid-nintendo` does the same flip -- so a profile
/// captured over USB through the kernel driver means the same thing when the
/// pad comes back over Bluetooth. Published raw, the stick works and is
/// upside down in every game, which is how it was reported.
pub fn decode_state(data: &[u8]) -> Option<State> {
    if data.len() < 12 {
        return None;
    }
    let twelve = |low: usize| -> (i32, i32) {
        let first = i32::from(data[low]) | ((i32::from(data[low + 1]) & 0x0F) << 8);
        let second = (i32::from(data[low + 1]) >> 4) | (i32::from(data[low + 2]) << 4);
        (first, STICK_MAX - second)
    };
    let (left_x, left_y) = twelve(6);
    let (right_x, right_y) = twelve(9);
    Some(State {
        right: data[3],
        shared: data[4],
        left: data[5],
        left_x,
        left_y,
        right_x,
        right_y,
    })
}

/// The d-pad's four bits as `(ABS_HAT0X, ABS_HAT0Y)`.
///
/// Opposite bits cancel -- which the hardware cannot physically do, but a
/// stuck bit can.
pub fn hat_for(left: u8) -> (i32, i32) {
    let bit = |mask: u8| i32::from(left & mask != 0);
    (
        bit(DPAD_RIGHT) - bit(DPAD_LEFT),
        bit(DPAD_DOWN) - bit(DPAD_UP),
    )
}

/// The 64-byte output report that asks for full mode.
pub fn full_mode_packet(counter: u8) -> [u8; 64] {
    let mut packet = [0u8; 64];
    packet[0] = 0x01;
    packet[1] = counter & 0x0F;
    packet[2..10].copy_from_slice(&RUMBLE_NEUTRAL);
    packet[10] = SUBCMD_REPORT_MODE;
    packet[11] = REPORT_FULL;
    packet
}

/// User decisions about the hidraw path: `{"057e:2017": true}`.
///
/// The escape hatch that keeps [`HID_DRIVERS`] from being another allowlist.
/// A pad the kernel binds to a driver padmap has never heard of, but which
/// speaks this protocol, can be switched on here; one that matches the driver
/// and is better off on evdev can be switched off. Neither needs a release.
pub fn parse_overrides(text: &str) -> BTreeMap<String, bool> {
    let Ok(raw) = serde_json::from_str::<serde_json::Value>(text) else {
        return BTreeMap::new();
    };
    let Some(object) = raw.as_object() else {
        return BTreeMap::new();
    };
    object
        .iter()
        // Booleans only. A `1` or a `"yes"` is someone guessing at the format,
        // and guessing wrong should not silently switch a controller's whole
        // input path.
        .filter_map(|(key, value)| value.as_bool().map(|on| (key.to_lowercase(), on)))
        .collect()
}

/// A Switch Pro controller, read as a stream of evdev events.
#[derive(Debug)]
pub struct Source {
    fd: OwnedFd,
    path: PathBuf,
    buttons: BTreeMap<u16, i32>,
    axes: BTreeMap<u16, i32>,
    hat: (i32, i32),
    counter: u8,
    /// Consecutive 0x3f reports since the last usable one. Reset by a 0x30
    /// rather than only counted up, so a pad that drops back into simple mode
    /// later in the session is caught the same way as one that never left it.
    simple_seen: u32,
}

impl Source {
    pub fn open(path: &Path) -> io::Result<Self> {
        use rustix::fs::{Mode, OFlags};
        let fd = rustix::fs::open(
            path,
            OFlags::RDWR | OFlags::NONBLOCK | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let mut source = Source {
            fd,
            path: path.to_path_buf(),
            buttons: BTreeMap::new(),
            axes: BTreeMap::new(),
            hat: (0, 0),
            counter: 0,
            simple_seen: 0,
        };
        source.request_full_mode();
        Ok(source)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Ask for report 0x30, the one carrying sticks and buttons.
    ///
    /// An output report, so a plain `write`. The Steam Controller's equivalent
    /// is a feature report and needs an ioctl; sending either the other way
    /// succeeds and does nothing.
    fn request_full_mode(&mut self) {
        let packet = full_mode_packet(self.counter);
        self.counter = self.counter.wrapping_add(1);
        if let Err(error) = rustix::io::write(&self.fd, &packet) {
            warn!(
                "{}: could not request full report mode: {error}",
                self.path.display()
            );
        }
    }

    /// A 0x3f arrived, which means the pad is not in the mode it was asked for.
    fn note_simple_report(&mut self) {
        self.simple_seen += 1;
        if self.simple_seen == 1 {
            warn!(
                "{}: sending report {REPORT_SIMPLE:#04x}, not the {REPORT_FULL:#04x} full mode \
                 it was asked for -- no input can be decoded until it switches; re-requesting",
                self.path.display()
            );
        }
        if self.simple_seen.is_multiple_of(SIMPLE_REPORTS_BEFORE_RETRY) {
            self.request_full_mode();
        }
    }

    /// Every change since the last call.
    pub fn fetch_events(&mut self, out: &mut Vec<InputEvent>) -> io::Result<()> {
        let before = out.len();
        let mut buffer = [0u8; 362];
        let mut read_any = false;
        // Bounded for the same reason the Steam Controller's loop is: this
        // runs on the thread that forwards every player's input.
        const MAX_REPORTS_PER_WAKE: usize = 128;
        for _ in 0..MAX_REPORTS_PER_WAKE {
            match rustix::io::read(&self.fd, &mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    read_any = true;
                    let report = &buffer[..size];
                    match report.first() {
                        Some(&REPORT_FULL) if size >= 12 => {
                            self.simple_seen = 0;
                            self.decode(report, out);
                        }
                        Some(&REPORT_SIMPLE) => self.note_simple_report(),
                        _ => {}
                    }
                }
                Err(rustix::io::Errno::AGAIN) => break,
                Err(error) => return Err(io::Error::from(error)),
            }
        }
        if !read_any && out.len() == before {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        if out.len() > before {
            out.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
        }
        Ok(())
    }

    fn decode(&mut self, report: &[u8], out: &mut Vec<InputEvent>) {
        let Some(state) = decode_state(report) else {
            return;
        };
        for (byte, table) in [
            (state.right, &BUTTONS_RIGHT[..]),
            (state.shared, &BUTTONS_SHARED[..]),
            (state.left, &BUTTONS_LEFT[..]),
        ] {
            for &(mask, key) in table {
                let value = i32::from(byte & mask != 0);
                // Absent counts as up, not as unknown: otherwise the first
                // report of a session emits a key-up for every button that is
                // not pressed.
                if self.buttons.insert(key.code(), value).unwrap_or(0) != value {
                    out.push(InputEvent::new(EventType::KEY.0, key.code(), value));
                }
            }
        }

        let hat = hat_for(state.left);
        if hat != self.hat {
            if hat.0 != self.hat.0 {
                out.push(InputEvent::new(
                    EventType::ABSOLUTE.0,
                    AbsoluteAxisCode::ABS_HAT0X.0,
                    hat.0,
                ));
            }
            if hat.1 != self.hat.1 {
                out.push(InputEvent::new(
                    EventType::ABSOLUTE.0,
                    AbsoluteAxisCode::ABS_HAT0Y.0,
                    hat.1,
                ));
            }
            self.hat = hat;
        }

        for (axis, value) in [
            (AbsoluteAxisCode::ABS_X, state.left_x),
            (AbsoluteAxisCode::ABS_Y, state.left_y),
            (AbsoluteAxisCode::ABS_RX, state.right_x),
            (AbsoluteAxisCode::ABS_RY, state.right_y),
        ] {
            // Only past the fuzz: these jitter by a few counts every report,
            // and forwarding that is a wake-up per axis per report for a pad
            // sitting still on a table. First sight always counts, because the
            // clone has to be told where the stick is.
            let changed = match self.axes.get(&axis.0) {
                None => true,
                Some(&previous) => (previous - value).abs() >= STICK_FUZZ,
            };
            if changed {
                self.axes.insert(axis.0, value);
                out.push(InputEvent::new(EventType::ABSOLUTE.0, axis.0, value));
            }
        }
    }

    /// What a clone of this pad is built from.
    pub fn capabilities(&self) -> (Vec<u16>, Vec<(u16, AbsInfo)>) {
        let keys = BUTTONS_RIGHT
            .iter()
            .chain(&BUTTONS_SHARED)
            .chain(&BUTTONS_LEFT)
            .map(|(_, key)| key.code())
            .collect();
        let stick = AbsInfo::new(
            (STICK_MIN + STICK_MAX) / 2,
            STICK_MIN,
            STICK_MAX,
            STICK_FUZZ,
            STICK_FLAT,
            0,
        );
        let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
        let axes = vec![
            (AbsoluteAxisCode::ABS_X.0, stick),
            (AbsoluteAxisCode::ABS_Y.0, stick),
            (AbsoluteAxisCode::ABS_RX.0, stick),
            (AbsoluteAxisCode::ABS_RY.0, stick),
            (AbsoluteAxisCode::ABS_HAT0X.0, hat),
            (AbsoluteAxisCode::ABS_HAT0Y.0, hat),
        ];
        (keys, axes)
    }

    pub fn held_keys(&self) -> Vec<u16> {
        self.buttons
            .iter()
            .filter(|(_, &value)| value != 0)
            .map(|(&code, _)| code)
            .collect()
    }
}

impl AsRawFd for Source {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl AsFd for Source {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl Source {
    /// Decode one report directly, for tests that have no device to read from.
    #[doc(hidden)]
    pub fn decode_for_test(&mut self, report: &[u8], out: &mut Vec<InputEvent>) {
        self.decode(report, out);
    }
}
