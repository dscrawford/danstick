//! Hold the Switch Pro decode to what the Python decodes.

use std::path::Path;

use evdev::{EventType, InputEvent};
use padmap_input::nintendo::{self, Source};
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../padmap-core/tests/corpus")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

/// A 0x30 report, built as the corpus generator builds one.
fn report(right: u8, shared: u8, left: u8, lx: i32, ly: i32, rx: i32, ry: i32) -> Vec<u8> {
    report_raw(right, shared, left, lx, 4095 - ly, rx, 4095 - ry)
}

fn report_raw(right: u8, shared: u8, left: u8, lx: i32, ly: i32, rx: i32, ry: i32) -> Vec<u8> {
    let mut data = vec![0u8; 64];
    data[0] = 0x30;
    data[3] = right;
    data[4] = shared;
    data[5] = left;
    for (at, x, y) in [(6, lx, ly), (9, rx, ry)] {
        data[at] = (x & 0xFF) as u8;
        data[at + 1] = (((x >> 8) & 0x0F) | ((y & 0x0F) << 4)) as u8;
        data[at + 2] = ((y >> 4) & 0xFF) as u8;
    }
    data
}

fn neutral() -> Vec<u8> {
    report(0, 0, 0, 2048, 2048, 2048, 2048)
}

struct Decoder(Source);

impl Decoder {
    fn new() -> Option<Decoder> {
        use rustix::pty::{grantpt, openpt, ptsname, unlockpt, OpenptFlags};
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).ok()?;
        grantpt(&master).ok()?;
        unlockpt(&master).ok()?;
        let name = ptsname(&master, Vec::new()).ok()?;
        let slave = std::path::PathBuf::from(name.to_string_lossy().into_owned());
        let source = Source::open(&slave).ok()?;
        std::mem::forget(master);
        Some(Decoder(source))
    }
}

macro_rules! decoder {
    () => {
        match Decoder::new() {
            Some(decoder) => decoder,
            None => {
                eprintln!("skipping: no usable /dev/ptmx here");
                return;
            }
        }
    };
}

fn triples(events: &[InputEvent]) -> Vec<[i32; 3]> {
    events
        .iter()
        .filter(|event| event.event_type() != EventType::SYNCHRONIZATION)
        .map(|event| {
            [
                i32::from(event.event_type().0),
                i32::from(event.code()),
                event.value(),
            ]
        })
        .collect()
}

fn want(case: &Value) -> Vec<[i32; 3]> {
    case["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| {
            let row = event.as_array().expect("[type, code, value]");
            [
                row[0].as_i64().expect("type") as i32,
                row[1].as_i64().expect("code") as i32,
                row[2].as_i64().expect("value") as i32,
            ]
        })
        .collect()
}

#[test]
fn every_recorded_report_decodes_to_the_same_events() {
    for case in corpus("switch_decode") {
        let mut decoder = decoder!();
        let mut settle = Vec::new();
        decoder.0.decode_for_test(&neutral(), &mut settle);

        let data = match (case["byte"].as_str(), case["bit"].as_i64()) {
            (Some("right"), Some(bit)) => report(bit as u8, 0, 0, 2048, 2048, 2048, 2048),
            (Some("shared"), Some(bit)) => report(0, bit as u8, 0, 2048, 2048, 2048, 2048),
            (Some("left"), Some(bit)) => report(0, 0, bit as u8, 2048, 2048, 2048, 2048),
            (Some("a and b"), _) => report(0x0C, 0, 0, 2048, 2048, 2048, 2048),
            (Some("up"), _) => report(0, 0, 0x02, 2048, 2048, 2048, 2048),
            (Some("up and down"), _) => report(0, 0, 0x03, 2048, 2048, 2048, 2048),
            (Some("left and right"), _) => report(0, 0, 0x0C, 2048, 2048, 2048, 2048),
            (Some("up and right"), _) => report(0, 0, 0x06, 2048, 2048, 2048, 2048),
            (Some("all four"), _) => report(0, 0, 0x0F, 2048, 2048, 2048, 2048),
            (Some("sticks pushed"), _) => report(0, 0, 0, 4095, 4095, 0, 0),
            (Some("sticks at zero"), _) => report(0, 0, 0, 0, 0, 0, 0),
            (Some("a nudge inside the fuzz"), _) => report(0, 0, 0, 2050, 2048, 2048, 2048),
            (Some("a move past the fuzz"), _) => report(0, 0, 0, 2100, 2048, 2048, 2048),
            other => panic!("unhandled case {other:?}"),
        };
        let mut got = Vec::new();
        decoder.0.decode_for_test(&data, &mut got);
        assert_eq!(triples(&got), want(&case), "case {:?}", case["byte"]);
    }
}

#[test]
fn the_opening_frame_says_where_the_sticks_are_and_nothing_else() {
    let mut decoder = decoder!();
    let mut got = Vec::new();
    decoder.0.decode_for_test(&neutral(), &mut got);
    let recorded = corpus("switch_first_frame");
    assert_eq!(triples(&got), want(&recorded[0]));
}

#[test]
fn y_is_inverted_and_x_is_not() {
    let state = nintendo::decode_state(&report_raw(0, 0, 0, 3000, 4095, 1000, 0)).expect("decodes");
    assert_eq!(state.left_x, 3000, "X passes through");
    assert_eq!(state.left_y, 0, "full up on the wire is zero in evdev");
    assert_eq!(state.right_x, 1000);
    assert_eq!(state.right_y, 4095, "full down on the wire is the maximum");
}

#[test]
fn a_report_too_short_to_decode_is_refused_rather_than_indexed() {
    for length in 0..12 {
        assert!(
            nintendo::decode_state(&vec![0x30u8; length]).is_none(),
            "{length} bytes should not decode"
        );
    }
    assert!(nintendo::decode_state(&[0x30u8; 12]).is_some());
}

#[test]
fn opposite_dpad_directions_cancel() {
    assert_eq!(nintendo::hat_for(0), (0, 0));
    assert_eq!(nintendo::hat_for(0x02), (0, -1));
    assert_eq!(nintendo::hat_for(0x01), (0, 1));
    assert_eq!(nintendo::hat_for(0x08), (-1, 0));
    assert_eq!(nintendo::hat_for(0x04), (1, 0));
    assert_eq!(nintendo::hat_for(0x03), (0, 0), "up and down");
    assert_eq!(nintendo::hat_for(0x0C), (0, 0), "left and right");
    assert_eq!(nintendo::hat_for(0x0F), (0, 0), "all four");
}

#[test]
fn the_mode_request_is_the_packet_the_python_writes() {
    let packet = nintendo::full_mode_packet(0);
    assert_eq!(packet.len(), 64);
    assert_eq!(packet[0], 0x01);
    assert_eq!(packet[1], 0, "the rolling counter");
    assert_eq!(
        &packet[2..10],
        &[0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40],
        "the neutral rumble frame some firmware insists on"
    );
    assert_eq!(packet[10], 0x03, "set input report mode");
    assert_eq!(packet[11], 0x30, "to the full report");
    assert!(packet[12..].iter().all(|byte| *byte == 0));
    assert_eq!(nintendo::full_mode_packet(0x0F)[1], 0x0F);
    assert_eq!(nintendo::full_mode_packet(0x10)[1], 0x00);
}

#[test]
fn every_override_file_parses_the_same() {
    for case in corpus("hidraw_overrides") {
        let text = case["file"].as_str().unwrap_or_default();
        let ours = nintendo::parse_overrides(text);
        let want = case["overrides"].as_object().expect("overrides");
        assert_eq!(ours.len(), want.len(), "for {text:?}");
        for (key, value) in want {
            assert_eq!(ours.get(key), value.as_bool().as_ref(), "{key} in {text:?}");
        }
    }
}

#[test]
fn an_override_that_is_not_a_boolean_is_ignored() {
    assert!(nintendo::parse_overrides(r#"{"057e:2009": 1}"#).is_empty());
    assert!(nintendo::parse_overrides(r#"{"057e:2009": "yes"}"#).is_empty());
    assert!(nintendo::parse_overrides(r#"{"057e:2009": null}"#).is_empty());
    assert_eq!(
        nintendo::parse_overrides(r#"{"057E:2009": true}"#).get("057e:2009"),
        Some(&true)
    );
}
