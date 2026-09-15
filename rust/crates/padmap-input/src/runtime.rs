//! Where padmap keeps state that lasts a login session.
//!
//! Every path here is also computed by the Python, and the two have to agree
//! exactly: for the whole of this port both will be installed, and a Rust
//! daemon that writes its assignments somewhere the Python launcher does not
//! read is a game that starts with no controller order at all.

use std::path::PathBuf;

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
