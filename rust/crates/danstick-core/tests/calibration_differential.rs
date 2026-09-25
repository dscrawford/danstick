//! Hold the calibration wizard's arithmetic to what the Python computes.

use std::path::Path;

use danstick_core::calibration::{AxisCalibration, Declared};
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/corpus/{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

fn int(value: &Value) -> i32 {
    value.as_i64().expect("an integer") as i32
}

fn declared(minimum: i32, maximum: i32, value: i32, flat: i32) -> Declared {
    Declared {
        minimum,
        maximum,
        value,
        flat,
    }
}

fn unmeasured(center: i32, minimum: i32, maximum: i32, flat: i32) -> AxisCalibration {
    AxisCalibration {
        center,
        minimum,
        maximum,
        flat,
        reach_min: None,
        reach_max: None,
    }
}

#[test]
fn a_rest_sample_becomes_the_same_centre_and_dead_band() {
    for case in corpus("calibration_rest") {
        let d = declared(
            int(&case["min"]),
            int(&case["max"]),
            int(&case["value"]),
            int(&case["flat"]),
        );
        let samples: Vec<i32> = case["samples"]
            .as_array()
            .expect("samples")
            .iter()
            .map(int)
            .collect();
        let observed = samples
            .iter()
            .min()
            .zip(samples.iter().max())
            .map(|(lo, hi)| (*lo, *hi));
        let got = d.rest_calibration(observed);
        assert_eq!(got.center, int(&case["center"]), "centre, for {case}");
        assert_eq!(got.flat, int(&case["cal_flat"]), "dead band, for {case}");
        assert_eq!(got.minimum, int(&case["cal_min"]));
        assert_eq!(got.maximum, int(&case["cal_max"]));
    }
}

#[test]
fn the_centre_floors_the_way_python_divides() {
    let d = declared(-32768, 32767, 0, 0);
    assert_eq!(d.rest_calibration(Some((-1, 0))).center, -1);
    assert_eq!(d.rest_calibration(Some((-3, 0))).center, -2);
    assert_eq!(d.rest_calibration(Some((0, 1))).center, 0);
}

#[test]
fn a_sweep_inside_the_dead_band_is_not_a_measurement() {
    for case in corpus("calibration_reach") {
        let cal = unmeasured(
            int(&case["center"]),
            int(&case["min"]),
            int(&case["max"]),
            int(&case["flat"]),
        );
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
    // Pinning it to centre gives `apply` a zero span and kills that direction outright.
    let cal = unmeasured(0, -32768, 32767, 128);
    let none = cal.merge_reach(None);
    assert_eq!((none.reach_min, none.reach_max), (None, None));
    let half = cal.merge_reach(Some((-30000, 1)));
    assert_eq!((half.reach_min, half.reach_max), (Some(-30000), None));
}

#[test]
fn which_axes_are_worth_centring_matches() {
    for case in corpus("calibratable_axes") {
        let mut ours: Vec<u16> = case["axes"]
            .as_array()
            .expect("axes")
            .iter()
            .map(|entry| entry.as_array().expect("[code, min, max, value]"))
            .filter(|row| {
                declared(int(&row[1]), int(&row[2]), int(&row[3]), 0)
                    .calibratable(int(&row[0]) as u16)
            })
            .map(|row| int(&row[0]) as u16)
            .collect();
        ours.sort_unstable();
        let want: Vec<u16> = case["calibratable"]
            .as_array()
            .expect("calibratable")
            .iter()
            .map(|c| int(c) as u16)
            .collect();
        assert_eq!(ours, want, "{}", case["what"]);
    }
}

#[test]
fn the_gamecube_adapters_triggers_are_not_taken_for_sticks() {
    for (code, rest) in [(0x03u16, 24), (0x04u16, 25)] {
        assert!(
            !declared(0, 255, rest, 0).calibratable(code),
            "axis {code:#x} resting at {rest} is a trigger"
        );
    }
    assert!(declared(0, 255, 128, 0).calibratable(0x00));
}
