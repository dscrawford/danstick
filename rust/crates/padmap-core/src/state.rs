//! The `state` event: what the daemon tells a client about itself.

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
    /// Mapped, not merely known.
    pub configured: bool,
    /// Scopes with captures for this controller.
    pub mappings: Vec<String>,
    /// Whether this seat has a clone on the air.
    #[serde(default)]
    pub published: bool,
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
