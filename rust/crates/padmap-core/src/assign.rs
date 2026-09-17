//! Press-and-hold controller assignment: a held button claims a slot, not a single press.

use std::collections::BTreeMap;

/// How long a button must be down to claim a slot.
pub const HOLD_SECONDS: f64 = 0.25;

/// Ignore the keyboard range on combo devices (first valid button code).
pub const BTN_FIRST: u16 = 0x100;

/// One pad's claim on a player slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assignment {
    /// 1-based.
    pub player: u32,
    /// Index into the caller's pad list.
    pub pad: usize,
    /// The button code that claimed it.
    pub button: u16,
}

/// What a tick produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    /// Pads with a hold in flight, and how far through it they are (0.0..1.0).
    pub progress: Vec<(usize, f64)>,
    /// Slots claimed by this tick.
    pub claimed: Vec<Assignment>,
}

/// Watches a set of pads and yields player order from held presses.
#[derive(Debug, Clone)]
pub struct Assigner {
    hold_seconds: f64,
    /// pad index -> (button code, when it went down).
    holding: BTreeMap<usize, (u16, f64)>,
    claimed: Vec<usize>,
    assignments: Vec<Assignment>,
}

impl Default for Assigner {
    fn default() -> Self {
        Assigner::new(HOLD_SECONDS)
    }
}

impl Assigner {
    pub fn new(hold_seconds: f64) -> Self {
        Assigner {
            hold_seconds,
            holding: BTreeMap::new(),
            claimed: Vec::new(),
            assignments: Vec::new(),
        }
    }

    pub fn assignments(&self) -> &[Assignment] {
        &self.assignments
    }

    /// Whether this pad has already claimed a slot.
    pub fn is_claimed(&self, pad: usize) -> bool {
        self.claimed.contains(&pad)
    }

    /// Offer one event.
    pub fn feed(&mut self, pad: usize, kind: u16, code: u16, value: i32, now: f64) {
        if kind != crate::capture::EV_KEY || code < BTN_FIRST {
            return;
        }
        if value == 1 {
            self.holding.entry(pad).or_insert((code, now));
        } else if value == 0 {
            // Only the button that started the hold: a thumb lifting off B must not
            // cancel a hold running on A.
            if self.holding.get(&pad).map(|(held, _)| *held) == Some(code) {
                self.holding.remove(&pad);
            }
        }
    }

    /// Advance the hold timers.
    pub fn tick(&mut self, now: f64) -> Tick {
        let mut out = Tick {
            progress: Vec::new(),
            claimed: Vec::new(),
        };
        let pending: Vec<(usize, (u16, f64))> = self
            .holding
            .iter()
            .map(|(pad, held)| (*pad, *held))
            .collect();

        for (pad, (code, started)) in pending {
            if self.is_claimed(pad) {
                continue;
            }
            let elapsed = now - started;
            if elapsed < self.hold_seconds {
                out.progress
                    .push((pad, (elapsed / self.hold_seconds).clamp(0.0, 1.0)));
                continue;
            }
            let assignment = Assignment {
                player: self.assignments.len() as u32 + 1,
                pad,
                button: code,
            };
            self.assignments.push(assignment);
            self.claimed.push(pad);
            self.holding.remove(&pad);
            out.progress.push((pad, 1.0));
            out.claimed.push(assignment);
        }
        out
    }

    /// Drop every claim.
    pub fn reset(&mut self) {
        self.assignments.clear();
        self.claimed.clear();
        self.holding.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{EV_ABS, EV_KEY};

    const A: u16 = 0x130;
    const B: u16 = 0x131;

    #[test]
    fn a_held_button_claims_the_first_free_slot() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assert!(assigner.tick(0.1).claimed.is_empty(), "too soon");
        let tick = assigner.tick(HOLD_SECONDS + 0.01);
        assert_eq!(tick.claimed.len(), 1);
        assert_eq!(
            tick.claimed[0],
            Assignment {
                player: 1,
                pad: 0,
                button: A
            }
        );
    }

    #[test]
    fn a_tap_claims_nothing() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, A, 0, 0.05);
        assert!(assigner.tick(1.0).claimed.is_empty());
        assert!(assigner.assignments().is_empty());
    }

    #[test]
    fn a_second_button_during_a_hold_does_not_restart_the_timer() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, B, 1, 0.2);
        assert_eq!(assigner.tick(HOLD_SECONDS + 0.01).claimed.len(), 1);
    }

    #[test]
    fn releasing_a_different_button_does_not_cancel_the_hold() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, B, 1, 0.05);
        assigner.feed(0, EV_KEY, B, 0, 0.10);
        assert_eq!(assigner.tick(HOLD_SECONDS + 0.01).claimed.len(), 1);
    }

    #[test]
    fn players_are_numbered_in_the_order_the_pads_claimed() {
        let mut assigner = Assigner::default();
        assigner.feed(2, EV_KEY, A, 1, 0.0);
        assigner.tick(HOLD_SECONDS + 0.01);
        assigner.feed(0, EV_KEY, A, 1, 1.0);
        assigner.tick(1.0 + HOLD_SECONDS + 0.01);
        let players: Vec<(u32, usize)> = assigner
            .assignments()
            .iter()
            .map(|a| (a.player, a.pad))
            .collect();
        assert_eq!(players, [(1, 2), (2, 0)]);
    }

    #[test]
    fn one_pad_cannot_claim_twice() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.tick(HOLD_SECONDS + 0.01);
        assigner.feed(0, EV_KEY, B, 1, 1.0);
        assert!(assigner.tick(1.0 + HOLD_SECONDS + 0.01).claimed.is_empty());
        assert_eq!(assigner.assignments().len(), 1);
    }

    #[test]
    fn a_keyboard_code_on_a_combo_adapter_cannot_claim_a_slot() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, 0x1e, 1, 0.0);
        assert!(assigner.tick(1.0).claimed.is_empty());
    }

    #[test]
    fn axis_noise_is_not_a_press() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_ABS, 0x00, 255, 0.0);
        assert!(assigner.tick(1.0).claimed.is_empty());
    }

    #[test]
    fn autorepeat_does_not_start_a_second_hold() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        for step in 1..10 {
            assigner.feed(0, EV_KEY, A, 2, f64::from(step) * 0.01);
        }
        assert_eq!(assigner.tick(HOLD_SECONDS + 0.01).claimed.len(), 1);
    }

    #[test]
    fn progress_climbs_to_one_and_is_reported_for_the_claiming_tick() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        let early = assigner.tick(HOLD_SECONDS / 2.0);
        assert!((early.progress[0].1 - 0.5).abs() < 1e-9);
        let done = assigner.tick(HOLD_SECONDS + 0.01);
        assert_eq!(done.progress, [(0, 1.0)]);
    }

    #[test]
    fn progress_never_exceeds_one() {
        let mut assigner = Assigner::new(1.0);
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        for (_, fraction) in assigner.tick(0.9).progress {
            assert!((0.0..=1.0).contains(&fraction));
        }
    }

    #[test]
    fn reset_drops_every_claim() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.tick(HOLD_SECONDS + 0.01);
        assigner.reset();
        assert!(assigner.assignments().is_empty());
        assert!(!assigner.is_claimed(0));
        assigner.feed(0, EV_KEY, A, 1, 2.0);
        assert_eq!(assigner.tick(2.0 + HOLD_SECONDS + 0.01).claimed.len(), 1);
    }

    #[test]
    fn two_pads_holding_at_once_both_claim_in_pad_order() {
        let mut assigner = Assigner::default();
        assigner.feed(1, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        let tick = assigner.tick(HOLD_SECONDS + 0.01);
        assert_eq!(
            tick.claimed
                .iter()
                .map(|a| (a.player, a.pad))
                .collect::<Vec<_>>(),
            [(1, 0), (2, 1)]
        );
    }
}
