//! Hold the launch override's arithmetic to what the Python computes.
//!
//! This is what decides which physical controller a game sees as player 1,
//! and getting it wrong is not a cosmetic failure: two slots sharing a pad,
//! or a slot left on whatever retroarch.cfg happened to hold. One of the
//! cases below ended the daemon -- a player number outside 1..MAX_PLAYERS was
//! counted as managed without consuming a spare index, the iterator ran dry,
//! and the exception escaped during startup after the pads were grabbed.

use std::collections::BTreeMap;
use std::path::Path;

use padmap_core::retroarch;
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

/// `padmap Player N`, as `clone::virtual_name` spells it.
fn virtual_name(player: u32) -> String {
    format!("padmap Player {player}")
}

#[test]
fn vacant_indices_are_distinct_and_never_past_the_last_slot() {
    for case in corpus("empty_indices") {
        let pads = case["pads"].as_u64().expect("pads") as usize;
        let wanted = case["wanted"].as_u64().expect("wanted") as usize;
        let want: Vec<usize> = case["indices"]
            .as_array()
            .expect("indices")
            .iter()
            .map(|index| index.as_u64().expect("an index") as usize)
            .collect();
        assert_eq!(
            retroarch::empty_indices(pads, wanted),
            want,
            "{pads} pads, {wanted} wanted"
        );
    }
}

#[test]
fn indices_run_out_gracefully_rather_than_colliding_silently() {
    // Every index occupied: the spares repeat at the ceiling. That is
    // deliberate -- the unmanaged slots are RETRO_DEVICE_NONE regardless, so
    // a repeated index there reaches no core port.
    let spares = retroarch::empty_indices(16, 4);
    assert_eq!(spares, vec![15, 15, 15, 15]);
    // And with room, they are distinct, which is what stops two slots
    // sharing one pad.
    let roomy = retroarch::empty_indices(2, 3);
    assert_eq!(roomy, vec![2, 3, 4]);
}

fn as_order(raw: &Value) -> BTreeMap<usize, String> {
    raw.as_object()
        .expect("an order")
        .iter()
        .map(|(index, path)| {
            (
                index.parse().expect("an index"),
                path.as_str().expect("a path").to_owned(),
            )
        })
        .collect()
}

fn as_paths(raw: &Value) -> BTreeMap<u32, String> {
    raw.as_object()
        .expect("paths")
        .iter()
        .map(|(player, path)| {
            (
                player.parse().expect("a player"),
                path.as_str().expect("a path").to_owned(),
            )
        })
        .collect()
}

fn as_players(raw: &Value) -> BTreeMap<u32, usize> {
    raw.as_object()
        .expect("a mapping")
        .iter()
        .map(|(player, index)| {
            (
                player.parse().expect("a player"),
                index.as_u64().expect("an index") as usize,
            )
        })
        .collect()
}

#[test]
fn every_enumeration_maps_players_to_the_same_indices() {
    for case in corpus("pad_indices") {
        let order = as_order(&case["order"]);
        let paths = as_paths(&case["paths"]);
        assert_eq!(
            retroarch::compute_pad_indices(&paths, &order),
            as_players(&case["indices"]),
            "{}",
            case["what"]
        );
        let players: Vec<u32> = paths.keys().copied().collect();
        assert_eq!(
            retroarch::managed_players(&players, &paths, &order),
            as_players(&case["managed"]),
            "{}",
            case["what"]
        );
    }
}

#[test]
fn a_clone_missing_from_the_enumeration_is_not_managed() {
    // padmap cannot bind a pad RetroArch will not see, and pretending
    // otherwise leaves that slot on whatever retroarch.cfg holds.
    let order = BTreeMap::from([(0usize, "/dev/input/event90".to_owned())]);
    let paths = BTreeMap::from([
        (1u32, "/dev/input/event90".to_owned()),
        (2u32, "/dev/input/event99".to_owned()),
    ]);
    let managed = retroarch::managed_players(&[1, 2], &paths, &order);
    assert_eq!(managed.len(), 1);
    assert_eq!(managed.get(&1), Some(&0));
    assert_eq!(managed.get(&2), None);
}

#[test]
fn every_reservation_block_is_written_the_same() {
    for case in corpus("reservation_lines") {
        let managed: Vec<u32> = case["managed"]
            .as_array()
            .expect("managed")
            .iter()
            .map(|player| player.as_u64().expect("a player") as u32)
            .collect();
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
        let players: Vec<u32> = case["players"]
            .as_array()
            .expect("players")
            .iter()
            .map(|player| player.as_u64().expect("a player") as u32)
            .collect();
        assert_eq!(
            retroarch::reservation_config(&players, virtual_name),
            case["text"].as_str().expect("text"),
            "for {players:?}"
        );
    }
}

#[test]
fn an_unmanaged_slot_is_cleared_rather_than_left_alone() {
    // An uncleared reservation naming a pad that is no longer republished
    // still occupies the slot, so a session with fewer players than the last
    // one would find slots held open for clones that no longer exist.
    let text = retroarch::reservation_lines(&[1], virtual_name);
    assert!(text.contains("input_player1_reserved_device = \"padmap Player 1\""));
    assert!(text.contains("input_player1_device_reservation_type = \"2\""));
    assert!(text.contains("input_player2_reserved_device = \"\""));
    assert!(text.contains("input_player2_device_reservation_type = \"0\""));
    // All sixteen, every time.
    assert!(text.contains("input_player16_reserved_device = \"\""));
    assert_eq!(text.lines().count(), 32);
}
