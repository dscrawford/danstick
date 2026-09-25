//! The keyboard takes the first free port in every emulator, in that emulator's own keys.

use danstick_core::{ares, cemu, dolphin, retroarch, ryujinx, userconfig};
use serde_json::{json, Value};

const GUID: &str = "03000000120900000100000001000000";

// ---- RetroArch ----

#[test]
fn retroarch_leaves_its_own_key_defaults_alone_when_player_one_is_free() {
    // The keys are RetroArch's own on player 1 already; only the mouse,
    // whose defaults spread one per port, still has to be said.
    for text in [
        retroarch::keyboard_config(&[], None),
        retroarch::keyboard_config(&[2, 3], None),
    ] {
        assert!(!text.contains(" = \"nul\""), "the keys moved: {text}");
        assert!(!text.contains("input_player1_a"), "{text}");
        assert!(
            text.contains("input_player1_mouse_index = \"0\"\n"),
            "{text}"
        );
    }
}

#[test]
fn retroarch_moves_the_keyboard_off_a_seated_player_one() {
    let text = retroarch::keyboard_config(&[1], None);
    for bind in userconfig::PLAYER_BINDS {
        assert!(
            text.contains(&format!("input_player1_{bind} = \"nul\"\n")),
            "{bind} still drives player 1"
        );
    }
    assert!(text.contains("input_player2_b = \"z\"\n"), "{text}");
    assert!(text.contains("input_player2_a = \"x\"\n"));
    assert!(text.contains("input_player2_start = \"enter\"\n"));
    assert!(text.contains("input_player2_select = \"rshift\"\n"));
    assert!(text.contains("input_player2_up = \"up\"\n"));
    assert!(text.contains("input_all_users_control_menu = \"true\"\n"));
    assert!(
        !text.contains("input_player2_b_btn"),
        "a keyboard bind has no suffix"
    );
    assert!(
        !text.contains("input_player3_a"),
        "only the keyboard's own port gets keys: {text}"
    );
}

#[test]
fn retroarch_gives_the_desks_mouse_to_the_keyboards_player_and_to_nobody_else() {
    // RetroArch defaults port i to mouse i, so player 1 holds the desk's
    // mouse whoever is sitting there. The keyboard's seat takes it instead.
    let text = retroarch::keyboard_config(&[1], Some(2));
    assert!(
        text.contains("input_player2_mouse_index = \"0\"\n"),
        "the keyboard's player has no mouse: {text}"
    );
    let none = format!("mouse_index = \"{}\"", retroarch::NO_MOUSE);
    assert!(
        text.contains(&format!("input_player1_{none}\n")),
        "the pad's port kept the desk's mouse: {text}"
    );
    for player in [3, 16] {
        assert!(
            text.contains(&format!("input_player{player}_{none}\n")),
            "player {player} kept a mouse: {text}"
        );
    }
    assert!(
        !text.contains("input_player17_mouse_index"),
        "RetroArch has sixteen ports: {text}"
    );

    // Unseated, the keyboard still sits on the first free port, and the
    // mouse follows it there rather than staying on the pad's player 1.
    let text = retroarch::keyboard_config(&[1], None);
    assert!(
        text.contains("input_player2_mouse_index = \"0\"\n"),
        "{text}"
    );
    assert!(text.contains(&format!("input_player1_{none}\n")), "{text}");

    // Every port seated: the keyboard drives nobody, and so does the mouse.
    let all: Vec<u32> = (1..=retroarch::MAX_PLAYERS).collect();
    let text = retroarch::keyboard_config(&all, None);
    assert!(
        !text.contains("mouse_index = \"0\""),
        "nobody is at the keyboard, so nobody has the mouse: {text}"
    );
}

#[test]
fn retroarch_with_every_port_seated_gives_the_keyboard_nobody() {
    let all: Vec<u32> = (1..=retroarch::MAX_PLAYERS).collect();
    let text = retroarch::keyboard_config(&all, None);
    assert!(text.contains("input_player1_a = \"nul\"\n"));
    assert!(!text.contains("= \"x\""), "{text}");
    assert!(!text.contains("all_users_control_menu"));
}

#[test]
fn a_seated_keyboard_keeps_its_seat_ahead_of_pads_seated_later() {
    // Keyboard seated first as player 1, a pad after it as player 2.
    let text = retroarch::keyboard_config(&[2], Some(1));
    assert!(
        !text.contains(" = \"nul\""),
        "player 1's key defaults stand: {text}"
    );
    assert!(
        text.contains("input_player1_mouse_index = \"0\"\n"),
        "{text}"
    );
    let text = retroarch::keyboard_config(&[1, 2], Some(3));
    assert!(text.contains("input_player3_a = \"x\"\n"), "{text}");
    let merged = ryujinx::merge(&Value::Null, vec![pad(2)], Some(1));
    assert_eq!(indices_of(&merged, "WindowKeyboard"), ["Player1"]);
    let merged = ryujinx::merge(&Value::Null, vec![pad(1)], Some(9));
    assert!(
        indices_of(&merged, "WindowKeyboard").is_empty(),
        "seat 9 is no Ryujinx player"
    );
}

// ---- Dolphin ----

#[test]
fn dolphin_writes_what_it_would_have_written_itself_for_port_one() {
    let section = dolphin::keyboard_section(2);
    assert!(section.starts_with("[GCPad2]\nDevice = XInput2/0/Virtual core pointer\n"));
    for line in [
        "Buttons/A = `X`",
        "Buttons/B = `Z`",
        "Buttons/Start = `Return`",
        "Main Stick/Up = `Up`",
        "Main Stick/Modifier = `Shift`",
        "C-Stick/Up = `I`",
        "Triggers/L = `Q`",
        "D-Pad/Up = `T`",
    ] {
        assert!(section.contains(&format!("{line}\n")), "{line}");
    }
    assert_eq!(dolphin::keyboard_port(&[1, 2], None), Some(3));
    assert_eq!(dolphin::keyboard_port(&[1, 2, 3, 4], None), None);
}

#[test]
fn dolphins_wii_remote_follows_the_keyboards_seat_and_the_pads_keep_theirs() {
    // The request's own check: keyboard on seat 2, a pad on seat 1.
    let text = dolphin::wiimote_sections(&[1], Some(2), |p| format!("danstick Player {p}"));

    // Remote 2 is the mouse and keyboard's, pointing with the cursor.
    assert!(
        text.contains("[Wiimote2]\nSource = 1\nDevice = XInput2/0/Virtual core pointer\n"),
        "{text}"
    );
    for line in [
        "IR/Up = `Cursor Y-`",
        "IR/Down = `Cursor Y+`",
        "IR/Left = `Cursor X-`",
        "IR/Right = `Cursor X+`",
        "Buttons/A = `Click 1`",
        "Buttons/B = `Click 3`",
        "Buttons/Home = `Return`",
        "Extension = Nunchuk",
        "Nunchuk/Stick/Up = `W`",
        "Nunchuk/Buttons/C = `Control_L`",
    ] {
        assert!(
            text.contains(&format!("{line}\n")),
            "{line} missing:\n{text}"
        );
    }

    // Remote 1 is the pad's: its own clone, and no cursor anywhere in it.
    let pad_section = text
        .split("[Wiimote2]")
        .next()
        .expect("remote 1 comes first");
    assert!(
        pad_section.contains("[Wiimote1]\nSource = 1\nDevice = SDL/0/danstick Player 1\n"),
        "{text}"
    );
    assert!(
        !pad_section.contains("Cursor") && !pad_section.contains("Click"),
        "the pad's remote reads the desk's mouse: {pad_section}"
    );
    assert!(
        pad_section.contains("IR/Up = `Right Y+`\n"),
        "the pad points with nothing: {pad_section}"
    );
    // A thumbstick is round: the Nunchuk's, and the right stick the pad
    // points with, whose square IR gate a round stick could never reach the
    // corners of uncalibrated.
    let round = format!("Calibration = {}\n", dolphin::ROUND_GATE);
    assert!(
        pad_section.contains(&format!("Nunchuk/Stick/{round}")),
        "{pad_section}"
    );
    assert!(
        pad_section.contains(&format!("IR/{round}")),
        "{pad_section}"
    );
    assert!(!pad_section.contains("141.42"), "{pad_section}");

    // Remotes nobody holds are declared off, not left from a bigger session.
    assert!(text.contains("[Wiimote3]\nSource = 0\n"), "{text}");
    assert!(text.contains("[Wiimote4]\nSource = 0\n"), "{text}");
    assert!(!text.contains("[Wiimote5]"), "Dolphin has four remotes");

    // Unseated, the keyboard still lands on the first remote no pad holds,
    // rather than on remote 1 the way Dolphin ships it.
    let text = dolphin::wiimote_sections(&[1, 2], None, |p| format!("danstick Player {p}"));
    assert!(
        text.contains("[Wiimote3]\nSource = 1\nDevice = XInput2/0/Virtual core pointer\n"),
        "{text}"
    );

    // Every remote seated: nobody gets the cursor.
    let text = dolphin::wiimote_sections(&[1, 2, 3, 4], None, |p| format!("danstick Player {p}"));
    assert!(!text.contains("Cursor"), "{text}");
    assert!(!text.contains("Source = 0"), "{text}");
}

// ---- Ryujinx ----

fn pad(player: u32) -> Value {
    ryujinx::input_config(player, GUID, &format!("danstick Player {player}"), 0).expect("entry")
}

fn indices_of(merged: &Value, backend: &str) -> Vec<String> {
    merged
        .as_array()
        .expect("array")
        .iter()
        .filter(|e| e["backend"] == backend)
        .map(|e| e["player_index"].as_str().unwrap_or("").to_owned())
        .collect()
}

#[test]
fn ryujinx_moves_the_users_own_keyboard_to_the_first_free_player() {
    let existing = json!([
        {"backend": "WindowKeyboard", "player_index": "Player1", "left_joycon": {"dpad_up": "Number8"}}
    ]);
    let merged = ryujinx::merge(&existing, vec![pad(1), pad(2)], None);
    assert_eq!(indices_of(&merged, "GamepadSDL2"), ["Player1", "Player2"]);
    assert_eq!(indices_of(&merged, "WindowKeyboard"), ["Player3"]);
    let keyboard = &merged.as_array().expect("array")[2];
    assert_eq!(
        keyboard["left_joycon"]["dpad_up"], "Number8",
        "the user's own keys were replaced"
    );
}

#[test]
fn ryujinx_seeds_its_own_default_keyboard_when_there_is_none() {
    let merged = ryujinx::merge(&Value::Null, vec![pad(1)], None);
    assert_eq!(indices_of(&merged, "WindowKeyboard"), ["Player2"]);
    let keyboard = &merged.as_array().expect("array")[1];
    assert_eq!(keyboard["controller_type"], "JoyconPair");
    assert_eq!(keyboard["left_joycon_stick"]["stick_up"], "W");
    assert_eq!(keyboard["right_joycon"]["button_a"], "Z");
    assert_eq!(keyboard["id"], "0");
}

#[test]
fn ryujinx_keeps_one_keyboard_and_no_more() {
    let existing = json!([
        {"backend": "WindowKeyboard", "player_index": "Player1"},
        {"backend": "WindowKeyboard", "player_index": "Player5"},
        {"backend": "GamepadSDL2", "player_index": "Player3", "name": "theirs"}
    ]);
    let merged = ryujinx::merge(&existing, vec![pad(1)], None);
    assert_eq!(indices_of(&merged, "WindowKeyboard"), ["Player2"]);
    assert_eq!(indices_of(&merged, "GamepadSDL2"), ["Player3", "Player1"]);
}

#[test]
fn ryujinx_with_eight_players_has_no_keyboard() {
    let merged = ryujinx::merge(&Value::Null, (1..=8).map(pad).collect(), None);
    assert!(indices_of(&merged, "WindowKeyboard").is_empty());
}

// ---- ares ----

#[test]
fn ares_binds_every_control_to_the_generic_keyboard() {
    let block = ares::keyboard_pad(2);
    assert!(block.starts_with("VirtualPad2\n"));
    let names: Vec<&str> = ares::CONTROLS.iter().map(|(name, _)| *name).collect();
    for (name, _) in ares::KEYBOARD {
        assert!(names.contains(&name), "{name} is not one of ares' controls");
        assert!(
            block.contains(&format!("  {name}: 0x1/0/")),
            "{name} unbound"
        );
    }
    assert!(
        block.contains("  A..South: 0x1/0/60;;\n"),
        "Z is index 60 in xlib"
    );
    assert!(
        block.contains("  Start: 0x1/0/89;;\n"),
        "Return is index 89"
    );
    // Arrows drive the d-pad and the left stick both.
    assert!(block.contains("  Pad.Up: 0x1/0/84;;\n"));
    assert!(block.contains("  L-Up: 0x1/0/84;;\n"));
    assert!(block.contains("  Rumble: ;;\n"));
}

#[test]
fn ares_gives_the_desks_mouse_to_the_keyboards_port_and_clears_the_rest() {
    // ares' generic mouse is vendor 0, product 2, path 0 -- id 0x2 -- with
    // its axes in group 0 and its buttons in group 1 (ruby's xlib driver).
    let block = ares::virtual_mouse(2, true);
    assert!(block.starts_with("VirtualMouse2\n"), "{block}");
    for (name, binding) in [
        ("X", "0x2/0/0"),
        ("Y", "0x2/0/1"),
        ("Left", "0x2/1/0"),
        ("Middle", "0x2/1/1"),
        ("Right", "0x2/1/2"),
    ] {
        assert!(
            block.contains(&format!("  {name}: {binding};;\n")),
            "{name} unbound: {block}"
        );
    }
    assert!(
        block.contains("  Extra: ;;\n"),
        "xlib's mouse has no Extra button to bind: {block}"
    );

    // Every other port names the controls and binds none, so a mouse left
    // at another port last time cannot make the mouse two players.
    let empty = ares::virtual_mouse(3, false);
    assert!(empty.starts_with("VirtualMouse3\n"), "{empty}");
    assert!(!empty.contains("0x2"), "{empty}");
    assert_eq!(empty.lines().count(), 1 + ares::MOUSE_CONTROLS.len());
}

#[test]
fn ares_empty_pad_names_every_control_and_binds_none() {
    let block = ares::empty_pad(4);
    assert!(block.starts_with("VirtualPad4\n"));
    assert_eq!(block.lines().count(), 1 + ares::CONTROLS.len() + 1);
    assert!(!block.contains("0x"), "{block}");
}

// ---- Cemu ----

fn cemu_pairs(xml: &str) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut mapping = None;
    for line in xml.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("<mapping>") {
            mapping = rest.strip_suffix("</mapping>").and_then(|n| n.parse().ok());
        } else if let Some(rest) = line.strip_prefix("<button>") {
            let button: u32 = rest
                .strip_suffix("</button>")
                .and_then(|n| n.parse().ok())
                .expect("n");
            out.push((mapping.take().expect("a mapping before its button"), button));
        }
    }
    out
}

#[test]
fn cemu_keyboard_profile_is_a_keyboard_of_the_right_type_in_gdk_keysyms() {
    let one = cemu::keyboard_profile(1);
    assert!(one.contains("<type>Wii U GamePad</type>"));
    assert!(one.contains("<api>Keyboard</api>"));
    assert!(one.contains("<uuid>keyboard</uuid>"));
    assert!(cemu::is_keyboard_profile(&one));
    assert!(!cemu::is_keyboard_profile(&cemu::profile(
        1,
        GUID,
        "danstick Player 1"
    )));
    let pairs = cemu_pairs(&one);
    assert_eq!(pairs.len(), 24);
    assert!(pairs.contains(&(cemu::GamePad::A as u32, 120)), "A is x");
    assert!(pairs.contains(&(cemu::GamePad::B as u32, 122)), "B is z");
    assert!(
        pairs.contains(&(cemu::GamePad::Plus as u32, 0xff0d)),
        "Plus is Return"
    );
    assert!(pairs.contains(&(cemu::GamePad::Up as u32, 0xff52)));
    assert!(
        pairs.contains(&(cemu::GamePad::StickLUp as u32, 0xff52)),
        "arrows drive the stick too"
    );

    let two = cemu::keyboard_profile(2);
    assert!(two.contains("<type>Wii U Pro Controller</type>"));
    let pairs = cemu_pairs(&two);
    assert!(
        pairs.contains(&(cemu::WiiU::Up as u32, 0xff52)),
        "Pro numbers the d-pad differently"
    );
    assert!(pairs.contains(&(cemu::WiiU::StickRUp as u32, 105)), "I");
}
