//! Emit writes where it is told; each game gets its own XDG directories.

use std::path::Path;
use std::process::{Command, Stdio};

use std::io::Write;

/// Two pads, as a launcher would hand them over.
const PADS: &str = r#"[
  {"player": 1, "guid": "03000000000000000100000001000000", "name": "danstick Player 1",
   "keys": [304, 305, 307, 308], "axes": [0, 1, 16, 17],
   "sdl_line": "03000000000000000100000001000000,danstick Player 1,a:b0,platform:Linux,"},
  {"player": 2, "guid": "03000000000000000100000002000000", "name": "danstick Player 2",
   "keys": [304, 305], "axes": [0, 1],
   "sdl_line": "03000000000000000100000002000000,danstick Player 2,a:b0,platform:Linux,"}
]"#;

fn emit(root: &Path, args: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_danstick-rs"))
        .arg("emit")
        .args(args)
        .env("XDG_CONFIG_HOME", root.join("fallback-config"))
        .env("XDG_DATA_HOME", root.join("fallback-data"))
        .env("XDG_RUNTIME_DIR", root.join("fallback-run"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn danstick emit");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(PADS.as_bytes())
        .expect("write the pad list");
    child.wait_with_output().expect("emit finishes")
}

#[test]
fn every_destination_can_be_pointed_somewhere_else() {
    let root = std::env::temp_dir().join(format!("danstick-emit-dest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let game = root.join("state/config");
    std::fs::create_dir_all(&game).expect("mkdir");

    // ares and Ryujinx are only rewritten, never invented -- their files hold every other setting those emulators have -- so a launch that wants them written has them in the environment already.
    let ares = game.join("ares/settings.bml");
    let ryujinx = game.join("Ryujinx/Config.json");
    std::fs::create_dir_all(ares.parent().expect("parent")).expect("mkdir");
    std::fs::create_dir_all(ryujinx.parent().expect("parent")).expect("mkdir");
    std::fs::write(&ares, "Video\n  Driver: OpenGL\n").expect("seed ares");
    std::fs::write(&ryujinx, r#"{"version": 50}"#).expect("seed ryujinx");
    let cemu = game.join("Cemu/controllerProfiles");
    let dolphin = game.join("dolphin-emu");
    let env_file = root.join("state/danstick-env.sh");

    let out = emit(
        &root,
        &[
            "--cemu-dir",
            cemu.to_str().expect("utf8"),
            "--dolphin-dir",
            dolphin.to_str().expect("utf8"),
            "--ares-settings",
            ares.to_str().expect("utf8"),
            "--ryujinx-config",
            ryujinx.to_str().expect("utf8"),
            "--env-file",
            env_file.to_str().expect("utf8"),
        ],
    );
    assert!(
        out.status.success(),
        "emit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let profile = std::fs::read_to_string(cemu.join("controller0.xml")).expect("Cemu profile");
    assert!(
        profile.contains("03000000000000000100000001000000"),
        "{profile}"
    );
    let settings = std::fs::read_to_string(&ares).expect("ares settings");
    assert!(
        settings.contains("Driver: OpenGL"),
        "ares' own settings went"
    );
    assert!(settings.contains("VirtualPad1") && settings.contains("VirtualPad2"));
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&ryujinx).expect("Ryujinx config"))
            .expect("json");
    assert_eq!(config["version"], 50, "Ryujinx's own settings went");
    let entries = config["input_config"].as_array().expect("entries");
    assert_eq!(entries.len(), 3, "two pads and the keyboard on Player3");
    assert_eq!(entries[2]["backend"], "WindowKeyboard");
    assert_eq!(entries[2]["player_index"], "Player3");
    let script = std::fs::read_to_string(&env_file).expect("env file");
    assert!(script.contains("danstick Player 1") && script.contains("danstick Player 2"));
    let pads = std::fs::read_to_string(dolphin.join("GCPadNew.ini")).expect("GCPadNew.ini");
    assert!(
        pads.contains("[GCPad1]\nDevice = SDL/0/danstick Player 1"),
        "{pads}"
    );
    assert!(pads.contains("[GCPad2]"), "{pads}");
    let core = std::fs::read_to_string(dolphin.join("Dolphin.ini")).expect("Dolphin.ini");
    assert!(
        core.contains("SIDevice0 = 6") && core.contains("SIDevice1 = 6"),
        "{core}"
    );
    assert!(
        core.contains("SIDevice2 = 6") && core.contains("SIDevice3 = 0"),
        "port 3 is the keyboard's, port 4 nobody's: {core}"
    );
    assert!(
        pads.contains("[GCPad3]\nDevice = XInput2/0/Virtual core pointer"),
        "{pads}"
    );

    assert!(
        !root.join("fallback-config").exists(),
        "emit wrote to the default config location as well"
    );
    assert!(!root.join("fallback-data").exists());

    let written = String::from_utf8_lossy(&out.stdout);
    for expected in [
        &cemu.join("controller0.xml"),
        &dolphin.join("GCPadNew.ini"),
        &dolphin.join("Dolphin.ini"),
        &ares,
        &ryujinx,
        &env_file,
    ] {
        assert!(
            written.contains(&expected.display().to_string()),
            "{} is not in:\n{written}",
            expected.display()
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_absent_flag_keeps_the_default_location() {
    let root = std::env::temp_dir().join(format!("danstick-emit-default-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cemu = root.join("elsewhere/Cemu");
    std::fs::create_dir_all(&root).expect("mkdir");

    let out = emit(&root, &["--cemu-dir", cemu.to_str().expect("utf8")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        cemu.join("controller0.xml").is_file(),
        "Cemu went elsewhere"
    );
    assert!(
        root.join("fallback-run/danstick/env.sh").is_file(),
        "the env file did not keep its default location"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_destination_flag_with_no_value_is_refused() {
    // `--ryujinx-config` with the path forgotten must not silently mean "the user's own Ryujinx config", which is the one file this exists to avoid.
    let root = std::env::temp_dir().join(format!("danstick-emit-noval-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&root);
    let out = emit(&root, &["--ryujinx-config"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("expects a value"));
    let _ = std::fs::remove_dir_all(&root);
}
