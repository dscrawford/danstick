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

    pub fn refresh(&mut self, wanted: Vec<Pad>) -> bool {
        let current: Vec<PathBuf> = self.pads.iter().map(|pad| pad.path.clone()).collect();
        let next: Vec<PathBuf> = wanted.iter().map(|pad| pad.path.clone()).collect();
        if current == next {
            return false;
        }
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
}
