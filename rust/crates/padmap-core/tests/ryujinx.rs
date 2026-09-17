//! Ryujinx input entries, and the collision that shapes them.
//!
//! The device id is the interesting part. Ryujinx blanks the name CRC out of
//! the SDL GUID to make the id "stable", and that CRC is the only thing
//! distinguishing padmap's pads from each other -- so every player collapses
//! to one id and only SDL connection order separates them. That is measured
//! below rather than described, because it is the constraint the whole module
//! is built around.

use padmap_core::ryujinx;
use serde_json::Value;

/// padmap's own GUIDs for players 1..4, in padmap identity mode.
const PADMAP_GUIDS: [&str; 4] = [
    "0600c9a7091200000100000001000000",
    "060089a6091200000100000002000000",
    "06004866091200000100000003000000",
    "060009a4091200000100000004000000",
];

#[test]
fn the_id_is_the_guid_rearranged_with_the_crc_blanked() {
    // Worked through by hand from Ryujinx's GenerateGamepadId.
    assert_eq!(
        ryujinx::device_id("03002854de2800000413000002006800", 0).as_deref(),
        Some("0-00000003-28de-0000-0413-000002006800")
    );
    // The ordinal is a prefix, not part of the body.
    assert_eq!(
        ryujinx::device_id("03002854de2800000413000002006800", 2).as_deref(),
        Some("2-00000003-28de-0000-0413-000002006800")
    );
}

#[test]
fn every_padmap_pad_gets_its_own_id() {
    // It did not always. Ryujinx blanks the name CRC out of the GUID, and
    // that CRC was the only field distinguishing padmap's pads -- four
    // players collapsed to one id and the binding fell back to SDL
    // connection order.
    //
    // padmap is the abstraction layer, so it fixed that upstream rather than
    // documenting it: `clone::version_for` puts the player number in the
    // version field, which Ryujinx keeps and SDL's database matching ignores.
    // This is the test that says so.
    let ids: Vec<String> = PADMAP_GUIDS
        .iter()
        .map(|guid| ryujinx::device_id(guid, 0).expect("a valid guid"))
        .collect();
    let distinct: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        PADMAP_GUIDS.len(),
        "players share an id again: {ids:?}"
    );
    // And with no help from the ordinal, so the binding does not depend on
    // the order the clones happened to be created in.
    assert!(ids.iter().all(|id| id.starts_with("0-")), "{ids:?}");
}

#[test]
fn only_the_name_crc_is_blanked() {
    // The fix relies on the version surviving. If a future Ryujinx blanked
    // more of the GUID, the ids would collide again silently.
    let a = ryujinx::device_id("0600c9a7091200000100000001000000", 0).expect("valid");
    let b = ryujinx::device_id("0600ffff091200000100000001000000", 0).expect("valid");
    assert_eq!(a, b, "the CRC must not reach the id");

    let versioned = ryujinx::device_id("0600c9a7091200000100000002000000", 0).expect("valid");
    assert_ne!(a, versioned, "the version must reach the id");
}

#[test]
fn a_guid_that_is_not_one_is_refused_rather_than_sliced() {
    // The id is built by slicing fixed offsets, so a short string would panic
    // on a range that does not exist -- from a profile store the user can edit.
    for guid in [
        "",
        "abc",
        "0600c9a70912000001000000010000",
        "zz00c9a7091200000100000001000000",
    ] {
        assert_eq!(ryujinx::device_id(guid, 0), None, "{guid:?}");
    }
}

#[test]
fn a_and_b_are_mirrored_the_way_nintendo_labels_them() {
    let entry = ryujinx::input_config(1, PADMAP_GUIDS[0], "padmap Player 1", 0).expect("an entry");
    let right = &entry["right_joycon"];
    // Ryujinx reads through SDL's gamepad layer, and Switch A is SDL's East,
    // which SDL calls "B". The obvious pairing swaps A and B in every game.
    assert_eq!(right["button_a"], "B");
    assert_eq!(right["button_b"], "A");
    assert_eq!(right["button_x"], "Y");
    assert_eq!(right["button_y"], "X");
}

#[test]
fn an_entry_carries_what_ryujinx_needs_to_read_it() {
    // Ordinal zero: padmap's pads no longer collide, so nothing has to
    // disambiguate them.
    let entry = ryujinx::input_config(2, PADMAP_GUIDS[1], "padmap Player 2", 0).expect("an entry");
    assert_eq!(entry["backend"], "GamepadSDL2");
    assert_eq!(entry["version"], 1);
    assert_eq!(entry["player_index"], "Player2");
    assert_eq!(entry["controller_type"], "ProController");
    assert_eq!(entry["name"], "padmap Player 2");
    assert_eq!(entry["id"], "0-00000006-1209-0000-0100-000002000000");
    // The objects a gamepad entry has and a keyboard entry does not.
    for field in [
        "left_joycon_stick",
        "right_joycon_stick",
        "deadzone_left",
        "trigger_threshold",
        "motion",
        "rumble",
    ] {
        assert!(entry.get(field).is_some(), "{field} is missing");
    }
}

#[test]
fn only_eight_players_exist() {
    assert!(ryujinx::input_config(8, PADMAP_GUIDS[0], "x", 0).is_some());
    assert!(ryujinx::input_config(9, PADMAP_GUIDS[0], "x", 0).is_none());
    assert!(ryujinx::input_config(0, PADMAP_GUIDS[0], "x", 0).is_none());
}

#[test]
fn merging_keeps_entries_padmap_is_not_managing() {
    // A user's keyboard entry, or a controller padmap did not assign, is
    // theirs. Replacing the whole array would silently delete it.
    let existing: Value = serde_json::json!([
        {"backend": "WindowKeyboard", "player_index": "Player1", "name": "Keyboard"},
        {"backend": "GamepadSDL2", "player_index": "Player4", "name": "Someone else's pad"}
    ]);
    let ours =
        vec![ryujinx::input_config(1, PADMAP_GUIDS[0], "padmap Player 1", 0).expect("entry")];
    let merged = ryujinx::merge(&existing, ours);
    let entries = merged.as_array().expect("an array");

    assert_eq!(entries.len(), 2, "{merged}");
    // Player 1 is padmap's now.
    let one: Vec<&Value> = entries
        .iter()
        .filter(|e| e["player_index"] == "Player1")
        .collect();
    assert_eq!(one.len(), 1, "the keyboard entry was not replaced");
    assert_eq!(one[0]["name"], "padmap Player 1");
    // Player 4 is untouched.
    assert!(entries.iter().any(|e| e["name"] == "Someone else's pad"));
}

#[test]
fn merging_into_nothing_is_just_our_entries() {
    let merged = ryujinx::merge(
        &Value::Null,
        vec![ryujinx::input_config(1, PADMAP_GUIDS[0], "p1", 0).expect("entry")],
    );
    assert_eq!(merged.as_array().map(Vec::len), Some(1));
}

#[test]
fn motion_is_asked_of_padmap_rather_than_of_sdl() {
    // The gamepad driver would ask SDL, and SDL pairs a joystick with its
    // sensor by EVIOCGUNIQ -- which a uinput clone cannot set, so with two
    // players it hands out the wrong one. padmap answers instead.
    let config = ryujinx::input_config(2, PADMAP_GUIDS[1], "padmap Player 2", 0).expect("a config");
    let motion = &config["motion"];
    assert_eq!(motion["motion_backend"], "CemuHook");
    assert_eq!(motion["enable_motion"], true);
    assert_eq!(motion["dsu_server_host"], padmap_core::dsu::HOST);
    assert_eq!(motion["dsu_server_port"], padmap_core::dsu::PORT);
}

#[test]
fn player_one_takes_slot_zero() {
    // DSU slots are zero-based and players are one-based. Off by one here and
    // every player gets the next player's gyro, silently.
    for player in 1..=4u32 {
        let config = ryujinx::input_config(player, PADMAP_GUIDS[0], "padmap", 0).expect("a config");
        assert_eq!(config["motion"]["slot"], player - 1);
        assert_eq!(config["motion"]["alt_slot"], player - 1);
    }
}

#[test]
fn the_backend_name_is_one_ryujinx_will_accept() {
    // Its JSON converter throws on an unrecognised `motion_backend` rather
    // than falling back, so a misspelling is a config file Ryujinx refuses to
    // load at all -- not a controller with no gyro.
    let config = ryujinx::input_config(1, PADMAP_GUIDS[0], "padmap", 0).expect("a config");
    assert!(["CemuHook", "GamepadDriver"].contains(
        &config["motion"]["motion_backend"]
            .as_str()
            .expect("a string")
    ));
}

#[test]
fn a_player_beyond_the_four_dsu_slots_has_motion_off() {
    // Slot 4 and up never receive a sample; leaving motion enabled there is a
    // gyro option that silently does nothing.
    for player in 5..=8u32 {
        let config = ryujinx::input_config(player, PADMAP_GUIDS[0], "padmap", 0).expect("a config");
        assert_eq!(config["motion"]["enable_motion"], false, "player {player}");
    }
    let config = ryujinx::input_config(4, PADMAP_GUIDS[0], "padmap", 0).expect("a config");
    assert_eq!(config["motion"]["enable_motion"], true);
}
