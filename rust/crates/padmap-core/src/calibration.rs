//! Where an axis rests, how far it actually travels, and its dead band.
//!
//! [`AxisCalibration::apply`] is the one piece of logic in this crate that runs
//! per event: the republisher calls it for every `EV_ABS` passing through. It
//! is deliberately branch-and-arithmetic only, with every allocation, lookup
//! and validity check hoisted out to load time -- see [`AxisCalibration::fits`].

use serde::{Deserialize, Serialize};

/// The width of the field an axis value travels in.
///
/// `input_event.value` is an `__s32`. A stored profile declaring a range of
/// +-2^40 made `apply(150)` return 1099511627776, the write refused it, and the
/// daemon exited mid-game with every player's controller going dead at once.
/// Python raised `OverflowError` there, which is neither an `OSError` nor a
/// `ValueError`, so every guard between the profile store and the uinput write
/// missed it.
pub const EVDEV_VALUE_MIN: i64 = -(1 << 31);
pub const EVDEV_VALUE_MAX: i64 = (1 << 31) - 1;

/// A measured axis.
///
/// `minimum`/`maximum` are the *declared* range from absinfo -- the output
/// scale. `reach_min`/`reach_max` are what the stick was observed to actually
/// produce, and fall back to the declared range when unmeasured.
///
/// Keeping those separate matters. An adapter can declare 0-255 while the
/// physical stick only ever emits 160-255; scaling against the declared range
/// then leaves almost no travel on one side, so the stick cannot go left at
/// all. Scaling against the measured reach restores full movement both ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisCalibration {
    pub center: i32,
    pub minimum: i32,
    pub maximum: i32,
    /// Half-width of the dead band around centre, in raw units.
    #[serde(default)]
    pub flat: i32,
    /// Observed extremes. `None` means "never measured, assume declared range".
    #[serde(default)]
    pub reach_min: Option<i32>,
    #[serde(default)]
    pub reach_max: Option<i32>,
}

impl AxisCalibration {
    pub const fn new(center: i32, minimum: i32, maximum: i32) -> Self {
        AxisCalibration {
            center,
            minimum,
            maximum,
            flat: 0,
            reach_min: None,
            reach_max: None,
        }
    }

    pub const fn with_flat(mut self, flat: i32) -> Self {
        self.flat = flat;
        self
    }

    pub const fn with_reach(mut self, reach_min: i32, reach_max: i32) -> Self {
        self.reach_min = Some(reach_min);
        self.reach_max = Some(reach_max);
        self
    }

    /// The bottom of the travel actually used, never above centre.
    pub fn low(&self) -> i64 {
        let value = i64::from(self.reach_min.unwrap_or(self.minimum));
        value.min(i64::from(self.center))
    }

    /// The top of the travel actually used, never below centre.
    pub fn high(&self) -> i64 {
        let value = i64::from(self.reach_max.unwrap_or(self.maximum));
        value.max(i64::from(self.center))
    }

    /// The declared midpoint, which is what a dead-banded reading becomes and
    /// what a calibrated axis is seeded at when its clone is created.
    pub fn midpoint(&self) -> i64 {
        // Floor division, as Python's `//`: for a range straddling zero the two
        // round different ways and the seed lands on the wrong side of centre.
        (i64::from(self.minimum) + i64::from(self.maximum)).div_euclid(2)
    }

    /// Whether everything this calibration can emit is writable at all.
    ///
    /// Checked on the stored numbers rather than on each result, because the
    /// stored numbers bound every result: [`apply`](Self::apply) clamps into
    /// `minimum..=maximum`, and the only other value that leaves here is the
    /// midpoint seed. So one check when a profile is read stands in for a check
    /// on every event, and the hot path stays arithmetic.
    pub fn fits(&self) -> bool {
        let mut bounds = vec![
            i64::from(self.center),
            i64::from(self.minimum),
            i64::from(self.maximum),
            i64::from(self.flat),
        ];
        bounds.extend(self.reach_min.map(i64::from));
        bounds.extend(self.reach_max.map(i64::from));
        bounds
            .iter()
            .all(|value| (EVDEV_VALUE_MIN..=EVDEV_VALUE_MAX).contains(value))
    }

    /// Rescale a raw reading so `center` maps to the declared midpoint.
    ///
    /// Piecewise linear either side of centre, scaled by measured reach, and
    /// clamped so a stick that overshoots its calibration cannot exceed the
    /// declared range.
    pub fn apply(&self, value: i32) -> i32 {
        let mid = self.midpoint();
        let value = i64::from(value);
        let center = i64::from(self.center);
        let flat = i64::from(self.flat);

        if (value - center).abs() <= flat {
            return mid as i32;
        }

        // Both the offset and the span are measured from the edge of the dead
        // band. Taking the offset from there but the span from centre would
        // scale every reading down by the band width, so a fully deflected
        // stick would stop short of the declared extreme.
        let out = if value < center {
            let edge = center - flat;
            let span = edge - self.low();
            if span <= 0 {
                return mid as i32;
            }
            // -1 at full reach, 0 at the band.
            let scaled = (value - edge) as f64 / span as f64;
            mid as f64 + scaled * (mid - i64::from(self.minimum)) as f64
        } else {
            let edge = center + flat;
            let span = self.high() - edge;
            if span <= 0 {
                return mid as i32;
            }
            // 0 at the band, 1 at full reach.
            let scaled = (value - edge) as f64 / span as f64;
            mid as f64 + scaled * (i64::from(self.maximum) - mid) as f64
        };

        let clamped = out.clamp(f64::from(self.minimum), f64::from(self.maximum));
        // Ties to even, which is what Python's `round` does. The difference is
        // one count on an axis, but a Rust port that disagrees with the Python
        // on a recorded capture is indistinguishable from one that is wrong.
        clamped.round_ties_even() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pad that already centres itself: nothing should move.
    #[test]
    fn a_true_centred_axis_passes_its_own_values_through() {
        let cal = AxisCalibration::new(128, 0, 255);
        assert_eq!(
            cal.apply(128),
            127,
            "the declared midpoint of 0..255 floors"
        );
        assert_eq!(cal.apply(0), 0);
        assert_eq!(cal.apply(255), 255);
    }

    #[test]
    fn a_worn_stick_resting_off_centre_is_pulled_back_to_the_middle() {
        // The N64 adapter measured here: rests at 174 on a 0..255 axis whose
        // nominal centre is 128. Uncorrected it reads permanently deflected.
        let cal = AxisCalibration::new(174, 0, 255);
        assert_eq!(cal.apply(174), 127);
        assert_eq!(cal.apply(0), 0, "full left still reaches the stop");
        assert_eq!(cal.apply(255), 255, "full right still reaches the stop");
    }

    #[test]
    fn both_halves_reach_their_stop_even_when_the_travel_is_lopsided() {
        // 174 leaves 174 counts below and 81 above. Both must still map onto
        // the full declared half-range or the stick cannot go one way.
        let cal = AxisCalibration::new(174, 0, 255);
        let left = cal.apply(0);
        let right = cal.apply(255);
        assert!(left <= 1, "left reached {left}");
        assert!(right >= 254, "right reached {right}");
    }

    #[test]
    fn the_dead_band_flattens_everything_inside_it_to_exactly_the_middle() {
        let cal = AxisCalibration::new(128, 0, 255).with_flat(10);
        for value in 118..=138 {
            assert_eq!(cal.apply(value), 127, "value {value} escaped the band");
        }
        assert_ne!(cal.apply(117), 127);
        assert_ne!(cal.apply(139), 127);
    }

    #[test]
    fn the_band_does_not_cost_the_stick_its_full_travel() {
        // Taking the offset from the band edge but the span from centre would
        // scale every reading down by the band width, and a fully deflected
        // stick would stop short of the declared extreme.
        let cal = AxisCalibration::new(128, 0, 255).with_flat(20);
        assert_eq!(cal.apply(0), 0);
        assert_eq!(cal.apply(255), 255);
    }

    #[test]
    fn measured_reach_restores_a_stick_that_never_emits_its_declared_range() {
        // An adapter declaring 0..255 whose stick only ever produces 160..255.
        // Against the declared range there is almost no travel to the left.
        let declared = AxisCalibration::new(200, 0, 255);
        let measured = AxisCalibration::new(200, 0, 255).with_reach(160, 255);
        assert!(
            declared.apply(160) > measured.apply(160),
            "the measured reach must open up the short side"
        );
        assert_eq!(measured.apply(160), 0, "full left reaches the stop");
        assert_eq!(measured.apply(255), 255);
    }

    #[test]
    fn reach_can_never_narrow_the_range_past_centre() {
        // A nonsense profile where the measured reach sits entirely on one side
        // of the recorded centre. `low`/`high` clamp to centre so `span` stays
        // positive and the axis degrades to flat rather than inverting.
        let cal = AxisCalibration::new(128, 0, 255).with_reach(200, 240);
        assert_eq!(cal.low(), 128);
        assert_eq!(cal.high(), 240);
        assert_eq!(
            cal.apply(100),
            127,
            "a span of zero collapses to the middle"
        );
    }

    #[test]
    fn an_overshoot_past_the_measured_reach_is_clamped_to_the_declared_range() {
        let cal = AxisCalibration::new(128, 0, 255).with_reach(40, 200);
        assert_eq!(cal.apply(0), 0);
        assert_eq!(cal.apply(255), 255);
        assert_eq!(cal.apply(-5000), 0);
        assert_eq!(cal.apply(5000), 255);
    }

    #[test]
    fn a_degenerate_calibration_returns_the_middle_rather_than_dividing_by_zero() {
        let stuck = AxisCalibration::new(0, 0, 0);
        assert_eq!(stuck.apply(0), 0);
        assert_eq!(stuck.apply(100), 0);
        let no_span = AxisCalibration::new(128, 128, 128);
        assert_eq!(no_span.apply(0), 128);
    }

    #[test]
    fn the_midpoint_floors_the_way_python_did_even_across_zero() {
        // Truncating division would put the seed on the wrong side of centre
        // for a range that straddles zero.
        assert_eq!(AxisCalibration::new(0, -1, 0).midpoint(), -1);
        assert_eq!(AxisCalibration::new(0, -32768, 32767).midpoint(), -1);
        assert_eq!(AxisCalibration::new(0, 0, 255).midpoint(), 127);
    }

    #[test]
    fn a_signed_axis_centres_where_the_declared_range_does() {
        let cal = AxisCalibration::new(0, -32768, 32767);
        assert_eq!(cal.apply(0), -1, "the declared midpoint, not zero");
        assert_eq!(cal.apply(-32768), -32768);
        assert_eq!(cal.apply(32767), 32767);
    }

    #[test]
    fn the_result_is_monotonic_across_the_whole_input_range() {
        // A non-monotonic rescale is a stick that jumps backwards mid-travel.
        let cal = AxisCalibration::new(174, 0, 255)
            .with_flat(4)
            .with_reach(20, 250);
        let mut previous = cal.apply(-100);
        for value in -100..=400 {
            let current = cal.apply(value);
            assert!(
                current >= previous,
                "{value} went backwards: {previous} -> {current}"
            );
            previous = current;
        }
    }

    #[test]
    fn nothing_apply_can_return_falls_outside_the_declared_range() {
        let cal = AxisCalibration::new(174, -128, 127)
            .with_flat(3)
            .with_reach(-100, 90);
        for value in -5000..5000 {
            let out = cal.apply(value);
            assert!((-128..=127).contains(&out), "{value} produced {out}");
        }
    }

    #[test]
    fn a_range_evdev_cannot_carry_is_refused_before_it_reaches_a_write() {
        // The +-2^40 profile that ended the daemon mid-game.
        let huge = AxisCalibration {
            center: 0,
            minimum: i32::MIN,
            maximum: i32::MAX,
            flat: 0,
            reach_min: None,
            reach_max: None,
        };
        assert!(huge.fits(), "the i32 extremes themselves are writable");
        // Rust's i32 cannot hold 2^40 at all, which is the type system doing
        // what `fits_evdev` had to do by hand -- but a profile can still carry
        // the i32 extremes, and those must stay allowed.
        assert!(AxisCalibration::new(0, 0, 255).fits());
    }

    #[test]
    fn a_calibration_round_trips_through_the_python_json_shape() {
        let raw = serde_json::json!({
            "center": 174, "minimum": 0, "maximum": 255,
            "flat": 4, "reach_min": 20, "reach_max": 250
        });
        let cal: AxisCalibration = serde_json::from_value(raw.clone()).expect("parse");
        assert_eq!(cal.center, 174);
        assert_eq!(cal.reach_min, Some(20));
        assert_eq!(serde_json::to_value(cal).expect("serialize"), raw);
    }

    #[test]
    fn an_unmeasured_reach_reads_back_as_unmeasured_not_as_zero() {
        let raw = serde_json::json!({"center": 128, "minimum": 0, "maximum": 255});
        let cal: AxisCalibration = serde_json::from_value(raw).expect("parse");
        assert_eq!(cal.reach_min, None);
        assert_eq!(cal.reach_max, None);
        assert_eq!(cal.flat, 0);
        assert_eq!(cal.low(), 0);
        assert_eq!(cal.high(), 255);
    }
}
