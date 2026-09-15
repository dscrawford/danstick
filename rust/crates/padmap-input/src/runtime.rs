//! Where padmap keeps state that lasts a login session.
//!
//! Every path here is also computed by the Python, and the two have to agree
//! exactly: for the whole of this port both will be installed, and a Rust
//! daemon that writes its assignments somewhere the Python launcher does not
//! read is a game that starts with no controller order at all.

use std::path::PathBuf;

/// `$XDG_RUNTIME_DIR/padmap`, falling back to `/tmp/padmap`.
pub fn dir() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(base).join("padmap")
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
    let marker = playing_marker();
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
        let previous = std::env::var("XDG_RUNTIME_DIR").ok();
        // SAFETY-adjacent: single-threaded test, restored below.
        std::env::set_var("XDG_RUNTIME_DIR", "/nonexistent-padmap-test");
        assert!(!game_is_running());
        match previous {
            Some(value) => std::env::set_var("XDG_RUNTIME_DIR", value),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }
}
