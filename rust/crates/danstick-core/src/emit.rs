//! Output formats: the SDL gamecontroller database and its environment variable.

use std::collections::BTreeMap;

use crate::binding::Binding;
use crate::control::Control;
use crate::fields::Fields;
use crate::sdl;

pub const VIRTUAL_PREFIX: &str = "danstick Player ";
pub const MARKER: &str = "# danstick";
pub const MAX_PLAYERS: u32 = 16;

pub fn virtual_name(player: u32) -> String {
    format!("{VIRTUAL_PREFIX}{player}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub bustype: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
}

pub const DANSTICK_VERSION: u16 = 0x0001;

/// Version per player for unique GUID: SDL matches versions; Ryujinx blanks checksum.
pub fn version_for(player: u32) -> u16 {
    if player == 0 {
        DANSTICK_VERSION
    } else {
        player.min(u32::from(u16::MAX)) as u16
    }
}

pub fn virtual_guid(player: u32, identity: Identity) -> String {
    sdl::guid(
        identity.bustype,
        identity.vendor,
        identity.product,
        identity.version,
        &virtual_name(player),
    )
}

pub fn sdl_line_for(
    player: u32,
    identity: Identity,
    bindings: &BTreeMap<Control, Binding>,
    sticks: Option<&Fields>,
) -> String {
    sdl::mapping_line(
        &virtual_guid(player, identity),
        &virtual_name(player),
        bindings,
        "Linux",
        sticks,
    )
}

/// SDL line from fields directly (used when no capture available).
pub fn sdl_line(guid: &str, name: &str, fields: &Fields) -> String {
    sdl::line(guid, name, fields, "Linux")
}

/// Rewrite SDL database, replacing danstick's lines; keep user-mapped ones.
pub fn rewrite_sdl_database(
    existing: &str,
    lines: &BTreeMap<u32, String>,
    notes: &BTreeMap<u32, String>,
    identity_for: impl Fn(u32) -> Identity,
) -> String {
    let ours: Vec<String> = (1..=MAX_PLAYERS)
        .map(|player| virtual_guid(player, identity_for(player)))
        .collect();

    let mut kept: Vec<&str> = Vec::new();
    for line in existing.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with(MARKER) {
            continue;
        }
        let fields: Vec<&str> = stripped.split(',').collect();
        if ours.iter().any(|guid| guid == fields[0]) {
            continue;
        }
        if fields.len() > 1 && fields[1].starts_with(VIRTUAL_PREFIX) {
            continue;
        }
        kept.push(line);
    }

    let mut body: Vec<String> = kept.into_iter().map(str::to_owned).collect();
    body.push(format!(
        "{MARKER} -- regenerated on every controller assignment"
    ));
    for (player, line) in lines {
        if let Some(note) = notes.get(player) {
            body.push(format!("{MARKER}: player {player} -- {note}"));
        }
        body.push(line.clone());
    }
    body.join("\n") + "\n"
}

/// Environment variable for mappings; SDL reads it before other sources.
pub const SDL_CONFIG_ENV: &str = "SDL_GAMECONTROLLERCONFIG";

/// All mappings as newline-separated value for SDL_GAMECONTROLLERCONFIG.
pub fn sdl_config_value(lines: &BTreeMap<u32, String>) -> String {
    lines
        .values()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIRRORED: Identity = Identity {
        bustype: 0x03,
        vendor: 0x0079,
        product: 0x1843,
        version: 0x0110,
    };
    const DANSTICK: Identity = Identity {
        bustype: 0x06,
        vendor: 0x1209,
        product: 0x0001,
        version: 0x0001,
    };

    #[test]
    fn a_virtual_pad_guid_carries_its_name_and_ids() {
        // SDL wrote 0600c9a7... for "padmap Player 1"; the CRC is the name's.
        assert_eq!(
            virtual_guid(1, DANSTICK),
            "0600724d091200000100000001000000"
        );
    }

    #[test]
    fn the_guid_depends_on_the_identity_the_pad_advertises() {
        assert_ne!(virtual_guid(1, MIRRORED), virtual_guid(1, DANSTICK));
    }

    #[test]
    fn each_player_gets_its_own_guid() {
        let guids: Vec<String> = (1..=4).map(|p| virtual_guid(p, DANSTICK)).collect();
        let mut unique = guids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 4, "two slots sharing a GUID collide in SDL");
    }

    #[test]
    fn a_rewrite_keeps_lines_danstick_does_not_own() {
        let existing = "030000005e040000e002000000000000,Someone's Xbox pad,a:b0,\n";
        let out = rewrite_sdl_database(existing, &BTreeMap::new(), &BTreeMap::new(), |_| DANSTICK);
        assert!(out.contains("Someone's Xbox pad"));
    }

    #[test]
    fn a_rewrite_drops_a_previous_generation_by_guid() {
        let stale = format!("{},danstick Player 1,a:b9,\n", virtual_guid(1, DANSTICK));
        let out = rewrite_sdl_database(&stale, &BTreeMap::new(), &BTreeMap::new(), |_| DANSTICK);
        assert!(!out.contains("a:b9"));
    }

    #[test]
    fn a_rewrite_drops_our_own_name_under_a_guid_we_cannot_recompute() {
        let stale = "0300ffff0000000000000000000000ff,danstick Player 3,a:b9,\n";
        let out = rewrite_sdl_database(stale, &BTreeMap::new(), &BTreeMap::new(), |_| DANSTICK);
        assert!(!out.contains("a:b9"), "{out}");
    }

    #[test]
    fn a_rewrite_drops_a_stale_slot_no_longer_being_written() {
        let stale = format!("{},danstick Player 9,a:b9,\n", virtual_guid(9, DANSTICK));
        let lines: BTreeMap<u32, String> = [(1, "aaa,danstick Player 1,a:b0,".to_owned())]
            .into_iter()
            .collect();
        let out = rewrite_sdl_database(&stale, &lines, &BTreeMap::new(), |_| DANSTICK);
        assert!(!out.contains("Player 9"));
        assert!(out.contains("Player 1"));
    }

    #[test]
    fn dansticks_own_comments_do_not_accumulate_across_rewrites() {
        let lines: BTreeMap<u32, String> = [(1, "aaa,danstick Player 1,a:b0,".to_owned())]
            .into_iter()
            .collect();
        let notes: BTreeMap<u32, String> =
            [(1, "carried from SDL".to_owned())].into_iter().collect();
        let once = rewrite_sdl_database("", &lines, &notes, |_| DANSTICK);
        let twice = rewrite_sdl_database(&once, &lines, &notes, |_| DANSTICK);
        assert_eq!(once, twice, "a rewrite must be idempotent");
        assert_eq!(twice.matches(MARKER).count(), 2);
    }

    #[test]
    fn blank_lines_are_not_preserved_into_the_rewrite() {
        let out = rewrite_sdl_database("\n\n   \n", &BTreeMap::new(), &BTreeMap::new(), |_| {
            DANSTICK
        });
        assert_eq!(out.lines().filter(|l| l.trim().is_empty()).count(), 0);
    }

    #[test]
    fn an_empty_database_still_gets_our_lines() {
        let lines: BTreeMap<u32, String> = [(1, "aaa,danstick Player 1,a:b0,".to_owned())]
            .into_iter()
            .collect();
        let out = rewrite_sdl_database("", &lines, &BTreeMap::new(), |_| DANSTICK);
        assert!(out.contains("danstick Player 1"));
        assert!(out.ends_with('\n'));
    }
}
