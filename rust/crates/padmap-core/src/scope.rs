//! Scope: a mapping is per-game or per-console or universal.

use std::fmt;

pub const UNIVERSAL: &str = "";
const CONSOLE_PREFIX: &str = "console:";
const GAME_PREFIX: &str = "game:";

/// Scope covering every game on one console (console = layout id).
pub fn console(layout_id: &str) -> String {
    format!("{CONSOLE_PREFIX}{layout_id}")
}

pub fn game(key: &str) -> String {
    format!("{GAME_PREFIX}{key}")
}

/// The layout id a console scope names, or `""` for any other scope.
pub fn console_of(scope: &str) -> &str {
    scope.strip_prefix(CONSOLE_PREFIX).unwrap_or("")
}

/// The game key a game scope names, or `""` for any other scope.
pub fn game_of(scope: &str) -> &str {
    scope.strip_prefix(GAME_PREFIX).unwrap_or("")
}

/// Stable game identity: filename stem (not path, not hash), normalised under the console.
pub fn game_key(console_id: &str, rom: &str) -> String {
    let name = rom.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let stem = match name.rfind('.') {
        Some(dot) => &name[..dot],
        None => name,
    };

    let mut slug = String::with_capacity(stem.len());
    let mut pending_dash = false;
    for ch in stem.to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(ch);
        } else {
            pending_dash = true;
        }
    }
    if slug.is_empty() {
        return String::new();
    }
    let owner = if console_id.is_empty() {
        "unknown"
    } else {
        console_id
    };
    format!("{owner}/{slug}")
}

/// Scopes to try, most specific first (game > console > universal).
pub fn order(console_id: &str, game_id: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(3);
    if !game_id.is_empty() {
        out.push(game(game_id));
    }
    if !console_id.is_empty() {
        out.push(console(console_id));
    }
    out.push(UNIVERSAL.to_owned());
    out
}

/// A scope, parsed (only for display; storage uses flat strings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope<'a> {
    Universal,
    Console(&'a str),
    Game(&'a str),
}

impl<'a> Scope<'a> {
    pub fn parse(scope: &'a str) -> Self {
        if let Some(key) = scope.strip_prefix(GAME_PREFIX) {
            Scope::Game(key)
        } else if let Some(id) = scope.strip_prefix(CONSOLE_PREFIX) {
            Scope::Console(id)
        } else {
            Scope::Universal
        }
    }
}

impl fmt::Display for Scope<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::Universal => f.write_str(UNIVERSAL),
            Scope::Console(id) => write!(f, "{CONSOLE_PREFIX}{id}"),
            Scope::Game(key) => write!(f, "{GAME_PREFIX}{key}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_kinds_round_trip_through_their_strings() {
        assert_eq!(console_of(&console("n64")), "n64");
        assert_eq!(game_of(&game("n64/goldeneye")), "n64/goldeneye");
        assert_eq!(console_of(UNIVERSAL), "");
        assert_eq!(game_of(UNIVERSAL), "");
    }

    #[test]
    fn a_console_scope_is_not_read_as_a_game_scope_or_the_reverse() {
        assert_eq!(game_of(&console("n64")), "");
        assert_eq!(console_of(&game("n64/x")), "");
    }

    #[test]
    fn precedence_is_game_then_console_then_everything_else() {
        assert_eq!(
            order("n64", "n64/goldeneye-007"),
            ["game:n64/goldeneye-007", "console:n64", ""]
        );
    }

    #[test]
    fn a_missing_level_is_skipped_not_filled_with_an_empty_key() {
        assert_eq!(order("n64", ""), ["console:n64", ""]);
        assert_eq!(order("", "n64/x"), ["game:n64/x", ""]);
        assert_eq!(order("", ""), [""]);
    }

    #[test]
    fn the_universal_scope_is_always_last_and_always_present() {
        for (console_id, game_id) in [("", ""), ("n64", ""), ("", "a/b"), ("n64", "a/b")] {
            let scopes = order(console_id, game_id);
            assert_eq!(scopes.last().map(String::as_str), Some(UNIVERSAL));
            assert_eq!(scopes.iter().filter(|s| s.is_empty()).count(), 1);
        }
    }

    #[test]
    fn a_game_key_is_the_stem_slugged_under_its_console() {
        assert_eq!(
            game_key("n64", "/roms/n64/Super Smash Bros. (U) [!].z64"),
            "n64/super-smash-bros-u"
        );
    }

    #[test]
    fn only_the_last_suffix_is_stripped() {
        // "Legend of Zelda, The (v1.2).z64" must not lose everything after the first dot.
        assert_eq!(
            game_key("n64", "Legend of Zelda, The (v1.2).z64"),
            "n64/legend-of-zelda-the-v1-2"
        );
    }

    #[test]
    fn a_directory_shaped_rom_keys_off_its_directory_name() {
        assert_eq!(
            game_key("ps2", "/roms/ps2/Final Fantasy X/"),
            "ps2/final-fantasy-x"
        );
        assert_eq!(
            game_key("ps2", "/roms/ps2/Final Fantasy X"),
            "ps2/final-fantasy-x"
        );
        assert_eq!(game_key("n64", "/"), "");
        assert_eq!(game_key("n64", "///"), "");
    }

    #[test]
    fn a_name_with_no_dot_keeps_all_of_itself() {
        assert_eq!(game_key("arcade", "/roms/mame/10yard"), "arcade/10yard");
    }

    #[test]
    fn the_key_ignores_the_directory_the_rom_sits_in() {
        let a = game_key("n64", "/mnt/old/roms/Mario 64.z64");
        let b = game_key("n64", "/home/x/games/n64/Mario 64.z64");
        assert_eq!(a, b);
        assert_eq!(a, "n64/mario-64");
    }

    #[test]
    fn runs_of_punctuation_collapse_to_one_dash_and_the_edges_are_trimmed() {
        assert_eq!(
            game_key("c", "  ...Hello___World!!!  .rom"),
            "c/hello-world"
        );
        assert_eq!(game_key("c", "--x--.rom"), "c/x");
    }

    #[test]
    fn a_console_that_was_not_identified_still_gets_a_usable_key() {
        assert_eq!(game_key("", "Sonic.bin"), "unknown/sonic");
        assert_ne!(game_key("arcade", "sonic"), game_key("genesis", "sonic"));
    }

    #[test]
    fn a_stem_that_normalises_to_nothing_gets_no_key_rather_than_a_bare_console() {
        assert_eq!(game_key("n64", "...z64"), "");
        assert_eq!(game_key("n64", "!!!.rom"), "");
        assert_eq!(game_key("n64", ""), "");
        assert_eq!(game_key("n64", "日本語.rom"), "");
    }

    #[test]
    fn the_key_is_case_insensitive() {
        assert_eq!(game_key("n64", "MARIO.z64"), game_key("n64", "mario.z64"));
    }

    #[test]
    fn parsing_a_scope_agrees_with_the_prefix_helpers() {
        assert_eq!(Scope::parse(""), Scope::Universal);
        assert_eq!(Scope::parse("console:snes"), Scope::Console("snes"));
        assert_eq!(Scope::parse("game:snes/mario"), Scope::Game("snes/mario"));
        assert_eq!(Scope::parse("nonsense"), Scope::Universal);
    }

    #[test]
    fn a_parsed_scope_prints_back_to_what_it_was_parsed_from() {
        for raw in ["", "console:n64", "game:n64/mario-64"] {
            assert_eq!(Scope::parse(raw).to_string(), raw);
        }
    }
}
