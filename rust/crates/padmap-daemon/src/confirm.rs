//! The confirm gesture: hold a button on an already-claimed pad.
//!
//! Longer than the claim hold, because confirming ends the session and should
//! take a deliberate press rather than the same flick that claims a slot.
//!
//! The release has to match the button that started the hold, exactly as the
//! assigner checks on the claim side. Keying on the pad alone and cancelling
//! on *any* release meant a second button going up cancelled a hold still
//! down on the first -- and since a held button emits no further events,
//! nothing would ever restart it: the bar sat wherever it had got to until
//! the user let go and began again. A thumb resting on B while holding A is
//! enough, which is why this was reported as "I have to reassign controllers
//! twice before they actually get assigned".

use std::collections::BTreeMap;

pub const CONFIRM_HOLD_SECONDS: f64 = 0.7;

/// Confirm holds in flight, one per pad.
#[derive(Debug, Default, Clone)]
pub struct ConfirmHold {
    /// pad -> when the hold began.
    started: BTreeMap<String, f64>,
    /// pad -> which button began it. Kept beside `started` rather than in it
    /// because "the hold is cancelled" is one fact, and splitting it across
    /// two maps that can be cleared separately is how it stops being one.
    button: BTreeMap<String, u16>,
}

impl ConfirmHold {
    /// A button went down or up on a claimed pad.
    pub fn feed(&mut self, pad: &str, code: u16, value: i32, now: f64) {
        match value {
            1 => {
                if !self.started.contains_key(pad) {
                    self.started.insert(pad.to_owned(), now);
                    self.button.insert(pad.to_owned(), code);
                }
            }
            0 if self.button.get(pad) == Some(&code) => {
                self.started.remove(pad);
                self.button.remove(pad);
            }
            _ => {}
        }
    }

    /// Forget every hold, on every pad. Both halves together.
    pub fn clear(&mut self) {
        self.started.clear();
        self.button.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.started.is_empty()
    }

    /// How far the longest hold has got, 0.0..1.0, or `None` with none held.
    pub fn fraction(&self, now: f64) -> Option<f64> {
        let elapsed = self
            .started
            .values()
            .map(|started| now - started)
            .fold(None, |best: Option<f64>, elapsed| {
                Some(best.map_or(elapsed, |b| b.max(elapsed)))
            })?;
        Some((elapsed / CONFIRM_HOLD_SECONDS).min(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u16 = 0x130;
    const B: u16 = 0x131;

    #[test]
    fn a_resting_thumb_does_not_cancel_the_hold() {
        // The reported bug: a second button's release cancelled the first's
        // hold, and nothing restarted it.
        let mut hold = ConfirmHold::default();
        hold.feed("pad", A, 1, 100.0);
        hold.feed("pad", B, 1, 100.2);
        hold.feed("pad", B, 0, 100.3);
        assert_eq!(hold.fraction(100.0 + CONFIRM_HOLD_SECONDS), Some(1.0));
    }

    #[test]
    fn releasing_the_held_button_ends_it() {
        let mut hold = ConfirmHold::default();
        hold.feed("pad", A, 1, 100.0);
        hold.feed("pad", A, 0, 100.3);
        assert_eq!(hold.fraction(101.0), None);
    }

    #[test]
    fn a_second_press_does_not_restart_the_timer() {
        let mut hold = ConfirmHold::default();
        hold.feed("pad", A, 1, 100.0);
        hold.feed("pad", B, 1, 100.5);
        assert_eq!(hold.fraction(100.7), Some(1.0));
    }

    #[test]
    fn the_longest_hold_across_pads_wins() {
        let mut hold = ConfirmHold::default();
        hold.feed("one", A, 1, 100.0);
        hold.feed("two", A, 1, 100.6);
        let fraction = hold.fraction(100.35).expect("held");
        assert!((fraction - 0.5).abs() < 1e-9, "{fraction}");
    }

    #[test]
    fn clear_forgets_both_halves() {
        let mut hold = ConfirmHold::default();
        hold.feed("pad", A, 1, 100.0);
        hold.clear();
        assert!(hold.is_empty());
        // A release of the old button after a clear must not do anything odd.
        hold.feed("pad", A, 0, 100.1);
        assert_eq!(hold.fraction(200.0), None);
    }
}
