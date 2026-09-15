//! Writing the files other programs read.
//!
//! The decisions are in `padmap_core::emit`; this is where they meet a disk.
//! Both destinations are shared with somebody: the SDL database has the user's
//! own hand-written lines in it, and the autoconfig directory is scanned whole
//! by RetroArch. So both are rewritten rather than appended, and both clear
//! what padmap wrote last time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use log::warn;
use padmap_core::emit::{self, Identity};

/// Where padmap writes the SDL database it generates.
///
/// padmap's own directory, and overridable, because any SDL program can be
/// pointed at it:
///
/// ```text
/// SDL_GAMECONTROLLERCONFIG_FILE=~/.config/padmap/sdl_controllers.txt
/// ```
pub const ENV_SDL_DB: &str = "PADMAP_SDL_DB";

pub fn sdl_database_path() -> PathBuf {
    if let Ok(path) = std::env::var(ENV_SDL_DB) {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    config_dir().join("padmap").join("sdl_controllers.txt")
}

fn config_dir() -> PathBuf {
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => home().join(".config"),
    }
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// The autoconfig directory padmap hands RetroArch for a launch.
///
/// The `udev` subdirectory matters: RetroArch looks in `<dir>/<driver>` first
/// and only falls back to the base directory when that is empty.
pub fn autoconfig_dir() -> PathBuf {
    crate::runtime::dir().join("autoconfig").join("udev")
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error(
        "{0} is there and could not be read, so rewriting it would lose \
             the mappings already in it"
    )]
    Unreadable(PathBuf),
    #[error("writing {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
}

/// Replace padmap's lines in the SDL database, keeping every other.
///
/// Refuses if the file exists and cannot be read. A read that failed cannot
/// tell "the file is empty" from "the file is there and I could not see it",
/// and publishing that difference is how a user's hand-written mapping is lost
/// on the next wizard run -- silently, which is the part that matters.
pub fn write_sdl_database(
    lines: &BTreeMap<u32, String>,
    notes: &BTreeMap<u32, String>,
    identity_for: impl Fn(u32) -> Identity,
    path: Option<&Path>,
) -> Result<PathBuf, WriteError> {
    let target = path
        .map(Path::to_path_buf)
        .unwrap_or_else(sdl_database_path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| WriteError::Io(parent.to_path_buf(), error))?;
    }

    let existing = match std::fs::read(&target) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        // No database yet -- a first run, or a symlink whose target has still
        // to be created. Nothing to preserve, so nothing to lose.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => {
            warn!(
                "cannot read {} -- refusing to rewrite it, because the mappings \
                 already in it would be lost",
                target.display()
            );
            return Err(WriteError::Unreadable(target));
        }
    };

    let body = emit::rewrite_sdl_database(&existing, lines, notes, identity_for);
    std::fs::write(&target, body).map_err(|error| WriteError::Io(target.clone(), error))?;
    Ok(target)
}

/// Write one autoconfig profile per virtual pad, clearing previous ones.
///
/// A profile for a player who no longer exists would still be scanned, and
/// could match a pad it was never meant for.
pub fn write_autoconfig(
    profiles: &BTreeMap<u32, String>,
    dir: Option<&Path>,
) -> Result<Vec<PathBuf>, WriteError> {
    let target = dir.map(Path::to_path_buf).unwrap_or_else(autoconfig_dir);
    std::fs::create_dir_all(&target).map_err(|error| WriteError::Io(target.clone(), error))?;

    if let Ok(entries) = std::fs::read_dir(&target) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(emit::VIRTUAL_PREFIX) && name.ends_with(".cfg") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    let mut written = Vec::new();
    for (player, text) in profiles {
        let path = target.join(format!("{}.cfg", emit::virtual_name(*player)));
        std::fs::write(&path, text).map_err(|error| WriteError::Io(path.clone(), error))?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PADMAP: Identity = Identity {
        bustype: 0x06,
        vendor: 0x1209,
        product: 0x0001,
        version: 0x0001,
    };

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("padmap-artefact-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn one_line() -> BTreeMap<u32, String> {
        [(1, "aaa,padmap Player 1,a:b0,".to_owned())]
            .into_iter()
            .collect()
    }

    #[test]
    fn a_first_run_with_no_database_still_writes_one() {
        let dir = scratch("first");
        let path = dir.join("nested").join("sdl_controllers.txt");
        let written = write_sdl_database(&one_line(), &BTreeMap::new(), |_| PADMAP, Some(&path))
            .expect("a missing file is nothing to lose");
        assert!(std::fs::read_to_string(&written)
            .expect("read")
            .contains("Player 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hand_written_line_survives_a_rewrite() {
        let dir = scratch("keep");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("sdl_controllers.txt");
        std::fs::write(&path, "030000005e040000e002000000000000,Their Pad,a:b0,\n").expect("seed");
        write_sdl_database(&one_line(), &BTreeMap::new(), |_| PADMAP, Some(&path)).expect("write");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(
            text.contains("Their Pad"),
            "the reason this rewrites at all"
        );
        assert!(text.contains("padmap Player 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_database_is_refused_rather_than_overwritten() {
        // "Could not read" became "was empty" once, and a user lost their
        // hand-written mappings on the next wizard run.
        if rustix::process::geteuid().is_root() {
            return; // root can read anything; the check cannot be staged
        }
        let dir = scratch("unreadable");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("sdl_controllers.txt");
        std::fs::write(&path, "030000005e040000e002000000000000,Their Pad,a:b0,\n").expect("seed");
        let mut perms = std::fs::metadata(&path).expect("stat").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o222);
        std::fs::set_permissions(&path, perms).expect("chmod");

        let result = write_sdl_database(&one_line(), &BTreeMap::new(), |_| PADMAP, Some(&path));
        assert!(
            matches!(result, Err(WriteError::Unreadable(_))),
            "{result:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_profile_for_a_player_who_no_longer_exists_is_cleared() {
        // It would still be scanned, and could match a pad it was never for.
        let dir = scratch("autoconfig");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("padmap Player 4.cfg"), "stale").expect("seed");
        std::fs::write(dir.join("Someone Else.cfg"), "theirs").expect("seed");

        let profiles: BTreeMap<u32, String> = [(1, "fresh\n".to_owned())].into_iter().collect();
        let written = write_autoconfig(&profiles, Some(&dir)).expect("write");

        assert_eq!(written.len(), 1);
        assert!(
            !dir.join("padmap Player 4.cfg").exists(),
            "a stale slot survived"
        );
        assert!(
            dir.join("Someone Else.cfg").exists(),
            "someone else's file was eaten"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("padmap Player 1.cfg")).expect("read"),
            "fresh\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_database_path_is_overridable_for_anything_that_wants_its_own() {
        let previous = std::env::var(ENV_SDL_DB).ok();
        std::env::set_var(ENV_SDL_DB, "/tmp/somewhere-else.txt");
        assert_eq!(
            sdl_database_path(),
            PathBuf::from("/tmp/somewhere-else.txt")
        );
        match previous {
            Some(value) => std::env::set_var(ENV_SDL_DB, value),
            None => std::env::remove_var(ENV_SDL_DB),
        }
    }

    #[test]
    fn the_autoconfig_directory_is_the_one_retroarch_scans_first() {
        // RetroArch looks in <dir>/<driver> before the base directory.
        assert!(autoconfig_dir().ends_with("autoconfig/udev"));
    }
}
