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
    /// The keyboard's seat: no device behind it, bound in every emulator by name.
    #[serde(default)]
    pub keyboard: bool,
}

/// The whole event.
// `Eq` is gone with `hold`: a length is a float and floats are not Eq.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateEvent {
    pub event: String,
    pub state: String,
    pub slots: u32,
    pub players: Vec<PlayerState>,
    pub build: String,
    pub pid: u32,
    pub identity: String,
    /// The pid this daemon ends with, when started with `--follow`.
    #[serde(default)]
    pub following: Option<u32>,
    /// Whether an unseated pad holding a button would take a seat right now.
    #[serde(default)]
    pub seating: bool,
    /// How long that hold has to run, in seconds.
    #[serde(default)]
    pub hold: f64,
    /// Seats published before anybody took them, for a launch to bind.
    #[serde(default)]
    pub reserved: Vec<ReservedSeat>,
}

/// One seat a launch can bind before anybody sits down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservedSeat {
    pub player: u32,
    /// The clone's `/dev/input/eventN`.
    pub node: String,
    /// What SDL will call it, and the GUID it will match.
    pub name: String,
    pub guid: String,
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
            following: None,
            seating: false,
            hold: crate::assign::HOLD_SECONDS,
            reserved: Vec::new(),
        }
    }
}
