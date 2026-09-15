//! Which picture stands for a controller.
//!
//! Every answer here is a *guess*, and the ordering says how much each source
//! is trusted. A vendor id names a model, not a controller: 0x0079 is
//! DragonRise, resold in a great many unrelated adapters, so any entry under it
//! is wrong for somebody. A learned profile always wins, because it is the only
//! source that reflects the controller actually in the user's hands.

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

/// Steam's virtual gamepad: the one device whose *name* must not be believed.
///
/// With Steam running it takes the controller over hidraw and publishes this
/// in its place -- a uinput pad on Valve's vendor id, called "Microsoft X-Box
/// 360 pad". The name is a deliberate impersonation, which is what makes games
/// treat it as XInput, so the rule that a name beats a vendor id is exactly
/// backwards here and this is checked first.
pub const STEAM_VIRTUAL_ID: (u16, u16) = (0x28DE, 0x11FF);

/// Fallback only, and treated as a guess: these ids are not reliable identity.
const BY_ID: [((u16, u16), &str); 13] = [
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
    // Nintendo's reissued pads for Switch Online really are those controllers,
    // button for button, so they get the layout of the console they came from
    // rather than the Switch one. A user handed the Switch wizard for an N64
    // pad would be asked to press an X and a Y it does not have.
    ((0x057E, 0x2017), SNES),  // SNES pad for Switch Online
    ((0x057E, 0x2019), N64),   // N64 pad for Switch Online
    ((0x28DE, 0x1304), STEAM), // Steam Controller Puck
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
    // After SNES and N64 on purpose: Nintendo's Switch Online reissues carry
    // both words ("Nintendo Co., Ltd. N64 Controller"), and the console they
    // copy is the more useful answer than the console they plug into.
    (r"pro controller|switch pro|joy-?con|\bnso\b", SWITCH),
    (r"genesis|mega ?drive|\bm30\b|retro-?bit|saturn", GENESIS),
    (r"dualshock|dualsense|playstation|\bps[3-5]\b", PLAYSTATION),
    (r"steam ?(controller|deck|puck)|\bvalve\b", STEAM),
    // "x-box", with the hyphen, because that is what the kernel's own xpad
    // driver calls every 360 pad. Matching only "xbox" meant the most common
    // controller on Linux fell through to the generic icon.
    (r"x-? ?box|xinput", XBOX),
];

// `wheel` is matched separately only because the array above is sized; keeping
// it in one table would be tidier and is the next thing to do if a tenth rule
// arrives.
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

/// Whether a name is one padmap can draw.
pub fn known(name: &str) -> bool {
    ICON_NAMES.contains(&name)
}

/// The icon for a pad, given everything already looked up for it.
///
/// Pure: the profile and the override file are read by the caller, because
/// this is the part that has to be the same answer everywhere and the I/O is
/// the part that differs between the daemon and a one-shot command.
///
/// Order of authority, most trusted first:
///
///   1. the learned per-device profile, set when the pad was first configured
///   2. the user's override file
///   3. Steam's virtual pad, by id, because its name is somebody else's
///   4. the device name
///   5. the built-in vid/pid table
///   6. a generic pad
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
    // Name patterns before the id table: a device that says "Fightstick" in
    // its name is better evidence than a resold vendor id.
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

/// Parse the user's override file: `{"0079:1879": "n64"}`.
///
/// Generic adapter ids are genuinely ambiguous, so this is the documented fix
/// rather than a workaround. An entry naming an icon padmap cannot draw is
/// dropped, because a blank square is worse than the guess it replaced.
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
        // A pattern naming an icon with no artwork draws a blank square.
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
        // xpad calls every one of them "Microsoft X-Box 360 pad", with the
        // hyphen, and a pattern of "xbox" alone matches none of them.
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
        // ...and a genuine 360 pad still is one.
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
        // First match wins, and an arcade stick that mentions Steam is still
        // an arcade stick.
        assert_eq!(icon("Steampunk Arcade Fightstick"), ARCADE);
        assert_eq!(icon("Nintendo Co., Ltd. N64 Controller"), N64);
    }

    #[test]
    fn a_profile_beats_everything_below_it() {
        // The only source that reflects the controller in the user's hands.
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
        // Otherwise a hand-edited profile leaves the pad with a blank square.
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
