//! Ryujinx input entries.

use padmap_core::ryujinx;
use serde_json::{json, Value};

/// padmap's own GUIDs for players 1..4.
const PADMAP_GUIDS: [&str; 4] = [
    "0600c9a7091200000100000001000000",
    "060089a6091200000100000002000000",
    "06004866091200000100000003000000",
    "060009a4091200000100000004000000",
];

fn config(player: u32, guid: &str) -> Value {
    ryujinx::input_config(player, guid, "padmap", 0).expect("a config")
}

#[test]
fn the_id_is_the_guid_rearranged_with_the_crc_blanked() {
    let guid = "03002854de2800000413000002006800";
    assert_eq!(
        ryujinx::device_id(guid, 0).as_deref(),
        Some("0-00000003-28de-0000-0413-000002006800")
    );
    assert_eq!(
        ryujinx::device_id(guid, 2).as_deref(),
        Some("2-00000003-28de-0000-0413-000002006800")
    );
}

#[test]
fn every_padmap_pad_gets_its_own_id() {
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
    assert!(
        ids.iter().all(|id| id.starts_with("0-")),
        "no help from the ordinal: {ids:?}"
    );
}

#[test]
fn only_the_name_crc_is_blanked() {
    let a = ryujinx::device_id("0600c9a7091200000100000001000000", 0).expect("valid");
    let b = ryujinx::device_id("0600ffff091200000100000001000000", 0).expect("valid");
    assert_eq!(a, b, "the CRC must not reach the id");
    let versioned = ryujinx::device_id("0600c9a7091200000100000002000000", 0).expect("valid");
    assert_ne!(a, versioned, "the version must reach the id");
}

#[test]
fn a_guid_that_is_not_one_is_refused_rather_than_sliced() {
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
    let right = &config(1, PADMAP_GUIDS[0])["right_joycon"];
    assert_eq!(right["button_a"], "B");
    assert_eq!(right["button_b"], "A");
    assert_eq!(right["button_x"], "Y");
    assert_eq!(right["button_y"], "X");
}

#[test]
fn an_entry_carries_what_ryujinx_needs_to_read_it() {
    let entry = ryujinx::input_config(2, PADMAP_GUIDS[1], "padmap Player 2", 0).expect("an entry");
    assert_eq!(entry["backend"], "GamepadSDL2");
    assert_eq!(entry["version"], 1);
    assert_eq!(entry["player_index"], "Player2");
    assert_eq!(entry["controller_type"], "ProController");
    assert_eq!(entry["name"], "padmap Player 2");
    assert_eq!(entry["id"], "0-00000006-1209-0000-0100-000002000000");
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
    let existing = json!([
        {"backend": "WindowKeyboard", "player_index": "Player1", "name": "Keyboard"},
        {"backend": "GamepadSDL2", "player_index": "Player4", "name": "Someone else's pad"}
    ]);
    let ours =
        vec![ryujinx::input_config(1, PADMAP_GUIDS[0], "padmap Player 1", 0).expect("entry")];
    let merged = ryujinx::merge(&existing, ours);
    let entries = merged.as_array().expect("an array");
    assert_eq!(entries.len(), 2, "{merged}");
    let one: Vec<&Value> = entries
        .iter()
        .filter(|e| e["player_index"] == "Player1")
        .collect();
    assert_eq!(one.len(), 1, "the keyboard entry was not replaced");
    assert_eq!(one[0]["name"], "padmap Player 1");
    assert!(entries.iter().any(|e| e["name"] == "Someone else's pad"));
}

#[test]
fn merging_into_nothing_is_just_our_entries() {
    let merged = ryujinx::merge(&Value::Null, vec![config(1, PADMAP_GUIDS[0])]);
    assert_eq!(merged.as_array().map(Vec::len), Some(1));
}

#[test]
fn motion_is_asked_of_padmap_rather_than_of_sdl() {
    let motion = &config(2, PADMAP_GUIDS[1])["motion"];
    assert_eq!(motion["motion_backend"], "CemuHook");
    assert_eq!(motion["enable_motion"], true);
    assert_eq!(motion["dsu_server_host"], padmap_core::dsu::HOST);
    assert_eq!(motion["dsu_server_port"], padmap_core::dsu::PORT);
}

#[test]
fn player_one_takes_slot_zero() {
    for player in 1..=4u32 {
        let motion = &config(player, PADMAP_GUIDS[0])["motion"];
        assert_eq!(motion["slot"], player - 1);
        assert_eq!(motion["alt_slot"], player - 1);
    }
}

#[test]
fn the_backend_name_is_one_ryujinx_will_accept() {
    let backend = config(1, PADMAP_GUIDS[0])["motion"]["motion_backend"]
        .as_str()
        .expect("a string")
        .to_owned();
    assert!(["CemuHook", "GamepadDriver"].contains(&backend.as_str()));
}

#[test]
fn a_player_beyond_the_four_dsu_slots_has_motion_off() {
    for player in 5..=8u32 {
        assert_eq!(
            config(player, PADMAP_GUIDS[0])["motion"]["enable_motion"],
            false,
            "player {player}"
        );
    }
    assert_eq!(config(4, PADMAP_GUIDS[0])["motion"]["enable_motion"], true);
}
