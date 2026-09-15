//! What a client may ask the daemon to do.
//!
//! This is the process boundary. The socket lives in `XDG_RUNTIME_DIR` and any
//! process running as this user may write to it, so every field here arrives
//! from outside and is coerced rather than trusted -- and where the coercion
//! fails, the refusal is part of the contract, because a front-end reads the
//! error reply and shows it to somebody.
//!
//! Named as an enum rather than matched as strings in a dispatch chain. A
//! command that is defined but never *routed* is a handler that exists, reads
//! correctly, and is unreachable -- and nothing says so until someone presses
//! the key on the setup screen, where the pads are grabbed and there is no
//! other feedback to fall back on.

use serde_json::Value;

/// Every command the socket accepts.
pub const COMMANDS: [&str; 14] = [
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
];

/// A parsed command, with its arguments already coerced.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

/// Why a message could not be acted on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// Not a command padmap has. Answered with `unknown command`.
    #[error("unknown command {0:?}")]
    Unknown(String),
    /// A field that must be a number was something else. The Python raises
    /// here and the daemon answers with the exception's text, so a client
    /// sees a refusal either way -- what matters is that neither
    /// implementation *acts* on it.
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
        // Python's `int(...)` on a bool is 0 or 1, and on a float truncates
        // toward zero. Both are reachable from a JSON message and both are
        // reproduced, because refusing what the Python accepts is as much a
        // divergence as accepting what it refuses.
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
        // Strings only, and anything else is empty rather than rendered.
        //
        // `str(...)` on a JSON null gives the four characters "None" and on a
        // `true` gives "True", and those are *stored* -- an icon called
        // "None" is written into a profile as though somebody chose it. It is
        // also a cross-language trap, since Rust renders the same boolean
        // "true" and the two would disagree about a message they both read.
        // The Python was changed to match; the same rule as
        // `runtime::recent_games_from`.
        //
        // Not a refusal, unlike a bad number: an empty layout or icon means
        // "unset", which every caller already handles, where an empty player
        // number would mean player zero.
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
            // Unreachable: the membership test above is the only way in.
            other => return Err(Refused::Unknown(other.to_owned())),
        })
    }
}
