//! Emulators that cannot find padmap's pads on their own.
//!
//! RetroArch and anything else reading `gamecontrollerdb.txt` is served by
//! [`crate::artefacts`]: padmap writes a mapping and the program looks it up.
//! Three emulators do not work that way, and each fails differently:
//!
//! * **Cemu** reads no mapping database at all. A pad SDL does not already
//!   recognise as a gamepad never appears in its device list, so writing a
//!   profile for it is not enough -- the mapping has to arrive in the
//!   environment, through `SDL_GAMECONTROLLERCONFIG`. See [`env_script`].
//! * **Ryujinx** blanks the name checksum out of the GUID before using it as a
//!   device id, so padmap's pads used to collapse into one. That is fixed in
//!   the GUID itself -- `padmap_core::emit::version_for` puts the player number
//!   in the version field, which Ryujinx keeps -- and this module is where the
//!   now-distinct ids get written down.
//! * **ares** binds raw SDL joystick indices, so a binding that does not fail
//!   binds the *wrong* button. The indices come from the clone's own
//!   capabilities, which is why [`Published`] carries them.
//!
//! Every write here is best-effort and reported rather than propagated. ares
//! and Ryujinx keep all of their settings in one file, so padmap refuses to
//! invent one for an emulator that has never run; that refusal is an ordinary
//! outcome -- most machines do not have all three installed -- and must not
//! stop the daemon from publishing pads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use padmap_core::{ares, emit, ryujinx};
use serde::{Deserialize, Serialize};

use crate::artefacts;

/// One published pad, as an emulator needs to see it.
///
/// The capabilities are the *clone's*, not the controller's, and for the same
/// reason the GUID is: ares counts indices over the device SDL opened, and SDL
/// opens the clone.
///
/// Serialisable because `padmap emit` takes this list on stdin: a launcher
/// that owns the roster -- and the directories the launch will use -- can have
/// the configs written without going through the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Published {
    pub player: u32,
    pub guid: String,
    pub name: String,
    #[serde(default)]
    pub keys: Vec<u16>,
    #[serde(default)]
    pub axes: Vec<u16>,
    /// The pad's line for the SDL database, or empty if it has no mapping.
    #[serde(default)]
    pub sdl_line: String,
}

/// Where each file goes.
///
/// `None` means the real location. Overriding one leaves the others alone,
/// which is what a test wanting to check exactly one emulator needs -- and
/// what a caller that runs each game in an isolated environment needs, since
/// two variants of one game are two configurations that must never see each
/// other, and neither of them is the one in the user's home.
#[derive(Debug, Clone, Default)]
pub struct Destinations {
    pub cemu_dir: Option<PathBuf>,
    pub ares_settings: Option<PathBuf>,
    pub ryujinx_config: Option<PathBuf>,
    pub env_file: Option<PathBuf>,
}

/// What actually happened, target by target.
///
/// Skips are carried rather than logged in place so the caller decides how
/// loud they are: the daemon says them once at INFO, and a test asserts on
/// them.
#[derive(Debug, Default)]
pub struct Written {
    pub paths: Vec<PathBuf>,
    pub skipped: Vec<(&'static str, String)>,
}

impl Written {
    fn record(&mut self, target: &'static str, result: Result<PathBuf, artefacts::WriteError>) {
        match result {
            Ok(path) => self.paths.push(path),
            Err(error) => self.skipped.push((target, error.to_string())),
        }
    }
}

/// The environment variable Cemu -- and any other SDL program -- is told
/// through.
pub const CONFIG_ENV: &str = emit::SDL_CONFIG_ENV;

/// Where the sourceable environment file is written.
pub fn env_path() -> PathBuf {
    crate::runtime::dir().join("env.sh")
}

/// A POSIX-sh fragment exporting [`CONFIG_ENV`].
///
/// Single-quoted with embedded newlines, which is valid sh and is what SDL's
/// reader expects: it parses the variable with the same code that reads a
/// database file, one mapping per line.
pub fn env_script(value: &str) -> String {
    // A mapping line cannot contain a quote today -- the name is "padmap
    // Player N" and the rest is hex and SDL control names -- but this file is
    // sourced by a shell, and "cannot happen" is how a shell injection gets
    // written.
    let quoted = value.replace('\'', r"'\''");
    format!(
        "# Written by padmap on every republish. Source it, or use\n\
         # `padmap-rs exec -- <program>`, to hand a mapping to a program that\n\
         # reads no controller database of its own.\n\
         {CONFIG_ENV}='{quoted}'\n\
         export {CONFIG_ENV}\n"
    )
}

/// The value back out of a file [`env_script`] wrote.
///
/// `padmap-rs exec` reads the file rather than recomputing the value: it has
/// no pads open, and opening them would take them from the daemon that does.
/// `None` if the file is not one of ours -- better than handing a program half
/// a mapping.
pub fn value_from_script(script: &str) -> Option<String> {
    let start = script.find(&format!("{CONFIG_ENV}='"))? + CONFIG_ENV.len() + 2;
    let rest = &script[start..];
    let end = rest.rfind('\'')?;
    Some(rest[..end].replace(r"'\''", "'"))
}

/// Write the environment file, creating its directory.
pub fn write_env(value: &str, path: Option<&Path>) -> Result<PathBuf, artefacts::WriteError> {
    let target = path.map(Path::to_path_buf).unwrap_or_else(env_path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| artefacts::WriteError::Io(target.clone(), error))?;
    }
    std::fs::write(&target, env_script(value))
        .map_err(|error| artefacts::WriteError::Io(target.clone(), error))?;
    Ok(target)
}

/// Write every emulator artefact for these pads.
///
/// Never fails: an emulator that is not installed, or whose config padmap
/// declines to invent, is a skip. Losing Cemu's profiles is not a reason to
/// stop republishing the pads themselves.
pub fn publish(pads: &[Published], dirs: &Destinations) -> Written {
    let mut written = Written::default();

    let by_player: BTreeMap<u32, &Published> = pads.iter().map(|pad| (pad.player, pad)).collect();
    let players: Vec<u32> = by_player.keys().copied().collect();

    // Cemu: one profile per player, addressed by GUID.
    match artefacts::write_cemu_profiles(
        &players,
        |player| by_player[&player].guid.clone(),
        |player| by_player[&player].name.clone(),
        dirs.cemu_dir.as_deref(),
    ) {
        Ok(paths) => written.paths.extend(paths),
        Err(error) => written.skipped.push(("cemu", error.to_string())),
    }

    // ares: one VirtualPad block per player, bound by raw SDL index.
    let blocks: BTreeMap<u32, String> = by_player
        .values()
        .filter(|pad| pad.player <= ares::MAX_PLAYERS)
        .map(|pad| {
            let indices = ares::Indices::of(&pad.keys, &pad.axes);
            (
                pad.player,
                ares::virtual_pad(pad.player, &pad.guid, &indices),
            )
        })
        .collect();
    written.record(
        "ares",
        artefacts::write_ares_settings(&blocks, dirs.ares_settings.as_deref()),
    );

    // Ryujinx: one input_config entry per player. The ordinal is always 0 --
    // each padmap pad's id is unique now that the player number rides in the
    // GUID's version field.
    let entries: Vec<serde_json::Value> = by_player
        .values()
        .filter_map(|pad| ryujinx::input_config(pad.player, &pad.guid, &pad.name, 0))
        .collect();
    written.record(
        "ryujinx",
        artefacts::write_ryujinx_config(entries, dirs.ryujinx_config.as_deref()),
    );

    // The environment file, which is the only way into Cemu and works for any
    // other SDL program that loads its database once and never again.
    let lines: BTreeMap<u32, String> = by_player
        .values()
        .filter(|pad| !pad.sdl_line.is_empty())
        .map(|pad| (pad.player, pad.sdl_line.clone()))
        .collect();
    written.record(
        "env",
        write_env(&emit::sdl_config_value(&lines), dirs.env_file.as_deref()),
    );

    written
}

/// The value [`CONFIG_ENV`] should hold for these pads.
///
/// Exposed for `padmap-rs exec`, which sets the variable directly rather than
/// going through a file.
pub fn config_value(pads: &[Published]) -> String {
    let lines: BTreeMap<u32, String> = pads
        .iter()
        .filter(|pad| !pad.sdl_line.is_empty())
        .map(|pad| (pad.player, pad.sdl_line.clone()))
        .collect();
    emit::sdl_config_value(&lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("padmap-emulators-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn pad(player: u32) -> Published {
        Published {
            player,
            guid: format!("0300000{player}5e040000e00200000{player}000000"),
            name: emit::virtual_name(player),
            keys: vec![0x130, 0x131, 0x133, 0x134],
            axes: vec![0x00, 0x01, 0x10, 0x11],
            sdl_line: format!("guid{player},padmap Player {player},a:b0,"),
        }
    }

    fn only(dir: &Path) -> Destinations {
        // Everything inside one scratch directory, so nothing reaches a real
        // config and a test that forgets an override fails loudly rather than
        // rewriting the developer's own ares settings.
        Destinations {
            cemu_dir: Some(dir.join("cemu")),
            ares_settings: Some(dir.join("ares.bml")),
            ryujinx_config: Some(dir.join("Config.json")),
            env_file: Some(dir.join("env.sh")),
        }
    }

    #[test]
    fn an_emulator_that_has_never_run_is_skipped_not_invented() {
        // ares and Ryujinx keep every setting in the one file. Writing one
        // from nothing would leave the emulator with padmap's ports and
        // defaults for everything else -- video, audio, paths.
        let dir = scratch("absent");
        let written = publish(&[pad(1)], &only(&dir));

        let skipped: Vec<&str> = written.skipped.iter().map(|(what, _)| *what).collect();
        assert!(skipped.contains(&"ares"), "{written:?}");
        assert!(skipped.contains(&"ryujinx"), "{written:?}");
        assert!(!dir.join("ares.bml").exists());
        assert!(!dir.join("Config.json").exists());

        // And the two that need nothing pre-existing still happened.
        assert!(dir.join("cemu").join("controller0.xml").exists());
        assert!(dir.join("env.sh").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_skip_never_stops_the_other_emulators() {
        // The whole point of collecting skips rather than returning an error:
        // one absent emulator used to be indistinguishable from a failure.
        let dir = scratch("partial");
        std::fs::write(dir.join("ares.bml"), "Video\n  Driver: OpenGL\n").expect("seed");
        let written = publish(&[pad(1), pad(2)], &only(&dir));

        let text = std::fs::read_to_string(dir.join("ares.bml")).expect("read");
        assert!(text.contains("Driver: OpenGL"), "ares' own settings went");
        assert!(text.contains("VirtualPad1") && text.contains("VirtualPad2"));
        assert_eq!(
            written.skipped.len(),
            1,
            "only Ryujinx should be missing: {written:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_player_gets_a_distinct_ryujinx_id() {
        // Ryujinx blanks the name checksum out of the GUID. Before the player
        // number moved into the version field, four pads produced one id and
        // three of them silently did nothing.
        let dir = scratch("ryujinx");
        std::fs::write(dir.join("Config.json"), r#"{"version": 50}"#).expect("seed");
        let pads: Vec<Published> = (1..=4).map(pad).collect();
        publish(&pads, &only(&dir));

        let text = std::fs::read_to_string(dir.join("Config.json")).expect("read");
        let config: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(config["version"], 50, "Ryujinx's own settings went");
        let entries = config["input_config"].as_array().expect("array");
        assert_eq!(entries.len(), 4);
        let ids: std::collections::BTreeSet<&str> = entries
            .iter()
            .filter_map(|entry| entry["id"].as_str())
            .collect();
        assert_eq!(ids.len(), 4, "ids collapsed: {ids:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_env_file_carries_every_mapping_and_is_sourceable() {
        let dir = scratch("env");
        publish(&[pad(1), pad(2)], &only(&dir));
        let text = std::fs::read_to_string(dir.join("env.sh")).expect("read");
        assert!(text.contains("padmap Player 1") && text.contains("padmap Player 2"));

        let quoted = text
            .split_once('\'')
            .and_then(|(_, rest)| rest.rsplit_once('\''))
            .map(|(value, _)| value.to_owned())
            .expect("a quoted value");
        assert_eq!(quoted.lines().count(), 2, "one mapping per line for SDL");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pad_with_no_mapping_contributes_no_line() {
        // An empty line in SDL_GAMECONTROLLERCONFIG is not harmless: the value
        // is parsed as a database, and a line claiming a pad with no buttons
        // is worse than SDL falling back to its own entry.
        let mut unmapped = pad(2);
        unmapped.sdl_line = String::new();
        let value = config_value(&[pad(1), unmapped]);
        assert_eq!(value, "guid1,padmap Player 1,a:b0,");
    }

    #[test]
    fn a_quote_in_a_mapping_cannot_escape_the_env_file() {
        let hostile = "x',; rm -rf /; '";
        let script = env_script(hostile);
        assert!(!script.contains("rm -rf /;\n"), "{script}");
        assert!(script.contains(r"'\''"), "{script}");
        // And `exec` still recovers exactly what went in, quotes and all --
        // otherwise escaping would be trading an injection for a wrong mapping.
        assert_eq!(value_from_script(&script).as_deref(), Some(hostile));
    }

    #[test]
    fn the_value_survives_the_round_trip_through_the_file() {
        let value = config_value(&[pad(1), pad(2)]);
        assert_eq!(
            value_from_script(&env_script(&value)).as_deref(),
            Some(value.as_str())
        );
    }

    #[test]
    fn a_file_that_is_not_ours_yields_nothing_rather_than_half_a_mapping() {
        assert_eq!(value_from_script("export FOO=bar\n"), None);
    }

    #[test]
    fn a_ninth_player_is_left_out_rather_than_wrapped_round() {
        // Cemu has eight slots and ares five. Wrapping player nine to
        // controller0.xml would take player one's pad away.
        let dir = scratch("cap");
        std::fs::write(dir.join("ares.bml"), "Video\n").expect("seed");
        publish(&[pad(1), pad(6), pad(9)], &only(&dir));

        assert!(
            dir.join("cemu").join("controller5.xml").exists(),
            "player 6"
        );
        assert!(
            !dir.join("cemu").join("controller8.xml").exists(),
            "player 9"
        );
        let text = std::fs::read_to_string(dir.join("ares.bml")).expect("read");
        assert!(text.contains("VirtualPad1"));
        assert!(!text.contains("VirtualPad6"), "ares only has five ports");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
