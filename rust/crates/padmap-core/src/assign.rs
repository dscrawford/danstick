//! Press-and-hold controller assignment.
//!
//! For pads that are indistinguishable by every static attribute, a human
//! pressing a button is the only available source of identity. This turns that
//! press into a player ordering.
//!
//! A *held* button, not a rising edge. During earlier testing an empty adapter
//! port registered a stray press that a first-edge scheme would have accepted,
//! silently burning a player slot. A transient cannot hold; a human cannot tell
//! the difference.
//!
//! Pure, with the clock passed in: the daemon drives this from its event loop
//! and the CLI from a blocking one, and neither should get a different answer.

use std::collections::BTreeMap;

/// How long a button must be down to claim a slot.
pub const HOLD_SECONDS: f64 = 0.25;

/// Ignore the keyboard range on combo devices.
///
/// An adapter that reports KEY_A alongside its twelve buttons would otherwise
/// let a keyboard press claim a controller slot.
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

    /// Offer one event from a pad.
    ///
    /// Only EV_KEY at or above [`BTN_FIRST`] counts. Everything else -- axis
    /// noise, a keyboard code on a combo adapter -- is not a claim.
    pub fn feed(&mut self, pad: usize, kind: u16, code: u16, value: i32, now: f64) {
        if kind != crate::capture::EV_KEY || code < BTN_FIRST {
            return;
        }
        if value == 1 {
            // Only the first button down. A second pressed during a hold must
            // not restart the timer, or a player mashing never claims.
            self.holding.entry(pad).or_insert((code, now));
        } else if value == 0 {
            // Released early: a tap, or a transient. Not a claim. Matched on
            // the button that started the hold, so a thumb resting on B and
            // lifting does not cancel a hold running on A.
            if self.holding.get(&pad).map(|(held, _)| *held) == Some(code) {
                self.holding.remove(&pad);
            }
        }
    }

    /// Advance the hold timers.
    ///
    /// Has to be called on a timer and not only on events: a held button emits
    /// nothing further, so a hold completing is only ever noticed by looking at
    /// the clock.
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

    /// Drop every claim and start over.
    ///
    /// The caller must also discard whatever the pads have queued, or a button
    /// still held from the previous round immediately re-claims a slot.
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
        // A stray press on an empty adapter port is what a rising edge would
        // have accepted, silently burning a slot.
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, A, 0, 0.05);
        assert!(assigner.tick(1.0).claimed.is_empty());
        assert!(assigner.assignments().is_empty());
    }

    #[test]
    fn a_second_button_during_a_hold_does_not_restart_the_timer() {
        // Otherwise a player resting a thumb on another button never claims.
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, B, 1, 0.2);
        assert_eq!(assigner.tick(HOLD_SECONDS + 0.01).claimed.len(), 1);
    }

    #[test]
    fn releasing_a_different_button_does_not_cancel_the_hold() {
        // A thumb resting on B and lifting must not cancel a hold on A. This
        // is the shape reported as "I have to assign controllers twice".
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, B, 1, 0.05);
        assigner.feed(0, EV_KEY, B, 0, 0.10);
        assert_eq!(assigner.tick(HOLD_SECONDS + 0.01).claimed.len(), 1);
    }

    #[test]
    fn players_are_numbered_in_the_order_the_pads_claimed() {
        // The whole point: which physical pad is player 1 is decided by who
        // pressed first, because nothing static can tell them apart.
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
        // An adapter reporting KEY_A alongside its twelve buttons.
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
        // A front-end draws a fill ring from this, and a ring that never
        // reaches full on the tick that claims looks like a failed press.
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
        // ...and the pad can claim again.
        assigner.feed(0, EV_KEY, A, 1, 2.0);
        assert_eq!(assigner.tick(2.0 + HOLD_SECONDS + 0.01).claimed.len(), 1);
    }

    #[test]
    fn two_pads_holding_at_once_both_claim_in_pad_order() {
        // Deterministic, because the alternative is that which of two
        // simultaneous holds becomes player 1 depends on map iteration order.
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
