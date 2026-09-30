//! Taking a seat without a session.

use std::path::PathBuf;

use danstick_core::assign::{is_press, press_at, Assigner, Gate, Tick};
use danstick_core::echo::{Echoes, Origin, Verdict};
use danstick_input::clone::{self, Source};
use danstick_input::pad::{self, Pad};
use log::{debug, warn};

#[derive(Debug, Default)]
pub struct Seating {
    seats: u32,
    open: bool,
    pads: Vec<Pad>,
    sources: Vec<Source>,
    /// When each source began being watched; no press is dated before it.
    watched: Vec<f64>,
    /// The pads last asked for, opened or not.
    wanted: Vec<PathBuf>,
    assigner: Assigner,
    buffer: Vec<evdev::InputEvent>,
    /// Every press that could be a Steam pad's, or one a Steam pad repeats.
    echoes: Echoes,
}

/// What makes a watched pad the same pad across a rebuild.
type Identity = (PathBuf, u16, u16, String, String, String);

/// A pad's identity, stable across node number reuse.
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
    /// Each claiming pad Steam surely repeats, and the Steam pad repeating it.
    pub twins: Vec<(PathBuf, Pad)>,
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

    /// Opens for `seats` players; `hold` omitted leaves the length unchanged.
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
        self.watched.clear();
        self.wanted.clear();
        self.assigner.reset();
        self.echoes.clear();
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

    /// Whether [`refresh`] would change the watched set.
    pub fn would_change(&self, wanted: &[Pad]) -> bool {
        self.wanted.len() != wanted.len()
            || self
                .wanted
                .iter()
                .zip(wanted)
                .any(|(current, next)| *current != next.path)
    }

    pub fn refresh(&mut self, wanted: Vec<Pad>, now: f64) -> bool {
        if !self.would_change(&wanted) {
            return false;
        }
        self.wanted = wanted.iter().map(|pad| pad.path.clone()).collect();
        // Pads as watched before the rebuild, so holds can follow to their new index.
        let before: Vec<Identity> = self.pads.iter().map(identity).collect();
        // A pad that stays watched keeps its source, so queued input is not lost.
        let mut kept: Vec<Option<(Source, f64)>> = std::mem::take(&mut self.sources)
            .into_iter()
            .zip(std::mem::take(&mut self.watched))
            .map(Some)
            .collect();
        self.pads.clear();
        for pad in wanted {
            let reused = before
                .iter()
                .position(|was| *was == identity(&pad))
                .and_then(|at| kept[at].take());
            let (source, since) = match reused {
                Some(kept) => kept,
                None => match clone::open_source(&pad, false) {
                    Ok(source) => {
                        // Ask SDL about it now, off the loop, so the answer is ready by claim time.
                        if let Some(guid) = source.physical_guid() {
                            danstick_input::sdlprobe::shared().prefetch(&guid);
                        }
                        (source, now)
                    }
                    Err(error) => {
                        warn!("seating: {} cannot be watched ({error})", pad.name);
                        continue;
                    }
                },
            };
            self.pads.push(pad);
            self.sources.push(source);
            self.watched.push(since);
        }
        // Whatever is left in `kept` has stopped being watched and closes here.
        let after: Vec<Identity> = self.pads.iter().map(identity).collect();
        let moved = danstick_core::assign::moved_indices(&before, &after);
        self.assigner.remap(|pad| moved.get(pad).copied().flatten());
        self.echoes.remap(|pad| moved.get(pad).copied().flatten());
        true
    }

    /// A button a seated player's pad pressed at `at`, or danstick wrote to its clone.
    pub fn note(&mut self, origin: Origin, at: f64) {
        if self.open {
            self.echoes.press(origin, at);
        }
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
        let steam = self.pads.get(index).is_some_and(pad::is_steam_virtual);
        let watched = self.watched.get(index).copied().unwrap_or(now);
        for event in &self.buffer {
            // When it happened, not when this got read: a claim can hold the loop a while.
            let at = press_at(now, clone::event_age(event), watched);
            let (kind, code, value) = (event.event_type().0, event.code(), event.value());
            if is_press(kind, code, value) {
                self.echoes.press(Origin::Watched { pad: index, steam }, at);
            }
            self.assigner.feed(index, kind, code, value, at);
        }
    }

    pub fn tick(&mut self, now: f64) -> Claimed {
        let holds = self.assigner.holds();
        let verdicts = self.echoes.judge(&holds, now);
        let pads = &self.pads;
        let verdict_of = |index: usize, since: f64| {
            holds
                .iter()
                .position(|hold| *hold == (index, since))
                .map(|at| verdicts[at])
        };
        let Tick {
            progress,
            claimed,
            released,
        } = self
            .assigner
            .tick_gated(now, |index, since| match verdict_of(index, since) {
                Some(verdict) if !verdict.may_claim() => {
                    if let Some(pad) = pads.get(index) {
                        debug!("seating: {} ({}) is {verdict:?}", pad.name, pad.event());
                    }
                    Gate::Drop
                }
                _ => Gate::Go,
            });
        let mut twins = Vec::new();
        // Only while it claims: once seated, telling its press from its clone's is no longer clear.
        for (&(index, _), verdict) in holds.iter().zip(&verdicts) {
            if let Verdict::Mirrored { steam, sure: true } = *verdict {
                if claimed.iter().any(|claim| claim.pad == index) {
                    if let (Some(pad), Some(steam)) = (pads.get(index), pads.get(steam)) {
                        twins.push((pad.path.clone(), steam.clone()));
                    }
                }
            }
        }
        Claimed {
            pads: claimed.into_iter().map(|claim| claim.pad).collect(),
            progress,
            released,
            twins,
        }
    }

    /// Drop one pad's hold by path, since a refresh this tick may have renumbered.
    pub fn forget(&mut self, path: &std::path::Path) {
        match self.pads.iter().position(|pad| pad.path == path) {
            Some(index) => self.assigner.forget(index),
            // Not reachable today; silent it would be a pad stuck holding a refused seat.
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
        seating.refresh(present.to_vec(), 0.0);
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
            !seating.refresh(present.to_vec(), 0.0),
            "a pad that cannot be opened must not be reopened every tick"
        );
    }
}
