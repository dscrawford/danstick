//! The state event, round-tripped against what the Python emits.

use std::path::Path;

use padmap_core::state::{PlayerState, StateEvent};
use serde_json::{json, Value};

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/corpus/{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

/// Every key on the left is present on the right with the same value; added keys break no client.
fn carries_everything_in(ours: &Value, want: &Value, path: &str) {
    match want {
        Value::Object(fields) => {
            let ours = ours
                .as_object()
                .unwrap_or_else(|| panic!("{path}: an object became {ours}"));
            for (key, value) in fields {
                let mine = ours
                    .get(key)
                    .unwrap_or_else(|| panic!("{path}.{key} is gone"));
                carries_everything_in(mine, value, &format!("{path}.{key}"));
            }
        }
        Value::Array(items) => {
            let ours = ours
                .as_array()
                .unwrap_or_else(|| panic!("{path}: an array became {ours}"));
            assert_eq!(ours.len(), items.len(), "{path}: length changed");
            for (at, value) in items.iter().enumerate() {
                carries_everything_in(&ours[at], value, &format!("{path}[{at}]"));
            }
        }
        _ => assert_eq!(ours, want, "{path}"),
    }
}

#[test]
fn every_recorded_state_event_round_trips() {
    for case in corpus("state_events") {
        let want = &case["event"];
        let parsed: StateEvent =
            serde_json::from_value(want.clone()).expect("the Python's event parses");
        let ours = serde_json::to_value(&parsed).expect("serialises");
        carries_everything_in(&ours, want, "state");
    }
}

#[test]
fn a_field_that_went_missing_is_still_caught() {
    let want = json!({"state": "idle", "slots": 4});
    let dropped = json!({"state": "idle"});
    assert!(std::panic::catch_unwind(|| carries_everything_in(&dropped, &want, "state")).is_err());
    let changed = json!({"state": "ready", "slots": 4});
    assert!(std::panic::catch_unwind(|| carries_everything_in(&changed, &want, "state")).is_err());
}

#[test]
fn the_event_is_named_state_whatever_the_state_is() {
    let event = StateEvent::new("idle", 4, Vec::new(), "x".to_owned(), 1, "mirror");
    assert_eq!(event.event, "state");
    let value = serde_json::to_value(&event).expect("serialises");
    assert_eq!(value["event"], "state");
    assert_eq!(value["state"], "idle");
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
        published: true,
        keyboard: false,
        mouse: false,
    };
    let value = serde_json::to_value(&player).expect("serialises");
    assert_eq!(value["mappings"], json!([]));
    // `configured` means mapped, not merely known: calibration writes a profile too.
    assert_eq!(value["configured"], false);
}
