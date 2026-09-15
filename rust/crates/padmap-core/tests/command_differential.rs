//! Hold the command surface to what the Python parses.
//!
//! The socket is the process boundary: it lives in `XDG_RUNTIME_DIR`, any
//! process running as this user may write to it, and a front-end reads back
//! whatever comes of that. So the two implementations have to agree on three
//! separate things -- which names are commands, what each field coerces to,
//! and which messages are refused -- and the third matters as much as the
//! others, because "refused" and "acted on with a nonsense argument" look
//! identical from the socket until a controller moves.

use std::path::Path;

use padmap_core::command::{Command, Refused, COMMANDS};
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

/// The parsed command, rendered the way the Python's dict renders.
fn as_fields(command: &Command) -> Value {
    match command {
        Command::Begin { players } => serde_json::json!({"cmd": "begin", "players": players}),
        Command::Reset => serde_json::json!({"cmd": "reset"}),
        Command::Accept => serde_json::json!({"cmd": "accept"}),
        Command::Cancel => serde_json::json!({"cmd": "cancel"}),
        Command::Map {
            player,
            layout,
            scope,
        } => serde_json::json!({"cmd": "map", "player": player, "layout": layout, "scope": scope}),
        Command::ChooseLayout { player } => {
            serde_json::json!({"cmd": "choose_layout", "player": player})
        }
        Command::ChooseScope { player } => {
            serde_json::json!({"cmd": "choose_scope", "player": player})
        }
        Command::MapForGame {
            player,
            console,
            key,
            title,
        } => serde_json::json!({
            "cmd": "map_for_game", "player": player,
            "console": console, "key": key, "title": title
        }),
        Command::ForgetPad { player } => {
            serde_json::json!({"cmd": "forget_pad", "player": player})
        }
        Command::SkipControl => serde_json::json!({"cmd": "skip_control"}),
        Command::Calibrate { player } => serde_json::json!({"cmd": "calibrate", "player": player}),
        Command::ConfigureEnd => serde_json::json!({"cmd": "configure_end"}),
        Command::SetIcon { player, icon } => {
            serde_json::json!({"cmd": "set_icon", "player": player, "icon": icon})
        }
        Command::Status => serde_json::json!({"cmd": "status"}),
    }
}

#[test]
fn the_two_agree_on_which_names_are_commands() {
    let recorded = corpus("daemon_command_names");
    let want: Vec<&str> = recorded[0]["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .map(|name| name.as_str().expect("a name"))
        .collect();
    assert_eq!(COMMANDS.to_vec(), want);
}

#[test]
fn every_recorded_message_parses_the_same_way() {
    for case in corpus("daemon_commands") {
        let message = &case["message"];
        let got = Command::parse(message);
        if case["ok"].as_bool().expect("ok") {
            match &case["value"] {
                // The Python answers None for a name it does not have; the
                // Rust answers an error. Both refuse, which is the property
                // that matters -- neither acts on it.
                Value::Null => assert!(
                    matches!(got, Err(Refused::Unknown(_))),
                    "{message} should be refused, got {got:?}"
                ),
                want => {
                    let parsed = got.unwrap_or_else(|error| {
                        panic!("{message} was refused as {error}, expected {want}")
                    });
                    assert_eq!(&as_fields(&parsed), want, "for {message}");
                }
            }
        } else {
            // The Python raised. It does not matter which exception, only
            // that the port refuses too rather than acting on a coercion it
            // invented.
            assert!(
                got.is_err(),
                "{message} raised {} in Python, parsed as {got:?} here",
                case["error"]
            );
        }
    }
}

#[test]
fn a_name_that_is_not_a_command_is_refused_rather_than_guessed() {
    for message in [
        serde_json::json!({}),
        serde_json::json!({"cmd": null}),
        serde_json::json!({"cmd": ""}),
        serde_json::json!({"cmd": "nope"}),
        serde_json::json!({"cmd": 7}),
        serde_json::json!({"cmd": "BEGIN"}),
        serde_json::json!({"cmd": " begin"}),
        serde_json::json!({"cmd": "begin "}),
    ] {
        assert!(
            matches!(Command::parse(&message), Err(Refused::Unknown(_))),
            "{message} should be unknown"
        );
    }
}

#[test]
fn a_field_that_must_be_a_number_refuses_rather_than_defaulting() {
    // Defaulting is the dangerous answer: `begin` with a player count of 0
    // opens a session nobody can complete, and it would look like the client
    // asked for that.
    for message in [
        serde_json::json!({"cmd": "begin", "players": "three"}),
        serde_json::json!({"cmd": "begin", "players": null}),
        serde_json::json!({"cmd": "begin", "players": []}),
        serde_json::json!({"cmd": "calibrate", "player": "x"}),
        serde_json::json!({"cmd": "map", "player": {}}),
    ] {
        assert!(
            matches!(Command::parse(&message), Err(Refused::NotANumber { .. })),
            "{message} should be refused"
        );
    }
}

#[test]
fn the_shapes_python_accepts_are_accepted_too() {
    // Refusing what the Python accepts is as much a divergence as accepting
    // what it refuses: a front-end that works today would stop working.
    assert_eq!(
        Command::parse(&serde_json::json!({"cmd": "begin", "players": "3"})),
        Ok(Command::Begin { players: 3 })
    );
    assert_eq!(
        Command::parse(&serde_json::json!({"cmd": "begin", "players": 2.9})),
        Ok(Command::Begin { players: 2 }),
        "int() truncates toward zero"
    );
    assert_eq!(
        Command::parse(&serde_json::json!({"cmd": "begin", "players": true})),
        Ok(Command::Begin { players: 1 }),
        "int(True) is 1"
    );
    // A missing field takes its default rather than refusing.
    assert_eq!(
        Command::parse(&serde_json::json!({"cmd": "begin"})),
        Ok(Command::Begin { players: 4 })
    );
    assert_eq!(
        Command::parse(&serde_json::json!({"cmd": "map"})),
        Ok(Command::Map {
            player: 0,
            layout: String::new(),
            scope: String::new()
        })
    );
    // An unexpected extra field is ignored, not a refusal.
    assert_eq!(
        Command::parse(&serde_json::json!({"cmd": "status", "extra": "ignored"})),
        Ok(Command::Status)
    );
}
