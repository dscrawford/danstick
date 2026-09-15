//! Where padmap keeps state that lasts a login session.
//!
//! Every path here is also computed by the Python, and the two have to agree
//! exactly: for the whole of this port both will be installed, and a Rust
//! daemon that writes its assignments somewhere the Python launcher does not
//! read is a game that starts with no controller order at all.

use std::path::{Path, PathBuf};

/// `$XDG_RUNTIME_DIR/padmap`, falling back to `/tmp/padmap`.
pub fn dir() -> PathBuf {
    dir_under(std::env::var("XDG_RUNTIME_DIR").ok().as_deref())
}

/// The same, for a base a caller already has.
///
/// Exists so tests never touch the environment. `std::env::set_var` is
/// process-wide and the test harness runs tests on threads, so a test that
/// set XDG_RUNTIME_DIR and restored it was changing the answer underneath
/// whichever other test happened to call `dir()` in that window. It failed
/// about one run in three, on whichever test lost the race rather than on the
/// one at fault -- and it failed here by letting a commit through on a red
/// suite. The old comment said "single-threaded test", which was simply not
/// true.
pub fn dir_under(base: Option<&str>) -> PathBuf {
    PathBuf::from(base.unwrap_or("/tmp")).join("padmap")
}

pub fn assignments_path() -> PathBuf {
    dir().join("assignments.json")
}

/// Where the daemon listens.
///
/// Under `XDG_RUNTIME_DIR` so it is per-user, mode 0700 by the spec, and
/// cleaned up on logout without us having to manage stale socket files.
pub fn socket_path() -> PathBuf {
    dir().join("padmap.sock")
}

/// The file `padmap-play` holds while a game is running, containing its pid.
///
/// Holds a pid rather than existing or not, so a launcher killed outright
/// cannot disable the feature forever.
pub fn playing_marker() -> PathBuf {
    dir().join("playing")
}

/// Whether a game is in progress.
///
/// The daemon needs this before it may grab the pads for anything: opening an
/// assignment session stops republishing and takes EVIOCGRAB on every physical
/// pad, so doing it during a game leaves the player holding a controller that
/// has silently stopped working.
pub fn game_is_running() -> bool {
    game_is_running_at(&playing_marker())
}

/// The same, for a marker path a caller already has.
pub fn game_is_running_at(marker: &std::path::Path) -> bool {
    let marker = marker.to_path_buf();
    let Ok(text) = std::fs::read_to_string(&marker) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return false;
    };
    if PathBuf::from(format!("/proc/{pid}")).exists() {
        return true;
    }
    // Stale: the launcher died without running its trap.
    let _ = std::fs::remove_file(&marker);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_path_hangs_off_one_directory() {
        // If these drift apart the Rust daemon and the Python launcher stop
        // seeing each other's state, and a game starts with no controller
        // order at all.
        let base = dir();
        for path in [assignments_path(), socket_path(), playing_marker()] {
            assert_eq!(path.parent(), Some(base.as_path()), "{path:?}");
        }
    }

    #[test]
    fn the_filenames_are_the_ones_the_python_writes() {
        assert_eq!(
            assignments_path().file_name().and_then(|n| n.to_str()),
            Some("assignments.json")
        );
        assert_eq!(
            socket_path().file_name().and_then(|n| n.to_str()),
            Some("padmap.sock")
        );
        assert_eq!(
            playing_marker().file_name().and_then(|n| n.to_str()),
            Some("playing")
        );
    }

    #[test]
    fn a_missing_marker_is_not_a_running_game() {
        // Never raises: this gates whether the daemon may take the controllers
        // away, and an unreadable file has to mean "no" rather than an error.
        let missing = dir_under(Some("/nonexistent-padmap-test")).join("playing");
        assert!(!game_is_running_at(&missing));
    }

    #[test]
    fn the_base_comes_from_the_environment_but_defaults_to_tmp() {
        assert_eq!(
            dir_under(Some("/run/user/1000")),
            PathBuf::from("/run/user/1000/padmap")
        );
        assert_eq!(dir_under(None), PathBuf::from("/tmp/padmap"));
    }
}

/// Where a daemon started by `ensure-daemon` writes its log.
///
/// Beside the socket, so it shares the socket's lifetime and a fresh login
/// starts a fresh log. The daemon is the only process that watches a
/// controller being claimed or a mapping captured; without this its output
/// went to /dev/null and none of it was recoverable after the fact.
pub fn daemon_log_path() -> PathBuf {
    dir().join("padmap.log")
}

/// Controller models the daemon has already offered a setup screen for.
///
/// Distinct from the profile store: a profile means "this has been
/// configured", this means "we already asked, do not ask again". Declining
/// has to be rememberable, or the offer reappears a second later.
pub fn prompted_path() -> PathBuf {
    dir().join("prompted")
}

/// The game most recently launched through `padmap-play`.
pub fn last_game_path() -> PathBuf {
    dir().join("lastgame.json")
}

/// Do two `XDG_RUNTIME_DIR` values name the same directory?
///
/// Compared as raw strings, `/run/user/1000` and `/run/user/1000/` are two
/// different daemons -- and they are not, they bind the same socket, because
/// every path here is built by joining `padmap` on and a trailing separator
/// disappears the moment it is. A daemon started from a shell where the
/// variable carried a slash was invisible to every caller: `ensure-daemon`
/// saw none and started a second on the socket the first was already
/// listening on.
///
/// Normalised lexically, never resolved: this compares a value read out of
/// another process's environ, and resolving it would touch the filesystem --
/// and hang on one that is not answering -- for a string comparison.
pub fn same_runtime(one: &str, other: &str) -> bool {
    normalise(one) == normalise(other)
}

/// `os.path.normpath`, which is not what `Path::components` does.
///
/// Rust's iterator drops `.` and collapses `//` but keeps `..` as a
/// component, because resolving it without touching the filesystem is wrong
/// in the presence of symlinks. Python's normpath pops it anyway, and this
/// has to agree with Python for as long as both are installed -- so it is
/// spelled out rather than borrowed.
fn normalise(path: &str) -> String {
    if path.is_empty() {
        // normpath("") is ".", not "".
        return ".".to_owned();
    }
    let absolute = path.starts_with('/');
    // normpath preserves exactly two leading slashes, and POSIX says an
    // implementation may treat that path specially. Python does; so does this.
    let double = path.starts_with("//") && !path.starts_with("///");

    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                match parts.last() {
                    Some(&last) if last != ".." => {
                        parts.pop();
                    }
                    // A leading `..` on an absolute path has nothing above it
                    // to pop, and normpath discards it.
                    _ if absolute => {}
                    _ => parts.push(".."),
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("{}{joined}", if double { "//" } else { "/" })
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

/// How many recently played games the scope picker may offer.
///
/// Small on purpose. The strip is worked from the pad one step at a time, and
/// every entry sits after the console entries -- a long tail turns "map this
/// for N64" into a scrolling exercise. Five covers an evening's play, which is
/// the span in which someone notices a control was wrong.
pub const RECENT_GAMES: usize = 5;

/// One launched game, as the picker needs it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Game {
    #[serde(default)]
    pub console: String,
    pub key: String,
    #[serde(default)]
    pub title: String,
}

/// Recently launched games, newest first.
///
/// Never fails: this decorates a picker, and a missing or malformed file means
/// fewer options rather than an error.
pub fn read_recent_games() -> Vec<Game> {
    recent_games_from(&std::fs::read_to_string(last_game_path()).unwrap_or_default())
}

/// The same, for text a caller already has -- and the whole of the parsing.
///
/// Reads the pre-list format too: a bare `{"console","key","title"}` object,
/// which is what earlier versions wrote. A user upgrading mid-session would
/// otherwise lose the per-game scope for the game they are playing right now,
/// which is exactly when they want it.
pub fn recent_games_from(text: &str) -> Vec<Game> {
    let Ok(raw) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    if let Some(games) = raw.get("games").and_then(|value| value.as_array()) {
        return games
            .iter()
            .filter_map(game_entry)
            .take(RECENT_GAMES)
            .collect();
    }
    game_entry(&raw).into_iter().collect()
}

/// One entry, or nothing if it cannot be one.
///
/// A key is the whole of the identity -- an entry without one names no game --
/// so it is required where console and title are not. Non-string values are
/// dropped rather than stringified: the Python reaches them through
/// `raw.get(...)` with a `""` default, and `str(None)` would be the word
/// "None" in a picker.
fn game_entry(raw: &serde_json::Value) -> Option<Game> {
    let object = raw.as_object()?;
    let key = object.get("key")?.as_str()?;
    if key.is_empty() {
        return None;
    }
    let text = |name: &str| {
        object
            .get(name)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    Some(Game {
        console: text("console"),
        key: key.to_owned(),
        title: text("title"),
    })
}

/// Record a launch, keeping the previous few.
///
/// Newest first, deduplicated by key: replaying a game moves it to the front
/// rather than filling the list with copies of itself and pushing out
/// everything else someone might want to correct.
pub fn write_last_game(game: &Game) -> std::io::Result<PathBuf> {
    let mut kept: Vec<Game> = read_recent_games()
        .into_iter()
        .filter(|previous| previous.key != game.key)
        .collect();
    kept.insert(0, game.clone());
    kept.truncate(RECENT_GAMES);
    let path = last_game_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::json!({ "games": kept });
    // Two-space indent, as `json.dumps(..., indent=2)` writes it: the file is
    // read by both implementations for as long as both are installed.
    std::fs::write(&path, serde_json::to_string_pretty(&body)? + "\n")?;
    Ok(path)
}

/// Identity of the code this process is running.
///
/// A daemon keeps the modules it started with, so changing a source file and
/// rebuilding does nothing until it restarts -- and a stale daemon is
/// indistinguishable from a fresh one by anything on disk, since it goes on
/// writing plausible-looking files. This is what lets a client notice.
///
/// Under Nix the source is a store path that changes with every edit, which is
/// exactly the property wanted. Outside it, the newest mtime in the directory:
/// coarser, but it still changes when you edit.
pub fn build_id(source: &Path) -> String {
    if let Ok(store) = std::env::var("PADMAP_BUILD_ID") {
        if !store.is_empty() {
            return store;
        }
    }
    let newest = std::fs::read_dir(source)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "py"))
        .filter_map(|entry| entry.metadata().ok())
        .filter_map(|meta| meta.modified().ok())
        .filter_map(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_nanos())
        .max();
    match newest {
        Some(nanos) => format!("mtime:{}:{nanos}", source.display()),
        None => "unknown".to_owned(),
    }
}
