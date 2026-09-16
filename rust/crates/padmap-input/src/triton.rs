//! The 2026 Steam Controller, driven directly.
//!
//! A port of `src/padmap/triton.py`, which is itself a port of SDL's
//! `SDL_hidapi_steam_triton.c` (zlib, upstream 2025-11-12). Both exist because
//! the kernel's `hid-steam` only learned these ids in Linux 7.3: on anything
//! older the receiver publishes a mouse and a keyboard per slot and no joypad,
//! and nothing reports an error.
//!
//! padmap is a virtual gamepad. Telling the user to upgrade a kernel is not
//! what it is for, so it speaks the protocol instead and the controller
//! becomes an ordinary pad like any other.
//!
//! Three things that are not obvious, all of them already paid for once in the
//! Python:
//!
//! * **Leaving lizard mode is a *feature* report**, not a write -- see
//!   [`send_lizard_off`].
//! * **It has to be re-sent every three seconds**, forever -- see
//!   [`Source::keepalive`].
//! * **An empty slot stalls the transfer with `EPIPE`**, the only cheap way
//!   to tell a slot with a controller in it from one without -- see
//!   [`slot_is_live`].

use std::collections::BTreeMap;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use evdev::{AbsInfo, AbsoluteAxisCode, EventType, InputEvent, KeyCode};
use log::{debug, info};

use crate::lizard;
use crate::pad::Pad;

/// Valve.
pub const VENDOR: u16 = 0x28DE;

/// What padmap calls one of these.
///
/// SDL's name for it, so a mapping captured through padmap and one captured
/// against SDL agree about what the controller is.
pub const NAME: &str = "Steam Controller";

/// The models whose gamepad state lives behind this protocol.
///
/// 1304 is the puck -- a four-slot wireless receiver. The others are the
/// controller itself wired and over Bluetooth, and a Steam Machine's built-in
/// receiver. Named IBEX, IBEX_BLE, PROTEUS and NEREID in mainline's hid-ids.h.
pub const PRODUCTS: [u16; 4] = [0x1302, 0x1303, 0x1304, 0x1305];

/// `controller_structs.h:26`.
const FEATURE_REPORT_BYTES: usize = 64;
/// `controller_constants.h:64`, `ID_SET_SETTINGS_VALUES`.
const SET_SETTINGS_VALUES: u8 = 0x87;
/// The tenth entry of an enum starting at `SETTING_MOUSE_SENSITIVITY = 0`.
const SETTING_LIZARD_MODE: u8 = 9;
/// `sizeof(ControllerSetting)` under `#pragma pack(1)`: u8 + u16.
const CONTROLLER_SETTING_BYTES: u8 = 3;

/// `_IOC(_IOC_READ|_IOC_WRITE, 'H', 0x06, len)` -- the **S**et direction only.
///
/// Spelled out because rustix's `Opcode` is a plain `u32` here and has no
/// constructor for this shape. Direction `READ|WRITE` is 3 at bit 30, type
/// `'H'` is 0x48 at bit 8, number is 6, and the length sits at bit 16 --
/// which for 64 bytes is 0xC0404806, the same value `triton.py` computes.
///
/// The direction bits claim read-write, but for number 0x06 the kernel's
/// `hidraw_send_report` only ever copies *from* userspace. That is what makes
/// [`rustix::ioctl::Setter`] -- whose `IS_MUTATING` is false, so the compiler
/// is told no userspace memory is written -- sound here. The **G**et opcode is
/// number 0x07, has an identical `_IOWR` shape, and copies *back*: writing it
/// with `Setter` by analogy would be undefined behaviour. It needs `Updater`.
pub const fn hidiocsfeature(len: u32) -> rustix::ioctl::Opcode {
    // The size field is fourteen bits. A length of 16384 or more runs into the
    // direction bits and silently requests a *different* ioctl -- and an
    // opcode encoding a size larger than the buffer makes the kernel
    // copy_from_user past it, reading the daemon's stack out to a USB device.
    // triton.py raises here for the same reason; `<<` in Rust does not.
    assert!(
        len > 0 && len <= 0x3FFF,
        "feature report length out of range"
    );
    0xC000_0000 | ((b'H' as u32) << 8) | 0x06 | ((len & 0x3FFF) << 16)
}
const HIDIOCSFEATURE_64: rustix::ioctl::Opcode = hidiocsfeature(FEATURE_REPORT_BYTES as u32);

/// The opcode's encoded size is the payload's size.
///
/// `Setter`'s `OPCODE` and `Input` are independent generic parameters and
/// rustix relates them not at all -- it is the caller's SAFETY obligation, and
/// the only thing holding it is that both come from `FEATURE_REPORT_BYTES`.
/// Drift is an out-of-bounds read by the kernel, so it is a build failure
/// rather than a comment asserting someone will notice.
const _: () = assert!(
    ((HIDIOCSFEATURE_64 >> 16) & 0x3FFF) as usize
        == core::mem::size_of::<[u8; FEATURE_REPORT_BYTES]>()
);

/// `ETritonReportIDTypes`, `controller_structs.h:552`.
const REPORT_STATE: u8 = 0x42;
const REPORT_STATE_BLE: u8 = 0x45;
const REPORT_STATE_TIMESTAMP: u8 = 0x47;
const REPORT_WIRELESS: u8 = 0x79;
const REPORT_WIRELESS_X: u8 = 0x46;
/// All three state reports open with the same 29 bytes and differ only in the
/// IMU block after them, which is why SDL parses 0x42 through the same prefix
/// it uses for 0x45.
const STATE_REPORTS: [u8; 3] = [REPORT_STATE, REPORT_STATE_BLE, REPORT_STATE_TIMESTAMP];

/// `ETritonWirelessState`, `controller_structs.h:563`.
const WIRELESS_CONNECT: u8 = 2;

/// How often the controller has to be told again.
/// `SDL_hidapi_steam_triton.c:496`.
const LIZARD_RESEND: Duration = Duration::from_secs(3);

/// `TritonButtons`, `SDL_hidapi_steam_triton.c:58`.
const BTN_A: u32 = 0x0000_0001;
const BTN_B: u32 = 0x0000_0002;
const BTN_X: u32 = 0x0000_0004;
const BTN_Y: u32 = 0x0000_0008;
const BTN_QAM: u32 = 0x0000_0010;
const BTN_R3: u32 = 0x0000_0020;
const BTN_VIEW: u32 = 0x0000_0040;
const BTN_R4: u32 = 0x0000_0080;
const BTN_R5: u32 = 0x0000_0100;
const BTN_R: u32 = 0x0000_0200;
const DPAD_DOWN: u32 = 0x0000_0400;
const DPAD_RIGHT: u32 = 0x0000_0800;
const DPAD_LEFT: u32 = 0x0000_1000;
const DPAD_UP: u32 = 0x0000_2000;
const BTN_MENU: u32 = 0x0000_4000;
const BTN_L3: u32 = 0x0000_8000;
const BTN_STEAM: u32 = 0x0001_0000;
const BTN_L4: u32 = 0x0002_0000;
const BTN_L5: u32 = 0x0004_0000;
const BTN_L: u32 = 0x0008_0000;
const RIGHT_PAD_CLICK: u32 = 0x0040_0000;
const RIGHT_TRIGGER_CLICK: u32 = 0x0080_0000;
const LEFT_PAD_CLICK: u32 = 0x0400_0000;
const LEFT_TRIGGER_CLICK: u32 = 0x0800_0000;

/// Triton bit -> the evdev code padmap publishes for it.
///
/// Face buttons follow SDL's mapping (A->SOUTH, B->EAST, X->WEST, Y->NORTH)
/// rather than the printed labels, because a pad reporting "A" where every
/// other pad reports SOUTH is a pad every stored mapping is wrong for. MENU is
/// SELECT and VIEW is START, which reads backwards and is what SDL does.
const BUTTONS: [(u32, KeyCode); 20] = [
    (BTN_A, KeyCode::BTN_SOUTH),
    (BTN_B, KeyCode::BTN_EAST),
    (BTN_X, KeyCode::BTN_WEST),
    (BTN_Y, KeyCode::BTN_NORTH),
    (BTN_L, KeyCode::BTN_TL),
    (BTN_R, KeyCode::BTN_TR),
    (LEFT_TRIGGER_CLICK, KeyCode::BTN_TL2),
    (RIGHT_TRIGGER_CLICK, KeyCode::BTN_TR2),
    (BTN_MENU, KeyCode::BTN_SELECT),
    (BTN_VIEW, KeyCode::BTN_START),
    (BTN_STEAM, KeyCode::BTN_MODE),
    (BTN_L3, KeyCode::BTN_THUMBL),
    (BTN_R3, KeyCode::BTN_THUMBR),
    // The paddles and pad clicks have no standard code, so they take the
    // BTN_TRIGGER_HAPPY block the kernel reserves for exactly this. A stored
    // mapping keys on the code, so what matters is that they never move.
    (BTN_L4, KeyCode::BTN_TRIGGER_HAPPY1),
    (BTN_R4, KeyCode::BTN_TRIGGER_HAPPY2),
    (BTN_L5, KeyCode::BTN_TRIGGER_HAPPY3),
    (BTN_R5, KeyCode::BTN_TRIGGER_HAPPY4),
    (BTN_QAM, KeyCode::BTN_TRIGGER_HAPPY5),
    (LEFT_PAD_CLICK, KeyCode::BTN_TRIGGER_HAPPY6),
    (RIGHT_PAD_CLICK, KeyCode::BTN_TRIGGER_HAPPY7),
    // The capacitive touch bits are deliberately absent. A finger resting on a
    // stick is not a press, and bound by the mapping wizard they would fire
    // constantly.
];

const STICK_MIN: i32 = -32768;
const STICK_MAX: i32 = 32767;
const TRIGGER_MAX: i32 = 32767;

/// Offsets into a state report's payload, `TritonMTUNoQuat_t` packed.
const OFF_BUTTONS: usize = 1;
/// Through the right stick, which is the last field padmap reads.
const STATE_PREFIX_BYTES: usize = 17;

/// The gamepad fields of one state report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct State {
    pub buttons: u32,
    pub trigger_left: i16,
    pub trigger_right: i16,
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
}

/// One state report's gamepad fields, or `None` if it is too short.
pub fn decode_state(payload: &[u8]) -> Option<State> {
    if payload.len() < STATE_PREFIX_BYTES {
        return None;
    }
    let at = OFF_BUTTONS;
    // The length was checked above, so every slice below is in range. Sized
    // explicitly rather than with try_into().unwrap(): a panic here would be
    // on the daemon's tick, and "cannot happen" is not a reason to leave one
    // reachable from a device.
    let u32_at =
        |i: usize| u32::from_le_bytes([payload[i], payload[i + 1], payload[i + 2], payload[i + 3]]);
    let i16_at = |i: usize| i16::from_le_bytes([payload[i], payload[i + 1]]);
    Some(State {
        buttons: u32_at(at),
        trigger_left: i16_at(at + 4),
        trigger_right: i16_at(at + 6),
        left_x: i16_at(at + 8),
        left_y: i16_at(at + 10),
        right_x: i16_at(at + 12),
        right_y: i16_at(at + 14),
    })
}

/// The d-pad as `(ABS_HAT0X, ABS_HAT0Y)`.
///
/// Opposites cancel, which is what a real hat does -- it cannot report both.
pub fn hat_for(buttons: u32) -> (i32, i32) {
    let bit = |mask: u32| i32::from(buttons & mask != 0);
    (
        bit(DPAD_RIGHT) - bit(DPAD_LEFT),
        bit(DPAD_DOWN) - bit(DPAD_UP),
    )
}

/// The 64 bytes that turn lizard mode off.
///
/// ```text
/// 01 87 03 09 00 00   then 58 zeros
/// ```
///
/// Report id 1, then a `FeatureReportMsg` whose header is
/// `{ID_SET_SETTINGS_VALUES, sizeof(ControllerSetting)}` and whose single
/// setting is `{SETTING_LIZARD_MODE, LIZARD_MODE_OFF}`.
pub fn lizard_off_packet() -> [u8; FEATURE_REPORT_BYTES] {
    let mut packet = [0u8; FEATURE_REPORT_BYTES];
    packet[0] = 0x01;
    packet[1] = SET_SETTINGS_VALUES;
    packet[2] = CONTROLLER_SETTING_BYTES;
    packet[3] = SETTING_LIZARD_MODE;
    // settingValue = LIZARD_MODE_OFF = 0, u16 little-endian. Already zero.
    packet
}

/// Send it. A feature report, which is not a write.
///
/// `write()` on this node would be an *output* report -- a different channel
/// that this message does not travel on. It does not fail loudly; the
/// controller simply stays a keyboard.
// The workspace warns on unsafe, and these two blocks are the only ones in
// padmap. An ioctl cannot be expressed safely: the kernel is handed a pointer
// and a number, and nothing in the type system relates them. Both invariants
// are checked by construction rather than asserted -- see the SAFETY notes --
// and `tests/triton_protocol.rs` pins the opcode to the value the kernel
// expects, which is the part a typo would break silently.
#[allow(unsafe_code)]
fn send_lizard_off(fd: impl AsFd) -> io::Result<()> {
    let packet = lizard_off_packet();
    // SAFETY: HIDIOCSFEATURE takes a buffer of exactly the length encoded in
    // the opcode, and `packet` is that length by construction -- both come
    // from FEATURE_REPORT_BYTES.
    let setter = unsafe { rustix::ioctl::Setter::<HIDIOCSFEATURE_64, _>::new(packet) };
    // SAFETY: the opcode and the payload type agree, and the fd is a hidraw
    // node opened read-write.
    unsafe { rustix::ioctl::ioctl(fd, setter) }.map_err(io::Error::from)
}

/// Mainline ignores this collection by name: the puck's pogo-pin dock.
const DOCK_USAGE: u32 = 0xFF00_0002;

/// Is a controller actually paired into this slot?
///
/// Asked by sending the lizard-mode request and seeing whether the device
/// takes it. An empty slot stalls the control transfer -- `EPIPE` -- because
/// there is nothing on the other end of the radio link to configure.
///
/// Without this, padmap finds four controllers for one physical pad and offers
/// four players, three of which never send an event. Any error *other* than a
/// stall leaves the slot reported: a permission problem is not evidence about
/// whether a controller is paired, and guessing "empty" would hide a working
/// pad.
pub fn slot_is_live(node: &Path) -> bool {
    let Ok(fd) = open_hidraw(node) else {
        return false;
    };
    match send_lizard_off(&fd) {
        Ok(()) => true,
        Err(error) => error.raw_os_error() != Some(rustix::io::Errno::PIPE.raw_os_error()),
    }
}

fn open_hidraw(node: &Path) -> io::Result<OwnedFd> {
    use rustix::fs::{FileType, Mode, OFlags};
    let fd = rustix::fs::open(
        node,
        OFlags::RDWR | OFlags::NONBLOCK | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    // Checked after the open rather than before it, so nothing can be swapped
    // in between the two. /dev is root-owned and udev-managed so this needs
    // root to matter, but the window is real: unplugging and replugging can
    // rebind /dev/hidrawN to a different device between discovery and here,
    // and the ioctl below is only meaningful to hidraw.
    let stat = rustix::fs::fstat(&fd).map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::CharacterDevice {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    Ok(fd)
}

/// Every Triton slot on the machine, as pads padmap can open.
///
/// Synthesised rather than found under `/sys/class/input`, because there is
/// nothing there to find: with no kernel driver the receiver publishes a mouse
/// and a keyboard per slot and no joypad at all. `path` is the hidraw node --
/// unusual for a `Pad`, and honest: it is the device padmap opens, and the
/// thing that makes two slots of one receiver different from each other.
///
/// `probe = false` reports every slot the receiver has, paired or not, which
/// is what a diagnostic wants and what a player list must not have.
pub fn slots(probe: bool) -> Vec<Pad> {
    slots_where(probe, crate::pad::wanted_by_name)
}

/// The same, with the "may padmap touch this?" question passed in.
///
/// A parameter rather than a read of the environment, so the gate below can
/// be tested for real instead of by reading it. `PADMAP_ONLY_DEVICE` is
/// process-wide, and a test that sets it races every other test that
/// enumerates -- which is how this file acquired a flaky test the first time.
pub fn slots_where(probe: bool, wanted: impl Fn(&str) -> bool) -> Vec<Pad> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/bus/hid/devices") else {
        return found;
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    dirs.sort();

    for dir in dirs {
        let uevent = read_uevent(&dir);
        let Some((vid, pid)) = uevent
            .get("HID_ID")
            .and_then(|raw| lizard::ids_from_hid_id(raw))
        else {
            continue;
        };
        if vid != VENDOR || !PRODUCTS.contains(&pid) {
            continue;
        }
        // A kernel driver that knows this device is a better answer than this
        // module, and on Linux 7.3 there will be one. Standing down when
        // hid-steam has it is what stops padmap fighting the kernel for the
        // same reports.
        match uevent.get("DRIVER").map(String::as_str) {
            None | Some("") | Some("hid-generic") => {}
            Some(_) => continue,
        }
        let Ok(descriptor) = std::fs::read(dir.join("report_descriptor")) else {
            continue;
        };
        let collections = lizard::application_collections(&descriptor);
        if collections.is_empty() || collections[0] == DOCK_USAGE {
            continue;
        }
        let Some(node) = first_hidraw(&dir) else {
            continue;
        };
        // PADMAP_ONLY_DEVICE means "do not touch the machine's real
        // controllers": it exists so an isolated test daemon cannot fight the
        // live one for a pad. Filtering the *list* is not enough here, because
        // the probe below is a write -- a feature report to real hardware --
        // and it happens before any list is filtered. Gated by the same
        // switch, so the guarantee is what it says.
        if !wanted(NAME) {
            continue;
        }
        if probe && !slot_is_live(&node) {
            continue;
        }
        found.push(Pad {
            path: node,
            // SDL's name for it, so a mapping captured here and one captured
            // against SDL agree about what the controller is called.
            name: NAME.to_owned(),
            phys: uevent.get("HID_PHYS").cloned().unwrap_or_default(),
            // The receiver's serial is the same on every slot, so on its own
            // it cannot tell slot 1 from slot 2. The interface directory is
            // what separates them.
            uniq: format!(
                "{}/{}",
                uevent.get("HID_UNIQ").cloned().unwrap_or_default(),
                dir.file_name().unwrap_or_default().to_string_lossy()
            ),
            vid,
            pid,
            syspath: dir.clone(),
            // Nothing else on the machine can see this as a joypad, because
            // there is no joypad node for it to see. Saying so keeps it out of
            // RetroArch's pad-index arithmetic, where it would shift every
            // other player by one.
            retroarch_visible: false,
            // Its gyro streams, and padmap decodes none of it: the kernel
            // publishes no evdev node for this controller at all, so there is
            // nothing a consumer could open. Saying so is better than a flag
            // that promises motion nothing can read.
            motion: None,
        });
    }
    found
}

/// Which slots have a controller in them, as something comparable.
///
/// Not cheap -- one open, one ioctl and one close per slot, 12ms measured on a
/// four-slot receiver -- so the caller is responsible for rationing it. See
/// the note on `_triton_live_slots` in the Python daemon, which pays for
/// calling this on a 50Hz loop.
pub fn live_signature() -> Vec<PathBuf> {
    slots(true).into_iter().map(|pad| pad.path).collect()
}

/// Is this pad one of ours? Cheap, and asked before anything opens anything.
pub fn owns(pad: &Pad) -> bool {
    pad.vid == VENDOR
        && PRODUCTS.contains(&pad.pid)
        && pad.path.to_string_lossy().starts_with("/dev/hidraw")
}

fn read_uevent(dir: &Path) -> BTreeMap<String, String> {
    // Lossy, not strict: HID_NAME is the device's own name string, copied into
    // uevent verbatim by the kernel, and for a Bluetooth pad that is whatever
    // the peer sent. One byte of it that is not UTF-8 is not a reason to stop
    // enumerating controllers.
    let Ok(bytes) = std::fs::read(dir.join("uevent")) else {
        return BTreeMap::new();
    };
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn first_hidraw(dir: &Path) -> Option<PathBuf> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join("hidraw"))
        .ok()?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("hidraw"))
        .collect();
    names.sort();
    names.first().map(|name| PathBuf::from("/dev").join(name))
}

/// One slot, read as a stream of evdev events.
///
/// Reports are diffed against the previous one, so `fetch_events` yields only
/// the changes an evdev node would have delivered.
#[derive(Debug)]
pub struct Source {
    fd: OwnedFd,
    path: PathBuf,
    buttons: BTreeMap<u16, i32>,
    axes: BTreeMap<u16, i32>,
    hat: (i32, i32),
    last_lizard: Option<Instant>,
    /// Whether a controller is paired into this slot. A slot with nothing in
    /// it opens cleanly and reads nothing for ever, so this is the difference
    /// between "no controller" and "broken".
    connected: bool,
}

impl Source {
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut source = Self {
            fd: open_hidraw(path)?,
            path: path.to_path_buf(),
            buttons: BTreeMap::new(),
            axes: BTreeMap::new(),
            hat: (0, 0),
            last_lizard: None,
            connected: false,
        };
        source.leave_lizard_mode();
        Ok(source)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    /// Turn lizard mode off, and remember when.
    ///
    /// Not fatal when it fails: a slot with no controller paired into it
    /// stalls the transfer, which is not a fault -- there is nothing there to
    /// configure.
    fn leave_lizard_mode(&mut self) {
        match send_lizard_off(&self.fd) {
            Ok(()) => self.last_lizard = Some(Instant::now()),
            Err(error) => debug!(
                "{}: lizard-mode request refused: {error}",
                self.path.display()
            ),
        }
    }

    /// Ask again, every three seconds, for ever.
    ///
    /// The controller reverts on its own. Sent once and never again, a pad
    /// works for a few seconds and then turns back into a keyboard mid-game,
    /// which reads as failing hardware rather than a missing message.
    fn keepalive(&mut self) {
        let due = match self.last_lizard {
            None => true,
            Some(at) => at.elapsed() >= LIZARD_RESEND,
        };
        if due {
            self.leave_lizard_mode();
        }
    }

    /// Every change since the last call.
    ///
    /// `WouldBlock` when there is nothing, which is what the republisher
    /// already knows how to treat as "no events this wake-up".
    pub fn fetch_events(&mut self, out: &mut Vec<InputEvent>) -> io::Result<()> {
        self.keepalive();
        let before = out.len();
        let mut buffer = [0u8; FEATURE_REPORT_BYTES];
        let mut read_any = false;
        // Bounded. The loop otherwise exits only when the device stops having
        // something to say, and a device that produces reports as fast as they
        // are drained keeps it resident -- on the thread that forwards every
        // player's input, not just this one's. Whatever is left stays in the
        // kernel's ring, the descriptor stays readable, and the next wake-up
        // takes it.
        const MAX_REPORTS_PER_WAKE: usize = 128;
        for _ in 0..MAX_REPORTS_PER_WAKE {
            match rustix::io::read(&self.fd, &mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    read_any = true;
                    self.consume(&buffer[..size], out);
                }
                // EAGAIN, which is the same value as EWOULDBLOCK on Linux.
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

    fn consume(&mut self, report: &[u8], out: &mut Vec<InputEvent>) {
        let Some((&id, payload)) = report.split_first() else {
            return;
        };
        if STATE_REPORTS.contains(&id) {
            if !self.connected {
                self.note_connected(true, out);
            }
            self.decode(payload, out);
        } else if (id == REPORT_WIRELESS || id == REPORT_WIRELESS_X) && !payload.is_empty() {
            self.note_connected(payload[0] == WIRELESS_CONNECT, out);
        }
    }

    fn note_connected(&mut self, connected: bool, out: &mut Vec<InputEvent>) {
        if connected == self.connected {
            return;
        }
        self.connected = connected;
        info!(
            "{}: controller {}",
            self.path.display(),
            if connected { "paired" } else { "disconnected" }
        );
        if connected {
            // A pad that has just woken is in lizard mode again regardless of
            // what was sent to the empty slot before it.
            self.leave_lizard_mode();
            return;
        }
        // Everything held goes up, in this frame rather than the next one. A
        // controller that vanishes mid-press otherwise leaves a button down on
        // the clone with nothing to lift it.
        for (&code, &value) in &self.buttons {
            if value != 0 {
                out.push(InputEvent::new(EventType::KEY.0, code, 0));
            }
        }
        self.buttons.clear();
    }

    fn decode(&mut self, payload: &[u8], out: &mut Vec<InputEvent>) {
        let Some(state) = decode_state(payload) else {
            return;
        };
        for &(bit, key) in &BUTTONS {
            let value = i32::from(state.buttons & bit != 0);
            // Absent counts as up, not as unknown. Treating it as unknown
            // makes the first report of a session emit a key-up for every
            // button that is not pressed -- twenty events describing nothing,
            // and a frame indistinguishable from a controller releasing
            // everything at once. `hidraw::Source` already defaults to 0 here
            // for the same reason; this port had dropped that.
            let previous = self.buttons.insert(key.code(), value).unwrap_or(0);
            if previous != value {
                out.push(InputEvent::new(EventType::KEY.0, key.code(), value));
            }
        }

        let hat = hat_for(state.buttons);
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

        // Y is negated for the same reason SDL negates it: the controller
        // reports up as positive and evdev's convention is down-positive. A
        // pad published the other way up is not obviously wrong on a menu and
        // completely wrong in a game.
        let readings = [
            (AbsoluteAxisCode::ABS_X, i32::from(state.left_x)),
            (AbsoluteAxisCode::ABS_Y, -i32::from(state.left_y)),
            (AbsoluteAxisCode::ABS_RX, i32::from(state.right_x)),
            (AbsoluteAxisCode::ABS_RY, -i32::from(state.right_y)),
            (AbsoluteAxisCode::ABS_Z, i32::from(state.trigger_left)),
            (AbsoluteAxisCode::ABS_RZ, i32::from(state.trigger_right)),
        ];
        for (axis, reading) in readings {
            // -32768 has no positive counterpart in an i16, so negating it
            // returns itself. Clamped rather than left to wrap, which would
            // read fully down for a stick held fully up.
            let value = reading.clamp(STICK_MIN, STICK_MAX);
            if self.axes.insert(axis.0, value) != Some(value) {
                out.push(InputEvent::new(EventType::ABSOLUTE.0, axis.0, value));
            }
        }
    }

    /// What a clone of this slot is built from -- with full ranges rather than
    /// bare codes, for the SIGFPE reason spelled out in `clone::assemble`.
    pub fn capabilities(&self) -> (Vec<u16>, Vec<(u16, AbsInfo)>) {
        let keys = BUTTONS.iter().map(|(_, key)| key.code()).collect();
        let stick = AbsInfo::new(0, STICK_MIN, STICK_MAX, 16, 1024, 0);
        // Triggers rest at zero on a 0.. range, which is what every analogue
        // trigger on Linux reports. On the stick range they would read
        // half-pulled at rest.
        let trigger = AbsInfo::new(0, 0, TRIGGER_MAX, 0, 0, 0);
        let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
        let axes = vec![
            (AbsoluteAxisCode::ABS_X.0, stick),
            (AbsoluteAxisCode::ABS_Y.0, stick),
            (AbsoluteAxisCode::ABS_RX.0, stick),
            (AbsoluteAxisCode::ABS_RY.0, stick),
            (AbsoluteAxisCode::ABS_Z.0, trigger),
            (AbsoluteAxisCode::ABS_RZ.0, trigger),
            (AbsoluteAxisCode::ABS_HAT0X.0, hat),
            (AbsoluteAxisCode::ABS_HAT0Y.0, hat),
        ];
        (keys, axes)
    }

    /// Keys currently down, for releasing them when a wizard pauses.
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
