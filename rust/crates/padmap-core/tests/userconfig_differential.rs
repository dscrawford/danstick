//! Hold the config cleaner to what the Python writes, byte for byte.
//!
//! This rewrites a file RetroArch owns and the user did not ask padmap to
//! touch beyond one command. The contract is that the reported lines are the
//! only lines that differ, so the assertion is on the whole file rather than
//! on the changes: a cleaner that got the right answer and reflowed everything
//! around it would pass a test that only checked the changes, and would still
//! have rewritten three thousand lines the user can see.
//!
//! The corpus is hex because the interesting cases are not valid UTF-8 -- a
//! latin-1 ROM path in `system_directory` is exactly the thing that must
//! survive untouched, and it is why the Python reads bytes rather than text.

use std::path::Path;

use padmap_core::userconfig;
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

fn unhex(value: &Value) -> Vec<u8> {
    let text = value.as_str().expect("hex");
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("a hex byte"))
        .collect()
}

#[test]
fn every_config_cleans_to_the_same_bytes() {
    for case in corpus("clean_user_config") {
        let input = unhex(&case["in"]);
        let want_out = unhex(&case["out"]);
        // On bytes, not text: the case that matters most is not UTF-8, and
        // skipping it would leave the whole point untested.
        let (changes, out) = userconfig::clean_bytes(&input);
        let want_changes: Vec<&str> = case["changes"]
            .as_array()
            .expect("changes")
            .iter()
            .map(|change| change.as_str().expect("a change"))
            .collect();
        let shown = String::from_utf8_lossy(&input);
        assert_eq!(changes, want_changes, "for {shown:?}");
        assert_eq!(out, want_out, "output bytes, for {shown:?}");

        // And where the input is text, both entry points must agree.
        if let Ok(text) = std::str::from_utf8(&input) {
            let cleaned = userconfig::clean(text);
            assert_eq!(cleaned.changes, changes, "for {text:?}");
            assert_eq!(cleaned.text.as_bytes(), out, "for {text:?}");
        }
    }
}

#[test]
fn a_config_with_nothing_to_clean_comes_back_identical() {
    // The property that matters most, asserted directly rather than inferred
    // from the corpus: if there is nothing to change, not one byte moves.
    for text in [
        "",
        "\n",
        "# just a comment\n",
        "video_fullscreen = \"true\"\n",
        "input_player1_reserved_device = \"Real Controller\"\n",
        "not a setting line at all\n",
        "=leading equals\n",
        "  spaced   =   \"value\"   \n",
        "no_trailing_newline = \"x\"",
        "crlf = \"x\"\r\nsecond = \"y\"\r\n",
        "\n\n\n",
    ] {
        let cleaned = userconfig::clean(text);
        assert!(cleaned.changes.is_empty(), "changed {text:?}");
        assert_eq!(cleaned.text, text, "rewrote {text:?}");
    }
}

#[test]
fn line_endings_and_spacing_survive_a_change() {
    // The failure this guards: read through a universal-newline translation,
    // every \r\n became \n before the cleaner saw the file, so a config off a
    // dual-boot install was rewritten end to end by a command that reported
    // changing one line -- and the backup, written from the same translated
    // text, could not put the endings back.
    let cleaned = userconfig::clean(
        "keep_me = \"yes\"\r\ninput_player1_reserved_device = \"padmap Player 1\"\r\nalso = \"kept\"\r\n",
    );
    assert_eq!(cleaned.changes.len(), 1, "{:?}", cleaned.changes);
    assert!(cleaned.text.starts_with("keep_me = \"yes\"\r\n"));
    assert!(cleaned.text.ends_with("also = \"kept\"\r\n"));
    assert!(
        cleaned
            .text
            .contains("input_player1_reserved_device = \"\""),
        "{}",
        cleaned.text
    );

    // Indentation and trailing space on the changed line are kept too.
    let spaced = userconfig::clean("  input_player1_reserved_device  =  \"padmap Player 1\"   \n");
    assert!(spaced
        .text
        .starts_with("  input_player1_reserved_device  =  "));
    assert!(spaced.text.ends_with("   \n"), "{:?}", spaced.text);
}

#[test]
fn an_unquoted_line_stays_unquoted_unless_it_cannot() {
    // Adding quotes would edit a line the user can see beyond the reported
    // change. The exception is a value that stops meaning the same thing:
    // empty is no token at all to strtok_r and the setting would vanish.
    assert_eq!(userconfig::render_value("2", false), "2");
    assert_eq!(userconfig::render_value("2", true), "\"2\"");
    assert_eq!(userconfig::render_value("", false), "\"\"");
    assert_eq!(
        userconfig::render_value("has space", false),
        "\"has space\""
    );
    assert_eq!(
        userconfig::render_value("has\"quote", false),
        "\"has\"quote\""
    );

    // A released reservation is the reachable empty case, and it must come
    // back quoted even though the line it replaces was not.
    let cleaned = userconfig::clean("input_player1_reserved_device = padmapPlayer\n");
    assert!(cleaned.changes.is_empty(), "not a virtual pad's name");
    let cleaned = userconfig::clean("input_player1_reserved_device = \"padmap Player 1\"\n");
    assert!(cleaned.text.contains("= \"\""), "{}", cleaned.text);
}

#[test]
fn a_reservation_type_is_judged_with_its_device_name() {
    // Alphabetical order puts the type first, so the names are collected up
    // front. A type left at RESERVED whose device name is empty holds the
    // slot for nothing at all.
    let cleaned = userconfig::clean(
        "input_player1_device_reservation_type = \"2\"\n\
         input_player1_reserved_device = \"padmap Player 1\"\n",
    );
    assert_eq!(cleaned.changes.len(), 2, "{:?}", cleaned.changes);

    // A real controller's reservation is the user's, and is left alone.
    let kept = userconfig::clean(
        "input_player1_device_reservation_type = \"2\"\n\
         input_player1_reserved_device = \"Real Controller\"\n",
    );
    assert!(kept.changes.is_empty(), "{:?}", kept.changes);
}

#[test]
fn a_joypad_index_resets_to_what_retroarch_would_default_to() {
    let cleaned = userconfig::clean("input_player3_joypad_index = \"7\"\n");
    assert_eq!(cleaned.changes.len(), 1);
    assert!(cleaned.text.contains("\"2\""), "{}", cleaned.text);
    // Already at the default: nothing to report, nothing to write.
    let already = userconfig::clean("input_player3_joypad_index = \"2\"\n");
    assert!(already.changes.is_empty());
}

#[test]
fn the_libretro_device_key_is_deliberately_left_alone() {
    // RetroArch neither loads nor saves it in retroarch.cfg -- it lives only
    // in .rmp remap files -- so it cannot have leaked here, and a rule for it
    // could only damage a remap file someone pointed --config at.
    let cleaned = userconfig::clean("input_libretro_device_p1 = \"1\"\n");
    assert!(cleaned.changes.is_empty());
    assert_eq!(cleaned.text, "input_libretro_device_p1 = \"1\"\n");
}

#[test]
fn every_autoconfig_profile_parses_the_same() {
    for case in corpus("parse_profile_text") {
        let text = case["text"].as_str().expect("text");
        let ours = userconfig::parse_profile_text(text);
        let want = case["settings"].as_object().expect("settings");
        assert_eq!(ours.len(), want.len(), "for {text:?}: {ours:?} vs {want:?}");
        for (key, value) in want {
            assert_eq!(
                ours.get(key).map(String::as_str),
                value.as_str(),
                "{key} in {text:?}"
            );
        }
    }
}

#[test]
fn a_latin_one_path_survives_beside_a_line_that_is_rewritten() {
    // The reason the Python reads bytes and decodes with surrogateescape. A
    // ROM directory named in latin-1 has to come back exactly as it went in,
    // on a pass that does change the line below it.
    let mut input = b"system_directory = \"/roms/caf\xe9\"\n".to_vec();
    input.extend_from_slice(b"input_player1_reserved_device = \"padmap Player 1\"\n");
    let (changes, out) = userconfig::clean_bytes(&input);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert!(
        out.starts_with(b"system_directory = \"/roms/caf\xe9\"\n"),
        "the latin-1 path was rewritten: {:?}",
        String::from_utf8_lossy(&out)
    );
    assert!(out.ends_with(b"input_player1_reserved_device = \"\"\n"));
}
