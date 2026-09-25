//! The parts of danstick that only exist when there is a device.

use std::collections::BTreeMap;
use std::time::Duration;

use danstick_input::{clone, pad, republish};
use evdev::{
    uinput::VirtualDevice, AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, Device, EventType,
    InputEvent, InputId, KeyCode, UinputAbsSetup,
};

const NAME: &str = "danstick test X-Box 360 pad";
const VID: u16 = 0x045E;
const PID: u16 = 0x028E;

fn uinput_available() -> bool {
    // Writable, not merely present: in a Nix build /dev/uinput may exist and.
    std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .is_ok()
}

macro_rules! needs_uinput {
    () => {
        if !uinput_available() {
            eprintln!("skipping: /dev/uinput is not writable");
            return;
        }
    };
}

/// A source device the kernel will publish as a joypad.
fn spawn_source() -> VirtualDevice {
    let mut keys = AttributeSet::<KeyCode>::new();
    for key in [
        KeyCode::BTN_SOUTH,
        KeyCode::BTN_EAST,
        KeyCode::BTN_NORTH,
        KeyCode::BTN_WEST,
        KeyCode::BTN_TL,
        KeyCode::BTN_TR,
        KeyCode::BTN_SELECT,
        KeyCode::BTN_START,
        KeyCode::BTN_MODE,
        KeyCode::BTN_THUMBL,
        KeyCode::BTN_THUMBR,
    ] {
        keys.insert(key);
    }
    let stick = AbsInfo::new(0, -32768, 32767, 16, 128, 0);
    let trigger = AbsInfo::new(0, 0, 255, 0, 0, 0);
    let mut builder = VirtualDevice::builder()
        .expect("open /dev/uinput")
        .name(NAME)
        .input_id(InputId::new(BusType::BUS_USB, VID, PID, 0x0110))
        .with_keys(&keys)
        .expect("keys");
    for axis in [
        AbsoluteAxisCode::ABS_X,
        AbsoluteAxisCode::ABS_Y,
        AbsoluteAxisCode::ABS_RX,
        AbsoluteAxisCode::ABS_RY,
    ] {
        builder = builder
            .with_absolute_axis(&UinputAbsSetup::new(axis, stick))
            .expect("stick");
    }
    for axis in [AbsoluteAxisCode::ABS_Z, AbsoluteAxisCode::ABS_RZ] {
        builder = builder
            .with_absolute_axis(&UinputAbsSetup::new(axis, trigger))
            .expect("trigger");
    }
    let device = builder.build().expect("build source");
    std::thread::sleep(Duration::from_millis(500));
    device
}

fn clone_of(found: &pad::Pad, player: u32) -> clone::VirtualPad {
    let axes: BTreeMap<u16, danstick_core::calibration::AxisCalibration> = BTreeMap::new();
    let tuning = danstick_core::tuning::Tuning::default();
    let mut virtual_pad = clone::create(
        found,
        player,
        clone::IdentityMode::Mirror,
        &axes,
        tuning,
        false,
        &danstick_core::profile::Mapping::default(),
    )
    .expect("create a clone");
    virtual_pad
        .source
        .set_nonblocking(true)
        .expect("set the source non-blocking");
    virtual_pad
}

fn find(source: &mut VirtualDevice) -> Option<pad::Pad> {
    let wanted: Vec<String> = source
        .enumerate_dev_nodes_blocking()
        .ok()?
        .filter_map(|node| node.ok())
        .map(|node| node.display().to_string())
        .collect();
    pad::discover(pad::Filter::default())
        .ok()?
        .into_iter()
        .find(|found| {
            wanted
                .iter()
                .any(|node| *node == found.path.display().to_string())
        })
}

#[test]
fn a_spawned_pad_is_discovered_with_the_identity_it_declared() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover() finds a joypad it just gained");
    assert_eq!(found.name, NAME);
    assert_eq!((found.vid, found.pid), (VID, PID));
    assert!(!found.event().is_empty());
    // It is a real pad, not one of danstick's own clones -- which discover().
    assert!(!pad::is_danstick_clone(&found.name, &found.phys));
}

#[test]
fn capabilities_are_read_back_with_their_ranges() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let device = Device::open(&found.path).expect("open the source");
    let (keys, axes) = clone::capabilities(&device);
    assert!(
        keys.contains(&KeyCode::BTN_SOUTH.code()),
        "the buttons it declared: {keys:?}"
    );
    assert!(
        axes.contains(&AbsoluteAxisCode::ABS_X.0),
        "the axes it declared: {axes:?}"
    );
}

#[test]
fn a_clone_mirrors_the_source_and_carries_dansticks_own_name() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let virtual_pad = clone_of(&found, 3);
    assert_eq!(virtual_pad.name(), clone::virtual_name(3));
    assert!(virtual_pad.name().contains('3'), "{}", virtual_pad.name());

    let identity = clone::Identity::for_source(
        clone::IdentityMode::Mirror,
        &Device::open(&found.path).expect("open"),
        3,
    );
    assert_eq!((identity.vendor, identity.product), (VID, PID));

    let danstick_identity = clone::Identity::for_source(
        clone::IdentityMode::Danstick,
        &Device::open(&found.path).expect("open"),
        3,
    );
    assert_ne!(
        (danstick_identity.vendor, danstick_identity.product),
        (VID, PID),
        "danstick identity must not mirror, or DANSTICK_ONLY_VIRTUAL hides everything"
    );
}

#[test]
fn a_press_on_the_source_arrives_on_the_clone() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let mut republisher = republish::Republisher::new(vec![clone_of(&found, 1)]);

    // Drain whatever the kernel queued while the clone was being built, so.
    let _ = republisher.forward(0);

    source
        .emit(&[
            InputEvent::new(EventType::KEY.0, KeyCode::BTN_SOUTH.code(), 1),
            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        ])
        .expect("emit a press");

    let mut events = 0;
    for _ in 0..50 {
        let pumped = republisher.forward(0);
        events += pumped.events;
        assert!(!pumped.gone, "the source vanished mid-test");
        if events > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(events > 0, "nothing was forwarded in a second");
    republisher.close();
}

#[test]
fn a_paused_republisher_forwards_nothing_but_still_drains() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let mut republisher = republish::Republisher::new(vec![clone_of(&found, 1)]);
    let _ = republisher.forward(0);

    assert!(!republisher.paused());
    republisher.set_paused(true);
    assert!(republisher.paused());

    source
        .emit(&[
            InputEvent::new(EventType::KEY.0, KeyCode::BTN_SOUTH.code(), 1),
            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        ])
        .expect("emit");
    std::thread::sleep(Duration::from_millis(50));

    let pumped = republisher.forward(0);
    assert!(!pumped.gone);

    republisher.set_paused(false);
    assert!(!republisher.paused());
    assert!(republisher.dead().is_empty());
    republisher.close();
}

#[test]
fn a_source_that_goes_away_is_reported_dead_rather_than_spinning() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let mut republisher = republish::Republisher::new(vec![clone_of(&found, 1)]);
    let _ = republisher.forward(0);

    drop(source);
    std::thread::sleep(Duration::from_millis(300));

    let mut gone = false;
    for _ in 0..50 {
        if republisher.forward(0).gone {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(gone, "the clone never noticed its source had gone");
    assert_eq!(republisher.dead(), vec![0]);
    republisher.close();
}

#[test]
fn discovery_survives_a_machine_with_no_pads_at_all() {
    let pads = pad::discover(pad::Filter::default()).expect("discover must not error");
    for found in &pads {
        assert!(!found.name.is_empty() || !found.event().is_empty());
    }
    let _ = pad::ambiguous_groups(&pads);
}

#[test]
fn undriven_controllers_are_included_by_default_but_never_for_retroarch() {
    // `Filter`'s Default is written out rather than derived because one field.
    let default = pad::Filter::default();
    assert!(!default.include_virtual);
    assert!(!default.retroarch_only);
    assert!(
        default.include_undriven,
        "a controller danstick drives itself must be findable without an env var"
    );

    // RetroArch cannot see a device with no evdev node, and counting one would.
    let filtered = pad::discover(pad::Filter {
        include_virtual: false,
        retroarch_only: true,
        include_undriven: true,
    })
    .expect("discover must not error");
    for found in &filtered {
        assert!(
            !danstick_input::triton::owns(found),
            "{} is driven by danstick but retroarch_only asked to exclude it",
            found.path.display()
        );
    }
}

#[test]
fn cemu_profiles_are_written_per_player_and_stop_at_cemus_limit() {
    use danstick_input::artefacts;

    let dir = std::env::temp_dir().join(format!("danstick-cemu-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Nine players: Cemu has eight slots, and a ninth must get no profile.
    let players: Vec<u32> = (1..=9).collect();
    let written = artefacts::write_cemu_profiles(
        &players,
        None,
        |player| format!("{player:032x}"),
        |player| format!("danstick Player {player}"),
        Some(&dir),
    )
    .expect("writes");

    assert_eq!(written.len(), 8, "{written:?}");
    assert!(written[0].ends_with("controller0.xml"));
    assert!(written[7].ends_with("controller7.xml"));
    assert!(!dir.join("controller8.xml").exists());

    let first = std::fs::read_to_string(&written[0]).expect("read back");
    assert!(
        first.contains("<uuid>0_00000000000000000000000000000001</uuid>"),
        "{first}"
    );
    assert!(first.contains("<display_name>danstick Player 1</display_name>"));

    std::fs::write(dir.join("controller5.xml"), "mine").expect("write");
    artefacts::write_cemu_profiles(
        &[1],
        None,
        |_| "g".to_owned(),
        |_| "n".to_owned(),
        Some(&dir),
    )
    .expect("writes");
    assert_eq!(
        std::fs::read_to_string(dir.join("controller5.xml")).expect("read"),
        "mine",
        "an unmanaged player's profile is theirs, not danstick's to clear"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rewriting_ares_settings_touches_only_the_ports_danstick_manages() {
    use danstick_input::artefacts;
    use std::collections::BTreeMap;

    let existing = "Video\n  Driver: OpenGL 3.2\n  Shader: None\n\
                    VirtualPad1\n  A..South: ;;\n  Start: ;;\n\
                    VirtualMouse1\n  X: stale;;\n\
                    VirtualPad2\n  A..South: old;;\n\
                    VirtualMouse2\n  X: theirs;;\n\
                    Audio\n  Driver: SDL\n";
    let mut blocks = BTreeMap::new();
    blocks.insert(
        1u32,
        "VirtualPad1\n  A..South: new;;\nVirtualMouse1\n  X: 0x2/0/0;;\n".to_owned(),
    );

    let out = artefacts::rewrite_ares_settings(existing, &blocks);

    assert!(out.contains("Video\n  Driver: OpenGL 3.2"), "{out}");
    assert!(out.contains("Audio\n  Driver: SDL"), "{out}");
    assert!(
        out.contains("VirtualMouse1\n  X: 0x2/0/0;;"),
        "the managed port's mouse was not replaced: {out}"
    );
    assert!(
        !out.contains("stale"),
        "a mouse danstick manages survived from last time: {out}"
    );
    assert!(
        out.contains("VirtualMouse2\n  X: theirs;;"),
        "an unmanaged port's mouse is not ours: {out}"
    );
    assert!(out.contains("A..South: new;;"), "player 1 was not replaced");
    assert!(
        out.contains("VirtualPad2\n  A..South: old;;"),
        "an unmanaged port kept whatever was there: {out}"
    );
    assert!(
        !out.contains("A..South: ;;"),
        "the old player 1 survived: {out}"
    );
}

#[test]
fn rewriting_ares_settings_survives_ordering_and_missing_sections() {
    use danstick_input::artefacts::rewrite_ares_settings;
    use std::collections::BTreeMap;

    struct Case {
        name: &'static str,
        existing: &'static str,
        blocks: &'static [(u32, &'static str)],
        must_contain: &'static [&'static str],
        must_not_contain: &'static [&'static str],
    }

    const NEW1: &str = "VirtualPad1\n  A..South: new1;;\nVirtualMouse1\n  X: 0x2/0/0;;\n";
    const NEW2: &str = "VirtualPad2\n  A..South: new2;;\nVirtualMouse2\n  X: 0x2/0/1;;\n";

    let cases = [
        Case {
            name: "the mouse block comes before the pad block in the file",
            existing: "Video\n  Driver: OpenGL\n\
                       VirtualMouse1\n  X: stale;;\n\
                       VirtualPad1\n  A..South: old;;\n\
                       Audio\n  Driver: SDL\n",
            blocks: &[(1, NEW1)],
            must_contain: &[
                "Video\n  Driver: OpenGL",
                "Audio\n  Driver: SDL",
                "A..South: new1;;",
                "VirtualMouse1\n  X: 0x2/0/0;;",
            ],
            must_not_contain: &["stale", "A..South: old;;"],
        },
        Case {
            name: "the file has a pad block but ares never wrote it a mouse block",
            existing: "Video\n  Driver: OpenGL\n\
                       VirtualPad1\n  A..South: old;;\n\
                       Audio\n  Driver: SDL\n",
            blocks: &[(1, NEW1)],
            must_contain: &[
                "Video\n  Driver: OpenGL",
                "Audio\n  Driver: SDL",
                "A..South: new1;;",
                "VirtualMouse1\n  X: 0x2/0/0;;",
            ],
            must_not_contain: &["A..South: old;;"],
        },
        Case {
            name: "the managed player is entirely absent from a fresh file",
            existing: "Video\n  Driver: OpenGL\nAudio\n  Driver: SDL\n",
            blocks: &[(1, NEW1)],
            must_contain: &["Video\n  Driver: OpenGL", "Audio\n  Driver: SDL", NEW1],
            must_not_contain: &[],
        },
        Case {
            name: "the file has no trailing newline before an appended block",
            existing: "Video\n  Driver: OpenGL",
            blocks: &[(1, NEW1)],
            must_contain: &["Video\n  Driver: OpenGL\nVirtualPad1"],
            must_not_contain: &["OpenGLVirtualPad1"],
        },
        Case {
            name: "two managed players interleaved by type rather than by port",
            existing: "VirtualPad1\n  A..South: old1;;\nVirtualPad2\n  A..South: old2;;\n\
                       VirtualMouse1\n  X: stale1;;\nVirtualMouse2\n  X: stale2;;\n",
            blocks: &[(1, NEW1), (2, NEW2)],
            must_contain: &["new1", "new2", "0x2/0/0", "0x2/0/1"],
            must_not_contain: &["old1", "old2", "stale1", "stale2"],
        },
    ];

    for case in cases {
        let blocks: BTreeMap<u32, String> = case
            .blocks
            .iter()
            .map(|(player, text)| (*player, (*text).to_owned()))
            .collect();
        let out = rewrite_ares_settings(case.existing, &blocks);
        for expected in case.must_contain {
            assert!(
                out.contains(expected),
                "[{}] missing {expected:?}:\n{out}",
                case.name
            );
        }
        for forbidden in case.must_not_contain {
            assert!(
                !out.contains(forbidden),
                "[{}] should not contain {forbidden:?}:\n{out}",
                case.name
            );
        }
    }
}

#[test]
fn rewriting_ryujinx_config_keeps_every_other_setting() {
    use danstick_input::artefacts;

    let dir = std::env::temp_dir().join(format!("danstick-ryujinx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("Config.json");
    std::fs::write(
        &path,
        r#"{"version": 70, "res_scale": 2, "input_config": [
             {"backend": "WindowKeyboard", "player_index": "Player1", "name": "Keyboard"}
           ]}"#,
    )
    .expect("write");

    let entry = danstick_core::ryujinx::input_config(
        1,
        "0600c9a7091200000100000001000000",
        "danstick Player 1",
        0,
    )
    .expect("an entry");
    artefacts::write_ryujinx_config(vec![entry], None, Some(&path)).expect("writes");

    let back: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert_eq!(back["version"], 70);
    assert_eq!(back["res_scale"], 2);
    let entries = back["input_config"].as_array().expect("an array");
    assert_eq!(
        entries.len(),
        2,
        "the pad took Player1 and the keyboard moved"
    );
    assert_eq!(entries[0]["name"], "danstick Player 1");
    assert_eq!(entries[0]["player_index"], "Player1");
    assert_eq!(
        entries[1]["name"], "Keyboard",
        "the user's own keyboard entry survived"
    );
    assert_eq!(entries[1]["player_index"], "Player2");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn neither_writer_invents_a_config_that_was_never_there() {
    use danstick_input::artefacts;
    use std::collections::BTreeMap;

    // Writing one from nothing would leave the emulator with danstick's ports.
    let missing = std::path::Path::new("/nonexistent-danstick-emulator/settings.bml");
    assert!(artefacts::write_ares_settings(&BTreeMap::new(), Some(missing)).is_err());
    assert!(artefacts::write_ryujinx_config(Vec::new(), None, Some(missing)).is_err());
}

#[test]
fn a_press_on_the_source_shows_in_the_dsu_picture() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let mut republisher = republish::Republisher::new(vec![clone_of(&found, 1)]);
    let _ = republisher.forward(0);

    source
        .emit(&[
            InputEvent::new(EventType::KEY.0, KeyCode::BTN_SOUTH.code(), 1),
            InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, -32768),
            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        ])
        .expect("emit");
    let mut seen = false;
    for _ in 0..50 {
        let _ = republisher.forward(0);
        let pad = republisher.pads[0].tracker.pad();
        if pad.buttons & danstick_core::dsu::button::CROSS != 0 {
            assert_eq!(pad.left_y, 255, "stick up is 255 in DSU");
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(seen, "the press never reached the DSU picture");
    republisher.close();
}

#[test]
fn pausing_releases_the_dsu_picture_as_well_as_the_clone() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let mut republisher = republish::Republisher::new(vec![clone_of(&found, 1)]);
    let _ = republisher.forward(0);

    source
        .emit(&[
            InputEvent::new(EventType::KEY.0, KeyCode::BTN_EAST.code(), 1),
            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        ])
        .expect("emit");
    let mut held = false;
    for _ in 0..50 {
        let _ = republisher.forward(0);
        if republisher.pads[0].tracker.pad().buttons & danstick_core::dsu::button::CIRCLE != 0 {
            held = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(held, "the press never arrived");

    republisher.set_paused(true);
    assert_eq!(
        republisher.pads[0].tracker.pad().buttons,
        0,
        "pausing left a button held in the DSU picture"
    );

    source
        .emit(&[
            InputEvent::new(EventType::KEY.0, KeyCode::BTN_SOUTH.code(), 1),
            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        ])
        .expect("emit");
    std::thread::sleep(Duration::from_millis(50));
    let _ = republisher.forward(0);
    assert_eq!(republisher.pads[0].tracker.pad().buttons, 0);
    republisher.set_paused(false);
    republisher.close();
}

/// A motion sensor the kernel will publish, declaring its own units.
fn spawn_imu(counts_per_g: i32, counts_per_dps: i32) -> VirtualDevice {
    let accel = AbsInfo::new(0, -32768, 32767, 0, 0, counts_per_g);
    let gyro = AbsInfo::new(0, -32768, 32767, 0, 0, counts_per_dps);
    let mut builder = VirtualDevice::builder()
        .expect("open /dev/uinput")
        .name("danstick test IMU")
        .input_id(InputId::new(BusType::BUS_USB, VID, PID, 0x0110));
    for axis in [
        AbsoluteAxisCode::ABS_X,
        AbsoluteAxisCode::ABS_Y,
        AbsoluteAxisCode::ABS_Z,
    ] {
        builder = builder
            .with_absolute_axis(&UinputAbsSetup::new(axis, accel))
            .expect("accel");
    }
    for axis in [
        AbsoluteAxisCode::ABS_RX,
        AbsoluteAxisCode::ABS_RY,
        AbsoluteAxisCode::ABS_RZ,
    ] {
        builder = builder
            .with_absolute_axis(&UinputAbsSetup::new(axis, gyro))
            .expect("gyro");
    }
    let device = builder.build().expect("build imu");
    std::thread::sleep(Duration::from_millis(300));
    device
}

fn node_of(device: &mut VirtualDevice) -> std::path::PathBuf {
    device
        .enumerate_dev_nodes_blocking()
        .expect("nodes")
        .filter_map(|node| node.ok())
        .next()
        .expect("a node")
}

#[test]
fn a_kernel_imu_is_scaled_by_the_resolution_it_declares() {
    needs_uinput!();
    let mut imu = spawn_imu(8192, 1024);
    let node = node_of(&mut imu);
    let mut sensor = danstick_input::motion::Sensor::open(&node).expect("open the imu");
    assert!(
        sensor.motion().is_none(),
        "nothing read yet is nothing to publish"
    );

    imu.emit(&[
        InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, 0),
        InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, 8192),
        InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Z.0, 0),
        InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_RX.0, 1024),
        InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_RY.0, 2048),
        InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_RZ.0, -512),
        InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
    ])
    .expect("emit a sample");
    let mut sample = None;
    for _ in 0..50 {
        assert!(sensor.read().expect("read"), "the imu vanished");
        if let Some(motion) = sensor.motion() {
            sample = Some(motion);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let sample = sample.expect("a sample within a second");
    assert_eq!(sample.accel, [0.0, -1.0, 0.0], "one g up, in DSU's frame");
    assert_eq!(
        sample.gyro,
        [1.0, -2.0, 0.5],
        "pitch kept, yaw and roll flipped"
    );
    assert!(sample.timestamp_us > 0, "stamped from the event, not zero");
}

#[test]
fn an_imu_that_vanishes_is_reported_gone_not_an_error() {
    needs_uinput!();
    let mut imu = spawn_imu(1, 1);
    let node = node_of(&mut imu);
    let mut sensor = danstick_input::motion::Sensor::open(&node).expect("open the imu");
    drop(imu);
    std::thread::sleep(Duration::from_millis(300));
    let mut gone = false;
    for _ in 0..50 {
        match sensor.read() {
            Ok(true) => std::thread::sleep(Duration::from_millis(20)),
            Ok(false) => {
                gone = true;
                break;
            }
            Err(error) => panic!("an error rather than a clean 'gone': {error}"),
        }
    }
    assert!(gone, "the sensor never noticed its device had gone");
}

/// A clone of `found` built with `tuning`, and its own node opened for.
fn tuned_clone_of(
    found: &pad::Pad,
    tuning: danstick_core::tuning::Tuning,
) -> (clone::VirtualPad, Device) {
    let axes: BTreeMap<u16, danstick_core::calibration::AxisCalibration> = BTreeMap::new();
    let mut virtual_pad = clone::create(
        found,
        1,
        clone::IdentityMode::Mirror,
        &axes,
        tuning,
        false,
        &danstick_core::profile::Mapping::default(),
    )
    .expect("create a clone");
    virtual_pad
        .source
        .set_nonblocking(true)
        .expect("set the source non-blocking");
    let node = virtual_pad.node().expect("the clone's node");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let reader = loop {
        match Device::open(&node) {
            Ok(device) => break device,
            Err(error) if std::time::Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => panic!("open the clone {node}: {error}"),
        }
    };
    reader.set_nonblocking(true).expect("non-blocking");
    (virtual_pad, reader)
}

fn drain_clone(reader: &mut Device) -> Vec<(u16, u16, i32)> {
    let mut out = Vec::new();
    for _ in 0..20 {
        match reader.fetch_events() {
            Ok(events) => out.extend(
                events
                    .filter(|e| e.event_type() != EventType::SYNCHRONIZATION)
                    .map(|e| (e.event_type().0, e.code(), e.value())),
            ),
            Err(_) => break,
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    out
}

#[test]
fn a_bouncing_button_reaches_the_clone_as_one_press() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let tuning = danstick_core::tuning::Tuning {
        debounce_ms: 40,
        ..danstick_core::tuning::Tuning::default()
    };
    let (vpad, mut reader) = tuned_clone_of(&found, tuning);
    let mut republisher = republish::Republisher::new(vec![vpad]);
    let _ = republisher.forward(0);
    let _ = drain_clone(&mut reader);

    let key = KeyCode::BTN_SOUTH.code();
    let press = |source: &mut VirtualDevice, value: i32| {
        source
            .emit(&[
                InputEvent::new(EventType::KEY.0, key, value),
                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
            ])
            .expect("emit");
    };
    press(&mut source, 1);
    std::thread::sleep(Duration::from_millis(20));
    let _ = republisher.forward(0);
    press(&mut source, 0);
    std::thread::sleep(Duration::from_millis(5));
    press(&mut source, 1);
    std::thread::sleep(Duration::from_millis(20));
    let _ = republisher.forward(0);
    republisher.flush_debounce();
    let seen = drain_clone(&mut reader);
    assert_eq!(
        seen,
        vec![(EventType::KEY.0, key, 1)],
        "one press, no bounce"
    );

    std::thread::sleep(Duration::from_millis(80));
    republisher.flush_debounce();
    assert!(drain_clone(&mut reader).is_empty());

    press(&mut source, 0);
    std::thread::sleep(Duration::from_millis(20));
    let _ = republisher.forward(0);
    republisher.flush_debounce();
    assert!(
        drain_clone(&mut reader).is_empty(),
        "held back, not delivered yet"
    );
    std::thread::sleep(Duration::from_millis(60));
    republisher.flush_debounce();
    assert_eq!(drain_clone(&mut reader), vec![(EventType::KEY.0, key, 0)]);
    republisher.close();
}

#[test]
fn a_deadzone_flattens_drift_and_keeps_the_ends() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let tuning = danstick_core::tuning::Tuning {
        deadzone: BTreeMap::from([(AbsoluteAxisCode::ABS_X.0, 0.2)]),
        ..danstick_core::tuning::Tuning::default()
    };
    let (vpad, mut reader) = tuned_clone_of(&found, tuning);
    let mut republisher = republish::Republisher::new(vec![vpad]);
    let _ = republisher.forward(0);
    let _ = drain_clone(&mut reader);

    let mut send = |value: i32| {
        source
            .emit(&[
                InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, value),
                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
            ])
            .expect("emit");
        std::thread::sleep(Duration::from_millis(20));
        let _ = republisher.forward(0);
        drain_clone(&mut reader)
    };
    assert_eq!(send(32767), vec![(EventType::ABSOLUTE.0, 0, 32767)]);
    // Drift inside the band reads as centred.
    assert_eq!(send(3000), vec![(EventType::ABSOLUTE.0, 0, 0)]);
    assert!(send(-5000).is_empty(), "drift inside the band is silence");
    source
        .emit(&[
            InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, 3000),
            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        ])
        .expect("emit");
    std::thread::sleep(Duration::from_millis(20));
    let _ = republisher.forward(0);
    assert_eq!(
        drain_clone(&mut reader),
        vec![(EventType::ABSOLUTE.0, 1, 3000)]
    );
    republisher.close();
}

#[test]
fn an_ignored_button_and_axis_never_reach_the_clone() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let tuning = danstick_core::tuning::Tuning {
        ignore_axes: [AbsoluteAxisCode::ABS_Z.0].into_iter().collect(),
        ignore_buttons: [KeyCode::BTN_EAST.code()].into_iter().collect(),
        ..danstick_core::tuning::Tuning::default()
    };
    let (vpad, mut reader) = tuned_clone_of(&found, tuning);
    let mut republisher = republish::Republisher::new(vec![vpad]);
    let _ = republisher.forward(0);
    let _ = drain_clone(&mut reader);

    source
        .emit(&[
            InputEvent::new(EventType::KEY.0, KeyCode::BTN_EAST.code(), 1),
            InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Z.0, 200),
            InputEvent::new(EventType::KEY.0, KeyCode::BTN_SOUTH.code(), 1),
            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        ])
        .expect("emit");
    std::thread::sleep(Duration::from_millis(20));
    let _ = republisher.forward(0);
    assert_eq!(
        drain_clone(&mut reader),
        vec![(EventType::KEY.0, KeyCode::BTN_SOUTH.code(), 1)],
        "only the button that is not ignored"
    );
    assert_eq!(
        republisher.pads[0].tracker.pad().buttons,
        danstick_core::dsu::button::CROSS,
        "and the DSU picture agrees"
    );
    republisher.close();
}

#[test]
fn holding_one_pad_back_leaves_the_others_forwarding() {
    needs_uinput!();
    let mut source_a = spawn_source();
    let found_a = find(&mut source_a).expect("discover a");
    let mut source_b = spawn_source();
    let found_b = find(&mut source_b).expect("discover b");
    assert_ne!(found_a.path, found_b.path, "two distinct source pads");

    let mut republisher =
        republish::Republisher::new(vec![clone_of(&found_a, 1), clone_of(&found_b, 2)]);
    let _ = republisher.forward(0);
    let _ = republisher.forward(1);

    // Hold pad 0 back, as a session-less rebind of player 1 would.
    republisher.hold_back(0, true);
    assert!(republisher.held_back(0));
    assert!(!republisher.held_back(1));
    assert!(
        !republisher.paused(),
        "only one pad is held, not the whole stream"
    );

    for source in [&mut source_a, &mut source_b] {
        source
            .emit(&[
                InputEvent::new(EventType::KEY.0, KeyCode::BTN_SOUTH.code(), 1),
                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
            ])
            .expect("emit");
    }
    std::thread::sleep(Duration::from_millis(60));

    let held = republisher.forward(0);
    let playing = republisher.forward(1);
    assert_eq!(
        held.frames, 0,
        "the held-back pad forwarded a press to its clone"
    );
    assert!(
        playing.frames > 0,
        "the other pad stopped forwarding while its neighbour was rebound"
    );

    // Letting go resumes forwarding for the held pad.
    republisher.hold_back(0, false);
    assert!(!republisher.held_back(0));
    source_a
        .emit(&[
            InputEvent::new(EventType::KEY.0, KeyCode::BTN_EAST.code(), 1),
            InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
        ])
        .expect("emit");
    std::thread::sleep(Duration::from_millis(60));
    assert!(
        republisher.forward(0).frames > 0,
        "the pad never resumed forwarding after the rebind ended"
    );
    republisher.close();
}

/// Under the 360 identity the clone is a wired Xbox 360 pad whatever is behind
/// it: xpad's id, xpad's layout, and xpad's X and Y, which are the kernel's
/// north and west swapped.
#[test]
fn under_the_360_identity_the_clone_is_an_xbox_pad_whatever_the_source() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let axes: BTreeMap<u16, danstick_core::calibration::AxisCalibration> = BTreeMap::new();
    let mut vpad = clone::create(
        &found,
        1,
        clone::IdentityMode::Xbox360,
        &axes,
        danstick_core::tuning::Tuning::default(),
        false,
        &danstick_core::profile::Mapping::default(),
    )
    .expect("create a clone");
    vpad.source
        .set_nonblocking(true)
        .expect("set the source non-blocking");
    let node = vpad.node().expect("the clone's node");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut reader = loop {
        match Device::open(&node) {
            Ok(device) => break device,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => panic!("open the clone {node}: {error}"),
        }
    };
    reader.set_nonblocking(true).expect("non-blocking");

    let id = reader.input_id();
    assert_eq!(
        (id.vendor(), id.product(), id.version()),
        (0x045e, 0x028e, 0x0110)
    );
    assert_eq!(reader.name(), Some("danstick Player 1"));
    let keys: Vec<u16> = reader
        .supported_keys()
        .expect("keys")
        .iter()
        .map(|key| key.0)
        .collect();
    assert_eq!(keys, danstick_core::xbox::KEYS.to_vec());
    let absinfo: BTreeMap<u16, AbsInfo> = reader
        .get_absinfo()
        .expect("absinfo")
        .map(|(code, info)| (code.0, info))
        .collect();
    assert_eq!(absinfo[&0x02].maximum(), 255, "ABS_Z is a trigger");
    assert_eq!(absinfo[&0x00].minimum(), -32768, "ABS_X is a stick");
    assert!(
        absinfo.contains_key(&0x10),
        "a hat, whatever the source had"
    );
    let guid = danstick_core::emit::virtual_guid(
        1,
        danstick_core::emit::Identity {
            bustype: vpad.identity.bustype,
            vendor: vpad.identity.vendor,
            product: vpad.identity.product,
            version: vpad.identity.version,
        },
    );
    assert!(
        guid.starts_with("0300") && guid.ends_with("5e0400008e02000010010000"),
        "{guid}"
    );

    let mut republisher = republish::Republisher::new(vec![vpad]);
    let _ = republisher.forward(0);
    let _ = drain_clone(&mut reader);
    let emit = |source: &mut VirtualDevice, kind: u16, code: u16, value: i32| {
        source
            .emit(&[
                InputEvent::new(kind, code, value),
                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
            ])
            .expect("emit");
        std::thread::sleep(Duration::from_millis(20));
    };
    // The kernel's BTN_NORTH is danstick's Y; xpad's Y is 0x134.
    emit(&mut source, EventType::KEY.0, KeyCode::BTN_NORTH.code(), 1);
    let _ = republisher.forward(0);
    assert_eq!(drain_clone(&mut reader), vec![(EventType::KEY.0, 0x134, 1)]);
    emit(&mut source, EventType::KEY.0, KeyCode::BTN_NORTH.code(), 0);
    let _ = republisher.forward(0);
    assert_eq!(drain_clone(&mut reader), vec![(EventType::KEY.0, 0x134, 0)]);
    // A trigger at the top of its range is 255 on the clone.
    emit(&mut source, EventType::ABSOLUTE.0, 0x02, 255);
    let _ = republisher.forward(0);
    assert_eq!(
        drain_clone(&mut reader),
        vec![(EventType::ABSOLUTE.0, 0x02, 255)]
    );
    // The left stick rides across, rescaled to the same full range.
    emit(&mut source, EventType::ABSOLUTE.0, 0x00, 32767);
    let _ = republisher.forward(0);
    assert_eq!(
        drain_clone(&mut reader),
        vec![(EventType::ABSOLUTE.0, 0x00, 32767)]
    );
    republisher.close();
}

#[test]
fn a_reserved_seat_is_a_pad_in_the_360_layout_with_a_node_of_its_own() {
    needs_uinput!();
    let mut device = clone::reserve(3, clone::Identity::XBOX360).expect("a seat can be published");
    let node = clone::node_of(&mut device).expect("udev makes it a node");
    // udev applies the ACL a moment after making the node; a launch opens it
    // well after that, so waiting here is the test catching up, not a promise.
    let mut opened = None;
    for _ in 0..40 {
        match Device::open(&node) {
            Ok(device) => {
                opened = Some(device);
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let Some(opened) = opened else {
        eprintln!("skipping: {node} never became readable here");
        return;
    };
    assert_eq!(
        opened.name().unwrap_or_default(),
        "danstick Player 3",
        "a launch binds it by name"
    );
    let keys = opened.supported_keys().expect("keys");
    assert!(keys.contains(KeyCode::BTN_SOUTH), "no A to press");
    assert!(keys.contains(KeyCode::BTN_START));
    let axes = opened.supported_absolute_axes().expect("axes");
    assert!(axes.contains(AbsoluteAxisCode::ABS_X), "no left stick");
    assert!(axes.contains(AbsoluteAxisCode::ABS_Z), "no left trigger");
}

#[test]
fn a_claim_that_cannot_open_its_pad_leaves_the_reserved_seat_alone() {
    needs_uinput!();
    // The device a launch is bound to must outlive a failed claim: nothing can
    // put a replacement inside a sandbox that has already started.
    let mut reserved =
        Some(clone::reserve(2, clone::Identity::XBOX360).expect("a seat can be published"));
    let missing = pad::Pad {
        path: std::path::PathBuf::from("/dev/input/event99997"),
        name: "Not Here".to_owned(),
        phys: String::new(),
        uniq: String::new(),
        vid: 1,
        pid: 2,
        syspath: std::path::PathBuf::from("/sys"),
        retroarch_visible: true,
        motion: None,
    };
    let made = clone::create_on(
        &missing,
        2,
        clone::IdentityMode::Xbox360,
        &BTreeMap::new(),
        Default::default(),
        false,
        &Default::default(),
        &BTreeMap::new(),
        &mut reserved,
    );
    assert!(made.is_err(), "a pad that is not there was cloned");
    assert!(
        reserved.is_some(),
        "the failed claim took the seat's device with it"
    );
    let node = clone::node_of(reserved.as_mut().expect("still there"));
    assert!(node.is_some(), "the device was left without a node");
}
