//! The state event, round-tripped against what the Python emits.
//!
//! It is the only thing a front-end has: there is no other way to ask what
//! padmap thinks is going on. A field renamed or a type changed is a client
//! that draws nothing and says nothing about why.

use std::path::Path;

use padmap_core::state::{PlayerState, StateEvent};
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

#[test]
fn every_recorded_state_event_round_trips() {
    for case in corpus("state_events") {
        let want = &case["event"];
        // Deserialising proves the field names and types match; serialising
        // back and comparing proves nothing was dropped on the way through,
        // which a struct with a missing field would otherwise hide.
        let parsed: StateEvent =
            serde_json::from_value(want.clone()).expect("the Python's event parses");
        let ours = serde_json::to_value(&parsed).expect("serialises");
        assert_eq!(&ours, want);
    }
}

#[test]
fn the_event_is_named_state_whatever_the_state_is() {
    let event = StateEvent::new("idle", 4, Vec::new(), "x".to_owned(), 1, "mirror");
    assert_eq!(event.event, "state");
    let value = serde_json::to_value(&event).expect("serialises");
    assert_eq!(value["event"], "state");
    assert_eq!(value["state"], "idle");
    // Every field a client reads is present even when there is nothing to
    // report: a missing key and a zero are different answers to a front-end.
    for field in ["slots", "players", "build", "pid", "identity"] {
        assert!(value.get(field).is_some(), "{field} is missing");
    }
}

#[test]
fn a_player_with_no_mappings_says_so_rather_than_omitting_them() {
    let player = PlayerState {
        player: 1,
        name: "Pad".to_owned(),
        node: "event9".to_owned(),
        icon: "xbox".to_owned(),
        configured: false,
        mappings: Vec::new(),
    };
    let value = serde_json::to_value(&player).expect("serialises");
    assert_eq!(value["mappings"], serde_json::json!([]));
    // `configured` means mapped, not merely known. A profile exists for
    // several reasons -- calibration writes one too -- so keying a front-end
    // on the profile's existence offered the wizard exactly once and never
    // again.
    assert_eq!(value["configured"], false);
}
