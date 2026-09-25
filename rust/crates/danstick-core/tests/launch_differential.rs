//! Reading a RetroArch command line, held to what the Python decided.

use std::path::Path;

use danstick_core::launch;
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/corpus/{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("corpus is JSON")
}

fn strings(raw: &Value) -> Vec<String> {
    raw.as_array()
        .expect("array")
        .iter()
        .map(|item| item.as_str().expect("string").to_owned())
        .collect()
}

#[test]
fn every_command_line_splits_the_same_way() {
    let cases = corpus("launch_split");
    assert!(!cases.is_empty());
    for case in cases {
        let argv = strings(&case["argv"]);
        let present = strings(&case["present"]);
        let (core, rom) = launch::split_args(&argv, |path| present.iter().any(|p| p == path));
        assert_eq!(core, case["core"].as_str().expect("core"), "{argv:?}");
        assert_eq!(rom, case["rom"].as_str().expect("rom"), "{argv:?}");
        let title = if rom.is_empty() {
            String::new()
        } else {
            launch::title_for(&rom)
        };
        assert_eq!(title, case["title"].as_str().expect("title"), "{argv:?}");
    }
}

#[test]
fn every_title_reads_the_same() {
    for case in corpus("launch_titles") {
        let rom = case["rom"].as_str().expect("rom");
        assert_eq!(
            launch::title_for(rom),
            case["out"].as_str().expect("out"),
            "{rom:?}"
        );
    }
}
