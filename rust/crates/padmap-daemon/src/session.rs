//! An assignment session: the pads, held open and grabbed, and who has
//! claimed what.
//!
//! Republishing grabs the physical pads, so it has to stop before a session
//! can open them; otherwise every press would be invisible. The daemon owns
//! that ordering; this owns the pads once they are its.

use std::collections::BTreeMap;

use evdev::InputEvent;
use log::warn;
use padmap_core::assign::{self, Assigner};
use padmap_core::capture::{EV_ABS, EV_KEY};
use padmap_input::clone::{self, Source};
use padmap_input::pad::Pad;

/// One event off a session pad, reduced to what the flows read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raw {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl Raw {
    pub fn is_press_or_release(&self) -> bool {
        self.kind == EV_KEY && self.code >= assign::BTN_FIRST
    }

    pub fn is_input(&self) -> bool {
        self.kind == EV_KEY || self.kind == EV_ABS
    }
}

/// Why no session could be opened at all.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("the controllers went away before setup could open them: {0}")]
    AllGone(String),
}

#[derive(Debug)]
pub struct Session {
    pub pads: Vec<Pad>,
    sources: Vec<Source>,
    pub assigner: Assigner,
    /// Pads that could not be grabbed exclusively; their presses reach other
    /// applications as well as us.
    pub grab_failures: Vec<String>,
    /// Reused between reads so the hot path allocates nothing.
    buffer: Vec<InputEvent>,
    /// Whether each pad has ever produced a read. See [`Session::read`].
    read_seen: Vec<bool>,
}

impl Session {
    /// Open and grab a set of pads, skipping any that have gone away.
    ///
    /// Discovery lists what /sys said a moment ago; opening reads what /dev
    /// has now. A controller unplugged in between fails to open, and that
    /// used to end the daemon -- at the moment padmap was trying to be
    /// helpful about the pad that had just arrived.
    ///
    /// Retries without the pad that failed rather than abandoning the
    /// session, because unplugging one controller is no reason the other
    /// three cannot be assigned. Every pass drops one pad, so it terminates.
    pub fn open(pads: Vec<Pad>) -> Result<Session, OpenError> {
        let mut remaining = pads;
        let mut last = String::new();
        while !remaining.is_empty() {
            let mut sources = Vec::with_capacity(remaining.len());
            let mut grab_failures = Vec::new();
            let mut gone: Option<usize> = None;
            for (index, pad) in remaining.iter().enumerate() {
                // Opened without grabbing; the grab comes once every pad is
                // open, so its failures can be remembered by name -- "presses
                // also reach the front-end" explains a setup screen that
                // seems to answer itself.
                match clone::open_source(pad, false) {
                    Ok(source) => sources.push(source),
                    Err(error) => {
                        last = error.to_string();
                        gone = Some(index);
                        break;
                    }
                }
            }
            match gone {
                None => {
                    let mut session = Session {
                        read_seen: vec![false; remaining.len()],
                        pads: remaining,
                        sources,
                        assigner: Assigner::default(),
                        grab_failures,
                        buffer: Vec::with_capacity(64),
                    };
                    for (pad, source) in session.pads.iter().zip(&mut session.sources) {
                        if source.grab().is_err() {
                            session.grab_failures.push(pad.event().to_owned());
                        }
                    }
                    session.drain();
                    return Ok(session);
                }
                Some(index) => {
                    // Whatever was opened is dropped here, and dropping a
                    // Source ungrabs it: nobody else will release those, and
                    // every one is a controller that has stopped working
                    // until the daemon dies.
                    drop(sources);
                    let dropped = remaining.remove(index);
                    grab_failures.clear();
                    warn!(
                        "{} went away between discovery and opening it; carrying on with {} other pad(s)",
                        dropped.path.display(),
                        remaining.len()
                    );
                }
            }
        }
        Err(OpenError::AllGone(last))
    }

    /// Discard anything queued before we started listening, so a button
    /// still held from before cannot claim a slot.
    pub fn drain(&mut self) {
        for source in &mut self.sources {
            source.drain();
        }
    }

    pub fn len(&self) -> usize {
        self.pads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pads.is_empty()
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    pub fn source_mut(&mut self, index: usize) -> Option<&mut Source> {
        self.sources.get_mut(index)
    }

    /// The session's index for a pad, by device path.
    pub fn index_of(&self, path: &std::path::Path) -> Option<usize> {
        self.pads.iter().position(|pad| pad.path == path)
    }

    /// Drop every claim and start over. Drains too, so a button still held
    /// from the previous round cannot immediately re-claim.
    pub fn reset(&mut self) {
        self.assigner.reset();
        self.drain();
    }

    /// Everything queued on one pad, reduced.
    ///
    /// The first read from each pad is logged. A pad that is never readable is
    /// never read, and that failure is otherwise entirely silent -- the screen
    /// says "hold a button", the person holds it, and no log anywhere says the
    /// descriptor never woke. It cost a day on a Steam Controller once.
    pub fn read(&mut self, index: usize) -> Vec<Raw> {
        let Some(source) = self.sources.get_mut(index) else {
            return Vec::new();
        };
        if !self.read_seen[index] {
            self.read_seen[index] = true;
            log::info!(
                "session pad {index} ({}) first read",
                self.pads.get(index).map(|pad| pad.event()).unwrap_or("?")
            );
        }
        self.buffer.clear();
        if let Err(error) = source.fetch_events(&mut self.buffer) {
            if error.kind() != std::io::ErrorKind::WouldBlock {
                warn!("reading session pad {index}: {error}");
            }
            return Vec::new();
        }
        self.buffer
            .iter()
            .map(|event| Raw {
                kind: event.event_type().0,
                code: event.code(),
                value: event.value(),
            })
            .collect()
    }

    /// The claims so far, as (player, pad index).
    pub fn claims(&self) -> Vec<(u32, usize)> {
        self.assigner
            .assignments()
            .iter()
            .map(|assignment| (assignment.player, assignment.pad))
            .collect()
    }

    /// Claims as player -> pad.
    pub fn claimed_pads(&self) -> BTreeMap<u32, &Pad> {
        self.claims()
            .into_iter()
            .filter_map(|(player, index)| self.pads.get(index).map(|pad| (player, pad)))
            .collect()
    }

    pub fn is_claimed(&self, index: usize) -> bool {
        self.assigner.is_claimed(index)
    }
}
