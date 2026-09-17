//! The confirm gesture: hold a button on an already-claimed pad.
//! Release must match the starting button to avoid a resting thumb cancelling the hold.

use std::collections::BTreeMap;

pub const CONFIRM_HOLD_SECONDS: f64 = 0.7;

#[derive(Debug, Default, Clone)]
pub struct ConfirmHold {
    // Invariant: started and button are always kept in sync; clear both together.
    started: BTreeMap<String, f64>,
    button: BTreeMap<String, u16>,
}

impl ConfirmHold {
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

    pub fn clear(&mut self) {
        self.started.clear();
        self.button.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.started.is_empty()
    }

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
        hold.feed("pad", A, 0, 100.1);
        assert_eq!(hold.fraction(200.0), None);
    }
}
