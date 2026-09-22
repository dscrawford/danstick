//! Press-and-hold controller assignment: a held button claims a slot, not a single press.

use std::collections::{BTreeMap, BTreeSet};

/// How long a button must be down to claim a slot, for whoever does not ask.
pub const HOLD_SECONDS: f64 = 0.25;

/// What a hold can be set to: nought is a press, and a minute is not a hold.
pub const HOLD_RANGE: std::ops::RangeInclusive<f64> = 0.05..=10.0;

/// The hold length asked for, or [`HOLD_SECONDS`] where none was or it is
/// outside [`HOLD_RANGE`]. A comfort setting is not worth refusing over, so
/// there is no error here and NaN falls through like any other non-length.
pub fn hold_or_default(seconds: Option<f64>) -> f64 {
    seconds
        .filter(|seconds| HOLD_RANGE.contains(seconds))
        .unwrap_or(HOLD_SECONDS)
}

/// The same, from text: what an environment variable carries.
pub fn hold_from(text: Option<&str>) -> f64 {
    hold_or_default(text.and_then(|text| text.trim().parse::<f64>().ok()))
}

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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tick {
    /// Pads with a hold in flight and how far through it they are, earliest press first.
    pub progress: Vec<(usize, f64)>,
    /// Slots claimed by this tick.
    pub claimed: Vec<Assignment>,
    /// Pads that were filling and are not any more, having claimed nothing.
    pub released: Vec<usize>,
}

/// Watches a set of pads and yields player order from held presses.
#[derive(Debug, Clone)]
pub struct Assigner {
    hold_seconds: f64,
    /// pad index -> (button code, when it went down).
    holding: BTreeMap<usize, (u16, f64)>,
    claimed: Vec<usize>,
    assignments: Vec<Assignment>,
    /// Pads the last tick reported filling, so a hold that stops is noticed.
    filling: BTreeSet<usize>,
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
            filling: BTreeSet::new(),
        }
    }

    pub fn assignments(&self) -> &[Assignment] {
        &self.assignments
    }

    /// How long a hold has to run here to claim.
    pub fn hold_seconds(&self) -> f64 {
        self.hold_seconds
    }

    /// Set how long a hold has to run. A *change* drops every hold in flight,
    /// because a press that became a claim when the number moved under it is
    /// the accident a longer hold exists to prevent; setting the same length
    /// again is not a change and costs nobody their fill.
    pub fn set_hold_seconds(&mut self, hold_seconds: f64) {
        if (hold_seconds - self.hold_seconds).abs() <= f64::EPSILON {
            return;
        }
        self.hold_seconds = hold_seconds;
        self.holding.clear();
    }

    /// Whether this pad has already claimed a slot.
    pub fn is_claimed(&self, pad: usize) -> bool {
        self.claimed.contains(&pad)
    }

    /// Offer one event. Only EV_KEY codes above BTN_FIRST count.
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

    /// Advance the hold timers. Must be called on a timer (events alone cannot detect completion).
    pub fn tick(&mut self, now: f64) -> Tick {
        let mut out = Tick::default();
        // Earliest press first: `holding` is keyed by pad index, which is the
        // order the pads were plugged in and not the order anybody pressed.
        let mut pending: Vec<(usize, (u16, f64))> = self
            .holding
            .iter()
            .map(|(pad, held)| (*pad, *held))
            .collect();
        pending.sort_by(|left, right| {
            left.1
                 .1
                .partial_cmp(&right.1 .1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(left.0.cmp(&right.0))
        });

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

        // A pad that was filling and is no longer: let go of, or gone.
        let seen: BTreeSet<usize> = out.progress.iter().map(|(pad, _)| *pad).collect();
        out.released = self.filling.difference(&seen).copied().collect();
        self.filling = out
            .progress
            .iter()
            .filter(|(_, fraction)| *fraction < 1.0)
            .map(|(pad, _)| *pad)
            .collect();
        out
    }

    /// Undo everything one pad did -- its hold, and its claim if it had one --
    /// leaving everybody else's alone, so it can try again later.
    pub fn forget(&mut self, pad: usize) {
        self.holding.remove(&pad);
        self.claimed.retain(|claimed| *claimed != pad);
        self.assignments.retain(|assignment| assignment.pad != pad);
    }

    /// Drop every claim. Caller must discard queued pad events.
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
    fn a_length_is_read_from_text_or_is_the_default() {
        assert_eq!(hold_from(Some("1.5")), 1.5);
        assert_eq!(hold_from(Some("  0.05  ")), 0.05, "the ends are inside");
        assert_eq!(hold_from(Some("10")), 10.0);
        // Nothing here is worth refusing to start over.
        for asked in ["", "soon", "0", "-2", "600", "nan", "inf", "1,5"] {
            assert_eq!(hold_from(Some(asked)), HOLD_SECONDS, "{asked:?}");
        }
        assert_eq!(hold_from(None), HOLD_SECONDS);
        assert_eq!(hold_or_default(Some(f64::NAN)), HOLD_SECONDS);
    }

    #[test]
    fn a_longer_hold_takes_longer_and_fills_at_its_own_pace() {
        let mut assigner = Assigner::new(1.5);
        assert_eq!(assigner.hold_seconds(), 1.5);
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assert!(assigner.tick(0.3).claimed.is_empty(), "0.3s is not a claim");
        assert!(assigner.tick(1.4).claimed.is_empty(), "nor is 1.4s");
        // Progress fills over whatever length is set, not over the default.
        let part = assigner.tick(0.75).progress[0].1;
        assert!((part - 0.5).abs() < 0.01, "progress at half was {part}");
        assert_eq!(assigner.tick(1.51).claimed.len(), 1);
    }

    #[test]
    fn changing_the_length_drops_the_hold_running_under_it() {
        let mut assigner = Assigner::new(1.5);
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.tick(1.0);
        // A press that became a claim because the number moved is the accident.
        assigner.set_hold_seconds(0.25);
        assert!(
            assigner.tick(1.1).claimed.is_empty(),
            "the hold in flight claimed on the new length"
        );
        assert!(assigner.tick(100.0).claimed.is_empty(), "and never does");
        // A fresh press measures the new length.
        assigner.feed(0, EV_KEY, A, 1, 2.0);
        assert_eq!(assigner.tick(2.26).claimed.len(), 1);
    }

    #[test]
    fn two_pads_take_their_seats_in_the_order_the_buttons_went_down() {
        // Pad 1 presses first, so pad index and press order disagree.
        let mut assigner = Assigner::default();
        assigner.feed(1, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, A, 1, 0.1);
        let tick = assigner.tick(HOLD_SECONDS + 0.2);
        assert_eq!(
            tick.claimed
                .iter()
                .map(|claim| (claim.player, claim.pad))
                .collect::<Vec<_>>(),
            [(1, 1), (2, 0)],
            "seats went by device number, not by who pressed first"
        );
    }

    #[test]
    fn a_fill_is_reported_for_every_pad_in_flight_earliest_press_first() {
        let mut assigner = Assigner::default();
        assigner.feed(2, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, B, 1, 0.05);
        let tick = assigner.tick(0.1);
        let pads: Vec<usize> = tick.progress.iter().map(|(pad, _)| *pad).collect();
        assert_eq!(pads, [2, 0]);
        assert!(
            tick.progress[0].1 > tick.progress[1].1,
            "{:?}",
            tick.progress
        );
        assert!(tick.claimed.is_empty());
    }

    #[test]
    fn letting_go_loses_the_place_and_the_one_still_holding_takes_seat_one() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.feed(1, EV_KEY, A, 1, 0.01);
        assigner.tick(HOLD_SECONDS * 0.8);
        // Pad 0 pressed first and lets go at 80%.
        assigner.feed(0, EV_KEY, A, 0, HOLD_SECONDS * 0.8);
        let tick = assigner.tick(HOLD_SECONDS + 0.1);
        assert_eq!(
            tick.claimed
                .iter()
                .map(|claim| (claim.player, claim.pad))
                .collect::<Vec<_>>(),
            [(1, 1)],
            "the one still holding takes seat one, not seat two"
        );
        assert_eq!(tick.released, [0], "the fill that stopped was not said");
    }

    #[test]
    fn a_hold_that_stops_is_said_once_and_not_again() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assert_eq!(assigner.tick(0.1).progress, [(0, 0.4)]);
        assigner.feed(0, EV_KEY, A, 0, 0.11);
        let tick = assigner.tick(0.12);
        assert_eq!(tick.released, [0]);
        assert!(tick.progress.is_empty());
        assert!(assigner.tick(0.2).released.is_empty(), "said twice");
    }

    #[test]
    fn a_claim_is_not_also_a_release() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.tick(0.1);
        let tick = assigner.tick(HOLD_SECONDS + 0.01);
        assert_eq!(tick.claimed.len(), 1);
        assert_eq!(tick.progress, [(0, 1.0)], "a claim fills to the end");
        assert!(tick.released.is_empty());
        assert!(assigner.tick(1.0).released.is_empty());
    }

    #[test]
    fn a_pad_that_goes_away_mid_hold_has_its_fill_taken_back() {
        // reset() is what the daemon calls when the set of pads changes.
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.feed(1, EV_KEY, A, 1, 0.0);
        assert_eq!(assigner.tick(0.1).progress.len(), 2);
        assigner.reset();
        let tick = assigner.tick(0.2);
        assert_eq!(tick.released, [0, 1], "the fills were left on the screen");
        assert!(tick.progress.is_empty());
    }

    #[test]
    fn a_pad_that_already_holds_a_seat_neither_fills_nor_claims_again() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.tick(HOLD_SECONDS + 0.01);
        // Holding B to block in a fighting game must not reseat anybody.
        assigner.feed(0, EV_KEY, B, 1, 1.0);
        let tick = assigner.tick(1.0 + HOLD_SECONDS + 0.01);
        assert!(tick.progress.is_empty(), "{:?}", tick.progress);
        assert!(tick.claimed.is_empty());
        assert!(tick.released.is_empty());
    }

    #[test]
    fn three_holding_and_the_seats_go_in_press_order() {
        let mut assigner = Assigner::default();
        assigner.feed(2, EV_KEY, A, 1, 0.00);
        assigner.feed(0, EV_KEY, A, 1, 0.01);
        assigner.feed(1, EV_KEY, A, 1, 0.02);
        let tick = assigner.tick(HOLD_SECONDS + 0.1);
        assert_eq!(
            tick.claimed
                .iter()
                .map(|claim| (claim.player, claim.pad))
                .collect::<Vec<_>>(),
            [(1, 2), (2, 0), (3, 1)]
        );
    }

    #[test]
    fn two_presses_the_clock_cannot_separate_are_ordered_by_pad() {
        // Deterministic rather than correct: nothing else can be known.
        let mut assigner = Assigner::default();
        assigner.feed(1, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        let tick = assigner.tick(HOLD_SECONDS + 0.01);
        assert_eq!(
            tick.claimed
                .iter()
                .map(|claim| claim.pad)
                .collect::<Vec<_>>(),
            [0, 1]
        );
    }

    #[test]
    fn setting_the_same_length_again_costs_nobody_their_fill() {
        // A front-end that sends `seating` on every screen sends the same
        // number each time; the hold in flight must survive that.
        let mut assigner = Assigner::new(1.5);
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.tick(0.5);
        assigner.set_hold_seconds(1.5);
        assert_eq!(assigner.tick(0.6).progress.len(), 1, "the fill was wiped");
        assert_eq!(assigner.tick(1.51).claimed.len(), 1, "and it still claims");
    }

    #[test]
    fn forgetting_one_hold_leaves_everybody_elses_alone() {
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assigner.feed(1, EV_KEY, A, 1, 0.0);
        assigner.tick(0.1);
        assigner.forget(0);
        let tick = assigner.tick(0.15);
        assert_eq!(tick.released, [0]);
        assert_eq!(tick.progress.len(), 1, "{:?}", tick.progress);
        assert_eq!(tick.progress[0].0, 1);
    }

    #[test]
    fn a_pad_that_was_forgotten_can_hold_again() {
        // The seats were full when it finished; one frees and it tries again.
        let mut assigner = Assigner::default();
        assigner.feed(0, EV_KEY, A, 1, 0.0);
        assert_eq!(assigner.tick(HOLD_SECONDS + 0.01).claimed.len(), 1);
        assigner.forget(0);
        assert!(!assigner.is_claimed(0));
        assert!(assigner.assignments().is_empty());
        assigner.feed(0, EV_KEY, A, 1, 1.0);
        assert_eq!(assigner.tick(1.0 + HOLD_SECONDS + 0.01).claimed.len(), 1);
    }

    #[test]
    fn forgetting_one_pad_keeps_another_pads_claim() {
        let mut assigner = Assigner::default();
        assigner.feed(1, EV_KEY, A, 1, 0.0);
        assigner.feed(0, EV_KEY, A, 1, 0.01);
        assigner.tick(HOLD_SECONDS + 0.1);
        assigner.forget(0);
        assert_eq!(
            assigner
                .assignments()
                .iter()
                .map(|claim| claim.pad)
                .collect::<Vec<_>>(),
            [1]
        );
        assert!(assigner.is_claimed(1));
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
}
