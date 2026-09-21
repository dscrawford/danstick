//! `unseat` parses with or without a player; player 0 is everybody.

use padmap_core::command::{Command, Refused, COMMANDS};
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
