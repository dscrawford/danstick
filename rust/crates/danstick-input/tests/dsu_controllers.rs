//! Real controllers, as the kernel declares them, through the DSU picture.

use std::collections::BTreeMap;

use danstick_core::dsu::{analog, button, CENTRE};
use danstick_core::dsupad::{Range, Tracker, EV_ABS, EV_KEY};
use danstick_input::fakepad::{self, Fixture};

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

/// Controls a DualShock-shaped packet has no field for, so a press of one is
/// correctly seen by nobody: an Xbox share button, a Deck's grips, its
/// trackpad clicks and its quick access button.
const NOTHING_DSU_CAN_CARRY: [&str; 8] = [
    "capture",
    "l4",
    "r4",
    "l5",
    "r5",
    "lpad_click",
    "rpad_click",
    "quickaccess",
];

#[test]
fn every_gamepad_button_a_fixture_declares_lands_somewhere_or_is_the_guide() {
    // A button the tracker does not know is a press nobody sees.
    for fixture in fakepad::EVERY {
        for (control, code) in fixture.buttons {
            let mut tracker = tracker_for(fixture);
            tracker.apply(EV_KEY, *code, 1);
            let pad = tracker.pad();
            let carried = (0x130..=0x13E).contains(code) || (0x220..=0x223).contains(code);
            if !carried {
                assert!(
                    NOTHING_DSU_CAN_CARRY.contains(control),
                    "{}: {control} (0x{code:x}) is outside every table and unnamed",
                    fixture.name
                );
                assert_eq!(pad.buttons, 0, "{}: {control}", fixture.name);
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
    // Up and left held at once, however the pad happens to say so.
    for fixture in fakepad::EVERY {
        let mut tracker = tracker_for(fixture);
        if fixture.dpad_is_hat {
            tracker.apply(EV_ABS, fakepad::ABS_HAT0X, -1);
            tracker.apply(EV_ABS, fakepad::ABS_HAT0Y, -1);
        } else {
            // A Steam Deck's d-pad, and a Switch Pro's: BTN_DPAD_UP..RIGHT.
            let up = fixture.button("up").expect("a d-pad key");
            let left = fixture.button("left").expect("a d-pad key");
            assert_eq!((up, left), (0x220, 0x222), "{}", fixture.name);
            tracker.apply(EV_KEY, up, 1);
            tracker.apply(EV_KEY, left, 1);
        }
        assert_eq!(
            tracker.pad().buttons,
            button::UP | button::LEFT,
            "{}",
            fixture.name
        );
        assert_eq!(
            tracker.pad().analog[analog::DPAD_UP],
            255,
            "{}",
            fixture.name
        );
        assert_eq!(
            tracker.pad().analog[analog::DPAD_LEFT],
            255,
            "{}",
            fixture.name
        );
    }
    // A pad declaring neither still answers to the keys, released and all.
    let mut tracker = Tracker::new(BTreeMap::new());
    tracker.apply(EV_KEY, 0x220, 1);
    tracker.apply(EV_KEY, 0x222, 1);
    assert_eq!(tracker.pad().buttons, button::UP | button::LEFT);
    tracker.apply(EV_KEY, 0x220, 0);
    tracker.apply(EV_KEY, 0x222, 0);
    assert_eq!(tracker.pad().buttons, 0);
}
