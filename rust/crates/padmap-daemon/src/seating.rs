//! Taking a seat without a session.

use std::path::PathBuf;

use log::warn;
use padmap_core::assign::{Assigner, Tick};
use padmap_input::clone::{self, Source};
use padmap_input::pad::Pad;

#[derive(Debug, Default)]
pub struct Seating {
    seats: u32,
    open: bool,
    pads: Vec<Pad>,
    sources: Vec<Source>,
    /// The pads last asked for, opened or not: a pad that cannot be opened
    /// must not look like a change on every tick.
    wanted: Vec<PathBuf>,
    assigner: Assigner,
    buffer: Vec<evdev::InputEvent>,
}

#[derive(Debug, Default)]
pub struct Claimed {
    pub pads: Vec<usize>,
    pub progress: Vec<(usize, f64)>,
}

impl Seating {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn seats(&self) -> u32 {
        self.seats
    }

    pub fn open(&mut self, seats: u32) {
        self.open = true;
        self.seats = seats.max(1);
    }

    pub fn close(&mut self) {
        self.open = false;
        self.pads.clear();
        self.sources.clear();
        self.wanted.clear();
        self.assigner.reset();
    }

    pub fn pads(&self) -> &[Pad] {
        &self.pads
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    pub fn wanted(&self, present: &[Pad], seated: &[PathBuf]) -> Vec<Pad> {
        present
            .iter()
            .filter(|pad| !seated.contains(&pad.path))
            .cloned()
            .collect()
    }

    /// Whether [`refresh`] would change the set, measured against what was last
    /// asked for: a pad that cannot be opened must not differ forever.
    pub fn would_change(&self, wanted: &[Pad]) -> bool {
        self.wanted.len() != wanted.len()
            || self
                .wanted
                .iter()
                .zip(wanted)
                .any(|(current, next)| *current != next.path)
    }

    pub fn refresh(&mut self, wanted: Vec<Pad>) -> bool {
        if !self.would_change(&wanted) {
            return false;
        }
        self.wanted = wanted.iter().map(|pad| pad.path.clone()).collect();
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
                    warn!("seating: {} cannot be watched ({error})", pad.name);
                }
            }
        }
        true
    }

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

    pub fn tick(&mut self, now: f64) -> Claimed {
        let Tick { progress, claimed } = self.assigner.tick(now);
        Claimed {
            pads: claimed.into_iter().map(|claim| claim.pad).collect(),
            progress,
        }
    }

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
        let mut seating = Seating::default();
        seating.open(0);
        assert_eq!(seating.seats(), 1);
    }

    #[test]
    fn the_same_set_of_pads_is_not_a_change() {
        let mut seating = Seating::default();
        seating.open(4);
        // Nodes no machine has, so nothing opens here or on a build machine.
        let present = [pad("event90001"), pad("event90002")];
        assert!(
            seating.would_change(&present),
            "an empty seating must open the present pads"
        );
        seating.refresh(present.to_vec());
        assert!(
            !seating.would_change(&present),
            "the same set must not churn the watch list every tick"
        );
        assert!(
            seating.would_change(&present[..1]),
            "a pad leaving is a change"
        );
        assert!(
            seating.pads().is_empty(),
            "these nodes do not exist, so none of them opened"
        );
        assert!(
            !seating.refresh(present.to_vec()),
            "a pad that cannot be opened must not be reopened every tick"
        );
    }
}
