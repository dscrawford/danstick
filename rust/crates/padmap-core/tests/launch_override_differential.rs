//! Hold the launch override's index arithmetic to what the Python computes.

use std::collections::BTreeMap;
use std::path::Path;

use padmap_core::retroarch;
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/corpus/{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

fn virtual_name(player: u32) -> String {
    format!("padmap Player {player}")
}

fn u32s(raw: &Value) -> Vec<u32> {
    raw.as_array().expect("array").iter().map(|v| v.as_u64().expect("an int") as u32).collect()
}

fn usizes(raw: &Value) -> Vec<usize> {
    raw.as_array().expect("array").iter().map(|v| v.as_u64().expect("an int") as usize).collect()
}

fn keyed<K: std::str::FromStr + Ord, V>(raw: &Value, value: impl Fn(&Value) -> V) -> BTreeMap<K, V>
where
    K::Err: std::fmt::Debug,
{
    raw.as_object()
        .expect("an object")
        .iter()
        .map(|(key, v)| (key.parse().expect("a key"), value(v)))
        .collect()
}

fn as_order(raw: &Value) -> BTreeMap<usize, String> {
    keyed(raw, |v| v.as_str().expect("a path").to_owned())
}

fn as_paths(raw: &Value) -> BTreeMap<u32, String> {
    keyed(raw, |v| v.as_str().expect("a path").to_owned())
}

fn as_players(raw: &Value) -> BTreeMap<u32, usize> {
    keyed(raw, |v| v.as_u64().expect("an index") as usize)
}

fn event(n: u32) -> String {
    format!("/dev/input/event{n}")
}

#[test]
fn vacant_indices_are_distinct_and_never_past_the_last_slot() {
    for case in corpus("empty_indices") {
        let pads = case["pads"].as_u64().expect("pads") as usize;
        let wanted = case["wanted"].as_u64().expect("wanted") as usize;
        assert_eq!(retroarch::empty_indices(pads, wanted), usizes(&case["indices"]), "{pads} pads, {wanted} wanted");
    }
}

#[test]
fn indices_run_out_gracefully_rather_than_colliding_silently() {
    // Spares repeat at the ceiling: unmanaged slots are RETRO_DEVICE_NONE, so no core port is reached.
    assert_eq!(retroarch::empty_indices(16, 4), vec![15, 15, 15, 15]);
    assert_eq!(retroarch::empty_indices(2, 3), vec![2, 3, 4]);
}

#[test]
fn every_enumeration_maps_players_to_the_same_indices() {
    for case in corpus("pad_indices") {
        let order = as_order(&case["order"]);
        let paths = as_paths(&case["paths"]);
        assert_eq!(retroarch::compute_pad_indices(&paths, &order), as_players(&case["indices"]), "{}", case["what"]);
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
    let order = BTreeMap::from([(0usize, event(90))]);
    let paths = BTreeMap::from([(1u32, event(90)), (2u32, event(99))]);
    let managed = retroarch::managed_players(&[1, 2], &paths, &order);
    assert_eq!(managed.len(), 1);
    assert_eq!(managed.get(&1), Some(&0));
    assert_eq!(managed.get(&2), None);
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
    // A stale reservation still holds the slot for a clone that no longer exists.
    let text = retroarch::reservation_lines(&[1], virtual_name);
    assert!(text.contains("input_player1_reserved_device = \"padmap Player 1\""));
    assert!(text.contains("input_player1_device_reservation_type = \"2\""));
    assert!(text.contains("input_player2_reserved_device = \"\""));
    assert!(text.contains("input_player2_device_reservation_type = \"0\""));
    assert!(text.contains("input_player16_reserved_device = \"\""));
    assert_eq!(text.lines().count(), 32);
}

#[test]
fn every_unassigned_core_port_is_emptied_the_same_way() {
    for case in corpus("launch_args") {
        let order = as_order(&case["order"]);
        let paths = as_paths(&case["paths"]);
        let players: Vec<u32> = paths.keys().copied().collect();
        let want: Vec<&str> = case["args"].as_array().expect("args").iter().map(|a| a.as_str().expect("an arg")).collect();
        assert_eq!(retroarch::launch_args(&players, &paths, &order), want, "{}", case["what"]);
    }
}

#[test]
fn a_port_with_a_player_on_it_is_left_alone() {
    let order = BTreeMap::from([(0usize, event(90))]);
    let paths = BTreeMap::from([(2u32, event(90))]);
    let args = retroarch::launch_args(&[2], &paths, &order);
    assert!(!args.windows(2).any(|pair| pair == ["--nodevice", "2"]));
    assert!(args.windows(2).any(|pair| pair == ["--nodevice", "1"]));
    assert_eq!(args.len(), (retroarch::MAX_PLAYERS as usize - 1) * 2);
}
