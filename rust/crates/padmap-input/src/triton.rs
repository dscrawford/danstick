//! The 2026 Steam Controller, driven directly via HID protocol.

use std::collections::BTreeMap;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use evdev::{AbsInfo, AbsoluteAxisCode, EventType, InputEvent, KeyCode};
use log::{debug, info};

use padmap_core::motion::Motion;

use crate::lizard;
use crate::pad::Pad;

pub const VENDOR: u16 = 0x28DE;
pub const NAME: &str = "Steam Controller";
/// 0x1302: wired, 0x1303: BLE, 0x1304: four-slot receiver, 0x1305: Steam Machine receiver.
pub const PRODUCTS: [u16; 4] = [0x1302, 0x1303, 0x1304, 0x1305];

const FEATURE_REPORT_BYTES: usize = 64;
const SET_SETTINGS_VALUES: u8 = 0x87;
const SETTING_LIZARD_MODE: u8 = 9;
const CONTROLLER_SETTING_BYTES: u8 = 3;

/// HIDIOCSFEATURE: encodes opcode 0x06 with length in bits 16-29, max 0x3FFF.
pub const fn hidiocsfeature(len: u32) -> rustix::ioctl::Opcode {
    assert!(
        len > 0 && len <= 0x3FFF,
        "feature report length out of range"
    );
    0xC000_0000 | ((b'H' as u32) << 8) | 0x06 | ((len & 0x3FFF) << 16)
}
const HIDIOCSFEATURE_64: rustix::ioctl::Opcode = hidiocsfeature(FEATURE_REPORT_BYTES as u32);

const _: () = assert!(
    ((HIDIOCSFEATURE_64 >> 16) & 0x3FFF) as usize
        == core::mem::size_of::<[u8; FEATURE_REPORT_BYTES]>()
);

const REPORT_STATE: u8 = 0x42;
const REPORT_STATE_BLE: u8 = 0x45;
const REPORT_STATE_TIMESTAMP: u8 = 0x47;
const REPORT_WIRELESS: u8 = 0x79;
const REPORT_WIRELESS_X: u8 = 0x46;
const STATE_REPORTS: [u8; 3] = [REPORT_STATE, REPORT_STATE_BLE, REPORT_STATE_TIMESTAMP];

const WIRELESS_CONNECT: u8 = 2;
const LIZARD_RESEND: Duration = Duration::from_secs(3);

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
    (BTN_L4, KeyCode::BTN_TRIGGER_HAPPY1),
    (BTN_R4, KeyCode::BTN_TRIGGER_HAPPY2),
    (BTN_L5, KeyCode::BTN_TRIGGER_HAPPY3),
    (BTN_R5, KeyCode::BTN_TRIGGER_HAPPY4),
    (BTN_QAM, KeyCode::BTN_TRIGGER_HAPPY5),
    (LEFT_PAD_CLICK, KeyCode::BTN_TRIGGER_HAPPY6),
    (RIGHT_PAD_CLICK, KeyCode::BTN_TRIGGER_HAPPY7),
];

const STICK_MIN: i32 = -32768;
const STICK_MAX: i32 = 32767;
const TRIGGER_MAX: i32 = 32767;

/// State report byte offsets and sizes.
const OFF_BUTTONS: usize = 1;
const STATE_PREFIX_BYTES: usize = 17;
/// Accelerometer starts at byte 33 in state reports.
const OFF_IMU_ACCEL: usize = 33;
const IMU_BYTES: usize = 12;
const STATE_IMU_BYTES: usize = OFF_IMU_ACCEL + IMU_BYTES;
const GYRO_FULL_SCALE_DPS: f32 = 2000.0;
const ACCEL_FULL_SCALE_G: f32 = 2.0;
const IMU_HALF_RANGE: f32 = 32768.0;
/// 0x47 timestamps in 32-us steps, others in microseconds.
const OFF_IMU_CLOCK_WIDE: usize = 29;
const OFF_IMU_CLOCK_SHORT: usize = 31;
const IMU_CLOCK_SHORT_US: u64 = 32;

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

pub fn decode_state(payload: &[u8]) -> Option<State> {
    if payload.len() < STATE_PREFIX_BYTES {
        return None;
    }
    let at = OFF_BUTTONS;
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

pub fn decode_motion(payload: &[u8], wide: bool) -> Option<Motion> {
    if payload.len() < STATE_IMU_BYTES {
        return None;
    }
    let i16_at = |i: usize| i16::from_le_bytes([payload[i], payload[i + 1]]) as f32;
    let at = OFF_IMU_ACCEL;
    let accel = |i: usize| i16_at(at + i * 2) / IMU_HALF_RANGE * ACCEL_FULL_SCALE_G;
    let gyro = |i: usize| i16_at(at + 6 + i * 2) / IMU_HALF_RANGE * GYRO_FULL_SCALE_DPS;
    let timestamp_us = if wide {
        u64::from(u32::from_le_bytes([
            payload[OFF_IMU_CLOCK_WIDE],
            payload[OFF_IMU_CLOCK_WIDE + 1],
            payload[OFF_IMU_CLOCK_WIDE + 2],
            payload[OFF_IMU_CLOCK_WIDE + 3],
        ]))
    } else {
        u64::from(u16::from_le_bytes([
            payload[OFF_IMU_CLOCK_SHORT],
            payload[OFF_IMU_CLOCK_SHORT + 1],
        ])) * IMU_CLOCK_SHORT_US
    };
    Some(Motion::from_sdl_frame(
        [accel(0), accel(2), -accel(1)],
        [gyro(0), gyro(2), -gyro(1)],
        timestamp_us,
    ))
}

pub fn hat_for(buttons: u32) -> (i32, i32) {
    let bit = |mask: u32| i32::from(buttons & mask != 0);
    (
        bit(DPAD_RIGHT) - bit(DPAD_LEFT),
        bit(DPAD_DOWN) - bit(DPAD_UP),
    )
}

/// Feature report to disable lizard mode: [0x01, 0x87, 0x03, 0x09, 0x00, ...zeros].
pub fn lizard_off_packet() -> [u8; FEATURE_REPORT_BYTES] {
    let mut packet = [0u8; FEATURE_REPORT_BYTES];
    packet[0] = 0x01;
    packet[1] = SET_SETTINGS_VALUES;
    packet[2] = CONTROLLER_SETTING_BYTES;
    packet[3] = SETTING_LIZARD_MODE;
    packet
}

/// Feature report to disable lizard mode.
#[allow(unsafe_code)]
fn send_lizard_off(fd: impl AsFd) -> io::Result<()> {
    let packet = lizard_off_packet();
    // SAFETY: packet and opcode both sized to FEATURE_REPORT_BYTES.
    let setter = unsafe { rustix::ioctl::Setter::<HIDIOCSFEATURE_64, _>::new(packet) };
    // SAFETY: opcode matches payload type, fd is open read-write hidraw node.
    unsafe { rustix::ioctl::ioctl(fd, setter) }.map_err(io::Error::from)
}

const DOCK_USAGE: u32 = 0xFF00_0002;

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
    let stat = rustix::fs::fstat(&fd).map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::CharacterDevice {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    Ok(fd)
}

pub fn slots(probe: bool) -> Vec<Pad> {
    slots_where(probe, crate::pad::wanted_by_name)
}

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
        if !wanted(NAME) {
            continue;
        }
        if probe && !slot_is_live(&node) {
            continue;
        }
        found.push(Pad {
            path: node,
            name: NAME.to_owned(),
            phys: uevent.get("HID_PHYS").cloned().unwrap_or_default(),
            uniq: format!(
                "{}/{}",
                uevent.get("HID_UNIQ").cloned().unwrap_or_default(),
                dir.file_name().unwrap_or_default().to_string_lossy()
            ),
            vid,
            pid,
            syspath: dir.clone(),
            retroarch_visible: false,
            motion: None,
        });
    }
    found
}

/// Live slot paths (expensive: one ioctl per slot).
pub fn live_signature() -> Vec<PathBuf> {
    slots(true).into_iter().map(|pad| pad.path).collect()
}

pub fn owns(pad: &Pad) -> bool {
    pad.vid == VENDOR
        && PRODUCTS.contains(&pad.pid)
        && pad.path.to_string_lossy().starts_with("/dev/hidraw")
}

fn read_uevent(dir: &Path) -> BTreeMap<String, String> {
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

#[derive(Debug)]
pub struct Source {
    fd: OwnedFd,
    path: PathBuf,
    buttons: BTreeMap<u16, i32>,
    axes: BTreeMap<u16, i32>,
    hat: (i32, i32),
    last_lizard: Option<Instant>,
    motion: Option<Motion>,
    imu_tick: Option<u64>,
    /// Accumulated microseconds since reading started (wraps handled).
    imu_clock: u64,
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
            motion: None,
            imu_tick: None,
            imu_clock: 0,
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

    pub fn motion(&self) -> Option<Motion> {
        self.motion
    }

    fn note_motion(&mut self, payload: &[u8], wide: bool) {
        let Some(sample) = decode_motion(payload, wide) else {
            return;
        };
        let raw = sample.timestamp_us;
        // Counter wraps: 32-bit for wide, (16-bit * 32us) for narrow.
        let period: u64 = if wide {
            1 << 32
        } else {
            (1 << 16) * IMU_CLOCK_SHORT_US
        };
        if let Some(last) = self.imu_tick {
            self.imu_clock += (raw + period - last) % period;
        }
        self.imu_tick = Some(raw);
        self.motion = Some(Motion {
            timestamp_us: self.imu_clock,
            ..sample
        });
    }

    fn leave_lizard_mode(&mut self) {
        match send_lizard_off(&self.fd) {
            Ok(()) => self.last_lizard = Some(Instant::now()),
            Err(error) => debug!(
                "{}: lizard-mode request refused: {error}",
                self.path.display()
            ),
        }
    }

    fn keepalive(&mut self) {
        let due = match self.last_lizard {
            None => true,
            Some(at) => at.elapsed() >= LIZARD_RESEND,
        };
        if due {
            self.leave_lizard_mode();
        }
    }

    pub fn fetch_events(&mut self, out: &mut Vec<InputEvent>) -> io::Result<()> {
        self.keepalive();
        let before = out.len();
        let mut buffer = [0u8; FEATURE_REPORT_BYTES];
        let mut read_any = false;
        const MAX_REPORTS_PER_WAKE: usize = 128;
        for _ in 0..MAX_REPORTS_PER_WAKE {
            match rustix::io::read(&self.fd, &mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    read_any = true;
                    self.consume(&buffer[..size], out);
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

    fn consume(&mut self, report: &[u8], out: &mut Vec<InputEvent>) {
        let Some((&id, payload)) = report.split_first() else {
            return;
        };
        if STATE_REPORTS.contains(&id) {
            if !self.connected {
                self.note_connected(true, out);
            }
            self.note_motion(payload, id != REPORT_STATE_TIMESTAMP);
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
            self.leave_lizard_mode();
            return;
        }
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

        let readings = [
            (AbsoluteAxisCode::ABS_X, i32::from(state.left_x)),
            (AbsoluteAxisCode::ABS_Y, -i32::from(state.left_y)),
            (AbsoluteAxisCode::ABS_RX, i32::from(state.right_x)),
            (AbsoluteAxisCode::ABS_RY, -i32::from(state.right_y)),
            (AbsoluteAxisCode::ABS_Z, i32::from(state.trigger_left)),
            (AbsoluteAxisCode::ABS_RZ, i32::from(state.trigger_right)),
        ];
        for (axis, reading) in readings {
            let value = reading.clamp(STICK_MIN, STICK_MAX);
            if self.axes.insert(axis.0, value) != Some(value) {
                out.push(InputEvent::new(EventType::ABSOLUTE.0, axis.0, value));
            }
        }
    }

    pub fn capabilities(&self) -> (Vec<u16>, Vec<(u16, AbsInfo)>) {
        let keys = BUTTONS.iter().map(|(_, key)| key.code()).collect();
        let stick = AbsInfo::new(0, STICK_MIN, STICK_MAX, 16, 1024, 0);
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
