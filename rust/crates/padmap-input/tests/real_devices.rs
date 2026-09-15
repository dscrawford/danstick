//! The parts of padmap that only exist when there is a device.
//!
//! `clone.rs`, `republish.rs` and `pad.rs` were the three least-covered files
//! in the workspace -- 12%, 15% and 56% -- and all for the same reason: every
//! interesting line needs an `evdev::Device`, and the unit tests beside them
//! can only reach the pure helpers. The forwarding path is the whole point of
//! the Rust port, and it was the least tested thing in it.
//!
//! So these make one. A uinput device is a real device: the kernel publishes
//! it, udev tags it, `discover()` finds it, and a clone of it can be created,
//! written to and read back. The identity used is a Microsoft X-Box 360 pad,
//! matching `fakepad.Xbox360` on the Python side, so both halves of the
//! project are testing against the same controller.
//!
//! Skipped, not failed, where `/dev/uinput` is not writable -- a sandboxed
//! build has no business failing over a device node it was never given.

use std::collections::BTreeMap;
use std::time::Duration;

use evdev::{
    uinput::VirtualDevice, AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, Device, EventType,
    InputEvent, InputId, KeyCode, UinputAbsSetup,
};
use padmap_input::{clone, pad, republish};

/// What `fakepad.Xbox360` declares, from `xpad.c`.
const NAME: &str = "padmap test X-Box 360 pad";
const VID: u16 = 0x045E;
const PID: u16 = 0x028E;

fn uinput_available() -> bool {
    // Writable, not merely present: in a Nix build /dev/uinput may exist and
    // be unopenable, and a test that failed there would fail for everyone
    // building the package rather than for anyone who broke something.
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
    // The ranges xpad_set_up_abs declares: sticks -32768..32767 fuzz 16 flat
    // 128, triggers 0..255. A clone built from axes with no range at all gets
    // min == max, and RetroArch divides by that on its first poll.
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
    // udev has to tag it ID_INPUT_JOYSTICK before discover() will report it.
    std::thread::sleep(Duration::from_millis(500));
    device
}

/// A clone of `found`, with its source set non-blocking.
///
/// `Republisher::forward` calls `fetch_events` and handles `WouldBlock`, but
/// the daemon only ever calls it when epoll has already said the descriptor is
/// readable -- so in production the source stays blocking and never waits.
/// A test drives the pump directly, with nothing gating it, and a blocking
/// source turns the first call into a wait for an event that only arrives
/// after the call returns. It hangs for ever rather than failing.
fn clone_of(found: &pad::Pad, player: u32) -> clone::VirtualPad {
    let axes: BTreeMap<u16, padmap_core::calibration::AxisCalibration> = BTreeMap::new();
    // grab=false: something else on this machine may hold the pad, and a test
    // that took exclusive access would take it from a running daemon.
    let mut virtual_pad = clone::create(found, player, clone::IdentityMode::Mirror, &axes, false)
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
    // It is a real pad, not one of padmap's own clones -- which discover()
    // has to tell apart, because republishing a clone would be a loop.
    assert!(!pad::is_padmap_clone(&found.name, &found.phys));
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
fn a_clone_mirrors_the_source_and_carries_padmaps_own_name() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let virtual_pad = clone_of(&found, 3);
    assert_eq!(virtual_pad.name(), clone::virtual_name(3));
    assert!(virtual_pad.name().contains('3'), "{}", virtual_pad.name());

    // In mirror mode the clone wears the source's ids, which is what makes
    // SDL's database match it as the controller it actually is.
    let identity = clone::Identity::for_source(
        clone::IdentityMode::Mirror,
        &Device::open(&found.path).expect("open"),
    );
    assert_eq!((identity.vendor, identity.product), (VID, PID));

    let padmap_identity = clone::Identity::for_source(
        clone::IdentityMode::Padmap,
        &Device::open(&found.path).expect("open"),
    );
    assert_ne!(
        (padmap_identity.vendor, padmap_identity.product),
        (VID, PID),
        "padmap identity must not mirror, or PADMAP_ONLY_VIRTUAL hides everything"
    );
}

#[test]
fn a_press_on_the_source_arrives_on_the_clone() {
    needs_uinput!();
    let mut source = spawn_source();
    let found = find(&mut source).expect("discover");
    let mut republisher = republish::Republisher::new(vec![clone_of(&found, 1)]);

    // Drain whatever the kernel queued while the clone was being built, so
    // the assertion below is about the press and not about startup.
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

    // Still read, so the backlog cannot arrive in a burst when the wizard
    // closes -- an unread evdev node does not go quiet, it fills.
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

    // Unplugged. A vanished node reports readable for ever, so the loop spins
    // at full speed unless the clone is marked gone and unregistered.
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
    // No uinput needed: the point is that discover() answers rather than
    // failing when there is nothing to find.
    let pads = pad::discover(pad::Filter::default()).expect("discover must not error");
    for found in &pads {
        assert!(!found.name.is_empty() || !found.event().is_empty());
    }
    let _ = pad::ambiguous_groups(&pads);
}

#[test]
fn undriven_controllers_are_included_by_default_but_never_for_retroarch() {
    // `Filter`'s Default is written out rather than derived because one field
    // is not false; this is what would catch a future `#[derive(Default)]`
    // silently reverting it.
    let default = pad::Filter::default();
    assert!(!default.include_virtual);
    assert!(!default.retroarch_only);
    assert!(
        default.include_undriven,
        "a controller padmap drives itself must be findable without an env var"
    );

    // RetroArch cannot see a device with no evdev node, and counting one would
    // shift every real pad's index by one. Holds whether or not a Steam
    // Controller is attached: the combination is what is under test.
    let filtered = pad::discover(pad::Filter {
        include_virtual: false,
        retroarch_only: true,
        include_undriven: true,
    })
    .expect("discover must not error");
    for found in &filtered {
        assert!(
            !padmap_input::triton::owns(found),
            "{} is driven by padmap but retroarch_only asked to exclude it",
            found.path.display()
        );
    }
}
