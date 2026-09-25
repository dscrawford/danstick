//! Hold the socket command surface to what the Python parses and refuses.

use std::path::Path;

use padmap_core::command::{Command, Refused, COMMANDS};
use serde_json::{json, Value};

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/corpus/{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

/// The parsed command, rendered the way the Python's dict renders.
fn as_fields(command: &Command) -> Value {
    match command {
        Command::Begin { players } => json!({"cmd": "begin", "players": players}),
        Command::Reset => json!({"cmd": "reset"}),
        Command::Accept => json!({"cmd": "accept"}),
        Command::Cancel => json!({"cmd": "cancel"}),
        Command::Map {
            player,
            layout,
            scope,
        } => {
            json!({"cmd": "map", "player": player, "layout": layout, "scope": scope})
        }
        Command::ChooseLayout { player } => json!({"cmd": "choose_layout", "player": player}),
        Command::ChooseScope { player } => json!({"cmd": "choose_scope", "player": player}),
        Command::MapForGame {
            player,
            console,
            key,
            title,
        } => json!({
            "cmd": "map_for_game", "player": player,
            "console": console, "key": key, "title": title
        }),
        Command::ForgetPad { player } => json!({"cmd": "forget_pad", "player": player}),
        Command::SkipControl => json!({"cmd": "skip_control"}),
        Command::Calibrate { player } => json!({"cmd": "calibrate", "player": player}),
        Command::ConfigureEnd => json!({"cmd": "configure_end"}),
        Command::SetIcon { player, icon } => {
            json!({"cmd": "set_icon", "player": player, "icon": icon})
        }
        Command::Status => json!({"cmd": "status"}),
        // Exhaustive on purpose: a command added and never routed is a compile error here.
        Command::Seating {
            open,
            players,
            hold,
        } => {
            json!({"cmd": "seating", "open": open, "players": players, "hold": hold})
        }
        Command::Unseat { player } => json!({"cmd": "unseat", "player": player}),
        Command::SeatKeyboard => json!({"cmd": "seat_keyboard"}),
        Command::Reserve { players } => json!({"cmd": "reserve", "players": players}),
        Command::Identity { mode } => json!({"cmd": "identity", "mode": mode}),
        Command::Slots(change) => json!({
            "cmd": "slots", "mode": change.mode, "count": change.count, "on_leave": change.on_leave
        }),
        Command::Bind {
            player,
            control,
            scope,
            add,
        } => {
            json!({"cmd": "bind", "player": player, "control": control, "scope": scope, "add": add})
        }
        Command::Tune {
            player,
            signature,
            request,
        } => {
            let mut out = request.to_json();
            out["cmd"] = "tune".into();
            out["player"] = (*player).into();
            out["signature"] = signature.as_str().into();
            out
        }
    }
}

#[test]
fn the_two_agree_on_which_names_are_commands() {
    let recorded = corpus("daemon_command_names");
    let want: Vec<&str> = recorded[0]["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .map(|n| n.as_str().expect("a name"))
        .collect();
    let ours = COMMANDS.to_vec();
    assert!(
        ours.len() >= want.len(),
        "commands went missing: {ours:?} against {want:?}"
    );
    assert_eq!(
        ours[..want.len()].to_vec(),
        want,
        "a recorded command was dropped, renamed or reordered"
    );
}

#[test]
fn every_recorded_message_parses_the_same_way() {
    for case in corpus("daemon_commands") {
        let message = &case["message"];
        let got = Command::parse(message);
        if case["ok"].as_bool().expect("ok") {
            match &case["value"] {
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
        json!({}),
        json!({"cmd": null}),
        json!({"cmd": ""}),
        json!({"cmd": "nope"}),
        json!({"cmd": 7}),
        json!({"cmd": "BEGIN"}),
        json!({"cmd": " begin"}),
        json!({"cmd": "begin "}),
    ] {
        assert!(
            matches!(Command::parse(&message), Err(Refused::Unknown(_))),
            "{message} should be unknown"
        );
    }
}

#[test]
fn a_field_that_must_be_a_number_refuses_rather_than_defaulting() {
    for message in [
        json!({"cmd": "begin", "players": "three"}),
        json!({"cmd": "begin", "players": null}),
        json!({"cmd": "begin", "players": []}),
        json!({"cmd": "calibrate", "player": "x"}),
        json!({"cmd": "map", "player": {}}),
    ] {
        assert!(
            matches!(Command::parse(&message), Err(Refused::NotANumber { .. })),
            "{message} should be refused"
        );
    }
}

#[test]
fn the_shapes_python_accepts_are_accepted_too() {
    for (name, message, want) in [
        (
            "a numeric string",
            json!({"cmd": "begin", "players": "3"}),
            Command::Begin { players: 3 },
        ),
        (
            "int() truncates toward zero",
            json!({"cmd": "begin", "players": 2.9}),
            Command::Begin { players: 2 },
        ),
        (
            "int(True) is 1",
            json!({"cmd": "begin", "players": true}),
            Command::Begin { players: 1 },
        ),
        (
            "a missing field takes its default",
            json!({"cmd": "begin"}),
            Command::Begin { players: 4 },
        ),
        (
            "map defaults every field",
            json!({"cmd": "map"}),
            Command::Map {
                player: 0,
                layout: String::new(),
                scope: String::new(),
            },
        ),
        (
            "an extra field is ignored",
            json!({"cmd": "status", "extra": "ignored"}),
            Command::Status,
        ),
    ] {
        assert_eq!(Command::parse(&message), Ok(want), "{name}");
    }
}

#[test]
fn reserving_seats_carries_how_many_a_launch_allows() {
    for (what, message, want) in [
        (
            "how many a launch asks for",
            json!({"cmd": "reserve", "players": 4}),
            4,
        ),
        // Nought is how a launcher gives the seats back when its game is over.
        ("left out is none", json!({"cmd": "reserve"}), 0),
        (
            "a numeric string",
            json!({"cmd": "reserve", "players": "10"}),
            10,
        ),
        (
            "past the slots there are, which the daemon clamps rather than refuses",
            json!({"cmd": "reserve", "players": 999}),
            999,
        ),
        (
            "below one, likewise",
            json!({"cmd": "reserve", "players": -3}),
            -3,
        ),
    ] {
        assert_eq!(
            Command::parse(&message),
            Ok(Command::Reserve { players: want }),
            "{what}"
        );
    }
    assert!(matches!(
        Command::parse(&json!({"cmd": "reserve", "players": "four"})),
        Err(Refused::NotANumber { field: "players" })
    ));
}

#[test]
fn slots_takes_only_what_it_is_told_and_refuses_a_count_that_is_not_a_number() {
    let parsed = Command::parse(&json!({"cmd": "slots", "mode": "fixed"})).expect("slots");
    assert_eq!(
        parsed,
        Command::Slots(padmap_core::slots::Change {
            mode: Some("fixed".into()),
            count: None,
            on_leave: None,
        })
    );
    let every = Command::parse(
        &json!({"cmd": "slots", "mode": "fixed", "count": 6, "on_leave": "destroy"}),
    )
    .expect("slots");
    assert_eq!(
        every,
        Command::Slots(padmap_core::slots::Change {
            mode: Some("fixed".into()),
            count: Some(6),
            on_leave: Some("destroy".into()),
        })
    );
    assert_eq!(
        Command::parse(&json!({"cmd": "slots", "count": "four"})),
        Err(Refused::NotANumber { field: "count" })
    );
}
