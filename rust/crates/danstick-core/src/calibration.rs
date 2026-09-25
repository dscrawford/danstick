//! Where an axis rests, how far it actually travels, and its dead band.

use serde::{Deserialize, Serialize};

/// i32 limits; constrains profile ranges to what uinput can write.
pub const EVDEV_VALUE_MIN: i64 = -(1 << 31);
pub const EVDEV_VALUE_MAX: i64 = (1 << 31) - 1;

/// Measured vs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisCalibration {
    pub center: i32,
    #[serde(rename = "min")]
    pub minimum: i32,
    #[serde(rename = "max")]
    pub maximum: i32,
    #[serde(default)]
    pub flat: i32,
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

    pub fn low(&self) -> i64 {
        let value = i64::from(self.reach_min.unwrap_or(self.minimum));
        value.min(i64::from(self.center))
    }

    pub fn high(&self) -> i64 {
        let value = i64::from(self.reach_max.unwrap_or(self.maximum));
        value.max(i64::from(self.center))
    }

    /// Floor division: ranges straddling zero must seed on correct side of centre.
    pub fn midpoint(&self) -> i64 {
        (i64::from(self.minimum) + i64::from(self.maximum)).div_euclid(2)
    }

    /// Bounds-check at load time; hot path then stays arithmetic only.
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

    /// Rescale to midpoint; piecewise linear scaled by measured reach, clamped to declared range.
    pub fn apply(&self, value: i32) -> i32 {
        let mid = self.midpoint();
        let value = i64::from(value);
        let center = i64::from(self.center);
        let flat = i64::from(self.flat);

        if (value - center).abs() <= flat {
            return mid as i32;
        }

        // Offset and span both from dead-band edge; span from centre would shorten travel.
        let out = if value < center {
            let edge = center - flat;
            let span = edge - self.low();
            if span <= 0 {
                return mid as i32;
            }
            let scaled = (value - edge) as f64 / span as f64;
            mid as f64 + scaled * (mid - i64::from(self.minimum)) as f64
        } else {
            let edge = center + flat;
            let span = self.high() - edge;
            if span <= 0 {
                return mid as i32;
            }
            let scaled = (value - edge) as f64 / span as f64;
            mid as f64 + scaled * (i64::from(self.maximum) - mid) as f64
        };

        let clamped = out
            .min(f64::from(self.maximum))
            .max(f64::from(self.minimum));
        clamped.round_ties_even() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let cal = AxisCalibration::new(174, 0, 255);
        assert_eq!(cal.apply(174), 127);
        assert_eq!(cal.apply(0), 0, "full left still reaches the stop");
        assert_eq!(cal.apply(255), 255, "full right still reaches the stop");
    }

    #[test]
    fn both_halves_reach_their_stop_even_when_the_travel_is_lopsided() {
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
        let cal = AxisCalibration::new(128, 0, 255).with_flat(20);
        assert_eq!(cal.apply(0), 0);
        assert_eq!(cal.apply(255), 255);
    }

    #[test]
    fn measured_reach_restores_a_stick_that_never_emits_its_declared_range() {
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
        let huge = AxisCalibration {
            center: 0,
            minimum: i32::MIN,
            maximum: i32::MAX,
            flat: 0,
            reach_min: None,
            reach_max: None,
        };
        assert!(huge.fits(), "the i32 extremes themselves are writable");
        assert!(AxisCalibration::new(0, 0, 255).fits());
    }

    #[test]
    fn a_calibration_round_trips_through_the_python_json_shape() {
        let raw = serde_json::json!({
            "center": 174, "min": 0, "max": 255,
            "flat": 4, "reach_min": 20, "reach_max": 250
        });
        let cal: AxisCalibration = serde_json::from_value(raw.clone()).expect("parse");
        assert_eq!(cal.center, 174);
        assert_eq!(cal.reach_min, Some(20));
        assert_eq!(serde_json::to_value(cal).expect("serialize"), raw);
    }

    #[test]
    fn an_unmeasured_reach_reads_back_as_unmeasured_not_as_zero() {
        let raw = serde_json::json!({"center": 128, "min": 0, "max": 255});
        let cal: AxisCalibration = serde_json::from_value(raw).expect("parse");
        assert_eq!(cal.reach_min, None);
        assert_eq!(cal.reach_max, None);
        assert_eq!(cal.flat, 0);
        assert_eq!(cal.low(), 0);
        assert_eq!(cal.high(), 255);
    }
}

/// Minimum dead band to prevent jitter from sampling noise.
pub const MIN_FLAT_FRACTION: f64 = 0.04;

/// Hat axes; never calibrated.
pub const SKIP_AXES: [u16; 8] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17];

/// Conventional trigger axes (ABS_Z, ABS_RZ, ABS_GAS, ABS_BRAKE).
pub const TRIGGER_AXES: [u16; 4] = [0x02, 0x05, 0x09, 0x0A];

/// One axis as its driver declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declared {
    pub minimum: i32,
    pub maximum: i32,
    pub value: i32,
    pub flat: i32,
}

impl Declared {
    /// Is this axis worth centring? Rest position decides; code list is a shortcut.
    pub fn calibratable(&self, code: u16) -> bool {
        if SKIP_AXES.contains(&code) || TRIGGER_AXES.contains(&code) {
            return false;
        }
        if self.maximum <= self.minimum {
            return false;
        }
        crate::sdl::AxisSpan::new(self.minimum, self.maximum, self.value).rests_centred()
    }

    /// Calibration from rest observation; floor division for Python compatibility.
    pub fn rest_calibration(&self, observed: Option<(i32, i32)>) -> AxisCalibration {
        let (low, high) = observed.unwrap_or((self.value, self.value));
        let travel = self.maximum - self.minimum;
        AxisCalibration {
            center: (low + high).div_euclid(2),
            minimum: self.minimum,
            maximum: self.maximum,
            flat: (high - low)
                .div_euclid(2)
                .saturating_add(1)
                .max((f64::from(travel) * MIN_FLAT_FRACTION) as i32)
                .max(self.flat.max(0)),
            reach_min: None,
            reach_max: None,
        }
    }
}

impl AxisCalibration {
    /// Record sweep reach only beyond dead band; inside it is indistinguishable from rest.
    #[must_use]
    pub fn merge_reach(&self, observed: Option<(i32, i32)>) -> AxisCalibration {
        let (low, high) = observed.unwrap_or((self.center, self.center));
        AxisCalibration {
            reach_min: (low < self.center - self.flat).then_some(low),
            reach_max: (high > self.center + self.flat).then_some(high),
            ..*self
        }
    }
}
