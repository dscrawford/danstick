//! Hold the calibration machine to what the Python computes.
//!
//! Two rules in here, and each was wrong once in a way nothing reported.
//!
//! `calibratable` decides by resting position rather than by axis code,
//! because a GameCube adapter puts its analogue triggers on ABS_RX/ABS_RY --
//! stick codes -- resting at 24 of 0-255, and centring a trigger costs it half
//! its travel.
//!
//! `merge_reach` ignores a direction that never left the dead band, because
//! recording one makes `apply` compute a span of zero and that whole direction
//! reads dead centre. That is the failure measuring reach exists to prevent,
//! produced by measuring reach.
//!
//! Both write a file the Python reads and the Rust reads, so the arithmetic
//! has to match exactly -- including the floor division, which is `//` in one
//! language and not `/` in the other.

use std::path::Path;

use padmap_core::calibration::{AxisCalibration, Declared};
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

fn int(value: &Value) -> i32 {
    value.as_i64().expect("an integer") as i32
}

#[test]
fn a_rest_sample_becomes_the_same_centre_and_dead_band() {
    for case in corpus("calibration_rest") {
        let declared = Declared {
            minimum: int(&case["min"]),
            maximum: int(&case["max"]),
            value: int(&case["value"]),
            flat: int(&case["flat"]),
        };
        let samples = case["samples"].as_array().expect("samples");
        let observed = if samples.is_empty() {
            None
        } else {
            let values: Vec<i32> = samples.iter().map(int).collect();
            Some((
                *values.iter().min().expect("a low"),
                *values.iter().max().expect("a high"),
            ))
        };
        let got = declared.rest_calibration(observed);
        assert_eq!(got.center, int(&case["center"]), "centre, for {case}");
        assert_eq!(got.flat, int(&case["cal_flat"]), "dead band, for {case}");
        assert_eq!(got.minimum, int(&case["cal_min"]));
        assert_eq!(got.maximum, int(&case["cal_max"]));
    }
}

#[test]
fn the_centre_floors_the_way_python_divides() {
    // `(low + high) // 2` floors toward negative infinity; Rust's `/`
    // truncates toward zero, so a centre of -1 and 0 is -1 in one and 0 in
    // the other. Both write the same profile file and a reader cannot tell
    // which produced it, so the difference is a stick that sits one unit off
    // centre depending on which implementation ran the wizard.
    let declared = Declared {
        minimum: -32768,
        maximum: 32767,
        value: 0,
        flat: 0,
    };
    assert_eq!(declared.rest_calibration(Some((-1, 0))).center, -1);
    assert_eq!(declared.rest_calibration(Some((-3, 0))).center, -2);
    assert_eq!(declared.rest_calibration(Some((0, 1))).center, 0);
}

#[test]
fn a_sweep_inside_the_dead_band_is_not_a_measurement() {
    for case in corpus("calibration_reach") {
        let cal = AxisCalibration {
            center: int(&case["center"]),
            minimum: int(&case["min"]),
            maximum: int(&case["max"]),
            flat: int(&case["flat"]),
            reach_min: None,
            reach_max: None,
        };
        let observed = case["reach"]
            .as_array()
            .map(|pair| (int(&pair[0]), int(&pair[1])));
        let got = cal.merge_reach(observed);
        let want = |name: &str| case[name].as_i64().map(|v| v as i32);
        assert_eq!(got.reach_min, want("reach_min"), "reach_min, for {case}");
        assert_eq!(got.reach_max, want("reach_max"), "reach_max, for {case}");
    }
}

#[test]
fn a_direction_that_never_moved_falls_back_to_the_declared_range() {
    // None, not the centre. Pinning it to the centre makes `apply` compute a
    // span of zero for that direction and the stick cannot move that way at
    // all -- which is worse than having no measurement, because no
    // measurement falls back to the range the driver declared.
    let cal = AxisCalibration {
        center: 0,
        minimum: -32768,
        maximum: 32767,
        flat: 128,
        reach_min: None,
        reach_max: None,
    };
    let unmeasured = cal.merge_reach(None);
    assert_eq!(unmeasured.reach_min, None);
    assert_eq!(unmeasured.reach_max, None);

    // And a sweep that only cleared the band one way keeps only that way.
    let half = cal.merge_reach(Some((-30000, 1)));
    assert_eq!(half.reach_min, Some(-30000));
    assert_eq!(half.reach_max, None);
}

#[test]
fn which_axes_are_worth_centring_matches() {
    for case in corpus("calibratable_axes") {
        let mut ours: Vec<u16> = Vec::new();
        for entry in case["axes"].as_array().expect("axes") {
            let row = entry.as_array().expect("[code, min, max, value]");
            let code = int(&row[0]) as u16;
            let declared = Declared {
                minimum: int(&row[1]),
                maximum: int(&row[2]),
                value: int(&row[3]),
                flat: 0,
            };
            if declared.calibratable(code) {
                ours.push(code);
            }
        }
        ours.sort_unstable();
        let want: Vec<u16> = case["calibratable"]
            .as_array()
            .expect("calibratable")
            .iter()
            .map(|code| int(code) as u16)
            .collect();
        assert_eq!(ours, want, "{}", case["what"]);
    }
}

#[test]
fn the_gamecube_adapters_triggers_are_not_taken_for_sticks() {
    // ABS_RX and ABS_RY are stick codes. On that adapter they are the
    // analogue triggers, resting at 24 and 25 of 0-255, and centring one
    // makes it read half pressed while untouched.
    for (code, rest) in [(0x03u16, 24), (0x04u16, 25)] {
        let trigger = Declared {
            minimum: 0,
            maximum: 255,
            value: rest,
            flat: 0,
        };
        assert!(
            !trigger.calibratable(code),
            "axis {code:#x} resting at {rest} of 0-255 is a trigger"
        );
    }
    // The same adapter's actual sticks do centre, and must still be caught.
    let stick = Declared {
        minimum: 0,
        maximum: 255,
        value: 128,
        flat: 0,
    };
    assert!(stick.calibratable(0x00));
}
