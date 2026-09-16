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
    "060089a6091200000100000001000000",
    "06004866091200000100000001000000",
    "060009a4091200000100000001000000",
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
fn every_padmap_pad_collapses_to_the_same_id() {
    // Not a bug being tested for -- a constraint being pinned. If a future
    // change to padmap's identity makes these distinct, this test fails and
    // whoever changed it can delete the ordinal plumbing that exists only
    // because of this.
    let ids: Vec<String> = PADMAP_GUIDS
        .iter()
        .map(|guid| ryujinx::device_id(guid, 0).expect("a valid guid"))
        .collect();
    let distinct: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(PADMAP_GUIDS.len(), 4, "four distinct GUIDs going in");
    assert_eq!(
        distinct.len(),
        1,
        "and they no longer collapse to one Ryujinx id: {distinct:?}"
    );

    // Which is why the ordinal has to do the separating.
    let separated: std::collections::BTreeSet<String> = PADMAP_GUIDS
        .iter()
        .enumerate()
        .map(|(at, guid)| ryujinx::device_id(guid, at as u32).expect("valid"))
        .collect();
    assert_eq!(separated.len(), 4);
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
    let entry = ryujinx::input_config(2, PADMAP_GUIDS[1], "padmap Player 2", 1).expect("an entry");
    assert_eq!(entry["backend"], "GamepadSDL2");
    assert_eq!(entry["version"], 1);
    assert_eq!(entry["player_index"], "Player2");
    assert_eq!(entry["controller_type"], "ProController");
    assert_eq!(entry["name"], "padmap Player 2");
    assert_eq!(entry["id"], "1-00000006-1209-0000-0100-000001000000");
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
