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
        COMMANDS[COMMANDS.len() - 6..],
        ["identity", "slots", "scope", "focus", "move", "native"],
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
    let at = |name: &str| COMMANDS.iter().position(|c| *c == name);
    assert!(at("scope") > at("slots"), "appended, never reordered");
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

#[test]
fn a_flag_refuses_a_number_and_an_explicit_null_alike() {
    assert!(matches!(
        Command::parse(&json!({"cmd": "bind", "player": 1, "control": "a", "strict": 1})),
        Err(Refused::NotANumber { field: "strict" })
    ));
    assert!(
        matches!(
            Command::parse(&json!({"cmd": "bind", "player": 1, "control": "a", "strict": null})),
            Err(Refused::NotANumber { field: "strict" })
        ),
        "an explicit null is not leaving it out"
    );
    assert!(matches!(
        Command::parse(&json!({"cmd": "scope", "lease": []})),
        Err(Refused::NotANumber { field: "lease" })
    ));
}

#[test]
fn a_console_or_game_that_is_not_a_string_is_the_default_not_a_refusal() {
    assert_eq!(
        Command::parse(&json!({"cmd": "scope", "console": 5, "game": null})),
        Ok(Command::Scope {
            console: String::new(),
            game: String::new(),
            lease: false,
        })
    );
}

#[test]
fn a_name_at_the_byte_limit_is_fine_and_one_byte_over_is_not() {
    assert!(Command::parse(&json!({"cmd": "map", "player": 1, "scope": "x".repeat(256)})).is_ok());
    assert!(matches!(
        Command::parse(&json!({"cmd": "map", "player": 1, "scope": "x".repeat(257)})),
        Err(Refused::NotAName { field: "scope" })
    ));
    // Bytes, not characters: "é" is two.
    assert!(Command::parse(&json!({"cmd": "scope", "console": "é".repeat(128)})).is_ok());
    assert!(matches!(
        Command::parse(&json!({"cmd": "scope", "console": "é".repeat(129)})),
        Err(Refused::NotAName { field: "console" })
    ));
}

#[test]
fn focus_opens_unless_told_to_close_and_move_names_both_seats() {
    assert_eq!(
        Command::parse(&json!({"cmd": "focus", "player": 2})),
        Ok(Command::Focus {
            player: 2,
            open: true,
            scope: String::new(),
        })
    );
    assert_eq!(
        Command::parse(&json!({"cmd": "focus", "player": 2, "open": false})),
        Ok(Command::Focus {
            player: 2,
            open: false,
            scope: String::new(),
        })
    );
    assert!(matches!(
        Command::parse(&json!({"cmd": "focus", "player": 2, "open": "no"})),
        Err(Refused::NotANumber { field: "open" })
    ));
    assert_eq!(
        Command::parse(&json!({"cmd": "move", "player": 1, "to": 3})),
        Ok(Command::Move { player: 1, to: 3 })
    );
    assert!(matches!(
        Command::parse(&json!({"cmd": "move", "player": 1, "to": "three"})),
        Err(Refused::NotANumber { field: "to" })
    ));
}

#[test]
fn focus_and_move_default_a_missing_player_or_seat_to_zero() {
    assert_eq!(
        Command::parse(&json!({"cmd": "focus"})),
        Ok(Command::Focus {
            player: 0,
            open: true,
            scope: String::new(),
        }),
        "no player named is player 0, refused by the daemon and not the parser"
    );
    assert_eq!(
        Command::parse(&json!({"cmd": "move", "player": 1})),
        Ok(Command::Move { player: 1, to: 0 })
    );
}

#[test]
fn native_opens_unless_told_to_close() {
    for (message, open) in [
        (json!({"cmd": "native"}), true),
        (json!({"cmd": "native", "open": true}), true),
        (json!({"cmd": "native", "open": false}), false),
        (json!({"cmd": "native", "extra": "ignored"}), true),
    ] {
        assert_eq!(
            Command::parse(&message),
            Ok(Command::Native {
                open,
                scope: String::new()
            }),
            "{message}"
        );
    }
    for open in [json!(1), json!("true"), json!(null), json!([]), json!({})] {
        assert!(
            matches!(
                Command::parse(&json!({"cmd": "native", "open": open})),
                Err(Refused::NotANumber { field: "open" })
            ),
            "{open} was taken for a bool"
        );
    }
}

#[test]
fn focus_and_native_may_name_the_level_a_pad_is_heard_through() {
    assert_eq!(
        Command::parse(&json!({"cmd": "native", "scope": "level:ui"})),
        Ok(Command::Native {
            open: true,
            scope: "level:ui".to_owned(),
        })
    );
    assert_eq!(
        Command::parse(&json!({"cmd": "focus", "player": 1, "scope": "level:ui"})),
        Ok(Command::Focus {
            player: 1,
            open: true,
            scope: "level:ui".to_owned(),
        })
    );
    assert!(matches!(
        Command::parse(&json!({"cmd": "native", "scope": "a\nb"})),
        Err(Refused::NotAName { field: "scope" })
    ));
}
