//! Real controllers, as the kernel declares them, through the DSU picture.
//!
//! Each fixture in `fakepad` is a controller model with its axes exactly as
//! the driver publishes them, rest values included. A DSU consumer sees the
//! pad through `dsupad::Tracker`, and the question here is whether a pad
//! nobody is touching looks untouched -- which is where a rest value that is
//! not the middle of its range bites.

use std::collections::BTreeMap;

use padmap_core::dsu::{analog, button, CENTRE};
use padmap_core::dsupad::{Range, Tracker, EV_ABS, EV_KEY};
use padmap_input::fakepad::{self, Fixture};

fn tracker_for(fixture: &Fixture) -> Tracker {
    let ranges: BTreeMap<u16, Range> = fixture
        .axes
        .iter()
        .map(|(_, axis)| {
            (
                axis.code,
                Range::declared(axis.minimum, axis.maximum, axis.rest),
            )
        })
        .collect();
    Tracker::new(ranges)
}

/// Every axis reported at rest, as the kernel does on open.
fn rest(tracker: &mut Tracker, fixture: &Fixture) {
    for (_, axis) in fixture.axes {
        tracker.apply(EV_ABS, axis.code, axis.rest);
    }
}

#[test]
fn a_controller_at_rest_presses_nothing() {
    for fixture in fakepad::EVERY {
        let mut tracker = tracker_for(fixture);
        rest(&mut tracker, fixture);
        let pad = tracker.pad();
        assert_eq!(pad.buttons, 0, "{}: buttons held at rest", fixture.name);
        assert_eq!(pad.home, 0, "{}", fixture.name);
        assert_eq!(
            pad.analog, [0; 12],
            "{}: analog pressed at rest",
            fixture.name
        );
    }
}

#[test]
fn a_controller_at_rest_has_centred_sticks() {
    for fixture in fakepad::EVERY {
        let mut tracker = tracker_for(fixture);
        rest(&mut tracker, fixture);
        let pad = tracker.pad();
        for (name, value) in [
            ("left x", pad.left_x),
            ("left y", pad.left_y),
            ("right x", pad.right_x),
            ("right y", pad.right_y),
        ] {
            // Within 2% of the middle. The Mayflash C-stick genuinely rests
            // at 131 of 255; that is what calibration is for, and the tracker
            // is fed the *corrected* stream in the daemon.
            assert!(
                (CENTRE - 5..=CENTRE + 5).contains(&value),
                "{}: {name} rests at {value}, not the centre",
                fixture.name
            );
        }
    }
}

#[test]
fn the_gamecube_c_stick_is_not_a_trigger() {
    // The Mayflash adapter puts the C-stick's Y on ABS_Z, resting at 131 of
    // 0..255. By code that is the left trigger, and 131 is past half way: a
    // tracker that goes by code alone reports L2 held for as long as the
    // adapter is plugged in.
    let fixture = &fakepad::MAYFLASH_GAMECUBE;
    let mut tracker = tracker_for(fixture);
    rest(&mut tracker, fixture);
    let pad = tracker.pad();
    assert_eq!(pad.buttons & button::L2, 0, "L2 held by a resting C-stick");
    assert_eq!(pad.buttons & button::R2, 0);
    assert_eq!(pad.analog[analog::L2], 0);
    assert_eq!(pad.analog[analog::R2], 0);
}

#[test]
fn the_gamecube_triggers_rest_released_and_press_to_full() {
    // Its triggers are on ABS_RX/ABS_RY, resting at 24 and 25 of 0..255 --
    // by code the right stick, and 81% deflected. Read as a stick, the right
    // stick is jammed in a corner; read by rest, they are triggers at rest.
    let fixture = &fakepad::MAYFLASH_GAMECUBE;
    let mut tracker = tracker_for(fixture);
    rest(&mut tracker, fixture);
    assert_eq!(tracker.pad().analog[analog::L2], 0, "left trigger at rest");
    assert_eq!(tracker.pad().analog[analog::R2], 0, "right trigger at rest");

    let lt = fixture.axis("lt").expect("a left trigger");
    tracker.apply(EV_ABS, lt.code, lt.maximum);
    assert_eq!(tracker.pad().analog[analog::L2], 255);
    assert_eq!(tracker.pad().buttons & button::L2, button::L2);
    tracker.apply(EV_ABS, lt.code, lt.rest);
    assert_eq!(tracker.pad().analog[analog::L2], 0);
    assert_eq!(tracker.pad().buttons & button::L2, 0);
}

#[test]
fn the_gamecube_c_stick_drives_the_right_stick() {
    // Which of the two axes is X is the adapter's secret; what matters is
    // that both reach the right stick and neither reaches a trigger.
    let fixture = &fakepad::MAYFLASH_GAMECUBE;
    let mut tracker = tracker_for(fixture);
    rest(&mut tracker, fixture);
    let cx = fixture.axis("cx").expect("c-stick x");
    let cy = fixture.axis("cy").expect("c-stick y");
    tracker.apply(EV_ABS, cx.code, cx.maximum);
    tracker.apply(EV_ABS, cy.code, cy.maximum);
    let pad = tracker.pad();
    assert_eq!(pad.right_x, 255);
    assert_eq!(pad.right_y, 0, "evdev's maximum is down, which is DSU's 0");
    assert_eq!(pad.analog[analog::L2], 0);
    assert_eq!(pad.analog[analog::R2], 0);
    tracker.apply(EV_ABS, cx.code, cx.minimum);
    tracker.apply(EV_ABS, cy.code, cy.minimum);
    assert_eq!(tracker.pad().right_x, 0);
    assert_eq!(tracker.pad().right_y, 255);
}

#[test]
fn an_xbox_trigger_pressed_half_way_sets_its_byte_and_not_its_bit() {
    for fixture in [&fakepad::XBOX_360, &fakepad::XBOX_SERIES_X] {
        let mut tracker = tracker_for(fixture);
        rest(&mut tracker, fixture);
        let rt = fixture.axis("rt").expect("a right trigger");
        tracker.apply(EV_ABS, rt.code, rt.maximum / 3);
        let pad = tracker.pad();
        assert!(
            pad.analog[analog::R2] > 60 && pad.analog[analog::R2] < 110,
            "{}",
            fixture.name
        );
        assert_eq!(
            pad.buttons & button::R2,
            0,
            "{}: a third is not a press",
            fixture.name
        );
        tracker.apply(EV_ABS, rt.code, rt.maximum);
        assert_eq!(
            tracker.pad().buttons & button::R2,
            button::R2,
            "{}",
            fixture.name
        );
    }
}

#[test]
fn every_gamepad_button_a_fixture_declares_lands_somewhere_or_is_the_guide() {
    // A button the tracker does not know is a press nobody sees. Everything
    // in the BTN_GAMEPAD block is mapped. The Series X's share button is
    // KEY_RECORD, outside it, and DSU has no byte for it: that one is allowed
    // to go nowhere, and is the only one.
    for fixture in fakepad::EVERY {
        for (control, code) in fixture.buttons {
            let mut tracker = tracker_for(fixture);
            tracker.apply(EV_KEY, *code, 1);
            let pad = tracker.pad();
            let gamepad_block = (0x130..=0x13E).contains(code);
            if !gamepad_block {
                assert_eq!(
                    *control, "capture",
                    "{}: 0x{code:x} is outside BTN_GAMEPAD",
                    fixture.name
                );
                assert_eq!(pad.buttons, 0);
                continue;
            }
            assert!(
                pad.buttons != 0 || pad.home == 1,
                "{}: {control} (0x{code:x}) pressed and nothing changed",
                fixture.name
            );
            tracker.apply(EV_KEY, *code, 0);
            assert_eq!(
                tracker.pad().buttons,
                0,
                "{}: {control} stuck",
                fixture.name
            );
            assert_eq!(tracker.pad().home, 0);
        }
    }
}

#[test]
fn the_xbox_face_buttons_land_by_position() {
    // Xbox A is the bottom button, which on a DualShock is Cross. Y is the
    // top: Triangle. X is left: Square. B is right: Circle.
    let fixture = &fakepad::XBOX_360;
    let expect = [
        ("a", button::CROSS),
        ("b", button::CIRCLE),
        ("x", button::SQUARE),
        ("y", button::TRIANGLE),
    ];
    for (control, bit) in expect {
        let mut tracker = tracker_for(fixture);
        let code = fixture.button(control).expect("a face button");
        tracker.apply(EV_KEY, code, 1);
        assert_eq!(tracker.pad().buttons, bit, "{control}");
    }
}

#[test]
fn a_hat_dpad_and_a_button_dpad_produce_the_same_bits() {
    // xpad reports the d-pad as a hat; hid-nintendo reports it as four
    // BTN_DPAD_* keys. A DSU consumer sees the same four bits either way.
    for fixture in fakepad::EVERY {
        let mut tracker = tracker_for(fixture);
        assert!(
            fixture.dpad_is_hat,
            "{}: every fixture here is a hat",
            fixture.name
        );
        tracker.apply(EV_ABS, fakepad::ABS_HAT0X, -1);
        tracker.apply(EV_ABS, fakepad::ABS_HAT0Y, -1);
        assert_eq!(
            tracker.pad().buttons,
            button::UP | button::LEFT,
            "{}",
            fixture.name
        );
        assert_eq!(tracker.pad().analog[analog::DPAD_UP], 255);
        assert_eq!(tracker.pad().analog[analog::DPAD_LEFT], 255);
    }
    // BTN_DPAD_UP..BTN_DPAD_RIGHT are 0x220..0x223.
    let mut tracker = Tracker::new(BTreeMap::new());
    tracker.apply(EV_KEY, 0x220, 1);
    tracker.apply(EV_KEY, 0x222, 1);
    assert_eq!(
        tracker.pad().buttons,
        button::UP | button::LEFT,
        "a Switch Pro's d-pad"
    );
    assert_eq!(tracker.pad().analog[analog::DPAD_LEFT], 255);
    tracker.apply(EV_KEY, 0x220, 0);
    tracker.apply(EV_KEY, 0x222, 0);
    assert_eq!(tracker.pad().buttons, 0);
}
