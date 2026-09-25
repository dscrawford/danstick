//! Socket protocol: what a client may ask the daemon to do.

use serde_json::Value;

/// Every command the socket accepts.
pub const COMMANDS: [&str; 22] = [
    "begin",
    "reset",
    "accept",
    "cancel",
    "map",
    "choose_layout",
    "choose_scope",
    "map_for_game",
    "forget_pad",
    "skip_control",
    "calibrate",
    "configure_end",
    "set_icon",
    "status",
    "seating",
    "tune",
    "unseat",
    "seat_keyboard",
    "bind",
    "reserve",
    "identity",
    "slots",
];

/// A parsed command, with its arguments already coerced.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Begin {
        players: i64,
    },
    Reset,
    Accept,
    Cancel,
    Map {
        player: i64,
        layout: String,
        scope: String,
    },
    ChooseLayout {
        player: i64,
    },
    ChooseScope {
        player: i64,
    },
    MapForGame {
        player: i64,
        console: String,
        key: String,
        title: String,
    },
    ForgetPad {
        player: i64,
    },
    SkipControl,
    Calibrate {
        player: i64,
    },
    ConfigureEnd,
    SetIcon {
        player: i64,
        icon: String,
    },
    Status,
    /// Listen for an unseated controller taking a free seat, with no session open and nothing grabbed.
    Seating {
        open: bool,
        /// How many seats exist.
        players: i64,
        /// How long a hold must run to claim one; omitted leaves it as it was.
        hold: Option<f64>,
    },
    /// Set what a misbehaving controller needs: a deadzone, a debounce, an axis or button to ignore.
    Tune {
        player: i64,
        signature: String,
        request: crate::tuning::Request,
    },
    /// Drop a seat (player 0: every seat), stop its clone, ungrab its pad; seating stays as it was.
    Unseat {
        player: i64,
    },
    /// Seat the keyboard as the next player: no device, no clone, just every emulator's keyboard on that port.
    SeatKeyboard,
    /// Capture the next press onto one control: replacing its binding, or with `add`, beside it.
    Bind {
        player: i64,
        control: String,
        scope: String,
        add: bool,
    },
    /// Publish a clone per seat a launch allows, so the seats exist before the
    /// people do and a game bound to them can be joined mid-play.
    Reserve {
        players: i64,
    },
    /// Publish every clone under another identity, keeping every seat.
    Identity {
        mode: String,
    },
    /// Change how slots are published; a field left out keeps what is in force.
    Slots(crate::slots::Change),
}

/// Why a message could not be acted on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// Not a command padmap has.
    #[error("unknown command {0:?}")]
    Unknown(String),
    /// A `tune` field that is not what it claims to be.
    #[error("{0}")]
    BadTuning(String),
    /// A field that must be a number was something else.
    #[error("{field} is not a number")]
    NotANumber { field: &'static str },
}

impl Command {
    /// Parse one message.
    pub fn parse(message: &Value) -> Result<Command, Refused> {
        let name = message.get("cmd").and_then(Value::as_str).unwrap_or("");
        if !COMMANDS.contains(&name) {
            return Err(Refused::Unknown(name.to_owned()));
        }
        let number = |field: &'static str, default: i64| -> Result<i64, Refused> {
            match message.get(field) {
                None => Ok(default),
                Some(Value::Number(value)) => value
                    .as_i64()
                    .or_else(|| value.as_f64().map(|float| float as i64))
                    .ok_or(Refused::NotANumber { field }),
                Some(Value::Bool(value)) => Ok(i64::from(*value)),
                Some(Value::String(text)) => text
                    .trim()
                    .parse()
                    .map_err(|_| Refused::NotANumber { field }),
                Some(_) => Err(Refused::NotANumber { field }),
            }
        };
        // Strings only; anything else (including bools/nulls) becomes empty, not rendered.
        let text = |field: &str| -> String {
            match message.get(field) {
                Some(Value::String(value)) => value.clone(),
                _ => String::new(),
            }
        };

        Ok(match name {
            "begin" => Command::Begin {
                players: number("players", 4)?,
            },
            "reset" => Command::Reset,
            "accept" => Command::Accept,
            "cancel" => Command::Cancel,
            "map" => Command::Map {
                player: number("player", 0)?,
                layout: text("layout"),
                scope: text("scope"),
            },
            "choose_layout" => Command::ChooseLayout {
                player: number("player", 0)?,
            },
            "choose_scope" => Command::ChooseScope {
                player: number("player", 0)?,
            },
            "map_for_game" => Command::MapForGame {
                player: number("player", 0)?,
                console: text("console"),
                key: text("key"),
                title: text("title"),
            },
            "forget_pad" => Command::ForgetPad {
                player: number("player", 0)?,
            },
            "skip_control" => Command::SkipControl,
            "calibrate" => Command::Calibrate {
                player: number("player", 0)?,
            },
            "configure_end" => Command::ConfigureEnd,
            "set_icon" => Command::SetIcon {
                player: number("player", 0)?,
                icon: text("icon"),
            },
            "status" => Command::Status,
            "tune" => Command::Tune {
                player: number("player", 0)?,
                signature: text("signature"),
                request: crate::tuning::Request::from_json(message).map_err(Refused::BadTuning)?,
            },
            "seating" => Command::Seating {
                open: match message.get("open") {
                    Some(Value::Bool(value)) => *value,
                    None => true,
                    Some(_) => return Err(Refused::NotANumber { field: "open" }),
                },
                players: number("players", 4)?,
                // Never refused: a comfort setting is not worth failing to open
                // seating over, so a length nobody can read is the default.
                hold: message
                    .get("hold")
                    .filter(|value| !value.is_null())
                    .map(|value| crate::assign::hold_or_default(value.as_f64())),
            },
            "unseat" => Command::Unseat {
                player: number("player", 0)?,
            },
            "seat_keyboard" => Command::SeatKeyboard,
            "reserve" => Command::Reserve {
                players: number("players", 0)?,
            },
            "identity" => Command::Identity { mode: text("mode") },
            "slots" => Command::Slots(crate::slots::Change {
                mode: message
                    .get("mode")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                count: match message.get("count") {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(
                        value
                            .as_i64()
                            .ok_or(Refused::NotANumber { field: "count" })?,
                    ),
                },
                on_leave: message
                    .get("on_leave")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }),
            "bind" => Command::Bind {
                player: number("player", 0)?,
                control: text("control"),
                scope: text("scope"),
                add: match message.get("add") {
                    Some(Value::Bool(value)) => *value,
                    None => false,
                    Some(_) => return Err(Refused::NotANumber { field: "add" }),
                },
            },
            other => return Err(Refused::Unknown(other.to_owned())),
        })
    }
}
