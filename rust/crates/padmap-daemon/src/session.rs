//! An assignment session: the pads, held open and grabbed, and who has claimed what.

use std::collections::BTreeMap;

use evdev::InputEvent;
use log::warn;
use padmap_core::assign::{self, Assigner};
use padmap_core::capture::{EV_ABS, EV_KEY};
use padmap_input::clone::{self, Source};
use padmap_input::pad::Pad;

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
    /// Pads not grabbed exclusively; their presses reach other applications too.
    pub grab_failures: Vec<String>,
    buffer: Vec<InputEvent>,
    read_seen: Vec<bool>,
    /// A pad whose node went away mid-session; its slot and claim stay.
    gone: Vec<bool>,
}

impl Session {
    /// Retries without the pad that failed, one fewer each pass, so it terminates.
    pub fn open(pads: Vec<Pad>) -> Result<Session, OpenError> {
        let mut remaining = pads;
        let mut last = String::new();
        while !remaining.is_empty() {
            let mut sources = Vec::with_capacity(remaining.len());
            let mut grab_failures = Vec::new();
            let mut gone: Option<usize> = None;
            for (index, pad) in remaining.iter().enumerate() {
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
                        gone: vec![false; remaining.len()],
                        pads: remaining,
                        sources,
                        assigner: Assigner::new(crate::configured_hold()),
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

    /// A pad switched on after the session opened: grabbed and read like the
    /// others, claimable by the same hold. Returns its index.
    pub fn admit(&mut self, pad: Pad) -> Result<usize, clone::CloneError> {
        let mut source = clone::open_source(&pad, false)?;
        if source.grab().is_err() {
            self.grab_failures.push(pad.event().to_owned());
        }
        source.drain();
        if let Some(index) = self.index_of(&pad.path) {
            // Back on the same node: take its old place, claim and all.
            self.sources[index] = source;
            self.pads[index] = pad;
            self.gone[index] = false;
            self.read_seen[index] = false;
            return Ok(index);
        }
        self.pads.push(pad);
        self.sources.push(source);
        self.read_seen.push(false);
        self.gone.push(false);
        Ok(self.pads.len() - 1)
    }

    /// Whether the pad at `index` has gone away since it was opened.
    pub fn is_gone(&self, index: usize) -> bool {
        self.gone.get(index).copied().unwrap_or(true)
    }

    /// How many pads are here to be read.
    pub fn present(&self) -> usize {
        self.gone.iter().filter(|gone| !**gone).count()
    }

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

    pub fn index_of(&self, path: &std::path::Path) -> Option<usize> {
        self.pads.iter().position(|pad| pad.path == path)
    }

    pub fn reset(&mut self) {
        self.assigner.reset();
        self.drain();
    }

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
                if !self.gone[index] {
                    self.gone[index] = true;
                    log::info!(
                        "session pad {index} ({}) went away; its claim is kept for its return",
                        self.pads.get(index).map(|pad| pad.event()).unwrap_or("?")
                    );
                }
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

    pub fn claims(&self) -> Vec<(u32, usize)> {
        self.assigner
            .assignments()
            .iter()
            .map(|assignment| (assignment.player, assignment.pad))
            .collect()
    }

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
