//! Reading and writing the profile store.
//!
//! The decisions are all in `padmap_core::profile`; this is the two file
//! operations and the directory they happen in. Same paths and same filenames
//! as the Python, because the store is the user's data and both
//! implementations will be installed for the whole of this port.

use std::path::{Path, PathBuf};

use log::warn;
use padmap_core::profile::{self, Profile};

use crate::pad::Pad;

pub const ENV_DIR: &str = "PADMAP_PROFILE_DIR";

/// Where profiles live.
pub fn dir() -> PathBuf {
    if let Ok(override_path) = std::env::var(ENV_DIR) {
        if !override_path.is_empty() {
            return PathBuf::from(override_path);
        }
    }
    let base = match std::env::var("XDG_DATA_HOME") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => home().join(".local").join("share"),
    };
    base.join("padmap").join("devices")
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

pub fn signature_of(pad: &Pad) -> String {
    profile::signature(pad.vid, pad.pid, &pad.name)
}

pub fn path_for(pad: &Pad, directory: Option<&Path>) -> PathBuf {
    let base = directory.map(Path::to_path_buf).unwrap_or_else(dir);
    base.join(profile::filename(&signature_of(pad)))
}

/// The stored profile for a pad, or `None` if it has never been configured.
///
/// Never fails. A damaged file is "no profile", not an error: this is called
/// for every pad during discovery, and one corrupt file must cost that
/// controller its settings rather than its existence.
pub fn load(pad: &Pad, directory: Option<&Path>) -> Option<Profile> {
    let path = path_for(pad, directory);
    let text = std::fs::read_to_string(&path).ok()?;
    let value = serde_json::from_str(&text).ok()?;
    let (profile, rejected) = Profile::from_value(&value);
    for axis in rejected {
        // Named, so it can be re-calibrated. The axis is forwarded
        // uncorrected, which is what an uncalibrated pad already does.
        warn!(
            "axis {} of {} has a calibration no evdev value can carry ({}); \
             ignoring it, so the axis is forwarded uncorrected -- re-run \
             calibration to restore it",
            axis.code,
            path.display(),
            axis.why
        );
    }
    Some(profile)
}

pub fn save(profile: &Profile, directory: Option<&Path>) -> std::io::Result<PathBuf> {
    let base = directory.map(Path::to_path_buf).unwrap_or_else(dir);
    std::fs::create_dir_all(&base)?;
    let path = base.join(profile::filename(&profile.signature));
    let text = serde_json::to_string_pretty(&profile.to_value())?;
    std::fs::write(&path, text + "\n")?;
    Ok(path)
}

/// Throw away everything stored for a pad. True if there was anything.
pub fn forget(pad: &Pad, directory: Option<&Path>) -> bool {
    std::fs::remove_file(path_for(pad, directory)).is_ok()
}

/// Whether this controller has been configured before.
pub fn is_known(pad: &Pad, directory: Option<&Path>) -> bool {
    load(pad, directory).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use padmap_core::calibration::AxisCalibration;

    fn pad(name: &str) -> Pad {
        Pad {
            path: PathBuf::from("/dev/input/event0"),
            name: name.to_owned(),
            phys: String::new(),
            uniq: String::new(),
            vid: 0x0079,
            pid: 0x1879,
            syspath: PathBuf::from("/sys"),
            retroarch_visible: true,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("padmap-profile-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_pad_maps_to_the_filename_the_python_would_have_used() {
        let path = path_for(&pad("N64 Adapter"), Some(Path::new("/tmp/x")));
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("0079_1879_N64_Adapter.json")
        );
    }

    #[test]
    fn an_unconfigured_pad_has_no_profile_rather_than_an_error() {
        let dir = scratch("absent");
        assert!(load(&pad("Nothing"), Some(&dir)).is_none());
        assert!(!is_known(&pad("Nothing"), Some(&dir)));
    }

    #[test]
    fn a_saved_profile_reads_back_as_itself() {
        let dir = scratch("roundtrip");
        let mut profile = Profile {
            signature: signature_of(&pad("N64 Adapter")),
            name: "N64 Adapter".to_owned(),
            icon: "n64".to_owned(),
            ..Profile::default()
        };
        profile
            .axes
            .insert(0, AxisCalibration::new(174, 0, 255).with_reach(20, 250));
        save(&profile, Some(&dir)).expect("save");

        let back = load(&pad("N64 Adapter"), Some(&dir)).expect("a stored profile");
        assert_eq!(back, profile);
        assert!(is_known(&pad("N64 Adapter"), Some(&dir)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_costs_the_settings_and_not_the_controller() {
        // load() is what is_known() asks, and discovery asks it for every pad.
        let dir = scratch("damaged");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = path_for(&pad("N64 Adapter"), Some(&dir));
        std::fs::write(&path, "{ this is not json").expect("write");
        assert!(load(&pad("N64 Adapter"), Some(&dir)).is_none());

        // Valid JSON that is not a profile still loads, empty.
        std::fs::write(&path, "[1, 2, 3]").expect("write");
        let profile = load(&pad("N64 Adapter"), Some(&dir)).expect("an empty profile");
        assert!(!profile.has_bindings());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forgetting_a_pad_that_was_never_stored_is_not_an_error() {
        let dir = scratch("forget");
        assert!(!forget(&pad("Nothing"), Some(&dir)));
    }

    #[test]
    fn two_identical_controllers_share_one_profile() {
        // Coarser than a physical pad on purpose: nothing padmap can read
        // distinguishes two of a model, and they should not need configuring
        // twice.
        let one = pad("MAYFLASH GameCube Adapter");
        let two = pad("MAYFLASH GameCube Adapter");
        assert_eq!(
            path_for(&one, Some(Path::new("/x"))),
            path_for(&two, Some(Path::new("/x")))
        );
    }

    #[test]
    fn the_override_wins_over_the_xdg_path() {
        // The escape hatch every check script uses to keep off the real store.
        let previous = std::env::var(ENV_DIR).ok();
        std::env::set_var(ENV_DIR, "/tmp/padmap-override");
        assert_eq!(dir(), PathBuf::from("/tmp/padmap-override"));
        match previous {
            Some(value) => std::env::set_var(ENV_DIR, value),
            None => std::env::remove_var(ENV_DIR),
        }
    }
}
