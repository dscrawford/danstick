//! Hold the retroarch.cfg cleaner to what the Python writes, byte for byte.

use std::path::Path;

use padmap_core::userconfig;
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/corpus/{name}.json"));
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
    // The corpus is hex because the interesting cases are not valid UTF-8.
    for case in corpus("clean_user_config") {
        let input = unhex(&case["in"]);
        let (changes, out) = userconfig::clean_bytes(&input);
        let want_changes: Vec<&str> = case["changes"]
            .as_array()
            .expect("changes")
            .iter()
            .map(|c| c.as_str().expect("a change"))
            .collect();
        let shown = String::from_utf8_lossy(&input);
        assert_eq!(changes, want_changes, "for {shown:?}");
        assert_eq!(out, unhex(&case["out"]), "output bytes, for {shown:?}");
        if let Ok(text) = std::str::from_utf8(&input) {
            let cleaned = userconfig::clean(text);
            assert_eq!(cleaned.changes, changes, "for {text:?}");
            assert_eq!(cleaned.text.as_bytes(), out, "for {text:?}");
        }
    }
}

#[test]
fn a_config_with_nothing_to_clean_comes_back_identical() {
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
    // A universal-newline read once rewrote a dual-boot config end to end while reporting one change.
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

    let spaced = userconfig::clean("  input_player1_reserved_device  =  \"padmap Player 1\"   \n");
    assert!(spaced
        .text
        .starts_with("  input_player1_reserved_device  =  "));
    assert!(spaced.text.ends_with("   \n"), "{:?}", spaced.text);
}

#[test]
fn an_unquoted_line_stays_unquoted_unless_it_cannot() {
    // Empty is no token at all to strtok_r, so it must be quoted or the setting vanishes.
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

    let cleaned = userconfig::clean("input_player1_reserved_device = padmapPlayer\n");
    assert!(cleaned.changes.is_empty(), "not a virtual pad's name");
    let cleaned = userconfig::clean("input_player1_reserved_device = \"padmap Player 1\"\n");
    assert!(cleaned.text.contains("= \"\""), "{}", cleaned.text);
}

#[test]
fn a_reservation_type_is_judged_with_its_device_name() {
    // Alphabetical order puts the type before the name it depends on.
    let cleaned = userconfig::clean(
        "input_player1_device_reservation_type = \"2\"\n\
         input_player1_reserved_device = \"padmap Player 1\"\n",
    );
    assert_eq!(cleaned.changes.len(), 2, "{:?}", cleaned.changes);
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
    assert!(userconfig::clean("input_player3_joypad_index = \"2\"\n")
        .changes
        .is_empty());
}

#[test]
fn the_libretro_device_key_is_deliberately_left_alone() {
    // It lives only in .rmp remap files, so a rule could only damage one pointed at by --config.
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
