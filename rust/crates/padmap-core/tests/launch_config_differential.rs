//! The `--appendconfig` override and the copied-profile fallback, held to what the Python wrote.

use std::collections::BTreeMap;
use std::path::Path;

use padmap_core::emit::virtual_name;
use padmap_core::retroarch::{self, LaunchFacts};
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/corpus/{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("corpus is JSON")
}

fn u32s(raw: &Value) -> Vec<u32> {
    raw.as_array()
        .expect("array")
        .iter()
        .map(|p| p.as_i64().expect("int") as u32)
        .collect()
}

fn strs(raw: &Value) -> Vec<String> {
    raw.as_array()
        .expect("array")
        .iter()
        .map(|p| p.as_str().expect("string").to_owned())
        .collect()
}

fn paths(raw: &Value) -> BTreeMap<u32, String> {
    raw.as_object()
        .expect("object")
        .iter()
        .map(|(k, v)| {
            (
                k.parse().expect("player"),
                v.as_str().expect("string").to_owned(),
            )
        })
        .collect()
}

#[test]
fn every_launch_override_is_written_the_same() {
    let cases = corpus("launch_config");
    assert!(!cases.is_empty());
    for case in cases {
        let players = u32s(&case["players"]);
        let paths = paths(&case["paths"]);
        let order: BTreeMap<usize, String> = strs(&case["order"]).into_iter().enumerate().collect();
        let calibrated = u32s(&case["calibrated"]);
        let managed = retroarch::managed_players(&players, &paths, &order);
        let all_calibrated = managed
            .keys()
            .filter(|p| (1..=retroarch::MAX_PLAYERS).contains(*p))
            .all(|p| calibrated.contains(p));
        let facts = LaunchFacts {
            all_calibrated,
            autoconfig_dir: case["autoconfig_dir"].as_str().expect("dir").to_owned(),
            verbose: case["verbose"].as_bool().expect("bool"),
        };
        let ours = retroarch::launch_config(&players, &paths, &order, &facts, virtual_name);
        assert_eq!(
            ours,
            case["out"].as_str().expect("out"),
            "players {players:?}"
        );
    }
}

#[test]
fn every_derived_profile_is_written_the_same() {
    for case in corpus("derive_profile") {
        let values: Vec<(String, String)> = case["values"]
            .as_array()
            .expect("pairs")
            .iter()
            .map(|pair| {
                (
                    pair[0].as_str().expect("key").to_owned(),
                    pair[1].as_str().expect("value").to_owned(),
                )
            })
            .collect();
        let source = case["source"]
            .as_str()
            .map(|name| (name, values.as_slice()));
        let player = case["player"].as_i64().expect("player") as u32;
        let vid = case["vid"].as_i64().map(|v| v as u16).unwrap_or(0x1209);
        let pid = case["pid"].as_i64().map(|v| v as u16).unwrap_or(0x0001);
        let ours = retroarch::derive_profile(source, player, vid, pid, virtual_name);
        assert_eq!(ours, case["out"].as_str().expect("out"), "{case}");
    }
}
