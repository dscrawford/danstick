//! Hold the RetroArch reservation config to what the Python wrote.

mod common;

use std::path::Path;

use danstick_core::retroarch;
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/corpus/{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&common::renamed(&text)).expect("the corpus is JSON")
}

fn virtual_name(player: u32) -> String {
    format!("danstick Player {player}")
}

fn u32s(raw: &Value) -> Vec<u32> {
    raw.as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_u64().expect("an int") as u32)
        .collect()
}

#[test]
fn every_reservation_block_is_written_the_same() {
    for case in corpus("reservation_lines") {
        let managed = u32s(&case["managed"]);
        assert_eq!(
            retroarch::reservation_lines(&managed, virtual_name),
            case["text"].as_str().expect("text"),
            "for {managed:?}"
        );
    }
}

#[test]
fn every_reservation_config_is_written_the_same() {
    for case in corpus("reservation_config") {
        let players = u32s(&case["players"]);
        assert_eq!(
            retroarch::reservation_config(&players, virtual_name),
            case["text"].as_str().expect("text"),
            "for {players:?}"
        );
    }
}

#[test]
fn an_unmanaged_slot_is_cleared_rather_than_left_alone() {
    let text = retroarch::reservation_lines(&[1], virtual_name);
    assert!(text.contains("input_player1_reserved_device = \"danstick Player 1\""));
    assert!(text.contains("input_player1_device_reservation_type = \"2\""));
    assert!(text.contains("input_player2_reserved_device = \"\""));
    assert!(text.contains("input_player2_device_reservation_type = \"0\""));
    assert!(text.contains("input_player16_reserved_device = \"\""));
    assert_eq!(text.lines().count(), 32);
}
