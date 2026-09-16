//! Taking a seat without a session.
//!
//! An assignment session grabs every pad, which is right for *reordering*
//! seats -- something done with everybody's attention -- and wrong for
//! somebody joining. The moments a controller needs a seat are the moments a
//! modal screen is most expensive: a second player arrives mid-game, a pad
//! dies and is swapped for a charged one, a controller is switched on after
//! the picker has already started, a wireless pad drops and comes back. In
//! each, one person joining would cost everybody else the thing they were
//! doing.
//!
//! So this reads the unseated pads **without grabbing them**. A press still
//! reaches whatever has focus, which is the whole point: nobody else's
//! controller stops working, and the pad being held is one nothing is playing
//! with anyway.
//!
//! Two bounds make it safe to leave on:
//!
//! * **Only unseated pads.** A pad that holds a seat is being *played with* --
//!   holding B to block in a fighting game must not reseat anybody.
//! * **Only free seats.** With every seat taken there is nothing to claim, and
//!   a held pad does nothing until somebody leaves.

use std::path::PathBuf;

use log::warn;
use padmap_core::assign::{Assigner, Tick};
use padmap_input::clone::{self, Source};
use padmap_input::pad::Pad;

/// The pads being watched for someone holding a button.
#[derive(Debug, Default)]
pub struct Seating {
    /// How many seats exist. Zero means seating is closed.
    seats: u32,
    open: bool,
    pads: Vec<Pad>,
    sources: Vec<Source>,
    assigner: Assigner,
    /// Reused between reads so the watch loop allocates nothing.
    buffer: Vec<evdev::InputEvent>,
}

/// What one pad's events did.
#[derive(Debug, Default)]
pub struct Claimed {
    /// Pads that just completed a hold, as indices into [`Seating::pads`].
    pub pads: Vec<usize>,
    /// Holds in flight, for the progress bar a front-end draws.
    pub progress: Vec<(usize, f64)>,
}

impl Seating {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn seats(&self) -> u32 {
        self.seats
    }

    /// Start listening. Idempotent: asking twice does not reopen anything.
    pub fn open(&mut self, seats: u32) {
        self.open = true;
        self.seats = seats.max(1);
    }

    /// Stop, and let go of every pad.
    pub fn close(&mut self) {
        self.open = false;
        self.pads.clear();
        self.sources.clear();
        self.assigner.reset();
    }

    pub fn pads(&self) -> &[Pad] {
        &self.pads
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// Which pads should be watched: those that are here and hold no seat.
    ///
    /// Returns whether the set changed, so the caller only re-registers
    /// descriptors when it has to -- this is asked on every scan.
    pub fn wanted(&self, present: &[Pad], seated: &[PathBuf]) -> Vec<Pad> {
        present
            .iter()
            .filter(|pad| !seated.contains(&pad.path))
            .cloned()
            .collect()
    }

    /// Open the pads in `wanted`, dropping any that are no longer in it.
    ///
    /// Opened **ungrabbed**, deliberately. A grab here would take the pad away
    /// from whatever is running, which is the cost this exists to avoid.
    ///
    /// Returns true if the watched set changed, so the caller can re-register.
    pub fn refresh(&mut self, wanted: Vec<Pad>) -> bool {
        let current: Vec<PathBuf> = self.pads.iter().map(|pad| pad.path.clone()).collect();
        let next: Vec<PathBuf> = wanted.iter().map(|pad| pad.path.clone()).collect();
        if current == next {
            return false;
        }
        // Rebuilt rather than patched: the indices are what the reactor's
        // tokens carry, and keeping them stable across an arbitrary
        // add-and-remove is more bookkeeping than reopening a handful of pads.
        self.pads.clear();
        self.sources.clear();
        self.assigner.reset();
        for pad in wanted {
            match clone::open_source(&pad, false) {
                Ok(source) => {
                    self.pads.push(pad);
                    self.sources.push(source);
                }
                Err(error) => {
                    // Ordinary: a pad may have gone between the scan and here,
                    // or belong to somebody else. It simply cannot take a seat
                    // until it can be read.
                    warn!("seating: {} cannot be watched ({error})", pad.name);
                }
            }
        }
        true
    }

    /// Everything queued on one watched pad, fed to the hold tracker.
    pub fn read(&mut self, index: usize, now: f64) {
        let Some(source) = self.sources.get_mut(index) else {
            return;
        };
        self.buffer.clear();
        if let Err(error) = source.fetch_events(&mut self.buffer) {
            if error.kind() != std::io::ErrorKind::WouldBlock {
                warn!("seating: reading pad {index}: {error}");
            }
            return;
        }
        for event in &self.buffer {
            self.assigner.feed(
                index,
                event.event_type().0,
                event.code(),
                event.value(),
                now,
            );
        }
    }

    /// Advance the hold timers.
    ///
    /// The assigner numbers its own claims from one; this ignores that and
    /// reports only *which pad* completed a hold, because the seat it gets is
    /// the lowest free one on the real roster rather than the next in this
    /// list.
    pub fn tick(&mut self, now: f64) -> Claimed {
        let Tick { progress, claimed } = self.assigner.tick(now);
        // Claiming here only marks the pad as counted; the caller decides
        // whether a seat was actually free, and resets if it was not.
        Claimed {
            pads: claimed.into_iter().map(|claim| claim.pad).collect(),
            progress,
        }
    }

    /// Forget every hold, so a pad that just took a seat -- or one that could
    /// not be given one -- starts again cleanly.
    pub fn reset(&mut self) {
        self.assigner.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(event: &str) -> Pad {
        Pad {
            path: PathBuf::from(format!("/dev/input/{event}")),
            name: "Pad".to_owned(),
            phys: String::new(),
            uniq: String::new(),
            vid: 1,
            pid: 2,
            syspath: PathBuf::from("/sys"),
            retroarch_visible: true,
            motion: None,
        }
    }

    #[test]
    fn a_pad_that_holds_a_seat_is_not_watched() {
        // It is being played with. Holding B to block in a fighting game must
        // not reseat anybody.
        let seating = Seating::default();
        let present = [pad("event1"), pad("event2")];
        let seated = [PathBuf::from("/dev/input/event1")];
        let wanted = seating.wanted(&present, &seated);
        assert_eq!(wanted.len(), 1);
        assert_eq!(wanted[0].path, PathBuf::from("/dev/input/event2"));
    }

    #[test]
    fn nothing_is_watched_when_every_pad_is_seated() {
        let seating = Seating::default();
        let present = [pad("event1")];
        let seated = [PathBuf::from("/dev/input/event1")];
        assert!(seating.wanted(&present, &seated).is_empty());
    }

    #[test]
    fn closing_lets_go_of_everything() {
        let mut seating = Seating::default();
        seating.open(4);
        assert!(seating.is_open());
        assert_eq!(seating.seats(), 4);
        seating.close();
        assert!(!seating.is_open());
        assert!(seating.pads().is_empty());
        assert!(seating.sources().is_empty());
    }

    #[test]
    fn opening_with_no_seats_still_has_one() {
        // `{"cmd":"seating","open":true,"players":0}` is a client saying
        // something it did not mean; a seating mode with no seats would
        // silently never claim.
        let mut seating = Seating::default();
        seating.open(0);
        assert_eq!(seating.seats(), 1);
    }
}
