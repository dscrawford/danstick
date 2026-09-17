//! The Cemu profile, checked against a `controller0.xml` Cemu itself wrote.

use std::collections::BTreeMap;
use std::path::Path;

use padmap_core::cemu;

const GUID: &str = "0600c9a7091200000100000001000000";

/// `<mapping>` -> `<button>` from an XML profile, however it is laid out.
fn pairs(xml: &str) -> BTreeMap<u32, u32> {
    let mut out = BTreeMap::new();
    let mut rest = xml;
    while let Some(at) = rest.find("<mapping>") {
        let after = &rest[at + "<mapping>".len()..];
        let Some(end) = after.find("</mapping>") else {
            break;
        };
        let mapping: u32 = after[..end].trim().parse().expect("a mapping id");
        let tail = &after[end..];
        let Some(bat) = tail.find("<button>") else {
            break;
        };
        let after_button = &tail[bat + "<button>".len()..];
        let Some(bend) = after_button.find("</button>") else {
            break;
        };
        let button: u32 = after_button[..bend].trim().parse().expect("a button id");
        out.insert(mapping, button);
        rest = &after_button[bend..];
    }
    out
}

fn cemus_own() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/cemu_controller0.xml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[test]
fn every_binding_matches_the_one_cemu_wrote_itself() {
    let theirs = pairs(&cemus_own());
    let ours = pairs(&cemu::profile(2, GUID, "padmap Player 1"));
    assert!(!theirs.is_empty(), "the reference profile has no mappings");
    for (mapping, button) in &theirs {
        assert_eq!(
            ours.get(mapping),
            Some(button),
            "Wii U control {mapping} should bind SDL id {button}"
        );
    }
    // Cemu left Home (11) unbound; padmap adds nothing Cemu did not.
    assert_eq!(
        ours.len(),
        theirs.len(),
        "ours {:?} vs Cemu's {:?}",
        ours.keys(),
        theirs.keys()
    );
}

#[test]
fn a_and_b_are_mirrored_the_way_nintendo_labels_them() {
    let ours = pairs(&cemu::profile(2, "0", "x"));
    assert_eq!(ours[&(cemu::WiiU::A as u32)], 1, "A is SDL East");
    assert_eq!(ours[&(cemu::WiiU::B as u32)], 0, "B is SDL South");
    assert_eq!(ours[&(cemu::WiiU::X as u32)], 3, "X is SDL North");
    assert_eq!(ours[&(cemu::WiiU::Y as u32)], 2, "Y is SDL West");
}

#[test]
fn the_uuid_carries_the_guid_with_the_ordinal_prefix() {
    // The prefix is the ordinal among devices sharing a GUID; padmap's GUIDs never collide.
    let xml = cemu::profile(1, GUID, "padmap Player 1");
    assert!(xml.contains(&format!("<uuid>0_{GUID}</uuid>")), "{xml}");
    assert!(xml.contains("<api>SDLController</api>"));
    assert!(xml.contains("<display_name>padmap Player 1</display_name>"));
}

#[test]
fn the_type_is_the_one_the_mapping_table_is_for() {
    // VPADController numbers the same controls differently (Home is 27 there, not 11).
    assert!(cemu::profile(2, "0", "x").contains("<type>Wii U Pro Controller</type>"));
    assert!(cemu::profile(1, "0", "x").contains("<type>Wii U GamePad</type>"));
}

#[test]
fn a_name_with_xml_in_it_cannot_break_the_file() {
    let xml = cemu::profile(1, "0", "Pad & \"quoted\" <thing>");
    assert!(
        xml.contains("Pad &amp; &quot;quoted&quot; &lt;thing&gt;"),
        "{xml}"
    );
    assert!(!xml.contains("<thing>"));
}

#[test]
fn the_filename_counts_from_zero_where_padmap_counts_from_one() {
    assert_eq!(cemu::profile_filename(1), "controller0.xml");
    assert_eq!(cemu::profile_filename(8), "controller7.xml");
    assert_eq!(cemu::MAX_PLAYERS, 8);
    assert_eq!(cemu::profile_filename(0), "controller0.xml", "no underflow");
}

#[test]
fn the_profile_is_well_formed_enough_for_cemu_to_read() {
    let xml = cemu::profile(1, GUID, "padmap Player 1");
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(xml.trim_end().ends_with("</emulated_controller>"));
    for tag in [
        "emulated_controller",
        "controller",
        "mappings",
        "axis",
        "rotation",
        "trigger",
    ] {
        assert_eq!(
            xml.matches(&format!("<{tag}>")).count(),
            xml.matches(&format!("</{tag}>")).count(),
            "unbalanced <{tag}>"
        );
    }
}

#[test]
fn player_one_is_a_gamepad_because_that_is_the_one_with_motion() {
    assert_eq!(cemu::emulated_for(1), cemu::Emulated::GamePad);
    for player in 2..=8u32 {
        assert_eq!(cemu::emulated_for(player), cemu::Emulated::Pro);
    }
}

#[test]
fn the_gamepad_numbers_its_controls_one_lower_from_the_dpad_on() {
    assert_eq!(cemu::GamePad::Minus as u8, cemu::WiiU::Minus as u8);
    assert_eq!(cemu::GamePad::Up as u8, cemu::WiiU::Up as u8 - 1);
    assert_eq!(
        cemu::GamePad::StickRRight as u8,
        cemu::WiiU::StickRRight as u8 - 1
    );
}

#[test]
fn both_tables_bind_the_same_controls_to_the_same_sdl_ids() {
    let pro: Vec<u8> = cemu::MAPPING.iter().map(|(_, sdl)| *sdl).collect();
    let pad: Vec<u8> = cemu::GAMEPAD_MAPPING.iter().map(|(_, sdl)| *sdl).collect();
    assert_eq!(pro, pad);
}

#[test]
fn motion_arrives_as_a_second_controller_on_the_same_profile() {
    let xml = cemu::profile(1, GUID, "padmap Player 1");
    assert_eq!(xml.matches("<controller>").count(), 2, "{xml}");
    assert!(xml.contains("<api>DSUController</api>"), "{xml}");
    assert!(xml.contains("<motion>true</motion>"), "{xml}");
    assert!(xml.contains("<motion>false</motion>"), "the SDL entry");
    assert!(xml.contains(&format!("<port>{}</port>", padmap_core::dsu::PORT)));
    assert!(xml.contains(&format!("<ip>{}</ip>", padmap_core::dsu::HOST)));
}

#[test]
fn the_motion_uuid_is_a_bare_slot_number() {
    for player in 1..=4u32 {
        let xml = cemu::profile(player, "0", "padmap");
        assert!(
            xml.contains(&format!("<uuid>{}</uuid>", player - 1)),
            "player {player}: {xml}"
        );
    }
}

#[test]
fn the_motion_entry_binds_no_buttons() {
    // Mappings on the DSU entry would deliver every press twice.
    let xml = cemu::profile(1, "0", "padmap");
    let after = xml
        .split("<api>DSUController</api>")
        .nth(1)
        .expect("a DSU entry");
    assert!(!after.contains("<mapping>"), "{after}");
}

#[test]
fn a_player_beyond_the_four_dsu_slots_gets_no_motion_entry() {
    for player in 5..=8u32 {
        let xml = cemu::profile(player, "0", "padmap");
        assert!(!xml.contains("DSUController"), "player {player}: {xml}");
        assert_eq!(xml.matches("<controller>").count(), 1);
    }
    assert!(cemu::profile(4, "0", "padmap").contains("DSUController"));
}
