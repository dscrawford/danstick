//! The 2026 Steam Controller protocol, in Rust, against the same numbers.

use padmap_input::triton;

fn state(buttons: u32, tl: i16, tr: i16, lx: i16, ly: i16, rx: i16, ry: i16) -> Vec<u8> {
    let mut out = vec![0u8]; // seq
    out.extend_from_slice(&buttons.to_le_bytes());
    for value in [tl, tr, lx, ly, rx, ry] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.resize(53, 0);
    out
}

#[test]
fn the_ioctl_opcode_is_the_one_the_kernel_expects() {
    // _IOC(_IOC_READ|_IOC_WRITE, 'H', 0x06, 64).
    assert_eq!(triton::hidiocsfeature(64), 0xC040_4806);
    assert_eq!(triton::hidiocsfeature(32), 0xC020_4806);
    assert_eq!(triton::hidiocsfeature(1) & 0xFFFF, 0x4806);
}

#[test]
#[should_panic(expected = "out of range")]
fn a_length_that_would_run_into_the_direction_bits_is_refused() {
    // 0x4000 overflows the fourteen-bit size field into bit 30, which is.
    let _ = triton::hidiocsfeature(0x4000);
}

#[test]
fn the_probe_is_gated_by_the_switch_that_protects_real_controllers() {
    let refused = triton::slots_where(true, |_| false);
    assert!(refused.is_empty(), "the probe ran anyway: {refused:?}");

    assert!(padmap_input::pad::wanted_by("Steam Controller", None));
    assert!(padmap_input::pad::wanted_by("Steam Controller", Some("")));
    assert!(padmap_input::pad::wanted_by(
        "Steam Controller",
        Some("Steam")
    ));
    assert!(!padmap_input::pad::wanted_by(
        "Steam Controller",
        Some("a name no pad has")
    ));
}

#[test]
fn the_lizard_mode_packet_is_the_six_bytes_sdl_sends() {
    let packet = triton::lizard_off_packet();
    assert_eq!(packet.len(), 64, "the feature report size");
    assert_eq!(
        &packet[..6],
        &[0x01, 0x87, 0x03, 0x09, 0x00, 0x00],
        "report id, ID_SET_SETTINGS_VALUES, one packed ControllerSetting, \
         SETTING_LIZARD_MODE, LIZARD_MODE_OFF as u16"
    );
    assert!(
        packet[6..].iter().all(|&byte| byte == 0),
        "the rest is zero"
    );
}

#[test]
fn the_packet_matches_the_python_byte_for_byte() {
    // Both are ports of the same six bytes.
    let python = std::process::Command::new("python3")
        .args([
            "-c",
            "import sys; sys.path.insert(0,'src'); from padmap import triton; \
             sys.stdout.write(triton.lizard_off_packet().hex())",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR").to_owned() + "/../../..")
        .output();
    let Ok(output) = python else {
        eprintln!("skipping: no python3");
        return;
    };
    if !output.status.success() {
        eprintln!("skipping: python could not import padmap.triton");
        return;
    }
    let theirs = String::from_utf8_lossy(&output.stdout);
    let ours = triton::lizard_off_packet()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        ours,
        theirs.trim(),
        "the two ports disagree about the packet"
    );
}

#[test]
fn the_state_reports_field_offsets_are_the_packed_struct() {
    let decoded = triton::decode_state(&state(0xDEAD_BEEF, 111, 222, 1111, -2222, 3333, -4444))
        .expect("a full-length payload decodes");
    assert_eq!(decoded.buttons, 0xDEAD_BEEF);
    assert_eq!(decoded.trigger_left, 111);
    assert_eq!(decoded.trigger_right, 222);
    assert_eq!(decoded.left_x, 1111);
    assert_eq!(decoded.left_y, -2222);
    assert_eq!(decoded.right_x, 3333);
    assert_eq!(decoded.right_y, -4444);
}

#[test]
fn a_short_payload_decodes_to_nothing_rather_than_panicking() {
    let full = state(0, 0, 0, 0, 0, 0, 0);
    for cut in 0..full.len() {
        let _ = triton::decode_state(&full[..cut]);
    }
    assert!(triton::decode_state(&[]).is_none());
    assert!(
        triton::decode_state(&full[..16]).is_none(),
        "one byte short"
    );
    assert!(
        triton::decode_state(&full[..17]).is_some(),
        "exactly enough"
    );
}

#[test]
fn opposite_dpad_directions_cancel() {
    const UP: u32 = 0x0000_2000;
    const DOWN: u32 = 0x0000_0400;
    const LEFT: u32 = 0x0000_1000;
    const RIGHT: u32 = 0x0000_0800;
    assert_eq!(triton::hat_for(0), (0, 0));
    assert_eq!(triton::hat_for(UP), (0, -1));
    assert_eq!(triton::hat_for(DOWN), (0, 1));
    assert_eq!(triton::hat_for(LEFT), (-1, 0));
    assert_eq!(triton::hat_for(RIGHT), (1, 0));
    assert_eq!(triton::hat_for(UP | RIGHT), (1, -1));
    assert_eq!(triton::hat_for(LEFT | RIGHT), (0, 0));
    assert_eq!(triton::hat_for(UP | DOWN), (0, 0));
}

#[test]
fn the_decode_agrees_with_the_python_port() {
    let cases: [(u32, i16, i16, i16, i16, i16, i16); 6] = [
        (0, 0, 0, 0, 0, 0, 0),
        (0x0000_0001, 0, 0, 0, 0, 0, 0),
        (0x0800_0000, 32767, 0, 0, 0, 0, 0),
        (0, 0, 0, 0, 20000, 0, 0),
        (0, 0, 0, 0, -32768, 0, 0),
        (0xFFFF_FFFF, 32767, 32767, -32768, -32768, 32767, 32767),
    ];
    for (buttons, tl, tr, lx, ly, rx, ry) in cases {
        let payload = state(buttons, tl, tr, lx, ly, rx, ry);
        let script = format!(
            "import sys; sys.path.insert(0,'src'); from padmap import triton; \
             got = triton.decode_state(bytes.fromhex('{}')); \
             sys.stdout.write(repr([got['buttons'], got['trigger_left'], \
             got['trigger_right'], got['left_x'], got['left_y'], \
             got['right_x'], got['right_y']]))",
            payload
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let Ok(output) = std::process::Command::new("python3")
            .args(["-c", &script])
            .current_dir(env!("CARGO_MANIFEST_DIR").to_owned() + "/../../..")
            .output()
        else {
            eprintln!("skipping: no python3");
            return;
        };
        if !output.status.success() {
            eprintln!("skipping: python could not decode");
            return;
        }
        let ours = triton::decode_state(&payload).expect("decodes");
        let theirs = String::from_utf8_lossy(&output.stdout);
        let mine = format!(
            "[{}, {}, {}, {}, {}, {}, {}]",
            ours.buttons,
            ours.trigger_left,
            ours.trigger_right,
            ours.left_x,
            ours.left_y,
            ours.right_x,
            ours.right_y
        );
        assert_eq!(mine, theirs.trim(), "buttons={buttons:#010x}");
    }
}

#[test]
fn only_valve_ids_on_a_hidraw_node_are_ours() {
    let ours = padmap_input::pad::Pad {
        path: "/dev/hidraw7".into(),
        name: "Steam Controller".into(),
        phys: String::new(),
        uniq: String::new(),
        vid: 0x28DE,
        pid: 0x1304,
        syspath: "/sys/x".into(),
        retroarch_visible: false,
        motion: None,
    };
    assert!(triton::owns(&ours));

    // An ordinary evdev pad, even from Valve, is the kernel's to drive.
    let evdev = padmap_input::pad::Pad {
        path: "/dev/input/event9".into(),
        ..ours.clone()
    };
    assert!(!triton::owns(&evdev));

    // Another vendor's hidraw device is nothing to do with this protocol.
    let other = padmap_input::pad::Pad {
        vid: 0x057E,
        pid: 0x2009,
        ..ours.clone()
    };
    assert!(!triton::owns(&other));
}

#[test]
fn scanning_a_real_machine_does_not_fail() {
    // Whatever is attached, this must answer rather than raise: it runs from.
    let all = triton::slots(false);
    let live = triton::slots(true);
    assert!(live.len() <= all.len(), "a live slot is one of the slots");
    for pad in &all {
        assert!(triton::owns(pad), "{} is not ours", pad.path.display());
        assert!(!pad.retroarch_visible, "nothing else can see a hidraw pad");
    }
    // Two slots of one receiver must be distinguishable, or assignment cannot.
    let mut uniqs: Vec<&String> = all.iter().map(|pad| &pad.uniq).collect();
    uniqs.sort();
    uniqs.dedup();
    assert_eq!(uniqs.len(), all.len(), "two slots share a uniq");
    let _ = triton::live_signature();
}

// -- the Source, fed real bytes ------------------------------------------

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use evdev::{AbsoluteAxisCode, EventType, InputEvent, KeyCode};

struct FakeHidraw {
    master: std::fs::File,
    slave: PathBuf,
}

impl FakeHidraw {
    fn open() -> Option<Self> {
        use rustix::pty::{grantpt, openpt, ptsname, unlockpt, OpenptFlags};
        use rustix::termios::{tcgetattr, tcsetattr, OptionalActions};

        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).ok()?;
        grantpt(&master).ok()?;
        unlockpt(&master).ok()?;
        let name = ptsname(&master, Vec::new()).ok()?;
        let mut attrs = tcgetattr(&master).ok()?;
        // Not an optimisation: a cooked tty line-buffers on \n and swallows.
        attrs.make_raw();
        tcsetattr(&master, OptionalActions::Now, &attrs).ok()?;
        Some(FakeHidraw {
            master: std::fs::File::from(master),
            slave: PathBuf::from(name.to_string_lossy().into_owned()),
        })
    }

    fn push(&mut self, id: u8, payload: &[u8]) {
        let mut report = vec![id];
        report.extend_from_slice(payload);
        self.master.write_all(&report).expect("write a report");
        std::thread::sleep(Duration::from_millis(2));
    }
}

macro_rules! needs_pty {
    () => {
        match FakeHidraw::open() {
            Some(fake) => fake,
            None => {
                eprintln!("skipping: no usable /dev/ptmx here");
                return;
            }
        }
    };
}

const REPORT_STATE: u8 = 0x42;
const REPORT_STATE_TIMESTAMP: u8 = 0x47;
const REPORT_WIRELESS: u8 = 0x79;
const WIRELESS_DISCONNECT: u8 = 1;
const BIT_A: u32 = 0x0000_0001;
const BIT_B: u32 = 0x0000_0002;

fn fetch(source: &mut triton::Source) -> std::io::Result<Vec<InputEvent>> {
    let mut out = Vec::new();
    source.fetch_events(&mut out)?;
    Ok(out)
}

fn keys(events: &[InputEvent]) -> Vec<(u16, i32)> {
    events
        .iter()
        .filter(|event| event.event_type() == EventType::KEY)
        .map(|event| (event.code(), event.value()))
        .collect()
}

#[test]
fn a_regular_file_is_refused_as_a_hidraw_node() {
    let path = std::env::temp_dir().join("padmap-triton-not-a-device");
    let _ = std::fs::remove_file(&path);
    std::fs::File::create(&path).expect("create a plain file");
    let error = triton::Source::open(&path).expect_err("a file is not a device");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_missing_node_is_an_error_not_a_panic() {
    let missing = PathBuf::from("/nonexistent-padmap-triton/hidraw0");
    assert!(triton::Source::open(&missing).is_err());
}

#[test]
fn nothing_written_yet_is_would_block() {
    let fake = needs_pty!();
    let mut source = triton::Source::open(fake.slave.as_path()).expect("open");
    let error = fetch(&mut source).expect_err("nothing to read");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    assert!(!source.connected(), "no report has proved a pad is paired");
}

#[test]
fn a_press_becomes_a_key_down_and_a_sync() {
    let mut fake = needs_pty!();
    let mut source = triton::Source::open(fake.slave.as_path()).expect("open");
    fake.push(REPORT_STATE, &state(BIT_A, 0, 0, 0, 0, 0, 0));

    let events = fetch(&mut source).expect("a report arrived");
    assert!(source.connected(), "a state report is proof of pairing");
    assert_eq!(keys(&events), vec![(KeyCode::BTN_SOUTH.code(), 1)]);
    assert_eq!(
        events.last().map(InputEvent::event_type),
        Some(EventType::SYNCHRONIZATION),
        "a frame ends with a sync, or half of it can be read as a state that \
         never happened"
    );
}

#[test]
fn an_unchanged_report_is_read_but_emits_nothing() {
    // Ok with an empty frame, *not* WouldBlock.
    let mut fake = needs_pty!();
    let mut source = triton::Source::open(fake.slave.as_path()).expect("open");
    fake.push(REPORT_STATE, &state(BIT_A, 0, 0, 0, 0, 0, 0));
    fetch(&mut source).expect("the press");

    fake.push(REPORT_STATE, &state(BIT_A, 0, 0, 0, 0, 0, 0));
    let events = fetch(&mut source).expect("read, not WouldBlock");
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn releasing_becomes_a_key_up() {
    let mut fake = needs_pty!();
    let mut source = triton::Source::open(fake.slave.as_path()).expect("open");
    fake.push(REPORT_STATE, &state(BIT_A, 0, 0, 0, 0, 0, 0));
    fetch(&mut source).expect("the press");
    assert_eq!(source.held_keys(), vec![KeyCode::BTN_SOUTH.code()]);

    fake.push(REPORT_STATE, &state(0, 0, 0, 0, 0, 0, 0));
    let events = fetch(&mut source).expect("the release");
    assert_eq!(keys(&events), vec![(KeyCode::BTN_SOUTH.code(), 0)]);
    assert!(source.held_keys().is_empty());
}

#[test]
fn a_disconnect_lifts_held_buttons_in_the_same_frame() {
    // A version that queued these for the next call would pass every other.
    let mut fake = needs_pty!();
    let mut source = triton::Source::open(fake.slave.as_path()).expect("open");
    fake.push(REPORT_STATE, &state(BIT_A | BIT_B, 0, 0, 0, 0, 0, 0));
    fetch(&mut source).expect("the press");
    assert!(source.connected());

    fake.push(REPORT_WIRELESS, &[WIRELESS_DISCONNECT]);
    let events = fetch(&mut source).expect("a disconnect is not an error");
    assert!(!source.connected());
    let ups = keys(&events);
    assert_eq!(ups.len(), 2, "{events:?}");
    assert!(ups.contains(&(KeyCode::BTN_SOUTH.code(), 0)));
    assert!(ups.contains(&(KeyCode::BTN_EAST.code(), 0)));
    assert!(source.held_keys().is_empty());
}

#[test]
fn only_the_hat_axis_that_moved_is_reported() {
    const UP: u32 = 0x0000_2000;
    const RIGHT: u32 = 0x0000_0800;
    let mut fake = needs_pty!();
    let mut source = triton::Source::open(fake.slave.as_path()).expect("open");

    fake.push(REPORT_STATE, &state(0, 0, 0, 0, 0, 0, 0));
    fetch(&mut source).expect("the resting frame");

    fake.push(REPORT_STATE, &state(UP | RIGHT, 0, 0, 0, 0, 0, 0));
    let first = fetch(&mut source).expect("a diagonal");
    let moved: Vec<u16> = first
        .iter()
        .filter(|event| event.event_type() == EventType::ABSOLUTE)
        .map(InputEvent::code)
        .collect();
    assert_eq!(moved.len(), 2, "a diagonal moves both axes: {moved:?}");

    fake.push(REPORT_STATE, &state(RIGHT, 0, 0, 0, 0, 0, 0));
    let second = fetch(&mut source).expect("letting go of up");
    let moved: Vec<(u16, i32)> = second
        .iter()
        .filter(|event| event.event_type() == EventType::ABSOLUTE)
        .map(|event| (event.code(), event.value()))
        .collect();
    assert_eq!(moved, vec![(AbsoluteAxisCode::ABS_HAT0Y.0, 0)]);
}

#[test]
fn an_unknown_report_id_is_read_and_ignored() {
    let mut fake = needs_pty!();
    let mut source = triton::Source::open(fake.slave.as_path()).expect("open");
    fake.push(0x99, &[1, 2, 3, 4]);
    let events = fetch(&mut source).expect("read, and ignored");
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn a_state_report_too_short_to_decode_does_not_panic() {
    let mut fake = needs_pty!();
    let mut source = triton::Source::open(fake.slave.as_path()).expect("open");
    fake.push(REPORT_STATE, &[0u8; 5]);
    let events = fetch(&mut source).expect("read without panicking");
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn capabilities_never_offer_a_zero_width_axis() {
    let fake = needs_pty!();
    let source = triton::Source::open(fake.slave.as_path()).expect("open");
    let (keys, axes) = source.capabilities();
    assert!(!keys.is_empty());
    assert_eq!(axes.len(), 8, "{axes:?}");
    for (code, info) in &axes {
        assert!(info.maximum() > info.minimum(), "axis {code}: {info:?}");
    }
}

#[test]
fn a_press_on_a_puck_claims_a_seat() {
    use padmap_core::assign::{Assigner, HOLD_SECONDS};

    let mut fake = needs_pty!();
    let mut source = triton::Source::open(&fake.slave).expect("open the pty as a source");

    // Nothing held: the resting report must claim nothing, or a pad sitting on.
    fake.push(REPORT_STATE, &state(0, 0, 0, 0, 0, 0, 0));
    let mut assigner = Assigner::default();
    let mut now = 0.0;
    for event in fetch(&mut source).expect("read") {
        assigner.feed(0, event.event_type().0, event.code(), event.value(), now);
    }
    now += HOLD_SECONDS * 2.0;
    assert!(
        assigner.tick(now).claimed.is_empty(),
        "an untouched pad claimed a seat"
    );

    fake.push(REPORT_STATE, &state(BIT_A, 0, 0, 0, 0, 0, 0));
    let pressed = fetch(&mut source).expect("read");
    assert!(
        pressed
            .iter()
            .any(|event| event.code() == KeyCode::BTN_SOUTH.0 && event.value() == 1),
        "A did not decode to a press: {:?}",
        keys(&pressed)
    );
    for event in pressed {
        assigner.feed(0, event.event_type().0, event.code(), event.value(), now);
    }
    // Not yet: a tap must not claim.
    assert!(assigner.tick(now + HOLD_SECONDS / 2.0).claimed.is_empty());

    let claimed = assigner.tick(now + HOLD_SECONDS * 1.5).claimed;
    assert_eq!(claimed.len(), 1, "holding A claimed no seat");
    assert_eq!(claimed[0].player, 1);
    assert_eq!(claimed[0].button, KeyCode::BTN_SOUTH.0);
}

fn state_imu(accel: [i16; 3], gyro: [i16; 3], tick: u32, wide: bool) -> Vec<u8> {
    let mut out = state(0, 0, 0, 0, 0, 0, 0);
    if wide {
        out[29..33].copy_from_slice(&tick.to_le_bytes());
    } else {
        out[31..33].copy_from_slice(&(tick as u16).to_le_bytes());
    }
    for (index, value) in accel.iter().chain(gyro.iter()).enumerate() {
        let at = 33 + index * 2;
        out[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    out
}

#[test]
fn the_two_report_shapes_put_the_imu_on_the_same_byte() {
    let common = 1 + 4 + 2 + 2 + 2 * 4; // seq, buttons, triggers, sticks
    let pads = (2 + 2 + 2) * 2; // x, y, pressure, twice
    let no_quat = common + pads + 4; // uint32_t imu timestamp
    let timestamped = common + 2 + pads + 2; // trackpad timestamp, uint16_t imu
    assert_eq!(no_quat, 33);
    assert_eq!(timestamped, 33);
}

#[test]
fn a_gyro_at_half_scale_is_a_thousand_degrees_a_second() {
    let payload = state_imu([0; 3], [16384, 0, 0], 0, true);
    let motion = triton::decode_motion(&payload, true).expect("an IMU block");
    assert!((motion.gyro[0] - 1000.0).abs() < 0.1, "{:?}", motion.gyro);
    assert_eq!(motion.gyro[1], 0.0);
    assert_eq!(motion.gyro[2], 0.0);
}

#[test]
fn the_controllers_own_axes_land_where_dsu_expects_them() {
    let flat = state_imu([0, 0, 16384], [0; 3], 0, true);
    let motion = triton::decode_motion(&flat, true).expect("an IMU block");
    assert!((motion.accel[1] + 1.0).abs() < 0.01, "{:?}", motion.accel);
    let turning = state_imu([0; 3], [0, 16384, 0], 0, true);
    let motion = triton::decode_motion(&turning, true).expect("an IMU block");
    assert!((motion.gyro[2] - 1000.0).abs() < 0.1, "{:?}", motion.gyro);
    let turning = state_imu([0; 3], [0, 0, 16384], 0, true);
    let motion = triton::decode_motion(&turning, true).expect("an IMU block");
    assert!((motion.gyro[1] + 1000.0).abs() < 0.1, "{:?}", motion.gyro);
}

#[test]
fn the_short_timestamp_counts_in_thirty_two_microsecond_steps() {
    let payload = state_imu([0; 3], [0; 3], 100, false);
    let motion = triton::decode_motion(&payload, false).expect("an IMU block");
    assert_eq!(motion.timestamp_us, 3200);
}

#[test]
fn a_report_too_short_for_an_imu_block_is_not_one() {
    assert!(triton::decode_motion(&[0u8; 44], true).is_none());
    assert!(triton::decode_motion(&[], true).is_none());
    assert!(triton::decode_motion(&[0u8; 45], true).is_some());
}

#[test]
fn the_published_clock_only_goes_forwards_across_a_wrap() {
    // The short counter wraps every 2.1 seconds.
    let mut fake = needs_pty!();
    let mut source = triton::Source::open(fake.slave.as_path()).expect("open");
    assert_eq!(source.motion(), None, "nothing read yet");

    fake.push(
        REPORT_STATE_TIMESTAMP,
        &state_imu([0; 3], [0; 3], 65_000, false),
    );
    let _ = fetch(&mut source);
    let first = source.motion().expect("a sample").timestamp_us;

    // 65_000 -> 1_000 is a wrap, not a jump backwards of two seconds.
    fake.push(
        REPORT_STATE_TIMESTAMP,
        &state_imu([0; 3], [1, 0, 0], 1_000, false),
    );
    let _ = fetch(&mut source);
    let second = source.motion().expect("a second sample").timestamp_us;
    assert!(second > first, "{second} came after {first}");
    assert_eq!(second - first, (65_536 - 65_000 + 1_000) * 32);
}
