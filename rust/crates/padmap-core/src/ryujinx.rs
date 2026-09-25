//! Ryujinx input config.

use serde_json::{json, Value};

/// Ryujinx allows eight.
pub const MAX_PLAYERS: u32 = 8;

/// SDL gamepad backend name: SDL2 works everywhere.
pub const BACKEND: &str = "GamepadSDL2";

/// Ryujinx device ID from GUID: .NET Guid format with name CRC zeroed.
pub fn device_id(guid: &str, ordinal: u32) -> Option<String> {
    if !crate::sdl::is_guid(guid) {
        return None;
    }
    let at = |range: std::ops::Range<usize>| &guid[range];
    Some(format!(
        "{ordinal}-0000{}{}-{}{}-{}-{}-{}",
        at(2..4),
        at(0..2),
        at(10..12),
        at(8..10),
        at(12..16),
        at(16..20),
        at(20..32),
    ))
}

/// `Player1`..`Player8`.
pub fn player_index(player: u32) -> Option<String> {
    (1..=MAX_PLAYERS)
        .contains(&player)
        .then(|| format!("Player{player}"))
}

/// input_config entry: GamepadInputId names.
pub fn input_config(player: u32, guid: &str, name: &str, ordinal: u32) -> Option<Value> {
    let id = device_id(guid, ordinal)?;
    let index = player_index(player)?;
    Some(json!({
        "left_joycon_stick": {
            "joystick": "Left",
            "invert_stick_x": false,
            "invert_stick_y": false,
            "rotate90_cw": false,
            "stick_button": "LeftStick"
        },
        "right_joycon_stick": {
            "joystick": "Right",
            "invert_stick_x": false,
            "invert_stick_y": false,
            "rotate90_cw": false,
            "stick_button": "RightStick"
        },
        "deadzone_left": 0.1,
        "deadzone_right": 0.1,
        "range_left": 1.0,
        "range_right": 1.0,
        "trigger_threshold": 0.5,
        "motion": {
            "motion_backend": "CemuHook",
            "sensitivity": 100,
            "gyro_deadzone": 1.0,
            "enable_motion": player <= crate::dsu::MAX_SLOTS as u32,
            "slot": player - 1,
            "alt_slot": player - 1,
            "mirror_input": false,
            "dsu_server_host": crate::dsu::HOST,
            "dsu_server_port": crate::dsu::PORT
        },
        "rumble": {
            "strong_rumble": 1.0,
            "weak_rumble": 1.0,
            "enable_rumble": false
        },
        "left_joycon": {
            "button_minus": "Minus",
            "button_l": "LeftShoulder",
            "button_zl": "LeftTrigger",
            "button_sl": "Unbound",
            "button_sr": "Unbound",
            "dpad_up": "DpadUp",
            "dpad_down": "DpadDown",
            "dpad_left": "DpadLeft",
            "dpad_right": "DpadRight"
        },
        "right_joycon": {
            "button_plus": "Plus",
            "button_r": "RightShoulder",
            "button_zr": "RightTrigger",
            "button_sl": "Unbound",
            "button_sr": "Unbound",
            "button_a": "B", // Switch labels mirrored; via SDL gamepad layer
            "button_b": "A",
            "button_x": "Y",
            "button_y": "X"
        },
        "version": 1,
        "backend": BACKEND,
        "id": id,
        "name": name,
        "controller_type": "ProController",
        "player_index": index
    }))
}

/// Ryujinx's own keyboard backend name.
pub const KEYBOARD_BACKEND: &str = "WindowKeyboard";

/// Ryujinx's own keyboard defaults (`ConfigurationState.LoadDefault`), on one player.
pub fn keyboard_entry(player: u32) -> Option<Value> {
    let index = player_index(player)?;
    Some(json!({
        "left_joycon_stick": {
            "stick_up": "W",
            "stick_down": "S",
            "stick_left": "A",
            "stick_right": "D",
            "stick_button": "F"
        },
        "right_joycon_stick": {
            "stick_up": "I",
            "stick_down": "K",
            "stick_left": "J",
            "stick_right": "L",
            "stick_button": "H"
        },
        "left_joycon": {
            "button_minus": "Minus",
            "button_l": "E",
            "button_zl": "Q",
            "button_sl": "Unbound",
            "button_sr": "Unbound",
            "dpad_up": "Up",
            "dpad_down": "Down",
            "dpad_left": "Left",
            "dpad_right": "Right"
        },
        "right_joycon": {
            "button_plus": "Plus",
            "button_r": "U",
            "button_zr": "O",
            "button_sl": "Unbound",
            "button_sr": "Unbound",
            "button_x": "C",
            "button_b": "X",
            "button_y": "V",
            "button_a": "Z"
        },
        "version": 1,
        "backend": KEYBOARD_BACKEND,
        "id": "0",
        "controller_type": "JoyconPair",
        "player_index": index
    }))
}

fn is_keyboard(entry: &Value) -> bool {
    entry.get("backend").and_then(Value::as_str) == Some(KEYBOARD_BACKEND)
}

fn index_of(entry: &Value) -> Option<u32> {
    entry
        .get("player_index")
        .and_then(Value::as_str)
        .and_then(|index| index.strip_prefix("Player"))
        .and_then(|number| number.parse().ok())
}

/// Merge padmap entries, keeping other controllers (match by player_index).
///
/// The keyboard is kept too, moved to the first port nobody holds: Ryujinx
/// seeds one on Player1, which is where padmap's first pad goes. The user's
/// own keyboard entry is the one moved when there is one, so their keys
/// survive; otherwise Ryujinx's defaults are written. One keyboard only.
pub fn merge(existing: &Value, ours: Vec<Value>, seat: Option<u32>) -> Value {
    let taken: Vec<&Value> = ours
        .iter()
        .filter_map(|entry| entry.get("player_index"))
        .collect();
    let (keyboards, others): (Vec<Value>, Vec<Value>) = existing
        .as_array()
        .map(|entries| entries.iter().cloned().partition(is_keyboard))
        .unwrap_or_default();
    let mut out: Vec<Value> = others
        .into_iter()
        .filter(|entry| !taken.contains(&entry.get("player_index").unwrap_or(&Value::Null)))
        .collect();
    out.extend(ours);

    let held: Vec<u32> = out.iter().filter_map(index_of).collect();
    if let Some(port) = crate::keyboard::port(seat, &held, MAX_PLAYERS) {
        let mut keyboard = keyboards
            .into_iter()
            .next()
            .or_else(|| keyboard_entry(port))
            .unwrap_or_else(|| json!({}));
        if let Some(index) = player_index(port) {
            keyboard["player_index"] = Value::String(index);
        }
        out.push(keyboard);
    }
    Value::Array(out)
}
