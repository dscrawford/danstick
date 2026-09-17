//! A calibration in flight, advanced from the daemon's tick.

use std::collections::BTreeMap;

use padmap_core::calibration::{AxisCalibration, Declared};
use padmap_core::capture::{EV_ABS, EV_KEY};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    AwaitRest,
    Rest,
    AwaitReach,
    Reach,
    Icon,
}

impl Phase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Phase::AwaitRest => "await_rest",
            Phase::Rest => "rest",
            Phase::AwaitReach => "await_reach",
            Phase::Reach => "reach",
            Phase::Icon => "icon",
        }
    }
}

pub const REST_SECONDS: f64 = 0.8;
pub const REACH_MINIMUM_SECONDS: f64 = 1.2;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Progress,
    Advanced,
    Measured(BTreeMap<u16, AxisCalibration>),
    Quiet,
}

#[derive(Debug, Clone)]
pub struct CalibrationRun {
    pub player: u32,
    /// Path of the pad being measured, so events from other pads are ignored.
    pub pad_path: String,
    pub axes: BTreeMap<u16, Declared>,
    pub phase: Phase,
    started: f64,
    /// Per axis, the lowest and highest reading this phase.
    seen: BTreeMap<u16, (i32, i32)>,
    rest: Option<BTreeMap<u16, AxisCalibration>>,
    /// Set when a button goes down during an await phase.
    advance_requested: bool,
}

impl CalibrationRun {
    pub fn new(player: u32, pad_path: String, axes: BTreeMap<u16, Declared>, now: f64) -> Self {
        let mut run = CalibrationRun {
            player,
            pad_path,
            axes,
            phase: Phase::AwaitRest,
            started: now,
            seen: BTreeMap::new(),
            rest: None,
            advance_requested: false,
        };
        run.reset_samples();
        run
    }

    fn reset_samples(&mut self) {
        // Each phase starts fresh; reach must not inherit rest phase's samples.
        self.seen = self
            .axes
            .iter()
            .map(|(code, axis)| (*code, (axis.value, axis.value)))
            .collect();
    }

    pub fn elapsed(&self, now: f64) -> f64 {
        now - self.started
    }

    pub fn fraction(&self, now: f64) -> f64 {
        match self.phase {
            Phase::Rest => (self.elapsed(now) / REST_SECONDS).min(1.0),
            Phase::Reach => self.coverage(),
            _ => 0.0,
        }
    }

    pub fn coverage(&self) -> f64 {
        if self.axes.is_empty() {
            return 0.0;
        }
        let total: f64 = self
            .axes
            .iter()
            .map(|(code, axis)| {
                let (low, high) = self
                    .seen
                    .get(code)
                    .copied()
                    .unwrap_or((axis.value, axis.value));
                let declared = axis.maximum - axis.minimum;
                if declared == 0 {
                    0.0
                } else {
                    f64::from(high - low) / f64::from(declared)
                }
            })
            .sum();
        (total / self.axes.len() as f64).min(1.0)
    }

    pub fn feed(&mut self, kind: u16, code: u16, value: i32) {
        if kind == EV_KEY && value == 1 {
            self.advance_requested = true;
            return;
        }
        if kind != EV_ABS {
            return;
        }
        if let Some((low, high)) = self.seen.get_mut(&code) {
            *low = (*low).min(value);
            *high = (*high).max(value);
        }
    }

    pub fn begin_phase(&mut self, phase: Phase, now: f64) {
        self.phase = phase;
        self.started = now;
        self.advance_requested = false;
        self.reset_samples();
    }

    fn rest_from_samples(&self) -> BTreeMap<u16, AxisCalibration> {
        self.axes
            .iter()
            .map(|(code, axis)| (*code, axis.rest_calibration(self.seen.get(code).copied())))
            .collect()
    }

    /// Advance the phase machine.
    pub fn tick(&mut self, now: f64) -> Step {
        match self.phase {
            Phase::AwaitRest => {
                if self.advance_requested {
                    self.begin_phase(Phase::Rest, now);
                    return Step::Advanced;
                }
                Step::Quiet
            }
            Phase::AwaitReach => {
                if self.advance_requested {
                    self.begin_phase(Phase::Reach, now);
                    return Step::Advanced;
                }
                Step::Quiet
            }
            Phase::Icon => Step::Quiet,
            Phase::Rest => {
                if self.elapsed(now) < REST_SECONDS {
                    return Step::Progress;
                }
                self.rest = Some(self.rest_from_samples());
                self.begin_phase(Phase::AwaitReach, now);
                Step::Advanced
            }
            Phase::Reach => {
                if !(self.advance_requested && self.elapsed(now) >= REACH_MINIMUM_SECONDS) {
                    return Step::Progress;
                }
                let rest = self
                    .rest
                    .clone()
                    .unwrap_or_else(|| self.rest_from_samples());
                let merged: BTreeMap<u16, AxisCalibration> = rest
                    .iter()
                    .map(|(code, calibration)| {
                        (*code, calibration.merge_reach(self.seen.get(code).copied()))
                    })
                    .collect();
                self.begin_phase(Phase::Icon, now);
                Step::Measured(merged)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stick() -> BTreeMap<u16, Declared> {
        [(
            0u16,
            Declared {
                minimum: 0,
                maximum: 255,
                value: 128,
                flat: 0,
            },
        )]
        .into_iter()
        .collect()
    }

    #[test]
    fn a_walk_through_every_phase() {
        let mut run = CalibrationRun::new(1, "p".into(), stick(), 0.0);
        assert_eq!(run.tick(0.1), Step::Quiet, "await phases are the user's");
        run.feed(EV_KEY, 0x130, 1);
        assert_eq!(run.tick(0.2), Step::Advanced);
        assert_eq!(run.phase, Phase::Rest);
        run.feed(EV_ABS, 0, 126);
        run.feed(EV_ABS, 0, 130);
        assert_eq!(run.tick(0.5), Step::Progress);
        assert_eq!(run.tick(0.2 + REST_SECONDS + 0.01), Step::Advanced);
        assert_eq!(run.phase, Phase::AwaitReach);
        run.feed(EV_KEY, 0x130, 1);
        assert_eq!(run.tick(1.1), Step::Advanced);
        assert_eq!(run.phase, Phase::Reach);
        run.feed(EV_ABS, 0, 3);
        run.feed(EV_ABS, 0, 250);
        // A press before the minimum does not end it.
        run.feed(EV_KEY, 0x130, 1);
        assert_eq!(run.tick(1.5), Step::Progress);
        let done = run.tick(1.1 + REACH_MINIMUM_SECONDS + 0.01);
        let Step::Measured(axes) = done else {
            panic!("{done:?}");
        };
        let axis = axes[&0];
        assert_eq!(axis.center, 128);
        assert_eq!(axis.reach_min, Some(3));
        assert_eq!(axis.reach_max, Some(250));
        assert_eq!(run.phase, Phase::Icon);
        assert_eq!(run.tick(5.0), Step::Quiet);
    }

    #[test]
    fn coverage_is_how_much_of_the_declared_travel_was_swept() {
        let mut run = CalibrationRun::new(1, "p".into(), stick(), 0.0);
        run.begin_phase(Phase::Reach, 0.0);
        assert_eq!(run.coverage(), 0.0);
        run.feed(EV_ABS, 0, 0);
        run.feed(EV_ABS, 0, 255);
        assert_eq!(run.coverage(), 1.0);
        assert_eq!(run.fraction(0.0), 1.0);
    }

    #[test]
    fn reach_does_not_inherit_the_rest_samples() {
        let mut run = CalibrationRun::new(1, "p".into(), stick(), 0.0);
        run.begin_phase(Phase::Rest, 0.0);
        run.feed(EV_ABS, 0, 0);
        run.begin_phase(Phase::Reach, 1.0);
        assert_eq!(run.coverage(), 0.0);
    }

    #[test]
    fn events_from_an_unknown_axis_are_ignored() {
        let mut run = CalibrationRun::new(1, "p".into(), stick(), 0.0);
        run.begin_phase(Phase::Reach, 0.0);
        run.feed(EV_ABS, 7, 999);
        assert_eq!(run.coverage(), 0.0);
    }
}
