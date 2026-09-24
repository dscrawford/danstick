//! Emulator configuration publishing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use padmap_core::{ares, emit, ryujinx};
use serde::{Deserialize, Serialize};

use crate::artefacts;

/// Published pad: capabilities are from the clone, GUID is SDL's index over it.
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

/// Output paths (None = real location).
#[derive(Debug, Clone, Default)]
pub struct Destinations {
    pub cemu_dir: Option<PathBuf>,
    pub dolphin_dir: Option<PathBuf>,
    pub ares_settings: Option<PathBuf>,
    pub ryujinx_config: Option<PathBuf>,
    pub env_file: Option<PathBuf>,
}

/// Write results and skips.
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

/// The environment variable Cemu -- and any other SDL program -- is told through.
pub const CONFIG_ENV: &str = emit::SDL_CONFIG_ENV;

/// Where the sourceable environment file is written.
pub fn env_path() -> PathBuf {
    crate::runtime::dir().join("env.sh")
}

/// POSIX-sh fragment exporting CONFIG_ENV: single-quoted for SDL parsing.
pub fn env_script(value: &str) -> String {
    let quoted = value.replace('\'', r"'\''");
    format!(
        "# Written by padmap on every republish. Source it, or use\n\
         # `padmap-rs exec -- <program>`, to hand a mapping to a program that\n\
         # reads no controller database of its own.\n\
         {CONFIG_ENV}='{quoted}'\n\
         export {CONFIG_ENV}\n"
    )
}

/// Extract value from env_script output; None if not ours.
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

/// Write all emulator configs; skips instead of failing.
/// `seat` is the keyboard's, when it holds one; otherwise it takes the first free port.
pub fn publish(pads: &[Published], dirs: &Destinations, seat: Option<u32>) -> Written {
    let mut written = Written::default();

    let by_player: BTreeMap<u32, &Published> = pads.iter().map(|pad| (pad.player, pad)).collect();
    let players: Vec<u32> = by_player.keys().copied().collect();

    match artefacts::write_cemu_profiles(
        &players,
        seat,
        |player| by_player[&player].guid.clone(),
        |player| by_player[&player].name.clone(),
        dirs.cemu_dir.as_deref(),
    ) {
        Ok(paths) => written.paths.extend(paths),
        Err(error) => written.skipped.push(("cemu", error.to_string())),
    }

    match artefacts::write_dolphin_config(
        &players,
        seat,
        |player| by_player[&player].name.clone(),
        dirs.dolphin_dir.as_deref(),
    ) {
        Ok(paths) => written.paths.extend(paths),
        Err(error) => written.skipped.push(("dolphin", error.to_string())),
    }

    // Every one of ares' five ports is written: a pad's, the keyboard's on the
    // first free one, and nothing on the rest -- a keyboard block left at
    // another port from last time would make the keyboard two players.
    let keyboard = padmap_core::keyboard::port(seat, &players, ares::MAX_PLAYERS);
    let blocks: BTreeMap<u32, String> = (1..=ares::MAX_PLAYERS)
        .map(|port| {
            let block = match by_player.get(&port) {
                Some(pad) => {
                    let indices = ares::Indices::of(&pad.keys, &pad.axes);
                    ares::virtual_pad(port, &pad.guid, &indices)
                }
                None if keyboard == Some(port) => ares::keyboard_pad(port),
                None => ares::empty_pad(port),
            };
            // The desk's mouse rides with the keyboard, on that port only.
            let mouse = ares::virtual_mouse(port, keyboard == Some(port));
            (port, block + &mouse)
        })
        .collect();
    written.record(
        "ares",
        artefacts::write_ares_settings(&blocks, dirs.ares_settings.as_deref()),
    );

    let entries: Vec<serde_json::Value> = by_player
        .values()
        .filter_map(|pad| ryujinx::input_config(pad.player, &pad.guid, &pad.name, 0))
        .collect();
    written.record(
        "ryujinx",
        artefacts::write_ryujinx_config(entries, seat, dirs.ryujinx_config.as_deref()),
    );

    // The environment file, which is the only way into Cemu and works for any other SDL program that loads its database once and never again.
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

/// VALUE for CONFIG_ENV variable (for `padmap-rs exec`).
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
        Destinations {
            cemu_dir: Some(dir.join("cemu")),
            dolphin_dir: Some(dir.join("dolphin-emu")),
            ares_settings: Some(dir.join("ares.bml")),
            ryujinx_config: Some(dir.join("Config.json")),
            env_file: Some(dir.join("env.sh")),
        }
    }

    #[test]
    fn an_emulator_that_has_never_run_is_skipped_not_invented() {
        let dir = scratch("absent");
        let written = publish(&[pad(1)], &only(&dir), None);

        let skipped: Vec<&str> = written.skipped.iter().map(|(what, _)| *what).collect();
        assert!(skipped.contains(&"ares"), "{written:?}");
        assert!(skipped.contains(&"ryujinx"), "{written:?}");
        assert!(!skipped.contains(&"dolphin"), "{written:?}");
        assert!(dir.join("dolphin-emu/GCPadNew.ini").exists());
        assert!(dir.join("dolphin-emu/Dolphin.ini").exists());
        assert!(dir.join("dolphin-emu/DSUClient.ini").exists());
        assert!(!dir.join("ares.bml").exists());
        assert!(!dir.join("Config.json").exists());

        assert!(dir.join("cemu").join("controller0.xml").exists());
        assert!(dir.join("env.sh").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_skip_never_stops_the_other_emulators() {
        let dir = scratch("partial");
        std::fs::write(dir.join("ares.bml"), "Video\n  Driver: OpenGL\n").expect("seed");
        let written = publish(&[pad(1), pad(2)], &only(&dir), None);

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
        let dir = scratch("ryujinx");
        std::fs::write(dir.join("Config.json"), r#"{"version": 50}"#).expect("seed");
        let pads: Vec<Published> = (1..=4).map(pad).collect();
        publish(&pads, &only(&dir), None);

        let text = std::fs::read_to_string(dir.join("Config.json")).expect("read");
        let config: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(config["version"], 50, "Ryujinx's own settings went");
        let entries = config["input_config"].as_array().expect("array");
        assert_eq!(entries.len(), 5, "four pads and the keyboard");
        let ids: std::collections::BTreeSet<&str> = entries
            .iter()
            .filter(|entry| entry["backend"] == "GamepadSDL2")
            .filter_map(|entry| entry["id"].as_str())
            .collect();
        assert_eq!(ids.len(), 4, "ids collapsed: {ids:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_env_file_carries_every_mapping_and_is_sourceable() {
        let dir = scratch("env");
        publish(&[pad(1), pad(2)], &only(&dir), None);
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
    fn the_keyboard_takes_the_first_free_port_everywhere_and_nowhere_else() {
        let dir = scratch("keyboard");
        std::fs::write(dir.join("ares.bml"), "Video\n").expect("seed");
        std::fs::write(dir.join("Config.json"), r#"{"version": 50}"#).expect("seed");
        // A keyboard profile padmap left at port 3 last time, and one the user made at port 5.
        std::fs::create_dir_all(dir.join("cemu")).expect("mkdir");
        std::fs::write(
            dir.join("cemu/controller2.xml"),
            padmap_core::cemu::keyboard_profile(3),
        )
        .expect("stale");
        std::fs::write(dir.join("cemu/controller4.xml"), "<emulated_controller/>\n")
            .expect("theirs");

        publish(&[pad(1)], &only(&dir), None);

        let cemu = std::fs::read_to_string(dir.join("cemu/controller1.xml")).expect("keyboard");
        assert!(cemu.contains("<api>Keyboard</api>"), "{cemu}");
        assert!(
            !dir.join("cemu/controller2.xml").exists(),
            "the stale keyboard stayed"
        );
        assert!(
            dir.join("cemu/controller4.xml").exists(),
            "the user's own profile went"
        );

        let dolphin = std::fs::read_to_string(dir.join("dolphin-emu/GCPadNew.ini")).expect("ini");
        assert!(
            dolphin.contains("[GCPad2]\nDevice = XInput2/0/Virtual core pointer\n"),
            "{dolphin}"
        );
        // The Wii side: the keyboard's seat holds the remote with the cursor.
        let wii = std::fs::read_to_string(dir.join("dolphin-emu/WiimoteNew.ini")).expect("ini");
        assert!(
            wii.contains("[Wiimote2]\nSource = 1\nDevice = XInput2/0/Virtual core pointer\n"),
            "{wii}"
        );
        assert!(wii.contains("IR/Up = `Cursor Y-`\n"), "{wii}");
        assert!(
            wii.contains("[Wiimote1]\nSource = 1\nDevice = SDL/0/"),
            "the pad has no remote: {wii}"
        );
        assert!(wii.contains("[Wiimote4]\nSource = 0\n"), "{wii}");

        let core = std::fs::read_to_string(dir.join("dolphin-emu/Dolphin.ini")).expect("ini");
        assert!(core.contains("SIDevice1 = 6"), "{core}");
        assert!(core.contains("SIDevice2 = 0"), "{core}");

        let ares = std::fs::read_to_string(dir.join("ares.bml")).expect("bml");
        assert!(
            ares.contains("VirtualPad2\n  Pad.Up: 0x1/0/84;;\n"),
            "{ares}"
        );
        assert!(ares.contains("VirtualPad3\n  Pad.Up: ;;\n"), "{ares}");
        assert!(ares.contains("VirtualPad5\n"));
        // The desk's mouse sits on the keyboard's port and nowhere else, so
        // an N64 or SNES Mouse there answers to the person at the keyboard.
        assert!(
            ares.contains("VirtualMouse2\n  X: 0x2/0/0;;\n"),
            "the keyboard's port has no mouse: {ares}"
        );
        assert!(
            ares.contains("VirtualMouse1\n  X: ;;\n") && ares.contains("VirtualMouse3\n  X: ;;\n"),
            "a port a pad holds kept the desk's mouse: {ares}"
        );

        let text = std::fs::read_to_string(dir.join("Config.json")).expect("json");
        let config: serde_json::Value = serde_json::from_str(&text).expect("json");
        let keyboards: Vec<&serde_json::Value> = config["input_config"]
            .as_array()
            .expect("array")
            .iter()
            .filter(|e| e["backend"] == "WindowKeyboard")
            .collect();
        assert_eq!(keyboards.len(), 1);
        assert_eq!(keyboards[0]["player_index"], "Player2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_seated_keyboard_is_player_one_ahead_of_a_pad_seated_after_it() {
        let dir = scratch("keyboard-seat");
        std::fs::write(dir.join("ares.bml"), "Video\n").expect("seed");
        publish(&[pad(2)], &only(&dir), Some(1));

        let cemu = std::fs::read_to_string(dir.join("cemu/controller0.xml")).expect("keyboard");
        assert!(cemu.contains("<api>Keyboard</api>") && cemu.contains("Wii U GamePad"));
        assert!(
            dir.join("cemu/controller1.xml").exists(),
            "the pad is player 2"
        );
        let dolphin = std::fs::read_to_string(dir.join("dolphin-emu/GCPadNew.ini")).expect("ini");
        assert!(dolphin.starts_with("[GCPad1]\nDevice = XInput2/0/Virtual core pointer\n"));
        assert!(dolphin.contains("[GCPad2]\nDevice = SDL/0/padmap Player 2\n"));
        let ares = std::fs::read_to_string(dir.join("ares.bml")).expect("bml");
        assert!(
            ares.contains("VirtualPad1\n  Pad.Up: 0x1/0/84;;\n"),
            "{ares}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_ninth_player_is_left_out_rather_than_wrapped_round() {
        let dir = scratch("cap");
        std::fs::write(dir.join("ares.bml"), "Video\n").expect("seed");
        publish(&[pad(1), pad(6), pad(9)], &only(&dir), None);

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
