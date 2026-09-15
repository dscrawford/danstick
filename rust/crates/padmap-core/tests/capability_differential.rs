//! Hold joypad detection to what the Python decides.
//!
//! This is the question "is this thing a controller", asked of every input
//! device on the machine, and the two implementations disagreeing means one of
//! them either misses a controller or grabs a keyboard. Neither failure says
//! anything at the time.
//!
//! The corpus includes every input device on the machine that recorded it --
//! 33 of them, twelve with an `abs` file containing "0" -- because those are
//! the shapes nobody would think to write down.

use std::path::Path;

use padmap_core::capability::{self, Mask};
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
fn every_bitmap_parses_to_the_same_bits() {
    for case in corpus("capability_masks") {
        // A null `raw` is "the file was not there", which reads as no text.
        let raw = case["raw"].as_str();
        let parsed = raw.and_then(Mask::parse);
        let want_none = case["none"].as_bool().expect("none");
        assert_eq!(parsed.is_none(), want_none, "for {raw:?}");

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
            .map(|bit| bit.as_u64().expect("a bit index") as usize)
            .collect();
        assert_eq!(mask.bits(1024), want, "for {raw:?}");
    }
}

#[test]
fn a_mask_of_zero_is_not_a_mask_that_could_not_be_read() {
    // The whole reason `parse` returns an Option rather than defaulting. A
    // device with no absolute axes has an `abs` file containing "0", and
    // reading that as "could not be read" sends it down the fallback that
    // opens the device -- 11ms of URB teardown per node, which was 390ms of a
    // 400ms scan.
    let zero = Mask::parse("0").expect("\"0\" is a readable bitmap");
    assert!(zero.is_empty());
    assert!(Mask::parse("").is_none());
    assert!(Mask::parse("   ").is_none());
    assert_eq!(
        capability::joypad(
            Some(&zero),
            Some(&Mask::parse("1").expect("a readable bitmap"))
        ),
        Some(false),
        "no axes is a definite no, not an 'ask the device'"
    );
    assert_eq!(
        capability::joypad(None, Some(&Mask::parse("1").expect("a readable bitmap"))),
        None,
        "an unreadable bitmap is 'cannot tell'"
    );
}

#[test]
fn the_words_are_most_significant_first() {
    // Read the other way round this is a bitmap that looks plausible and
    // describes a different device. "1 0" is bit 64, not bit 0.
    let mask = Mask::parse("1 0").expect("parses");
    assert!(mask.bit(64));
    assert!(!mask.bit(0));
    let mask = Mask::parse("0 1").expect("parses");
    assert!(mask.bit(0));
    assert!(!mask.bit(64));
}

#[test]
fn every_recorded_decision_matches() {
    for case in corpus("joypad_by_capability") {
        // The corpus carries the masks as decimal strings, because a 768-bit
        // integer does not fit in JSON. Rebuilt here as hex words.
        let to_mask = |value: &Value| -> Option<Mask> {
            let text = value.as_str()?;
            let number: u128 = text.parse().ok()?;
            Mask::parse(&format!("{:x} {:x}", (number >> 64) as u64, number as u64))
        };
        let absolute = to_mask(&case["abs"]);
        let keys = to_mask(&case["keys"]);
        // A mask the corpus recorded as a >128-bit number cannot be rebuilt
        // this way; those are covered by the live set below.
        if case["abs"].is_string() && absolute.is_none() {
            continue;
        }
        if case["keys"].is_string() && keys.is_none() {
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
    // Thirty-three real devices: keyboards, a webcam, lid switches, power
    // buttons, a touchpad, and one joypad. This is the set that would catch a
    // rule which is right in principle and wrong about a real machine.
    let cases = corpus("live_capabilities");
    assert!(!cases.is_empty(), "the corpus recorded no devices");
    let mut joypads = 0;
    for case in &cases {
        let absolute = case["abs_raw"].as_str().and_then(Mask::parse);
        let keys = case["key_raw"].as_str().and_then(Mask::parse);
        let want = case["joypad"].as_bool().expect("joypad");
        // `None` here means the Python fell back to opening the device, which
        // this cannot replay -- but on the recording machine every input node
        // had both files, so a None is itself a divergence worth failing on.
        let verdict = capability::joypad(absolute.as_ref(), keys.as_ref()).unwrap_or_else(|| {
            panic!(
                "could not judge a device the Python judged: abs {:?}",
                case["abs_raw"]
            )
        });
        assert_eq!(verdict, want, "abs {:?}", case["abs_raw"]);
        joypads += usize::from(verdict);
    }
    assert_eq!(
        joypads,
        cases.iter().filter(|c| c["joypad"] == true).count(),
        "the number of joypads must agree too"
    );
}
