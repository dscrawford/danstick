//! The whole `--appendconfig` override and the copied-profile fallback, held
//! to what the Python wrote.
//!
//! The file is what a user diffs when a launch behaves oddly, so the exact
//! comment lines are part of the contract: a port that reads differently is
//! one that cannot be diffed against what the Python wrote yesterday.

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

fn strings(raw: &Value) -> BTreeMap<String, String> {
    raw.as_object()
        .expect("object")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().expect("string").to_owned()))
        .collect()
}

#[test]
fn every_launch_override_is_written_the_same() {
    let cases = corpus("launch_config");
    assert!(!cases.is_empty());
    for case in cases {
        let players: Vec<u32> = case["players"]
            .as_array()
            .expect("players")
            .iter()
            .map(|p| p.as_i64().expect("int") as u32)
            .collect();
        let paths: BTreeMap<u32, String> = strings(&case["paths"])
            .into_iter()
            .map(|(k, v)| (k.parse().expect("player"), v))
            .collect();
        let order: BTreeMap<usize, String> = case["order"]
            .as_array()
            .expect("order")
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.as_str().expect("path").to_owned()))
            .collect();
        let calibrated: Vec<u32> = case["calibrated"]
            .as_array()
            .expect("calibrated")
            .iter()
            .map(|p| p.as_i64().expect("int") as u32)
            .collect();
        // The Python pins the gain when every *managed* pad is calibrated;
        // the daemon computes that fact and hands it over.
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
        // The Python takes None to mean "padmap's own id"; the daemon always
        // knows the clone's identity, so the corpus writes that down.
        let vid = case["vid"].as_i64().map(|v| v as u16).unwrap_or(0x1209);
        let pid = case["pid"].as_i64().map(|v| v as u16).unwrap_or(0x0001);
        let ours = retroarch::derive_profile(source, player, vid, pid, virtual_name);
        assert_eq!(ours, case["out"].as_str().expect("out"), "{case}");
    }
}
