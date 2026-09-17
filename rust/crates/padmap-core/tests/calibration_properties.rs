//! `AxisCalibration::apply` runs per `EV_ABS` event: it must never panic and
//! never leave `minimum..=maximum` (a +-2^40 profile once killed the daemon mid-game).

use padmap_core::calibration::{AxisCalibration, EVDEV_VALUE_MAX, EVDEV_VALUE_MIN};
use proptest::prelude::*;

fn cal(
    center: i32,
    minimum: i32,
    maximum: i32,
    flat: i32,
    reach: (Option<i32>, Option<i32>),
) -> AxisCalibration {
    AxisCalibration {
        center,
        minimum,
        maximum,
        flat,
        reach_min: reach.0,
        reach_max: reach.1,
    }
}

/// Declared ranges real absinfo hands out; uniform i32 pairs would never hit the two that run.
const DECLARED_RANGES: &[(i32, i32)] = &[
    (0, 1),
    (0, 255),
    (-128, 127),
    (0, 1023),
    (-32768, 32767),
    (-32767, 32767),
    (0, 65535),
];

fn plausible_calibration() -> impl Strategy<Value = AxisCalibration> {
    proptest::sample::select(DECLARED_RANGES).prop_flat_map(|(minimum, maximum)| {
        let span = i64::from(maximum) - i64::from(minimum);
        let widest_band = i32::try_from(span / 8).unwrap_or(i32::MAX).max(1);
        (
            minimum..=maximum,
            0..=widest_band,
            prop::option::of(minimum..=maximum),
            prop::option::of(minimum..=maximum),
        )
            .prop_map(move |(center, flat, reach_min, reach_max)| {
                cal(center, minimum, maximum, flat, (reach_min, reach_max))
            })
    })
}

/// A hand-edited or lying profile: every field arbitrary, with `minimum <= maximum` forced.
fn adversarial_calibration() -> impl Strategy<Value = AxisCalibration> {
    (
        any::<i32>(),
        any::<i32>(),
        any::<i32>(),
        any::<i32>(),
        prop::option::of(any::<i32>()),
        prop::option::of(any::<i32>()),
    )
        .prop_map(|(center, first, second, flat, reach_min, reach_max)| {
            cal(
                center,
                first.min(second),
                first.max(second),
                flat,
                (reach_min, reach_max),
            )
        })
}

fn any_calibration() -> impl Strategy<Value = AxisCalibration> {
    prop_oneof![3 => plausible_calibration(), 1 => adversarial_calibration()]
}

/// A negative `flat` is an inverted band, pinned separately; centred-at-rest properties want a real one.
fn calibration_with_a_real_dead_band() -> impl Strategy<Value = AxisCalibration> {
    any_calibration().prop_map(|cal| AxisCalibration {
        flat: cal.flat.saturating_abs(),
        ..cal
    })
}

fn any_reading() -> impl Strategy<Value = i32> {
    prop_oneof![1 => Just(i32::MIN), 1 => Just(i32::MAX), 4 => any::<i32>(), 10 => -70_000i32..=70_000i32]
}

/// Generated jointly: filtering independent pairs would discard almost every case.
fn inside_the_dead_band() -> impl Strategy<Value = (AxisCalibration, i32)> {
    calibration_with_a_real_dead_band().prop_flat_map(|cal| {
        let lowest = cal.center.saturating_sub(cal.flat);
        let highest = cal.center.saturating_add(cal.flat);
        (Just(cal), lowest..=highest)
    })
}

fn midpoint_of(cal: &AxisCalibration) -> i32 {
    i32::try_from(cal.midpoint()).expect("a midpoint of two i32s fits in an i32")
}

fn low_of(cal: &AxisCalibration) -> i32 {
    i32::try_from(cal.low()).expect("low() is a stored i32 clamped to centre")
}

fn high_of(cal: &AxisCalibration) -> i32 {
    i32::try_from(cal.high()).expect("high() is a stored i32 clamped to centre")
}

/// Python's `//` by two, written independently of `midpoint` so the property compares two things.
fn floor_halve(sum: i64) -> i64 {
    if sum >= 0 {
        sum / 2
    } else {
        (sum - 1) / 2
    }
}

/// Sampled readings across and past the travel, plus every branch boundary and the i32 limits.
fn sweep(cal: &AxisCalibration) -> Vec<i32> {
    const SAMPLES: i64 = 192;
    let low = cal.low();
    let high = cal.high();
    let margin = ((high - low) / 8).max(4);
    let from = low - margin;
    let to = high + margin;
    let step = ((to - from) / SAMPLES).max(1);
    let mut points: Vec<i64> = (0..)
        .map(|n| from + n * step)
        .take_while(|at| *at < to)
        .collect();
    points.push(to);
    for anchor in [
        i64::from(cal.center),
        i64::from(cal.center) - i64::from(cal.flat),
        i64::from(cal.center) + i64::from(cal.flat),
        i64::from(cal.minimum),
        i64::from(cal.maximum),
        low,
        high,
    ] {
        points.extend([-2, -1, 0, 1, 2].map(|delta| anchor.saturating_add(delta)));
    }
    points.push(i64::from(i32::MIN));
    points.push(i64::from(i32::MAX));
    points.sort_unstable();
    points.dedup();
    points
        .into_iter()
        .filter_map(|value| i32::try_from(value).ok())
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn no_reading_at_all_can_make_apply_panic(cal in any_calibration(), value in any_reading()) {
        let out = std::hint::black_box(cal.apply(value));
        prop_assert!(i64::from(out) >= EVDEV_VALUE_MIN, "an answer, of some kind: {out}");
    }

    /// The invariant that lets `fits` be a load-time check on six numbers instead of per-event.
    #[test]
    fn nothing_apply_can_return_falls_outside_the_declared_range(
        cal in any_calibration(),
        value in any_reading(),
    ) {
        let out = cal.apply(value);
        prop_assert!(
            (cal.minimum..=cal.maximum).contains(&out),
            "{value} produced {out}, outside {}..={}",
            cal.minimum,
            cal.maximum
        );
    }

    #[test]
    fn apply_never_moves_backwards_as_the_reading_rises(cal in any_calibration()) {
        let mut previous: Option<(i32, i32)> = None;
        for value in sweep(&cal) {
            let out = cal.apply(value);
            if let Some((before, was)) = previous {
                prop_assert!(out >= was, "{before} -> {value} went backwards: {was} -> {out}");
            }
            previous = Some((value, out));
        }
    }

    #[test]
    fn the_resting_position_always_reads_as_the_declared_midpoint(
        cal in calibration_with_a_real_dead_band(),
    ) {
        prop_assert_eq!(cal.apply(cal.center), midpoint_of(&cal), "the rest position did not come out centred");
    }

    #[test]
    fn every_reading_within_flat_of_centre_reads_as_the_midpoint((cal, value) in inside_the_dead_band()) {
        prop_assert_eq!(
            cal.apply(value),
            midpoint_of(&cal),
            "a reading of {} escaped a band of {} around {}",
            value,
            cal.flat,
            cal.center
        );
    }

    #[test]
    fn the_ends_of_the_measured_reach_map_onto_the_declared_stops(
        cal in calibration_with_a_real_dead_band(),
    ) {
        let low = cal.low();
        let high = cal.high();
        let centre = i64::from(cal.center);
        let flat = i64::from(cal.flat);
        // A degenerate side collapses to the midpoint instead; that is the next property.
        if centre - flat - low > 0 {
            prop_assert_eq!(cal.apply(low_of(&cal)), cal.minimum, "full deflection to {} missed the bottom stop", low);
        }
        if high - centre - flat > 0 {
            prop_assert_eq!(cal.apply(high_of(&cal)), cal.maximum, "full deflection to {} missed the top stop", high);
        }
    }

    #[test]
    fn a_reading_past_the_measured_reach_is_clamped_rather_than_extrapolated(
        cal in calibration_with_a_real_dead_band(),
        overshoot in 1i64..=100_000i64,
    ) {
        let centre = i64::from(cal.center);
        let flat = i64::from(cal.flat);
        if centre - flat - cal.low() > 0 {
            let past = i32::try_from((cal.low() - overshoot).max(i64::from(i32::MIN))).expect("clamped");
            prop_assert_eq!(cal.apply(past), cal.minimum, "an overshoot below the reach");
        }
        if cal.high() - centre - flat > 0 {
            let past = i32::try_from((cal.high() + overshoot).min(i64::from(i32::MAX))).expect("clamped");
            prop_assert_eq!(cal.apply(past), cal.maximum, "an overshoot above the reach");
        }
    }

    #[test]
    fn a_side_with_no_travel_collapses_to_the_midpoint_instead_of_dividing_by_zero(
        cal in plausible_calibration(),
        value in any_reading(),
    ) {
        let pinned = AxisCalibration {
            reach_min: Some(cal.center),
            reach_max: Some(cal.center),
            flat: cal.flat.max(0),
            ..cal
        };
        prop_assert_eq!(pinned.apply(value), midpoint_of(&pinned), "a zero-width span produced something else");
    }

    #[test]
    fn a_range_of_zero_width_answers_with_its_single_value(
        center in any::<i32>(),
        only in any::<i32>(),
        flat in 0i32..=1000i32,
        value in any_reading(),
    ) {
        prop_assert_eq!(cal(center, only, only, flat, (None, None)).apply(value), only, "a stuck axis moved");
    }

    /// Without the clamp the span goes negative, the scale inverts, and pushing left moves right.
    #[test]
    fn the_measured_reach_can_never_cross_the_recorded_centre(cal in adversarial_calibration()) {
        let centre = i64::from(cal.center);
        prop_assert!(cal.low() <= centre, "low() {} rose above centre {centre}", cal.low());
        prop_assert!(cal.high() >= centre, "high() {} fell below centre {centre}", cal.high());
        prop_assert!(cal.low() <= cal.high(), "the travel inverted");
    }

    #[test]
    fn a_reading_below_centre_never_reads_above_the_middle(
        cal in calibration_with_a_real_dead_band(),
        value in any_reading(),
    ) {
        let out = cal.apply(value);
        let mid = midpoint_of(&cal);
        if value < cal.center {
            prop_assert!(out <= mid, "{value} is below rest but read {out} > {mid}");
        } else {
            prop_assert!(out >= mid, "{value} is at or above rest but read {out} < {mid}");
        }
    }

    /// Always true for i32 fields; the guard stays because it is what a profile loader calls.
    #[test]
    fn fits_agrees_with_checking_every_stored_number_by_hand(cal in adversarial_calibration()) {
        let stored: Vec<i64> =
            [Some(cal.center), Some(cal.minimum), Some(cal.maximum), Some(cal.flat), cal.reach_min, cal.reach_max]
                .into_iter()
                .flatten()
                .map(i64::from)
                .collect();
        let by_hand = stored.iter().all(|value| (EVDEV_VALUE_MIN..=EVDEV_VALUE_MAX).contains(value));
        prop_assert_eq!(cal.fits(), by_hand);
        prop_assert!(by_hand, "every i32 is inside the evdev value range by construction");
    }

    /// Truncation would seed every signed-range virtual pad one count off centre.
    #[test]
    fn the_midpoint_floors_where_truncation_would_round_towards_zero(
        minimum in any::<i32>(),
        maximum in any::<i32>(),
        center in any::<i32>(),
    ) {
        let cal = AxisCalibration::new(center, minimum, maximum);
        let sum = i64::from(minimum) + i64::from(maximum);
        prop_assert_eq!(cal.midpoint(), floor_halve(sum));
        prop_assert!(cal.midpoint() * 2 <= sum, "a floored half can never exceed half the sum");
    }

    #[test]
    fn a_calibration_survives_a_json_round_trip_unchanged(cal in adversarial_calibration()) {
        let text = serde_json::to_string(&cal).expect("a calibration serialises");
        let back: AxisCalibration = serde_json::from_str(&text).expect("and parses back");
        prop_assert_eq!(cal, back);
    }
}

#[test]
fn a_true_centred_pad_moves_nothing_it_does_not_have_to() {
    let cal = AxisCalibration::new(128, 0, 255);
    assert_eq!(
        cal.apply(128),
        127,
        "the declared midpoint of 0..255 floors"
    );
    assert_eq!(cal.apply(0), 0);
    assert_eq!(cal.apply(255), 255);
    for value in 0..=255 {
        let out = cal.apply(value);
        assert!((out - value).abs() <= 1, "{value} was displaced to {out}");
    }
}

#[test]
fn the_worn_n64_stick_resting_at_174_still_reaches_both_of_its_stops() {
    // Measured: rests at 174 on 0-255, 36% deflection, which scrolls a menu forever uncorrected.
    let cal = AxisCalibration::new(174, 0, 255);
    assert_eq!(cal.apply(174), 127, "rest must read as the middle");
    assert_eq!(cal.apply(0), 0, "full left");
    assert_eq!(cal.apply(255), 255, "full right");
    assert!(
        cal.apply(173) < 127 && cal.apply(175) > 127,
        "either side of rest"
    );
}

#[test]
fn a_gamecube_trigger_resting_at_its_minimum_reads_centred_until_it_is_pressed() {
    // All of its travel is on one side; the bottom span is zero and the division is skipped.
    let trigger = AxisCalibration::new(0, 0, 255);
    assert_eq!(trigger.low(), 0, "there is no travel below rest");
    assert_eq!(trigger.high(), 255);
    assert_eq!(
        trigger.apply(0),
        127,
        "rest is the midpoint, as for any axis"
    );
    assert_eq!(trigger.apply(255), 255, "fully pressed reaches the stop");
    assert_eq!(
        trigger.apply(-1),
        127,
        "noise below rest falls back to the middle"
    );
    assert_eq!(trigger.apply(i32::MIN), 127);
}

#[test]
fn an_adapter_that_declares_more_travel_than_the_stick_has_still_gets_full_left() {
    // Declares 0-255, physically emits 160-255: five counts of travel left of rest.
    let declared = AxisCalibration::new(200, 0, 255);
    let measured = AxisCalibration::new(200, 0, 255).with_reach(160, 255);
    assert!(
        declared.apply(160) > measured.apply(160),
        "the measured reach must open the short side up"
    );
    assert_eq!(measured.apply(160), 0, "full left now reaches the stop");
    assert_eq!(measured.apply(200), 127, "rest is still the middle");
    assert_eq!(measured.apply(255), 255, "full right is unchanged");
}

#[test]
fn a_signed_sixteen_bit_axis_centres_on_the_declared_midpoint_not_on_zero() {
    let cal = AxisCalibration::new(0, -32768, 32767);
    assert_eq!(cal.midpoint(), -1, "(-32768 + 32767) // 2");
    assert_eq!(cal.apply(0), -1, "the declared midpoint, not zero");
    assert_eq!(cal.apply(-32768), -32768);
    assert_eq!(cal.apply(32767), 32767);
    assert_eq!(cal.apply(i32::MIN), -32768, "clamped, not wrapped");
    assert_eq!(cal.apply(i32::MAX), 32767);
}

#[test]
fn a_dead_band_costs_the_stick_none_of_its_travel() {
    // Offset and span both come from the band edge, or full deflection stops `flat` counts short.
    for band in [10, 20] {
        let cal = AxisCalibration::new(128, 0, 255).with_flat(band);
        for value in 128 - band..=128 + band {
            assert_eq!(cal.apply(value), 127, "{value} escaped a band of {band}");
        }
        assert_ne!(cal.apply(128 - band - 1), 127, "the band has an edge");
        assert_ne!(cal.apply(128 + band + 1), 127);
        assert_eq!(cal.apply(0), 0, "full left still reaches the stop");
        assert_eq!(cal.apply(255), 255, "full right still reaches the stop");
        let mut previous = cal.apply(0);
        for value in 0..=255 {
            let out = cal.apply(value);
            assert!(
                out >= previous,
                "band {band}: {value} went backwards: {previous} -> {out}"
            );
            previous = out;
        }
    }
}

#[test]
fn a_dead_band_wider_than_the_axis_flattens_every_reading_to_the_midpoint() {
    let swallowed = AxisCalibration::new(128, 0, 255).with_flat(1000);
    for value in [i32::MIN, -5000, 0, 128, 255, 5000, i32::MAX] {
        assert_eq!(
            swallowed.apply(value),
            127,
            "{value} escaped a band of 1000"
        );
    }
}

#[test]
fn a_negative_dead_band_widens_the_scale_instead_of_narrowing_it() {
    // Matches the Python: `abs(..) <= flat` is never true, so the band edges move outward from centre.
    let inverted_band = AxisCalibration::new(128, 0, 255).with_flat(-10);
    assert_eq!(
        inverted_band.apply(128),
        136,
        "rest is pushed off the middle"
    );
    assert_eq!(inverted_band.apply(127), 117);
    assert_eq!(inverted_band.apply(0), 0);
    assert_eq!(inverted_band.apply(255), 255);
}

#[test]
fn a_reach_recorded_entirely_on_one_side_of_centre_collapses_that_side() {
    let cal = AxisCalibration::new(128, 0, 255).with_reach(200, 240);
    assert_eq!(cal.low(), 128, "the reach cannot be dragged past centre");
    assert_eq!(cal.high(), 240);
    assert_eq!(
        cal.apply(100),
        127,
        "a span of zero collapses to the middle"
    );
    assert_eq!(
        cal.apply(240),
        255,
        "the side that does have travel still works"
    );
}

#[test]
fn a_reach_recorded_inverted_is_still_pulled_back_around_centre() {
    let cal = AxisCalibration::new(128, 0, 255).with_reach(240, 20);
    assert_eq!((cal.low(), cal.high()), (128, 128));
    for value in [0, 100, 128, 200, 255] {
        assert_eq!(cal.apply(value), 127, "an inverted reach leaves no travel");
    }
}

#[test]
fn an_overshoot_past_the_measured_reach_is_clamped_to_the_declared_range() {
    let cal = AxisCalibration::new(128, 0, 255).with_reach(40, 200);
    for (value, want) in [
        (40, 0),
        (200, 255),
        (0, 0),
        (255, 255),
        (-5000, 0),
        (5000, 255),
        (i32::MIN, 0),
        (i32::MAX, 255),
    ] {
        assert_eq!(cal.apply(value), want, "apply({value})");
    }
}

#[test]
fn a_tie_rounds_to_even_as_python_does_where_f64_round_would_go_away_from_zero() {
    // Each reading lands on an exact binary half; `apply` uses `round_ties_even` for parity.
    for (name, cal, reading, want) in [
        ("2.5 -> 2", cal(0, 0, 3, 0, (None, Some(4))), 3, 2),
        ("0.5 -> 0", cal(0, 0, 1, 0, (None, Some(2))), 1, 0),
        ("-0.5 -> 0", cal(0, -1, 1, 0, (Some(-2), None)), -1, 0),
        ("-2.5 -> -2", cal(0, -3, 1, 0, (Some(-4), None)), -3, -2),
    ] {
        assert_eq!(cal.apply(reading), want, "{name}");
    }
}

#[test]
fn the_midpoint_floors_towards_negative_infinity_across_zero() {
    assert_eq!(AxisCalibration::new(0, -32768, 32767).midpoint(), -1);
    assert_eq!((-32768i64 + 32767).div_euclid(2), -1, "and not 0");
    assert_eq!(AxisCalibration::new(0, -1, 0).midpoint(), -1);
    assert_eq!(AxisCalibration::new(0, -3, 0).midpoint(), -2);
    assert_eq!(AxisCalibration::new(0, 0, 255).midpoint(), 127);
    assert_eq!(AxisCalibration::new(0, i32::MIN, i32::MAX).midpoint(), -1);
}

#[test]
fn every_range_an_i32_can_hold_is_one_evdev_can_carry() {
    // The i32 extremes are legal __s32 values; the guard must not over-correct.
    let extremes = cal(
        0,
        i32::MIN,
        i32::MAX,
        i32::MAX,
        (Some(i32::MIN), Some(i32::MAX)),
    );
    assert!(extremes.fits(), "the i32 extremes themselves are writable");
    assert!(AxisCalibration::new(0, 0, 255).fits());
    assert_eq!(
        EVDEV_VALUE_MIN,
        i64::from(i32::MIN),
        "the evdev floor is an __s32 floor"
    );
    assert_eq!(EVDEV_VALUE_MAX, i64::from(i32::MAX));
}

#[test]
fn a_calibration_round_trips_through_the_exact_python_json_keys() {
    let raw = serde_json::json!({"center": 174, "min": 0, "max": 255, "flat": 4, "reach_min": 20, "reach_max": 250});
    let parsed: AxisCalibration = serde_json::from_value(raw.clone()).expect("parse");
    assert_eq!(parsed, cal(174, 0, 255, 4, (Some(20), Some(250))));
    assert_eq!(serde_json::to_value(parsed).expect("serialize"), raw);
}

#[test]
fn an_unmeasured_reach_reads_back_as_unmeasured_rather_than_as_zero() {
    // `Some(0)` would pin the bottom of travel to zero on every pre-reach profile.
    let raw = serde_json::json!({"center": 128, "min": 0, "max": 255});
    let parsed: AxisCalibration = serde_json::from_value(raw).expect("parse");
    assert_eq!((parsed.reach_min, parsed.reach_max), (None, None));
    assert_eq!(
        (parsed.low(), parsed.high()),
        (0, 255),
        "falls back to the declared range"
    );
}

#[test]
fn an_absent_dead_band_reads_back_as_no_band_at_all() {
    let raw = serde_json::json!({"center": 128, "min": 0, "max": 255});
    let parsed: AxisCalibration = serde_json::from_value(raw).expect("parse");
    assert_eq!(parsed.flat, 0);
    assert_ne!(
        parsed.apply(129),
        parsed.apply(128),
        "with no band, one count moves"
    );
}

#[test]
fn a_half_written_profile_missing_a_required_field_is_refused_rather_than_guessed() {
    let raw = serde_json::json!({"min": 0, "max": 255});
    let parsed: Result<AxisCalibration, _> = serde_json::from_value(raw);
    assert!(
        parsed.is_err(),
        "a profile with no recorded centre must not parse"
    );
}

#[test]
fn an_inverted_declared_range_must_not_panic_the_per_event_path() {
    // Regression: `f64::clamp` asserts min <= max. The Python clamps to `minimum` and carries on.
    let inverted = cal(-1, 1, 0, 0, (None, None));
    assert_eq!(inverted.apply(0), 1);
}
