//! Everything `AxisCalibration` must be true of, for every input rather than
//! for a chosen few.
//!
//! [`AxisCalibration::apply`] is the only function in this crate that runs per
//! controller event: the republisher calls it for every `EV_ABS` passing
//! through. That gives it two obligations an ordinary function does not have.
//! It must never panic -- `panic = "unwind"` and a `catch_unwind` at the
//! forwarding boundary exist so that one bad reading cannot end the daemon,
//! but an unwind per event is still a stall -- and it must never return a
//! value the uinput write will refuse.
//!
//! The second obligation has a name and a date. A stored profile declaring a
//! range of +-2^40 made `apply(150)` return 1099511627776; the uinput write
//! refused it with an `OverflowError`, which is neither an `OSError` nor a
//! `ValueError`, so every guard between the profile store and the write
//! missed it; the daemon exited mid-game and every player's controller went
//! dead at once. `fits` is the check that stands in front of that, and the
//! reason it can be a load-time check on the stored numbers rather than a
//! per-event check on each result is precisely the range invariant asserted
//! here: `apply` cannot leave `minimum..=maximum`.
//!
//! Two smaller wounds shape the rest of the file. An adapter can declare
//! 0-255 while the physical stick only ever emits 160-255, so scaling against
//! the declared range leaves almost no travel on one side and the stick
//! cannot go left at all -- hence `reach_min`/`reach_max`. And a worn N64
//! stick rests at 174 on a 0-255 axis that nominally centres at 128, 36%
//! deflection, so an uncorrected pad reads permanently pushed and a front-end
//! acts on it immediately: runaway menu navigation before anyone has touched
//! the controller.

use padmap_core::calibration::{AxisCalibration, EVDEV_VALUE_MAX, EVDEV_VALUE_MIN};
use proptest::prelude::*;

// -- strategies --------------------------------------------------------------

/// Declared ranges real absinfo hands out.
///
/// Generating `minimum`/`maximum` uniformly at random would spend almost every
/// case on a range no controller has, and none on the two that actually run:
/// an unsigned byte axis and a signed 16-bit one.
const DECLARED_RANGES: &[(i32, i32)] = &[
    (0, 1),
    (0, 255),
    (-128, 127),
    (0, 1023),
    (-32768, 32767),
    (-32767, 32767),
    (0, 65535),
];

/// A profile that could plausibly have come off a real pad.
///
/// Centre inside the declared range, a dead band that is a small fraction of
/// the travel, and a measured reach that may or may not have been recorded.
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
            .prop_map(
                move |(center, flat, reach_min, reach_max)| AxisCalibration {
                    center,
                    minimum,
                    maximum,
                    flat,
                    reach_min,
                    reach_max,
                },
            )
    })
}

/// A profile nobody would write on purpose, which is why it has to be tested.
///
/// A profile is a JSON file under the user's data dir. It can be hand-edited,
/// it can have been written by an older padmap, and it can have been captured
/// from a pad that lied in its absinfo. Every field is therefore an arbitrary
/// `i32` -- except that `minimum <= maximum` is forced, because an inverted
/// declared range currently panics. See
/// `an_inverted_declared_range_must_not_panic_the_per_event_path`, which is
/// ignored rather than deleted: it is a reported bug, not a fixed one.
fn adversarial_calibration() -> impl Strategy<Value = AxisCalibration> {
    (
        any::<i32>(),
        any::<i32>(),
        any::<i32>(),
        any::<i32>(),
        prop::option::of(any::<i32>()),
        prop::option::of(any::<i32>()),
    )
        .prop_map(
            |(center, first, second, flat, reach_min, reach_max)| AxisCalibration {
                center,
                minimum: first.min(second),
                maximum: first.max(second),
                flat,
                reach_min,
                reach_max,
            },
        )
}

/// Both sets at once, so every whole-range invariant sees the plausible cases
/// densely and the hostile ones as well.
fn any_calibration() -> impl Strategy<Value = AxisCalibration> {
    prop_oneof![
        3 => plausible_calibration(),
        1 => adversarial_calibration(),
    ]
}

/// Any calibration, with the dead band the right way round.
///
/// A negative `flat` is not a narrower band, it is an inverted one: the
/// comparison `abs(value - center) <= flat` is then never true, the band
/// edges move outward from centre, and rest no longer reads as the midpoint.
/// That behaviour is faithful to the Python and is pinned by
/// `a_negative_dead_band_widens_the_scale_instead_of_narrowing_it`; the
/// properties about resting at centre are about real bands.
fn calibration_with_a_real_dead_band() -> impl Strategy<Value = AxisCalibration> {
    any_calibration().prop_map(|cal| AxisCalibration {
        flat: cal.flat.saturating_abs(),
        ..cal
    })
}

/// A raw reading, weighted towards the neighbourhood a real axis lives in but
/// including the values an `__s32` can carry at its limits.
fn any_reading() -> impl Strategy<Value = i32> {
    prop_oneof![
        1 => Just(i32::MIN),
        1 => Just(i32::MAX),
        4 => any::<i32>(),
        10 => -70_000i32..=70_000i32,
    ]
}

/// A calibration paired with a reading that is inside its dead band.
///
/// Generating the two independently and filtering would throw away almost
/// every case -- the band is a few counts wide and the reading space is 2^32.
fn inside_the_dead_band() -> impl Strategy<Value = (AxisCalibration, i32)> {
    calibration_with_a_real_dead_band().prop_flat_map(|cal| {
        let lowest = cal.center.saturating_sub(cal.flat);
        let highest = cal.center.saturating_add(cal.flat);
        (Just(cal), lowest..=highest)
    })
}

// -- helpers -----------------------------------------------------------------

/// The declared midpoint, as an `i32`. Safe for any `AxisCalibration`: the
/// midpoint of two `i32`s lies between them.
fn midpoint_of(cal: &AxisCalibration) -> i32 {
    i32::try_from(cal.midpoint()).expect("a midpoint of two i32s fits in an i32")
}

fn low_of(cal: &AxisCalibration) -> i32 {
    i32::try_from(cal.low()).expect("low() is a stored i32 clamped to centre")
}

fn high_of(cal: &AxisCalibration) -> i32 {
    i32::try_from(cal.high()).expect("high() is a stored i32 clamped to centre")
}

/// Floor division by two, spelled out, as Python's `//` does it.
///
/// Kept separate from `midpoint` so the property compares two independent
/// pieces of arithmetic rather than one against itself.
fn floor_halve(sum: i64) -> i64 {
    if sum >= 0 {
        sum / 2
    } else {
        // Rust's `/` truncates towards zero, so an odd negative sum needs
        // nudging down before the division to land on the floor.
        (sum - 1) / 2
    }
}

/// A sorted, deduplicated sweep of readings across and past a calibration's
/// travel, plus the `i32` limits.
///
/// Sampled rather than exhaustive: a 0..65535 axis swept one count at a time
/// under 512 proptest cases is minutes of nothing, and monotonicity is
/// piecewise linear -- the places it can break are the branch boundaries, all
/// of which are anchored below.
fn sweep(cal: &AxisCalibration) -> Vec<i32> {
    const SAMPLES: i64 = 192;

    let low = cal.low();
    let high = cal.high();
    // Past both ends, so the overshoot clamp is exercised by the same sweep.
    let margin = ((high - low) / 8).max(4);
    let from = low - margin;
    let to = high + margin;
    let step = ((to - from) / SAMPLES).max(1);

    let mut points: Vec<i64> = Vec::new();
    let mut at = from;
    while at < to {
        points.push(at);
        at += step;
    }
    points.push(to);
    // Every branch boundary and its immediate neighbours: the dead band edges,
    // the measured reach, the declared stops.
    for anchor in [
        i64::from(cal.center),
        i64::from(cal.center) - i64::from(cal.flat),
        i64::from(cal.center) + i64::from(cal.flat),
        i64::from(cal.minimum),
        i64::from(cal.maximum),
        low,
        high,
    ] {
        for delta in [-2, -1, 0, 1, 2] {
            points.push(anchor.saturating_add(delta));
        }
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

// -- properties --------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// The per-event path cannot be allowed to unwind. `catch_unwind` at the
    /// forwarding boundary keeps a panic from ending the daemon, but a pad
    /// that panics on every reading is a pad that has stopped working.
    #[test]
    fn no_reading_at_all_can_make_apply_panic(
        cal in any_calibration(),
        value in any_reading(),
    ) {
        // Reaching here at all is the assertion. The result is kept alive so
        // no part of the call can be optimised out from under the test.
        let out = std::hint::black_box(cal.apply(value));
        prop_assert!(i64::from(out) >= EVDEV_VALUE_MIN, "an answer, of some kind: {out}");
    }

    /// The invariant the uinput write depends on, and the reason `fits` can be
    /// a load-time check on six stored numbers instead of a check on every
    /// event. Break this and the +-2^40 profile comes back.
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

    /// A non-monotonic rescale is a stick that jumps backwards mid-travel:
    /// push right, the cursor goes left for a few counts, then resumes. It is
    /// not a crash, so nothing reports it; it just makes the pad feel broken.
    #[test]
    fn apply_never_moves_backwards_as_the_reading_rises(cal in any_calibration()) {
        let readings = sweep(&cal);
        let mut previous: Option<(i32, i32)> = None;
        for value in readings {
            let out = cal.apply(value);
            if let Some((before, was)) = previous {
                prop_assert!(
                    out >= was,
                    "{before} -> {value} went backwards: {was} -> {out}"
                );
            }
            previous = Some((value, out));
        }
    }

    /// Where the stick rests must read as the middle. This is the whole point
    /// of calibration: the worn N64 stick resting at 174 has to come out at
    /// the declared midpoint, or the front-end sees a permanent deflection and
    /// scrolls the menu forever.
    #[test]
    fn the_resting_position_always_reads_as_the_declared_midpoint(
        cal in calibration_with_a_real_dead_band(),
    ) {
        prop_assert_eq!(
            cal.apply(cal.center),
            midpoint_of(&cal),
            "the recorded rest position did not come out centred"
        );
    }

    /// Everything inside the dead band is the same as resting. A band that
    /// leaks is idle noise reaching the front-end, which is the thing the band
    /// exists to stop.
    #[test]
    fn every_reading_within_flat_of_centre_reads_as_the_midpoint(
        (cal, value) in inside_the_dead_band(),
    ) {
        prop_assert_eq!(
            cal.apply(value),
            midpoint_of(&cal),
            "a reading of {} escaped a band of {} around {}",
            value,
            cal.flat,
            cal.center
        );
    }

    /// The ends of the travel the stick actually has must land exactly on the
    /// ends of the range the consumer is told about. Landing short is the
    /// 160-255 adapter again: the stick physically bottoms out and the game
    /// still sees a half-press.
    #[test]
    fn the_ends_of_the_measured_reach_map_onto_the_declared_stops(
        cal in calibration_with_a_real_dead_band(),
    ) {
        let low = cal.low();
        let high = cal.high();
        let centre = i64::from(cal.center);
        let flat = i64::from(cal.flat);

        // A degenerate side has no travel to scale and collapses to the
        // midpoint instead; that is the next property, not this one.
        if centre - flat - low > 0 {
            prop_assert_eq!(
                cal.apply(low_of(&cal)),
                cal.minimum,
                "full deflection to {} did not reach the bottom stop",
                low
            );
        }
        if high - centre - flat > 0 {
            prop_assert_eq!(
                cal.apply(high_of(&cal)),
                cal.maximum,
                "full deflection to {} did not reach the top stop",
                high
            );
        }
    }

    /// A stick that overshoots the range it was calibrated against is clamped,
    /// not extrapolated. Extrapolation is how a reading becomes a number the
    /// write refuses.
    #[test]
    fn a_reading_past_the_measured_reach_is_clamped_rather_than_extrapolated(
        cal in calibration_with_a_real_dead_band(),
        overshoot in 1i64..=100_000i64,
    ) {
        let centre = i64::from(cal.center);
        let flat = i64::from(cal.flat);

        if centre - flat - cal.low() > 0 {
            let past = i32::try_from((cal.low() - overshoot).max(i64::from(i32::MIN)))
                .expect("clamped into i32 above");
            prop_assert_eq!(cal.apply(past), cal.minimum, "an overshoot below the reach");
        }
        if cal.high() - centre - flat > 0 {
            let past = i32::try_from((cal.high() + overshoot).min(i64::from(i32::MAX)))
                .expect("clamped into i32 above");
            prop_assert_eq!(cal.apply(past), cal.maximum, "an overshoot above the reach");
        }
    }

    /// A side with no travel divides by zero if it is scaled. It has to
    /// degrade to "this axis does not move" instead -- a dead axis is a bad
    /// day, a crashed daemon is everyone's controller at once.
    #[test]
    fn a_side_with_no_travel_collapses_to_the_midpoint_instead_of_dividing_by_zero(
        cal in plausible_calibration(),
        value in any_reading(),
    ) {
        // Pin the measured reach to centre, which is exactly the shape
        // `low`/`high` produce from a reach recorded on the wrong side.
        let pinned = AxisCalibration {
            reach_min: Some(cal.center),
            reach_max: Some(cal.center),
            flat: cal.flat.max(0),
            ..cal
        };
        prop_assert_eq!(
            pinned.apply(value),
            midpoint_of(&pinned),
            "a zero-width span produced something other than the middle"
        );
    }

    /// A range of zero width has nowhere to go. Every reading is the one value
    /// the axis can hold, and no division happens at all.
    #[test]
    fn a_range_of_zero_width_answers_with_its_single_value(
        center in any::<i32>(),
        only in any::<i32>(),
        flat in 0i32..=1000i32,
        value in any_reading(),
    ) {
        let stuck = AxisCalibration { center, minimum: only, maximum: only, flat,
            reach_min: None, reach_max: None };
        prop_assert_eq!(stuck.apply(value), only, "a stuck axis moved");
    }

    /// `low` never above centre and `high` never below it, for any recorded
    /// reach at all -- including one entirely on the wrong side of centre, and
    /// one recorded inverted. Without the clamp the span goes negative, the
    /// scale inverts, and pushing left moves right.
    #[test]
    fn the_measured_reach_can_never_cross_the_recorded_centre(
        cal in adversarial_calibration(),
    ) {
        let centre = i64::from(cal.center);
        prop_assert!(cal.low() <= centre, "low() {} rose above centre {centre}", cal.low());
        prop_assert!(cal.high() >= centre, "high() {} fell below centre {centre}", cal.high());
        prop_assert!(cal.low() <= cal.high(), "the travel inverted");
    }

    /// Readings below rest stay below the middle and readings above stay above
    /// it. A sign flip here is a pad whose left is right.
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

    /// `fits` is true exactly when every stored number is writable.
    ///
    /// In Rust that is always, and the answer is the interesting part: an
    /// `i32` cannot represent 2^40, so the type system does by construction
    /// what Python's `fits_evdev` had to do by hand. The guard stays because
    /// it is what a profile loader calls, and because the original wound was a
    /// Python `int` -- unbounded -- reaching a write that takes an `__s32`.
    #[test]
    fn fits_agrees_with_checking_every_stored_number_by_hand(
        cal in adversarial_calibration(),
    ) {
        let stored: Vec<i64> = [
            Some(cal.center),
            Some(cal.minimum),
            Some(cal.maximum),
            Some(cal.flat),
            cal.reach_min,
            cal.reach_max,
        ]
        .into_iter()
        .flatten()
        .map(i64::from)
        .collect();
        let by_hand = stored
            .iter()
            .all(|value| (EVDEV_VALUE_MIN..=EVDEV_VALUE_MAX).contains(value));
        prop_assert_eq!(cal.fits(), by_hand);
        prop_assert!(by_hand, "every i32 is inside the evdev value range by construction");
    }

    /// The midpoint floors towards negative infinity, as Python's `//` does.
    ///
    /// Truncating division would put the centre seed on the wrong side of
    /// centre for every signed-range axis: -32768..32767 floors to -1 and
    /// truncates to 0, and the seed is what the virtual pad is created
    /// holding.
    #[test]
    fn the_midpoint_floors_where_truncation_would_round_towards_zero(
        minimum in any::<i32>(),
        maximum in any::<i32>(),
        center in any::<i32>(),
    ) {
        let cal = AxisCalibration::new(center, minimum, maximum);
        let sum = i64::from(minimum) + i64::from(maximum);
        prop_assert_eq!(cal.midpoint(), floor_halve(sum));
        prop_assert!(
            cal.midpoint() * 2 <= sum,
            "a floored half can never exceed half the sum"
        );
    }

    /// Serde must carry every field through unchanged, under the key names the
    /// Python wrote. A profile on disk outlives the port.
    #[test]
    fn a_calibration_survives_a_json_round_trip_unchanged(
        cal in adversarial_calibration(),
    ) {
        let text = serde_json::to_string(&cal).expect("a calibration serialises");
        let back: AxisCalibration = serde_json::from_str(&text).expect("and parses back");
        prop_assert_eq!(cal, back);
    }
}

// -- the hardware these numbers came off --------------------------------------

#[test]
fn a_true_centred_pad_moves_nothing_it_does_not_have_to() {
    let cal = AxisCalibration::new(128, 0, 255);
    // 0..255 floors to 127, not 128: there is no exact middle of an even-width
    // range, and the whole file agrees on which side to take.
    assert_eq!(
        cal.apply(128),
        127,
        "the declared midpoint of 0..255 floors"
    );
    assert_eq!(cal.apply(0), 0);
    assert_eq!(cal.apply(255), 255);
    // Nothing between the stops should move either, to within a count of the
    // straight line through them.
    for value in 0..=255 {
        let out = cal.apply(value);
        assert!((out - value).abs() <= 1, "{value} was displaced to {out}");
    }
}

#[test]
fn the_worn_n64_stick_resting_at_174_still_reaches_both_of_its_stops() {
    // The measured controller: rests at 174 on a 0-255 axis whose nominal
    // centre is 128. That is 36% deflection. Uncorrected, a front-end starts
    // scrolling the moment the pad is plugged in and does not stop.
    let cal = AxisCalibration::new(174, 0, 255);
    assert_eq!(cal.apply(174), 127, "rest must read as the middle");
    // 174 leaves 174 counts of travel below and only 81 above. Both halves
    // must still map onto the full declared half-range, or the short side
    // cannot reach its stop.
    assert_eq!(cal.apply(0), 0, "full left");
    assert_eq!(cal.apply(255), 255, "full right");
    assert!(
        cal.apply(173) < 127 && cal.apply(175) > 127,
        "either side of rest"
    );
}

#[test]
fn a_gamecube_trigger_resting_at_its_minimum_reads_centred_until_it_is_pressed() {
    // A GameCube analogue trigger rests at 0 on a 0-255 axis: all of its
    // travel is on one side. `low()` clamps to centre, the bottom span is
    // zero, and the division is skipped rather than attempted.
    let trigger = AxisCalibration::new(0, 0, 255);
    assert_eq!(trigger.low(), 0, "there is no travel below rest");
    assert_eq!(trigger.high(), 255);
    assert_eq!(
        trigger.apply(0),
        127,
        "rest is the midpoint, as for any axis"
    );
    assert_eq!(trigger.apply(255), 255, "fully pressed reaches the stop");
    // A reading below rest -- electrical noise on a released trigger -- has no
    // span to scale against and must fall back to the middle, not divide by
    // zero.
    assert_eq!(trigger.apply(-1), 127);
    assert_eq!(trigger.apply(i32::MIN), 127);
}

#[test]
fn an_adapter_that_declares_more_travel_than_the_stick_has_still_gets_full_left() {
    // Declares 0-255, physically emits 160-255. Against the declared range
    // there are five counts of usable travel to the left of rest and the stick
    // cannot go left at all.
    let declared = AxisCalibration::new(200, 0, 255);
    let measured = AxisCalibration::new(200, 0, 255).with_reach(160, 255);
    assert!(
        declared.apply(160) > measured.apply(160),
        "scaling against the measured reach must open the short side up"
    );
    assert_eq!(measured.apply(160), 0, "full left now reaches the stop");
    assert_eq!(measured.apply(200), 127, "rest is still the middle");
    assert_eq!(measured.apply(255), 255, "full right is unchanged");
}

#[test]
fn a_signed_sixteen_bit_axis_centres_on_the_declared_midpoint_not_on_zero() {
    let cal = AxisCalibration::new(0, -32768, 32767);
    // -1, because (-32768 + 32767) // 2 == -1. Truncation would say 0, and the
    // virtual pad would be seeded one count to the right of where the real one
    // rests on every signed axis there is.
    assert_eq!(cal.midpoint(), -1);
    assert_eq!(cal.apply(0), -1, "the declared midpoint, not zero");
    assert_eq!(cal.apply(-32768), -32768);
    assert_eq!(cal.apply(32767), 32767);
    assert_eq!(cal.apply(i32::MIN), -32768, "clamped, not wrapped");
    assert_eq!(cal.apply(i32::MAX), 32767);
}

#[test]
fn a_dead_band_of_ten_costs_the_stick_none_of_its_travel() {
    // Both the offset and the span come from the edge of the band. Taking the
    // offset from the edge but the span from centre would scale every reading
    // down by the band width, and a fully deflected stick would stop ten
    // counts short of the declared extreme -- which is a game that never sees
    // a full turn.
    let cal = AxisCalibration::new(128, 0, 255).with_flat(10);
    for value in 118..=138 {
        assert_eq!(cal.apply(value), 127, "{value} escaped a band of 10");
    }
    assert_ne!(cal.apply(117), 127, "the band has an edge");
    assert_ne!(cal.apply(139), 127);
    assert_eq!(cal.apply(0), 0, "full left still reaches the stop");
    assert_eq!(cal.apply(255), 255, "full right still reaches the stop");
}

#[test]
fn a_dead_band_of_twenty_costs_the_stick_none_of_its_travel_either() {
    let cal = AxisCalibration::new(128, 0, 255).with_flat(20);
    for value in 108..=148 {
        assert_eq!(cal.apply(value), 127, "{value} escaped a band of 20");
    }
    assert_eq!(cal.apply(0), 0);
    assert_eq!(cal.apply(255), 255);
    // And it is still monotonic across the discontinuity at each edge.
    let mut previous = cal.apply(0);
    for value in 0..=255 {
        let out = cal.apply(value);
        assert!(
            out >= previous,
            "{value} went backwards: {previous} -> {out}"
        );
        previous = out;
    }
}

#[test]
fn a_dead_band_wider_than_the_axis_flattens_every_reading_to_the_midpoint() {
    // A hand-edited or mis-captured band that swallows the whole travel. The
    // axis stops working, which is the correct failure: the alternative is a
    // negative span and an inverted scale.
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
    // Recorded here because it is the one place `apply` does something a
    // reader would not guess, and it matches the Python exactly: `abs(...) <=
    // flat` is never true for a negative band, so there is no band at all and
    // the edges move *outward* from centre. Rest therefore no longer reads as
    // the midpoint -- which is why every centred-at-rest property above
    // assumes `flat >= 0`.
    let inverted_band = AxisCalibration::new(128, 0, 255).with_flat(-10);
    assert_eq!(
        inverted_band.apply(128),
        136,
        "rest is pushed off the middle"
    );
    assert_eq!(inverted_band.apply(127), 117);
    // It is still monotonic and still in range, which is what actually
    // matters on the event path.
    assert_eq!(inverted_band.apply(0), 0);
    assert_eq!(inverted_band.apply(255), 255);
}

#[test]
fn a_reach_recorded_entirely_on_one_side_of_centre_collapses_that_side() {
    // A nonsense profile: the capture only ever saw the stick between 200 and
    // 240, so `reach_min` sits above the recorded centre. `low` clamps to
    // centre, the span comes out zero, and the axis degrades to flat rather
    // than inverting.
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
    // reach_min above reach_max: a capture that recorded the two the wrong way
    // round. Both are clamped to the correct side of centre, so `low <= high`
    // survives and no span goes negative.
    let cal = AxisCalibration::new(128, 0, 255).with_reach(240, 20);
    assert_eq!(cal.low(), 128);
    assert_eq!(cal.high(), 128);
    assert!(cal.low() <= cal.high(), "the travel must not invert");
    for value in [0, 100, 128, 200, 255] {
        assert_eq!(cal.apply(value), 127, "an inverted reach leaves no travel");
    }
}

#[test]
fn an_overshoot_past_the_measured_reach_is_clamped_to_the_declared_range() {
    let cal = AxisCalibration::new(128, 0, 255).with_reach(40, 200);
    assert_eq!(cal.apply(40), 0, "the measured bottom is the bottom stop");
    assert_eq!(cal.apply(200), 255, "the measured top is the top stop");
    assert_eq!(
        cal.apply(0),
        0,
        "past the measured bottom stays at the stop"
    );
    assert_eq!(cal.apply(255), 255);
    assert_eq!(cal.apply(-5000), 0);
    assert_eq!(cal.apply(5000), 255);
    assert_eq!(
        cal.apply(i32::MIN),
        0,
        "the limits of an __s32 are still readings"
    );
    assert_eq!(cal.apply(i32::MAX), 255);
}

// -- rounding: ties to even, as Python's `round` is ---------------------------
//
// Rust's `f64::round` rounds half away from zero. Python's `round` rounds half
// to even. `apply` uses `round_ties_even` for that reason, and these four
// cases are the ones where the two disagree: a port that gets this wrong is
// off by one count on exactly the readings that land on a half, which is
// invisible in play and glaring in the differential corpus.

#[test]
fn a_tie_at_two_and_a_half_rounds_down_to_the_even_neighbour() {
    // 0..3 has a midpoint of 1 and a top half-range of 2. A reach of 4 makes
    // the reading 3 scale to exactly 3/4, so the result is 1 + 0.75 * 2 = 2.5
    // -- an exact binary half, not an artefact of the division.
    let cal = AxisCalibration {
        center: 0,
        minimum: 0,
        maximum: 3,
        flat: 0,
        reach_min: None,
        reach_max: Some(4),
    };
    assert_eq!(
        cal.apply(3),
        2,
        "2.5 ties to even: 2, where f64::round gives 3"
    );
}

#[test]
fn a_tie_at_a_half_rounds_down_to_zero() {
    // 0..1 has a midpoint of 0 and a top half-range of 1; a reach of 2 makes
    // the reading 1 land on exactly 0.5.
    let cal = AxisCalibration {
        center: 0,
        minimum: 0,
        maximum: 1,
        flat: 0,
        reach_min: None,
        reach_max: Some(2),
    };
    assert_eq!(
        cal.apply(1),
        0,
        "0.5 ties to even: 0, where f64::round gives 1"
    );
}

#[test]
fn a_tie_at_minus_a_half_rounds_towards_zero_not_away_from_it() {
    // The same construction below centre. -0.5 ties to even at -0, where
    // rounding half away from zero gives -1.
    let cal = AxisCalibration {
        center: 0,
        minimum: -1,
        maximum: 1,
        flat: 0,
        reach_min: Some(-2),
        reach_max: None,
    };
    assert_eq!(
        cal.apply(-1),
        0,
        "-0.5 ties to even: 0, where f64::round gives -1"
    );
}

#[test]
fn a_tie_at_minus_two_and_a_half_rounds_up_to_the_even_neighbour() {
    // -3..1 has a midpoint of -1 and a bottom half-range of 2; a reach of -4
    // makes the reading -3 land on exactly -2.5.
    let cal = AxisCalibration {
        center: 0,
        minimum: -3,
        maximum: 1,
        flat: 0,
        reach_min: Some(-4),
        reach_max: None,
    };
    assert_eq!(
        cal.apply(-3),
        -2,
        "-2.5 ties to even: -2, where f64::round gives -3"
    );
}

// -- the midpoint ------------------------------------------------------------

#[test]
fn the_midpoint_floors_towards_negative_infinity_across_zero() {
    // (-32768 + 32767) // 2 == -1 in Python. In Rust, `/` would give 0 and put
    // the centre seed on the wrong side of rest for every signed-range axis
    // there is -- which is a virtual pad created already leaning.
    assert_eq!(AxisCalibration::new(0, -32768, 32767).midpoint(), -1);
    assert_eq!((-32768i64 + 32767).div_euclid(2), -1, "and not 0");
    assert_eq!(AxisCalibration::new(0, -1, 0).midpoint(), -1);
    assert_eq!(AxisCalibration::new(0, -3, 0).midpoint(), -2);
    assert_eq!(AxisCalibration::new(0, 0, 255).midpoint(), 127);
    assert_eq!(AxisCalibration::new(0, i32::MIN, i32::MAX).midpoint(), -1);
}

// -- what `fits` is for ------------------------------------------------------

#[test]
fn every_range_an_i32_can_hold_is_one_evdev_can_carry() {
    // The wound: a stored profile declaring +-2^40 made `apply(150)` return
    // 1099511627776, the uinput write refused it, and the daemon exited
    // mid-game with every player's controller going dead at once. Python's
    // `int` is unbounded, so the number existed happily right up to the write.
    //
    // Rust's `i32` cannot represent 2^40 at all, so the type system does by
    // construction what `fits_evdev` did by hand. What the guard must not do
    // is over-correct: the `i32` extremes are legal `__s32` values and a
    // profile carrying them has to stay loadable.
    let extremes = AxisCalibration {
        center: 0,
        minimum: i32::MIN,
        maximum: i32::MAX,
        flat: i32::MAX,
        reach_min: Some(i32::MIN),
        reach_max: Some(i32::MAX),
    };
    assert!(extremes.fits(), "the i32 extremes themselves are writable");
    assert!(AxisCalibration::new(0, 0, 255).fits());
    assert_eq!(
        EVDEV_VALUE_MIN,
        i64::from(i32::MIN),
        "the evdev floor is an __s32 floor"
    );
    assert_eq!(EVDEV_VALUE_MAX, i64::from(i32::MAX));
}

// -- the shape on disk -------------------------------------------------------

#[test]
fn a_calibration_round_trips_through_the_exact_python_json_keys() {
    // Key names are load-bearing: a profile written by the Python has to be
    // readable by the Rust and the other way round, or a user's calibration
    // silently reverts to "never measured" on upgrade.
    let raw = serde_json::json!({
        "center": 174,
        "min": 0,
        "max": 255,
        "flat": 4,
        "reach_min": 20,
        "reach_max": 250
    });
    let cal: AxisCalibration = serde_json::from_value(raw.clone()).expect("parse");
    assert_eq!(cal.center, 174);
    assert_eq!(cal.minimum, 0);
    assert_eq!(cal.maximum, 255);
    assert_eq!(cal.flat, 4);
    assert_eq!(cal.reach_min, Some(20));
    assert_eq!(cal.reach_max, Some(250));
    assert_eq!(serde_json::to_value(cal).expect("serialize"), raw);
}

#[test]
fn an_unmeasured_reach_reads_back_as_unmeasured_rather_than_as_zero() {
    // `None` and `Some(0)` are entirely different calibrations: the first
    // falls back to the declared range, the second pins the bottom of travel
    // to zero. A `#[serde(default)]` that produced `Some(0)` would silently
    // clamp every axis of every profile written before reach was recorded.
    let raw = serde_json::json!({"center": 128, "min": 0, "max": 255});
    let cal: AxisCalibration = serde_json::from_value(raw).expect("parse");
    assert_eq!(cal.reach_min, None);
    assert_eq!(cal.reach_max, None);
    assert_eq!(
        cal.low(),
        0,
        "an unmeasured reach falls back to the declared range"
    );
    assert_eq!(cal.high(), 255);
}

#[test]
fn an_absent_dead_band_reads_back_as_no_band_at_all() {
    let raw = serde_json::json!({"center": 128, "min": 0, "max": 255});
    let cal: AxisCalibration = serde_json::from_value(raw).expect("parse");
    assert_eq!(
        cal.flat, 0,
        "a missing band is no band, not an undefined one"
    );
    assert_ne!(
        cal.apply(129),
        cal.apply(128),
        "with no band, one count moves"
    );
}

#[test]
fn a_half_written_profile_missing_a_required_field_is_refused_rather_than_guessed() {
    // `center`, `minimum` and `maximum` have no sensible default: guessing
    // them produces an axis that is wrong in a way nothing reports. Parsing
    // must fail so the caller can fall back to "uncalibrated".
    let raw = serde_json::json!({"min": 0, "max": 255});
    let parsed: Result<AxisCalibration, _> = serde_json::from_value(raw);
    assert!(
        parsed.is_err(),
        "a profile with no recorded centre must not parse"
    );
}

// -- reported, not fixed ------------------------------------------------------

/// KNOWN BUG, reported rather than fixed here.
///
/// `f64::clamp` asserts `min <= max`, so a stored profile whose declared range
/// is inverted panics inside the one function that must not: every `EV_ABS`
/// from that pad unwinds. The Python it was ported from writes the clamp as
/// `max(self.minimum, min(self.maximum, out))`, which for an inverted range
/// quietly returns `minimum` and keeps the daemon running.
///
/// Shrunk counterexample:
///     AxisCalibration { center: -1, minimum: 1, maximum: 0, flat: 0,
///                       reach_min: None, reach_max: None }.apply(0)
///     thread panicked: "min > max, or either was NaN. min = 1.0, max = 0.0"
///
/// Ignored so the suite stays green while the report stands. `cargo test --
/// --ignored` reproduces it.
#[test]
fn an_inverted_declared_range_must_not_panic_the_per_event_path() {
    let inverted = AxisCalibration {
        center: -1,
        minimum: 1,
        maximum: 0,
        flat: 0,
        reach_min: None,
        reach_max: None,
    };
    assert_eq!(
        inverted.apply(0),
        1,
        "the Python clamps to `minimum` and carries on"
    );
}
