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

/// What makes a watched pad the same pad across a rebuild.
type Identity = (PathBuf, u16, u16, String, String, String);

/// The kernel reuses `/dev/input/eventN`, so a hold must not follow the node on
/// its own: a pad unplugged and another plugged into its number would inherit it.
fn identity(pad: &Pad) -> Identity {
    (
        pad.path.clone(),
        pad.vid,
        pad.pid,
        pad.name.clone(),
        pad.phys.clone(),
        pad.uniq.clone(),
    )
}

#[derive(Debug, Default)]
pub struct Claimed {
    pub pads: Vec<usize>,
    /// Pads filling and how far, earliest press first.
    pub progress: Vec<(usize, f64)>,
    /// Pads that stopped filling without claiming.
    pub released: Vec<usize>,
}

impl Seating {
    /// A seating not yet open, whose holds run for `hold_seconds`.
    pub fn with_hold(hold_seconds: f64) -> Seating {
        let mut seating = Seating::default();
        seating.assigner.set_hold_seconds(hold_seconds);
        seating
    }

    /// How long a hold has to run here to claim a seat.
    pub fn hold_seconds(&self) -> f64 {
        self.assigner.hold_seconds()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn seats(&self) -> u32 {
        self.seats
    }

    /// Open for `seats` players. `hold` omitted leaves the length as it was,
    /// so a caller that does not care never resets one that does.
    pub fn open(&mut self, seats: u32, hold: Option<f64>) {
        self.open = true;
        self.seats = seats.max(1);
        if let Some(seconds) = hold {
            self.assigner.set_hold_seconds(seconds);
        }
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
        // Pads as watched before the rebuild, so holds can follow to their new index.
        let before: Vec<Identity> = self.pads.iter().map(identity).collect();
        // A pad that stays watched keeps its descriptor. Closing it to rebuild
        // the list threw away whatever was queued on it, and a button that went
        // down in that moment stays down and never sends another edge.
        let mut kept: Vec<Option<Source>> = std::mem::take(&mut self.sources)
            .into_iter()
            .map(Some)
            .collect();
        self.pads.clear();
        for pad in wanted {
            let reused = before
                .iter()
                .position(|was| *was == identity(&pad))
                .and_then(|at| kept[at].take());
            let source = match reused {
                Some(source) => source,
                None => match clone::open_source(&pad, false) {
                    Ok(source) => {
                        // Ask SDL about it now, off the loop, so the answer is
                        // there by the time this pad claims.
                        if let Some(guid) = source.physical_guid() {
                            padmap_input::sdlprobe::shared().prefetch(&guid);
                        }
                        source
                    }
                    Err(error) => {
                        warn!("seating: {} cannot be watched ({error})", pad.name);
                        continue;
                    }
                },
            };
            self.pads.push(pad);
            self.sources.push(source);
        }
        // Whatever is left in `kept` has stopped being watched and closes here.
        let after: Vec<Identity> = self.pads.iter().map(identity).collect();
        let moved = padmap_core::assign::moved_indices(&before, &after);
        self.assigner.remap(|pad| moved.get(pad).copied().flatten());
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
            // When it happened, not when this got round to reading it: a
            // claim just before can hold the loop for a good part of a second.
            self.assigner.feed(
                index,
                event.event_type().0,
                event.code(),
                event.value(),
                now - clone::event_age(event),
            );
        }
    }

    pub fn tick(&mut self, now: f64) -> Claimed {
        let Tick {
            progress,
            claimed,
            released,
        } = self.assigner.tick(now);
        Claimed {
            pads: claimed.into_iter().map(|claim| claim.pad).collect(),
            progress,
            released,
        }
    }

    /// Drop one pad's hold by path, since a refresh this tick may have renumbered.
    pub fn forget(&mut self, path: &std::path::Path) {
        match self.pads.iter().position(|pad| pad.path == path) {
            Some(index) => self.assigner.forget(index),
            // Not reachable today, and silent it would be a pad that can never
            // retry: it keeps a claim on a seat it was refused.
            None => warn!("seating: no pad at {} to forget", path.display()),
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
    fn opening_without_a_length_leaves_the_one_already_set() {
        let mut seating = Seating::with_hold(1.5);
        assert_eq!(seating.hold_seconds(), 1.5);
        seating.open(4, None);
        assert_eq!(
            seating.hold_seconds(),
            1.5,
            "a caller who did not ask reset it"
        );
        seating.open(4, Some(0.5));
        assert_eq!(seating.hold_seconds(), 0.5);
        // Closing frees the pads; the length is a setting, not a claim.
        seating.close();
        assert_eq!(seating.hold_seconds(), 0.5);
    }

    #[test]
    fn closing_lets_go_of_everything() {
        let mut seating = Seating::default();
        seating.open(4, None);
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
        seating.open(0, None);
        assert_eq!(seating.seats(), 1);
    }

    #[test]
    fn the_same_set_of_pads_is_not_a_change() {
        let mut seating = Seating::default();
        seating.open(4, None);
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
