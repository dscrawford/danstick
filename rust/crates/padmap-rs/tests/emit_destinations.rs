//! `emit` writes where it is told.
//!
//! A caller that runs each game in an environment of its own -- its own state
//! directory, its own `XDG_CONFIG_HOME` -- is not writing to the user's home,
//! and two variants of one game are two configurations that must never see
//! each other. Driven through the real binary, because the thing under test is
//! the command line reaching `Destinations` at all.

use std::path::Path;
use std::process::{Command, Stdio};

use std::io::Write;

/// Two pads, as a launcher would hand them over.
const PADS: &str = r#"[
  {"player": 1, "guid": "03000000000000000100000001000000", "name": "padmap Player 1",
   "keys": [304, 305, 307, 308], "axes": [0, 1, 16, 17],
   "sdl_line": "03000000000000000100000001000000,padmap Player 1,a:b0,platform:Linux,"},
  {"player": 2, "guid": "03000000000000000100000002000000", "name": "padmap Player 2",
   "keys": [304, 305], "axes": [0, 1],
   "sdl_line": "03000000000000000100000002000000,padmap Player 2,a:b0,platform:Linux,"}
]"#;

fn emit(root: &Path, args: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_padmap-rs"))
        .arg("emit")
        .args(args)
        // Somewhere harmless for anything not overridden, so a flag that fails
        // to reach `Destinations` writes into the sandbox rather than the
        // developer's own config.
        .env("XDG_CONFIG_HOME", root.join("fallback-config"))
        .env("XDG_DATA_HOME", root.join("fallback-data"))
        .env("XDG_RUNTIME_DIR", root.join("fallback-run"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn padmap emit");
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
    let root = std::env::temp_dir().join(format!("padmap-emit-dest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let game = root.join("state/config");
    std::fs::create_dir_all(&game).expect("mkdir");

    // ares and Ryujinx are only rewritten, never invented -- their files hold
    // every other setting those emulators have -- so a launch that wants them
    // written has them in the environment already.
    let ares = game.join("ares/settings.bml");
    let ryujinx = game.join("Ryujinx/Config.json");
    std::fs::create_dir_all(ares.parent().expect("parent")).expect("mkdir");
    std::fs::create_dir_all(ryujinx.parent().expect("parent")).expect("mkdir");
    std::fs::write(&ares, "Video\n  Driver: OpenGL\n").expect("seed ares");
    std::fs::write(&ryujinx, r#"{"version": 50}"#).expect("seed ryujinx");
    let cemu = game.join("Cemu/controllerProfiles");
    let dolphin = game.join("dolphin-emu");
    let env_file = root.join("state/padmap-env.sh");

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

    // Every one of them, where it was told.
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
    assert_eq!(config["input_config"].as_array().map(Vec::len), Some(2));
    let script = std::fs::read_to_string(&env_file).expect("env file");
    assert!(script.contains("padmap Player 1") && script.contains("padmap Player 2"));
    let pads = std::fs::read_to_string(dolphin.join("GCPadNew.ini")).expect("GCPadNew.ini");
    assert!(
        pads.contains("[GCPad1]\nDevice = SDL/0/padmap Player 1"),
        "{pads}"
    );
    assert!(pads.contains("[GCPad2]"), "{pads}");
    let core = std::fs::read_to_string(dolphin.join("Dolphin.ini")).expect("Dolphin.ini");
    // Two players seated, so the other two ports are emptied rather than left
    // holding a controller from a previous session.
    assert!(
        core.contains("SIDevice0 = 6") && core.contains("SIDevice1 = 6"),
        "{core}"
    );
    assert!(
        core.contains("SIDevice2 = 0") && core.contains("SIDevice3 = 0"),
        "{core}"
    );

    // ...and nothing was written to the locations it was pointed away from.
    assert!(
        !root.join("fallback-config").exists(),
        "emit wrote to the default config location as well"
    );
    assert!(!root.join("fallback-data").exists());

    // The paths it reports are the ones it was given, so a caller can act on
    // stdout rather than guessing where its own flags landed.
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
    let root = std::env::temp_dir().join(format!("padmap-emit-default-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cemu = root.join("elsewhere/Cemu");
    std::fs::create_dir_all(&root).expect("mkdir");

    // Only Cemu is redirected. The env file must still land under the
    // runtime directory the environment names -- overriding one destination
    // must not move the others.
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
        root.join("fallback-run/padmap/env.sh").is_file(),
        "the env file did not keep its default location"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_destination_flag_with_no_value_is_refused() {
    // `--ryujinx-config` with the path forgotten must not silently mean "the
    // user's own Ryujinx config", which is the one file this exists to avoid.
    let root = std::env::temp_dir().join(format!("padmap-emit-noval-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&root);
    let out = emit(&root, &["--ryujinx-config"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("expects a value"));
    let _ = std::fs::remove_dir_all(&root);
}
