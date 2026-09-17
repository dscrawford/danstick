//! Ryujinx input configuration.
//!
//! An entry per player in `input_config` in `~/.config/Ryujinx/Config.json`.
//! The button fields are `GamepadInputId` names, so the table is a constant
//! for the same reason Cemu's is: padmap's clone is already presented to SDL
//! as a standard gamepad, and Ryujinx reads it through SDL's gamepad layer.
//!
//! # The device id used to collide, and padmap fixes it upstream
//!
//! Ryujinx builds its device id from the SDL GUID and then **blanks the name
//! CRC** -- its own comment says "Remove the first 4 char of the guid (CRC
//! part) to make it stable". Those four characters are the *only* thing that
//! differs between padmap's pads: every player shares a vendor, product and
//! bus, and differs only in the CRC of `padmap Player N`.
//!
//! Measured on four players:
//!
//! ```text
//! 0600c9a7091200000100000001000000  ->  0-00000006-1209-0000-0100-000001000000
//! 060089a6091200000100000001000000  ->  0-00000006-1209-0000-0100-000001000000
//! 06004866091200000100000001000000  ->  0-00000006-1209-0000-0100-000001000000
//! 060009a4091200000100000001000000  ->  0-00000006-1209-0000-0100-000001000000
//! ```
//!
//! Four distinct GUIDs, one id -- leaving the binding to depend on SDL
//! connection order, via the `n-` prefix.
//!
//! padmap is the abstraction layer, so this is padmap's problem rather than
//! Ryujinx's fault. `clone::version_for` now puts the **player number in the
//! GUID's version field**, which Ryujinx preserves and SDL's own database
//! matching ignores -- measured: two pads identical but for their version got
//! distinct GUIDs and both still matched "Xbox 360 Controller" from SDL's
//! built-in database. Every player now has a distinct id:
//!
//! ```text
//! 0600c9a7091200000100000001000000  ->  0-00000006-1209-0000-0100-000001000000
//! 060089a6091200000100000002000000  ->  0-00000006-1209-0000-0100-000002000000
//! ```
//!
//! The ordinal remains an argument because it is Ryujinx's to assign when two
//! devices really do collide -- two identical physical pads in mirror mode
//! still can -- but for padmap's own pads it is always zero.
//!
//! # Motion comes from padmap, not from the gamepad driver
//!
//! `motion_backend` is `CemuHook`, pointed at padmap's own DSU server, rather
//! than `GamepadDriver`. The gamepad driver would ask SDL, and SDL pairs a
//! joystick with its sensor by comparing `EVIOCGUNIQ` -- which a uinput clone
//! cannot set. See [`crate::dsu`]. The slot is the player number less one,
//! which is the off-by-one that gives player one player two's gyro.

use serde_json::{json, Value};

/// Ryujinx allows eight.
pub const MAX_PLAYERS: u32 = 8;

/// The backend name for an SDL gamepad.
///
/// Current Ryujinx writes `GamepadSDL3`, but both are accepted on read and
/// older builds know only this one -- so this is the spelling that works
/// everywhere.
pub const BACKEND: &str = "GamepadSDL2";

/// The id Ryujinx will generate for a pad with this SDL GUID.
///
/// `ordinal` is the pad's position among devices whose id would otherwise be
/// identical, in SDL connection order. See the module note: for padmap that is
/// every pad, so this is the player's creation order rather than a property of
/// the device.
pub fn device_id(guid: &str, ordinal: u32) -> Option<String> {
    if guid.len() != 32 || !guid.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let at = |range: std::ops::Range<usize>| &guid[range];
    // The SDL GUID's bytes re-ordered into .NET Guid text layout, with the
    // first four hex characters -- the name CRC -- replaced by zeros.
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

/// One `input_config` entry.
///
/// The button names are `GamepadInputId` values, serialised as the C#
/// identifier verbatim. `Back` and `Start` are aliases of `Minus` and `Plus`
/// and are written as the latter, which is what Ryujinx itself writes.
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
            "enable_motion": true,
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
            // Switch labels are mirrored against everyone else's, and Ryujinx
            // reads through SDL's gamepad layer -- so Switch A is SDL's East,
            // which SDL calls "B". Writing the obvious pairing swaps A and B
            // in every game.
            "button_a": "B",
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

/// Replace padmap's entries in an existing `input_config`, keeping the rest.
///
/// A user's keyboard entry, or a controller padmap is not managing, is theirs.
/// Matching is by `player_index`: a slot padmap is binding is replaced, and
/// every other entry is left exactly as it was.
pub fn merge(existing: &Value, ours: Vec<Value>) -> Value {
    let taken: Vec<&Value> = ours
        .iter()
        .filter_map(|entry| entry.get("player_index"))
        .collect();
    let mut out: Vec<Value> = existing
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| !taken.contains(&entry.get("player_index").unwrap_or(&Value::Null)))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    out.extend(ours);
    Value::Array(out)
}
