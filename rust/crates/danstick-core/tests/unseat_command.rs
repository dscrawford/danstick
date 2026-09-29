//! `unseat` parses with or without a player; player 0 is everybody.

use danstick_core::command::{Command, Refused, COMMANDS};
use serde_json::json;

#[test]
fn unseat_is_a_command_and_parses_its_player() {
    assert!(COMMANDS.contains(&"unseat"));
    assert_eq!(
        Command::parse(&json!({"cmd": "unseat"})),
        Ok(Command::Unseat { player: 0 })
    );
    assert_eq!(
        Command::parse(&json!({"cmd": "unseat", "player": 2})),
        Ok(Command::Unseat { player: 2 })
    );
    assert!(matches!(
        Command::parse(&json!({"cmd": "unseat", "player": "two"})),
        Err(Refused::NotANumber { .. })
    ));
}

#[test]
fn seat_keyboard_takes_no_arguments() {
    assert!(COMMANDS.contains(&"seat_keyboard"));
    assert_eq!(
        Command::parse(&json!({"cmd": "seat_keyboard"})),
        Ok(Command::SeatKeyboard)
    );
}

#[test]
fn bind_names_a_control_and_says_whether_it_replaces_or_adds() {
    assert!(COMMANDS.contains(&"bind"));
    assert_eq!(
        Command::parse(&json!({"cmd": "bind", "player": 1, "control": "righttrigger"})),
        Ok(Command::Bind {
            player: 1,
            control: "righttrigger".to_owned(),
            scope: String::new(),
            add: false,
            strict: false,
        })
    );
    assert_eq!(
        Command::parse(&json!({
            "cmd": "bind", "player": 2, "control": "a",
            "scope": "console:gamecube", "add": true
        })),
        Ok(Command::Bind {
            player: 2,
            control: "a".to_owned(),
            scope: "console:gamecube".to_owned(),
            add: true,
            strict: false,
        })
    );
    assert!(matches!(
        Command::parse(&json!({"cmd": "bind", "control": "a", "add": "yes"})),
        Err(Refused::NotANumber { .. })
    ));
}

#[test]
fn map_and_bind_share_an_input_unless_told_to_be_strict() {
    assert!(matches!(
        Command::parse(&json!({"cmd": "map", "player": 1})),
        Ok(Command::Map { strict: false, .. })
    ));
    assert!(matches!(
        Command::parse(&json!({"cmd": "map", "player": 1, "strict": true})),
        Ok(Command::Map { strict: true, .. })
    ));
    assert!(matches!(
        Command::parse(&json!({"cmd": "bind", "player": 1, "control": "a", "strict": true})),
        Ok(Command::Bind { strict: true, .. })
    ));
    assert!(matches!(
        Command::parse(&json!({"cmd": "map", "strict": "no"})),
        Err(Refused::NotANumber { field: "strict" })
    ));
}

#[test]
fn identity_names_the_mode_and_slots_was_appended_after_it() {
    assert_eq!(
        COMMANDS[COMMANDS.len() - 3..],
        ["identity", "slots", "scope"],
        "commands are only appended"
    );
    assert_eq!(
        Command::parse(&json!({"cmd": "identity", "mode": "xbox360"})),
        Ok(Command::Identity {
            mode: "xbox360".to_owned()
        })
    );
    // An unknown mode is the daemon's to refuse, with the modes it has.
    assert_eq!(
        Command::parse(&json!({"cmd": "identity"})),
        Ok(Command::Identity {
            mode: String::new()
        })
    );
}

#[test]
fn scope_names_what_is_being_played_and_empty_is_the_default() {
    assert_eq!(COMMANDS.last(), Some(&"scope"), "appended, never reordered");
    assert_eq!(
        Command::parse(&json!({"cmd": "scope", "console": "n64", "game": "n64/dk64"})),
        Ok(Command::Scope {
            console: "n64".to_owned(),
            game: "n64/dk64".to_owned(),
            lease: false,
        })
    );
    assert_eq!(
        Command::parse(&json!({"cmd": "scope", "lease": true})),
        Ok(Command::Scope {
            console: String::new(),
            game: String::new(),
            lease: true,
        })
    );
}

#[test]
fn a_scope_or_game_name_ends_no_line_of_a_file_it_is_written_into() {
    // A name reaches an autoconfig comment; a newline in it would end that comment.
    assert!(matches!(
        Command::parse(&json!({"cmd": "scope", "game": "n64/x\ninput_a_btn = \"9\""})),
        Err(Refused::NotAName { field: "game" })
    ));
    assert!(matches!(
        Command::parse(&json!({"cmd": "bind", "player": 1, "control": "a", "scope": "a\tb"})),
        Err(Refused::NotAName { field: "scope" })
    ));
    assert!(matches!(
        Command::parse(&json!({"cmd": "map", "player": 1, "scope": "x".repeat(300)})),
        Err(Refused::NotAName { field: "scope" })
    ));
    assert!(
        Command::parse(&json!({"cmd": "scope", "console": "n64", "game": "n64/dk 64"})).is_ok()
    );
}
