//! Hold joypad detection to what the Python decides.

use std::path::Path;

use padmap_core::capability::{self, Mask};
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/corpus/{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

fn mask(text: &str) -> Mask {
    Mask::parse(text).expect("a readable bitmap")
}

#[test]
fn every_bitmap_parses_to_the_same_bits() {
    for case in corpus("capability_masks") {
        // A null `raw` is "the file was not there".
        let raw = case["raw"].as_str();
        let parsed = raw.and_then(Mask::parse);
        assert_eq!(
            parsed.is_none(),
            case["none"].as_bool().expect("none"),
            "for {raw:?}"
        );
        let Some(mask) = parsed else { continue };
        assert_eq!(
            mask.is_empty(),
            case["zero"].as_bool().expect("zero"),
            "for {raw:?}"
        );
        let want: Vec<usize> = case["bits"]
            .as_array()
            .expect("bits")
            .iter()
            .map(|b| b.as_u64().expect("a bit") as usize)
            .collect();
        assert_eq!(mask.bits(1024), want, "for {raw:?}");
    }
}

#[test]
fn a_mask_of_zero_is_not_a_mask_that_could_not_be_read() {
    // Reading "0" as unreadable sends the device to the open() fallback: 11ms of URB teardown per node.
    let zero = mask("0");
    assert!(zero.is_empty());
    assert!(Mask::parse("").is_none());
    assert!(Mask::parse("   ").is_none());
    assert_eq!(
        capability::joypad(Some(&zero), Some(&mask("1"))),
        Some(false),
        "no axes is a definite no"
    );
    assert_eq!(
        capability::joypad(None, Some(&mask("1"))),
        None,
        "unreadable is 'cannot tell'"
    );
}

#[test]
fn the_words_are_most_significant_first() {
    // "1 0" is bit 64, not bit 0.
    assert!(mask("1 0").bit(64));
    assert!(!mask("1 0").bit(0));
    assert!(mask("0 1").bit(0));
    assert!(!mask("0 1").bit(64));
}

#[test]
fn every_recorded_decision_matches() {
    // The corpus carries masks as decimal strings; a >128-bit one cannot be rebuilt here.
    let to_mask = |value: &Value| -> Option<Mask> {
        let number: u128 = value.as_str()?.parse().ok()?;
        Mask::parse(&format!("{:x} {:x}", (number >> 64) as u64, number as u64))
    };
    for case in corpus("joypad_by_capability") {
        let absolute = to_mask(&case["abs"]);
        let keys = to_mask(&case["keys"]);
        if (case["abs"].is_string() && absolute.is_none())
            || (case["keys"].is_string() && keys.is_none())
        {
            continue;
        }
        let want = match &case["verdict"] {
            Value::Null => None,
            Value::Bool(value) => Some(*value),
            other => panic!("unexpected verdict {other:?}"),
        };
        assert_eq!(
            capability::joypad(absolute.as_ref(), keys.as_ref()),
            want,
            "abs {:?}, keys {:?}",
            case["abs"],
            case["keys"]
        );
    }
}

#[test]
fn every_device_on_the_recording_machine_is_judged_the_same() {
    let cases = corpus("live_capabilities");
    assert!(!cases.is_empty(), "the corpus recorded no devices");
    let mut joypads = 0;
    for case in &cases {
        let absolute = case["abs_raw"].as_str().and_then(Mask::parse);
        let keys = case["key_raw"].as_str().and_then(Mask::parse);
        // Every recorded node had both files, so `None` is itself a divergence.
        let verdict = capability::joypad(absolute.as_ref(), keys.as_ref()).unwrap_or_else(|| {
            panic!(
                "could not judge a device the Python judged: abs {:?}",
                case["abs_raw"]
            )
        });
        assert_eq!(
            verdict,
            case["joypad"].as_bool().expect("joypad"),
            "abs {:?}",
            case["abs_raw"]
        );
        joypads += usize::from(verdict);
    }
    assert_eq!(
        joypads,
        cases.iter().filter(|c| c["joypad"] == true).count()
    );
}
