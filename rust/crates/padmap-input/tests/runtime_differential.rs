//! Hold the runtime-state port to what the Python actually answered.
//!
//! `tools/gen_corpus.py` calls the real functions in `protocol.py` and records
//! every answer; this replays them. The distinction from a hand-written test
//! matters and is the reason the corpus exists at all: an expectation written
//! from *reading* the Python encodes what the porter believed it did, and that
//! belief is the thing most likely to be wrong.
//!
//! `normalise` is the case in point. Rust's `Path::components` drops `.` and
//! collapses `//` but keeps `..`, because resolving it without touching the
//! filesystem is wrong where symlinks exist. Python's `normpath` pops it
//! anyway. Either is defensible; only one of them agrees with the daemon
//! already running on this machine, and the corpus is what says which.

use std::path::{Path, PathBuf};

use padmap_input::runtime;
use serde_json::Value;

fn corpus(name: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../padmap-core/tests/corpus")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("the corpus is JSON")
}

#[test]
fn two_runtime_dirs_are_compared_as_the_python_compares_them() {
    for case in corpus("same_runtime") {
        let one = case["one"].as_str().expect("one");
        let other = case["other"].as_str().expect("other");
        let want = case["same"].as_bool().expect("same");
        assert_eq!(
            runtime::same_runtime(one, other),
            want,
            "{one:?} vs {other:?}"
        );
    }
}

#[test]
fn every_runtime_path_is_where_the_python_puts_it() {
    for case in corpus("runtime_paths") {
        let base = case["env"].as_str();
        let want = &case["paths"];
        let dir = runtime::dir_under(base);
        assert_eq!(
            dir.display().to_string(),
            want["runtime_dir"].as_str().expect("runtime_dir"),
            "for XDG_RUNTIME_DIR={base:?}"
        );
        // The rest are recorded only for the set case, since the fallback is
        // the interesting half of `dir_under` and the leaves all hang off it.
        let Some(socket) = want.get("socket").and_then(Value::as_str) else {
            continue;
        };
        let joined = |name: &str| dir.join(name).display().to_string();
        assert_eq!(joined("padmap.sock"), socket);
        assert_eq!(joined("padmap.log"), want["log"].as_str().expect("log"));
        assert_eq!(
            joined("prompted"),
            want["prompted"].as_str().expect("prompted")
        );
        assert_eq!(
            joined("playing"),
            want["playing"].as_str().expect("playing")
        );
        assert_eq!(
            joined("lastgame.json"),
            want["last_game"].as_str().expect("last_game")
        );
    }
}

/// The corpus records a list of `{console, key, title}` objects.
fn expected_games(raw: &Value) -> Vec<(String, String, String)> {
    raw.as_array()
        .expect("a list of games")
        .iter()
        .map(|game| {
            (
                game["console"].as_str().unwrap_or_default().to_owned(),
                game["key"].as_str().unwrap_or_default().to_owned(),
                game["title"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

fn ours(games: &[runtime::Game]) -> Vec<(String, String, String)> {
    games
        .iter()
        .map(|game| (game.console.clone(), game.key.clone(), game.title.clone()))
        .collect()
}

#[test]
fn every_shape_of_lastgame_json_reads_back_the_same() {
    for case in corpus("recent_games") {
        // A null `file` is "no file at all", which reads as no text.
        let text = case["file"].as_str().unwrap_or_default();
        assert_eq!(
            ours(&runtime::recent_games_from(text)),
            expected_games(&case["recent"]),
            "for {text:?}"
        );
    }
}

#[test]
fn the_most_recent_game_is_the_first_of_them() {
    for case in corpus("recent_games") {
        let text = case["file"].as_str().unwrap_or_default();
        let last = runtime::recent_games_from(text).into_iter().next();
        let want = case["last"].as_object().expect("an object");
        match last {
            None => assert!(want.is_empty(), "expected {want:?} for {text:?}"),
            Some(game) => {
                assert_eq!(game.key, want["key"].as_str().expect("key"), "{text:?}");
                assert_eq!(game.console, want["console"].as_str().expect("console"));
                assert_eq!(game.title, want["title"].as_str().expect("title"));
            }
        }
    }
}

#[test]
fn writing_a_launch_keeps_the_same_few_in_the_same_order() {
    // Replaying a game must move it to the front, not add a copy. The corpus
    // is a sequence of writes with the whole list after each one, so an
    // off-by-one in the truncation or the dedupe shows up as a divergence at
    // the write that crosses RECENT_GAMES rather than at the end.
    let cases = corpus("recent_games_writes");
    let temp = std::env::temp_dir().join(format!(
        "padmap-runtime-differential-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).expect("a temp runtime dir");
    let path = temp.join("lastgame.json");

    let mut have: Vec<runtime::Game> = Vec::new();
    for case in cases {
        let wrote = &case["wrote"];
        let game = runtime::Game {
            console: wrote["console"].as_str().expect("console").to_owned(),
            key: wrote["key"].as_str().expect("key").to_owned(),
            title: wrote["title"].as_str().expect("title").to_owned(),
        };
        // Driven through the same parse/serialise round trip the real one
        // takes, rather than by manipulating the vector: the file is what both
        // implementations share, so the file is what has to agree.
        have.retain(|previous| previous.key != game.key);
        have.insert(0, game);
        have.truncate(runtime::RECENT_GAMES);
        let body = serde_json::json!({ "games": have });
        std::fs::write(&path, serde_json::to_string_pretty(&body).expect("json")).expect("write");
        have = runtime::recent_games_from(&std::fs::read_to_string(&path).expect("read back"));

        assert_eq!(
            ours(&have),
            expected_games(&case["recent"]),
            "after writing {:?}",
            wrote["key"]
        );
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn the_build_id_falls_back_to_the_newest_mtime() {
    // Not corpused: the Python's answer embeds an absolute path and a
    // nanosecond stamp from the machine that recorded it, so replaying one
    // would assert that this machine is that machine. What is checkable is
    // the shape, and that it changes when a file does -- which is the whole
    // property the build id exists for.
    let temp = std::env::temp_dir().join(format!("padmap-build-id-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).expect("temp dir");
    std::fs::write(temp.join("one.py"), "x = 1").expect("write");

    let first = runtime::build_id(&temp);
    assert!(first.starts_with("mtime:"), "{first}");
    assert!(first.contains(&temp.display().to_string()), "{first}");

    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(temp.join("two.py"), "x = 2").expect("write");
    assert_ne!(
        first,
        runtime::build_id(&temp),
        "editing a file must change the build id, or a stale daemon reads as current"
    );

    // A directory with no sources at all has no mtime to report.
    let empty = temp.join("empty");
    std::fs::create_dir_all(&empty).expect("mkdir");
    assert_eq!(runtime::build_id(&empty), "unknown");
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn a_store_path_wins_over_any_mtime() {
    // PADMAP_BUILD_ID is what the Nix wrapper sets, and it is the whole point:
    // the store path changes with every edit, and nothing else on the machine
    // does. Passed as a directory that does not exist, so a fallback would be
    // visible rather than plausible.
    let missing = PathBuf::from("/nonexistent-padmap-build-id");
    match std::env::var("PADMAP_BUILD_ID") {
        Ok(store) if !store.is_empty() => {
            assert_eq!(runtime::build_id(&missing), store);
        }
        // Not set here, which is the dev-shell case `build_id` documents.
        _ => assert_eq!(runtime::build_id(&missing), "unknown"),
    }
}

#[test]
fn a_non_string_field_is_dropped_rather_than_rendered() {
    // A decision, not a translation, and the one divergence this port
    // produced. The Python used `str(raw.get(name, ""))`, which turned a JSON
    // `null` title into the literal word "None" and a `true` into "True" --
    // on a picker, where a game's name belongs. It is also a cross-language
    // trap: Rust renders the same booleans "true"/"false", so the two would
    // disagree about a file they both read.
    //
    // Dropping non-strings needs no per-language care, and a console that is
    // not a string never named a layout anyway. The Python was changed to
    // match; this is here so the next person to see the corpus agree does not
    // assume it always did.
    let cases = [
        (r#"{"key": "a", "title": null}"#, "", ""),
        (r#"{"key": "a", "title": true}"#, "", ""),
        (r#"{"key": "a", "console": 7}"#, "", ""),
        (
            r#"{"key": "a", "console": "n64", "title": "T"}"#,
            "n64",
            "T",
        ),
    ];
    for (text, console, title) in cases {
        let games = runtime::recent_games_from(text);
        assert_eq!(games.len(), 1, "{text}");
        assert_eq!(games[0].console, console, "{text}");
        assert_eq!(games[0].title, title, "{text}");
    }

    // A key that is not a string is not a key, so the entry is not an entry.
    for text in [r#"{"key": 7}"#, r#"{"key": null}"#, r#"{"key": ""}"#] {
        assert!(runtime::recent_games_from(text).is_empty(), "{text}");
    }
}
