//! `seating` carries how long a hold must run to claim a seat.
//!
//! The length is a comfort setting: a quarter of a second is too quick at the
//! front of a launch, where picking a pad up claims a seat nobody meant to
//! claim. Nothing here is ever refused, because failing to open seating is a
//! worse answer than opening it at the default.

use padmap_core::assign::HOLD_SECONDS;
use padmap_core::command::Command;
use serde_json::json;

fn hold_of(message: serde_json::Value) -> Option<f64> {
    match Command::parse(&message).expect("seating always parses") {
        Command::Seating { hold, .. } => hold,
        other => panic!("not a seating command: {other:?}"),
    }
}

#[test]
fn a_length_asked_for_is_carried_as_it_was_asked() {
    assert_eq!(
        hold_of(json!({"cmd": "seating", "open": true, "players": 4, "hold": 1.5})),
        Some(1.5)
    );
    assert_eq!(
        hold_of(json!({"cmd": "seating", "open": true, "hold": 2})),
        Some(2.0),
        "a whole number is a length too"
    );
}

#[test]
fn a_length_left_out_leaves_the_one_already_set() {
    assert_eq!(hold_of(json!({"cmd": "seating", "open": true})), None);
    assert_eq!(
        hold_of(json!({"cmd": "seating", "open": true, "hold": null})),
        None,
        "null is not asking"
    );
}

#[test]
fn a_length_nobody_can_read_is_the_default_and_never_a_refusal() {
    for asked in [
        json!("1.5"),
        json!("soon"),
        json!(0),
        json!(-2),
        json!(600),
        json!(0.01),
        json!(true),
        json!([1.5]),
    ] {
        assert_eq!(
            hold_of(json!({"cmd": "seating", "open": true, "hold": asked})),
            Some(HOLD_SECONDS),
            "{asked}"
        );
    }
}

#[test]
fn closing_seating_carries_a_length_nobody_will_use_rather_than_failing() {
    assert_eq!(
        hold_of(json!({"cmd": "seating", "open": false, "hold": 1.5})),
        Some(1.5)
    );
}
