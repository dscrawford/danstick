//! The `state` event: what the daemon tells a client about itself.
//!
//! Sent on connect, and again whenever anything a client could be drawing
//! changes. It is the only thing a front-end has: there is no other way to ask
//! what padmap thinks is going on.
//!
//! Three of its fields exist because nothing else on the machine reveals them.
//! A stale daemon keeps answering and keeps writing files that look right, so
//! only the build id says the code changed underneath it. Matching a daemon by
//! its command line hits every one the user is running, including ones on
//! another `XDG_RUNTIME_DIR` that are none of the caller's business, so the pid
//! is reported. And a daemon started without `PADMAP_ONLY_VIRTUAL` and a
//! front-end started with it disagree about which pads exist, with a machine
//! that has no controllers at all as the symptom -- so the identity mode is
//! reported too.

use serde::{Deserialize, Serialize};

/// Where the daemon is in the assignment flow.
pub const STATE_IDLE: &str = "idle";
pub const STATE_ASSIGNING: &str = "assigning";
pub const STATE_READY: &str = "ready";

/// One player, as a client needs to draw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerState {
    pub player: u32,
    /// Printable characters only: this reaches a filename and a screen.
    pub name: String,
    /// `eventN`, not the whole path.
    pub node: String,
    pub icon: String,
    /// **Mapped**, not merely known.
    ///
    /// A profile exists for several reasons -- calibration writes one, and so
    /// does finishing a session -- so keying this on a profile's existence
    /// meant a controller counted as set up before anyone had told padmap
    /// where its buttons were, and the wizard was offered exactly once and
    /// never again.
    pub configured: bool,
    /// Which scopes this controller has a capture under, so a front-end can
    /// say what already exists rather than making re-mapping a blind,
    /// destructive act.
    ///
    /// Scope strings, not labels: the labels are built where the picker is,
    /// and a second set here would be a second thing to keep in step.
    pub mappings: Vec<String>,
}

/// The whole event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateEvent {
    pub event: String,
    pub state: String,
    pub slots: u32,
    pub players: Vec<PlayerState>,
    pub build: String,
    pub pid: u32,
    pub identity: String,
}

impl StateEvent {
    pub fn new(
        state: &str,
        slots: u32,
        players: Vec<PlayerState>,
        build: String,
        pid: u32,
        identity: &str,
    ) -> StateEvent {
        StateEvent {
            event: "state".to_owned(),
            state: state.to_owned(),
            slots,
            players,
            build,
            pid,
            identity: identity.to_owned(),
        }
    }
}
