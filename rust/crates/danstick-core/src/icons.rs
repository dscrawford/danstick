//! Map controller vendor/model, name, or profile to an icon.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;

/// Icon names, each backed by `<name>.svg` under `assets/icons`.
pub const ARCADE: &str = "arcade";
pub const N64: &str = "n64";
pub const GAMECUBE: &str = "gamecube";
pub const SNES: &str = "snes";
pub const SWITCH: &str = "switch";
pub const GENESIS: &str = "genesis";
pub const PLAYSTATION: &str = "playstation";
pub const XBOX: &str = "xbox";
pub const STEAM: &str = "steam";
pub const WHEEL: &str = "wheel";
/// The fallback.
pub const GAMEPAD: &str = "gamepad";

pub const ICON_NAMES: [&str; 11] = [
    ARCADE,
    N64,
    GAMECUBE,
    SNES,
    SWITCH,
    GENESIS,
    PLAYSTATION,
    XBOX,
    STEAM,
    WHEEL,
    GAMEPAD,
];

/// Steam's virtual gamepad: checked by id before name (it impersonates Xbox).
pub const STEAM_VIRTUAL_ID: (u16, u16) = (0x28DE, 0x11FF);

/// Fallback only, and treated as a guess: these ids are not reliable identity.
const BY_ID: [((u16, u16), &str); 14] = [
    ((0x0079, 0x1830), ARCADE),      // MAYFLASH Arcade Fightstick F300
    ((0x0079, 0x1843), GAMECUBE),    // Mayflash GameCube adapter
    ((0x057E, 0x0337), GAMECUBE),    // Nintendo official GC adapter
    ((0x054C, 0x0268), PLAYSTATION), // DualShock 3
    ((0x054C, 0x05C4), PLAYSTATION), // DualShock 4
    ((0x054C, 0x09CC), PLAYSTATION), // DualShock 4 v2
    ((0x054C, 0x0CE6), PLAYSTATION), // DualSense
    ((0x045E, 0x028E), XBOX),        // Xbox 360 pad
    ((0x045E, 0x02FD), XBOX),        // Xbox One S pad
    ((0x057E, 0x2009), SWITCH),      // Switch Pro
    ((0x057E, 0x2017), SNES),        // SNES pad for Switch Online (layout of original console)
    ((0x057E, 0x2019), N64),         // N64 pad for Switch Online
    ((0x28DE, 0x1205), STEAM),       // Steam Deck, built-in controls
    ((0x28DE, 0x1304), STEAM),       // Steam Controller Puck
];

/// Ordered: first match wins, against the lowercased name.
const BY_NAME: [(&str, &str); 9] = [
    (
        r"fight ?stick|arcade|joystick|street ?fighter|qanba|hori.*stick",
        ARCADE,
    ),
    (r"\bn64\b|nintendo 64|retrolink.*64", N64),
    (r"gamecube|\bgc\b|wii ?u? ?gc", GAMECUBE),
    (r"\bsnes\b|super nintendo|\bsfc\b", SNES),
    (r"pro controller|switch pro|joy-?con|\bnso\b", SWITCH),
    (r"genesis|mega ?drive|\bm30\b|retro-?bit|saturn", GENESIS),
    (r"dualshock|dualsense|playstation|\bps[3-5]\b", PLAYSTATION),
    (r"steam ?(controller|deck|puck)|\bvalve\b", STEAM),
    (r"x-? ?box|xinput", XBOX),
];

const WHEEL_PATTERN: &str = r"wheel|racing|g29|g27|driving";

fn compiled() -> &'static Vec<(Regex, &'static str)> {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    RULES.get_or_init(|| {
        BY_NAME
            .iter()
            .copied()
            .chain(std::iter::once((WHEEL_PATTERN, WHEEL)))
            .map(|(pattern, icon)| {
                (
                    Regex::new(pattern).unwrap_or_else(|e| panic!("{pattern}: {e}")),
                    icon,
                )
            })
            .collect()
    })
}

/// Whether a name is one danstick can draw.
pub fn known(name: &str) -> bool {
    ICON_NAMES.contains(&name)
}

/// The icon for a pad (profile > override > Steam id > name > id table > fallback).
pub fn for_pad(
    vid: u16,
    pid: u16,
    name: &str,
    profile_icon: Option<&str>,
    overrides: &BTreeMap<String, String>,
) -> &'static str {
    if let Some(icon) = profile_icon.filter(|icon| !icon.is_empty()) {
        if let Some(known) = ICON_NAMES.iter().find(|candidate| **candidate == icon) {
            return known;
        }
    }
    let key = format!("{vid:04x}:{pid:04x}");
    if let Some(chosen) = overrides.get(&key) {
        if let Some(known) = ICON_NAMES.iter().find(|candidate| *candidate == chosen) {
            return known;
        }
    }
    if (vid, pid) == STEAM_VIRTUAL_ID {
        return STEAM;
    }
    let lowered = name.to_lowercase();
    for (pattern, icon) in compiled() {
        if pattern.is_match(&lowered) {
            return icon;
        }
    }
    for ((candidate_vid, candidate_pid), icon) in BY_ID {
        if (candidate_vid, candidate_pid) == (vid, pid) {
            return icon;
        }
    }
    GAMEPAD
}

/// Parse the user's override file (JSON).
pub fn parse_overrides(text: &str) -> BTreeMap<String, String> {
    let Ok(serde_json::Value::Object(raw)) = serde_json::from_str(text) else {
        return BTreeMap::new();
    };
    raw.into_iter()
        .filter_map(|(key, value)| {
            let icon = value.as_str()?;
            known(icon).then(|| (key.to_lowercase(), icon.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(name: &str) -> &'static str {
        for_pad(0x1234, 0x5678, name, None, &BTreeMap::new())
    }

    #[test]
    fn every_rule_names_an_icon_that_exists() {
        for (_, name) in BY_NAME
            .iter()
            .chain(std::iter::once(&(WHEEL_PATTERN, WHEEL)))
        {
            assert!(known(name), "{name} is not an icon");
        }
        for (_, name) in BY_ID {
            assert!(known(name), "{name} is not an icon");
        }
    }

    #[test]
    fn every_pattern_compiles() {
        assert_eq!(compiled().len(), BY_NAME.len() + 1);
    }

    #[test]
    fn the_kernels_own_name_for_an_xbox_pad_is_recognised() {
        assert_eq!(icon("Microsoft X-Box 360 pad 0"), XBOX);
        assert_eq!(icon("Microsoft X-Box One pad"), XBOX);
        assert_eq!(icon("Xbox Wireless Controller"), XBOX);
        assert_eq!(icon("xinput device"), XBOX);
    }

    #[test]
    fn steams_virtual_pad_is_not_believed_when_it_says_it_is_an_xbox() {
        let (vid, pid) = STEAM_VIRTUAL_ID;
        assert_eq!(
            for_pad(
                vid,
                pid,
                "Microsoft X-Box 360 pad 0",
                None,
                &BTreeMap::new()
            ),
            STEAM
        );
        assert_eq!(
            for_pad(
                0x045E,
                0x028E,
                "Microsoft X-Box 360 pad 0",
                None,
                &BTreeMap::new()
            ),
            XBOX
        );
    }

    #[test]
    fn a_decks_built_in_controls_are_valve_hardware_by_id_and_by_name() {
        // hid-steam names the node "Steam Deck"; the lizard nodes beside it
        // say only "Valve Software Steam Controller".
        let deck = |name: &str| for_pad(0x28DE, 0x1205, name, None, &BTreeMap::new());
        assert_eq!(deck("Steam Deck"), STEAM);
        assert_eq!(deck(""), STEAM, "the id alone is enough");
        assert_eq!(deck("Valve Software Steam Controller"), STEAM);
        // Not to be confused with Steam's mirror, which wears an Xbox name.
        assert_eq!(
            for_pad(
                0x28DE,
                0x11FF,
                "Microsoft X-Box 360 pad 0",
                None,
                &BTreeMap::new()
            ),
            STEAM
        );
    }

    #[test]
    fn valves_controllers_are_recognised_by_name() {
        for name in [
            "Steam Controller",
            "Steam Deck",
            "Valve Software Steam Controller Puck",
        ] {
            assert_eq!(icon(name), STEAM, "{name}");
        }
    }

    #[test]
    fn a_more_specific_rule_wins_over_a_vaguer_one() {
        assert_eq!(icon("Steampunk Arcade Fightstick"), ARCADE);
        assert_eq!(icon("Nintendo Co., Ltd. N64 Controller"), N64);
    }

    #[test]
    fn a_profile_beats_everything_below_it() {
        let icon = for_pad(
            0x045E,
            0x028E,
            "Microsoft X-Box 360 pad",
            Some("n64"),
            &BTreeMap::new(),
        );
        assert_eq!(icon, N64);
    }

    #[test]
    fn a_profile_naming_an_icon_that_does_not_exist_is_ignored() {
        let icon = for_pad(
            0x045E,
            0x028E,
            "Microsoft X-Box 360 pad",
            Some("nonsense"),
            &BTreeMap::new(),
        );
        assert_eq!(icon, XBOX);
    }

    #[test]
    fn an_override_beats_the_name_and_the_id() {
        let overrides: BTreeMap<String, String> = [("0079:1879".to_owned(), N64.to_owned())]
            .into_iter()
            .collect();
        assert_eq!(for_pad(0x0079, 0x1879, "Some Pad", None, &overrides), N64);
    }

    #[test]
    fn the_id_table_answers_only_when_the_name_says_nothing() {
        assert_eq!(
            for_pad(0x0079, 0x1843, "USB Gamepad", None, &BTreeMap::new()),
            GAMECUBE
        );
        assert_eq!(icon("Totally Unknown Device"), GAMEPAD);
    }

    #[test]
    fn overrides_are_parsed_case_insensitively_and_validated() {
        let parsed = parse_overrides(
            r#"{"0079:1879": "n64", "AAAA:BBBB": "SNES",
                                         "cccc:dddd": "nonsense", "e:f": 5}"#,
        );
        assert_eq!(parsed.get("0079:1879").map(String::as_str), Some(N64));
        assert_eq!(
            parsed.get("aaaa:bbbb"),
            None,
            "icon names are case-sensitive"
        );
        assert_eq!(parsed.get("cccc:dddd"), None, "an unknown icon is dropped");
        assert_eq!(parsed.get("e:f"), None, "a non-string is dropped");
    }

    #[test]
    fn a_damaged_override_file_is_no_overrides_rather_than_a_failure() {
        for text in ["", "not json", "[1,2]", "null", "\"text\""] {
            assert!(parse_overrides(text).is_empty(), "{text:?}");
        }
    }
}
