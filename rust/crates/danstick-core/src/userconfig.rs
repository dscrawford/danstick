//! Clean danstick's leaked settings out of the user's `retroarch.cfg`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

/// The virtual pads' name prefix, which is how a leaked value is recognised.
pub const VIRTUAL_PREFIX: &str = "danstick Player ";
/// What the virtual pads were called before the project was renamed.
pub const LEGACY_PREFIX: &str = "padmap Player ";

/// Whether a device name is one of our virtual pads, under either name.
fn is_ours(name: &str) -> bool {
    name.starts_with(VIRTUAL_PREFIX) || name.starts_with(LEGACY_PREFIX)
}

/// `input_playerN_*` binds that RetroArch saves.
pub const PLAYER_BINDS: [&str; 24] = [
    "b",
    "y",
    "select",
    "start",
    "up",
    "down",
    "left",
    "right",
    "a",
    "x",
    "l",
    "r",
    "l2",
    "r2",
    "l3",
    "r3",
    "l_x_plus",
    "l_x_minus",
    "l_y_plus",
    "l_y_minus",
    "r_x_plus",
    "r_x_minus",
    "r_y_plus",
    "r_y_minus",
];

const RESERVATION_NONE: i32 = 0;

/// One `key = value` line, split the way RetroArch's parser splits it.
static SETTING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)^(?P<indent>[^\S\n]*)(?P<key>[A-Za-z0-9_]+)(?P<sep>[^\S\n]*=[^\S\n]*)(?:"(?P<quoted>[^"]*)"|(?P<bare>[^\s"]\S*))(?P<trail>.*)$"#,
    )
    .expect("a valid regex")
});
static RESERVED_DEVICE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^input_player(\d+)_reserved_device$").expect("a valid regex"));
static RESERVATION_TYPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^input_player(\d+)_device_reservation_type$").expect("a valid regex")
});
static JOYPAD_INDEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^input_player(\d+)_joypad_index$").expect("a valid regex"));
static PLAYER_BIND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^input_player\d+_({})_(btn|axis)$",
        PLAYER_BINDS.join("|")
    ))
    .expect("a valid regex")
});

/// `(content, line ending)` pairs, splitting on `\n` to preserve CRLF in content.
pub fn split_lines(text: &str) -> Vec<(&str, &str)> {
    let parts: Vec<&str> = text.split('\n').collect();
    let mut lines: Vec<(&str, &str)> = parts[..parts.len() - 1]
        .iter()
        .map(|part| (*part, "\n"))
        .collect();
    if let Some(last) = parts.last() {
        if !last.is_empty() {
            lines.push((last, ""));
        }
    }
    lines
}

/// Write a value preserving its original quoting, unless the value requires quotes.
pub fn render_value(value: &str, quoted: bool) -> String {
    if quoted || value.is_empty() || value.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{value}\"")
    } else {
        value.to_owned()
    }
}

/// The reset value for a leaked key, or `None` to leave it alone.
pub fn cleaned_value(key: &str, value: &str, reserved: &BTreeMap<u32, String>) -> Option<String> {
    if RESERVED_DEVICE.is_match(key) {
        return is_ours(value).then(String::new);
    }
    if let Some(found) = RESERVATION_TYPE.captures(key) {
        let player: u32 = found[1].parse().ok()?;
        let name = reserved.get(&player).map_or("", String::as_str);
        let stale = is_ours(name);
        if stale || (name.is_empty() && value != RESERVATION_NONE.to_string()) {
            return Some(RESERVATION_NONE.to_string());
        }
        return None;
    }
    if PLAYER_BIND.is_match(key) {
        return (value != "nul").then(|| "nul".to_owned());
    }
    if let Some(found) = JOYPAD_INDEX.captures(key) {
        let player: i64 = found[1].parse().ok()?;
        return Some((player - 1).to_string());
    }
    None
}

/// What a clean would change, and the text it would write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cleaned {
    /// `key: "old" -> "new"`, one per altered line.
    pub changes: Vec<String>,
    pub text: String,
}

/// Clean a config's text.
pub fn clean(text: &str) -> Cleaned {
    let lines = split_lines(text);
    let mut reserved: BTreeMap<u32, String> = BTreeMap::new();
    for (content, _) in &lines {
        let Some(found) = SETTING.captures(content) else {
            continue;
        };
        if let Some(slot) = RESERVED_DEVICE.captures(&found["key"]) {
            if let Ok(player) = slot[1].parse::<u32>() {
                reserved.insert(player, setting_value(&found).to_owned());
            }
        }
    }

    let mut out = Cleaned::default();
    for (content, ending) in &lines {
        let Some(found) = SETTING.captures(content) else {
            out.text.push_str(content);
            out.text.push_str(ending);
            continue;
        };
        let key = &found["key"];
        let value = setting_value(&found);
        let Some(new) = cleaned_value(key, value, &reserved).filter(|new| new != value) else {
            out.text.push_str(content);
            out.text.push_str(ending);
            continue;
        };
        out.changes.push(format!("{key}: \"{value}\" -> \"{new}\""));
        out.text.push_str(&found["indent"]);
        out.text.push_str(key);
        out.text.push_str(&found["sep"]);
        out.text
            .push_str(&render_value(&new, found.name("quoted").is_some()));
        out.text.push_str(&found["trail"]);
        out.text.push_str(ending);
    }
    out
}

fn setting_value<'a>(found: &regex::Captures<'a>) -> &'a str {
    found
        .name("quoted")
        .or_else(|| found.name("bare"))
        .map_or("", |m| m.as_str())
}

/// An autoconfig profile's settings, for a profile that is not on disk yet.
pub fn parse_profile_text(text: &str) -> BTreeMap<String, String> {
    static LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"^\s*([A-Za-z0-9_]+)\s*=\s*"?([^"]*)"?\s*$"#).expect("a valid regex")
    });
    text.lines()
        .filter_map(|line| LINE.captures(line))
        .map(|found| (found[1].to_owned(), found[2].to_owned()))
        .collect()
}

/// The same parse, keeping order and repeated keys (for copying profiles).
pub fn parse_profile_pairs(text: &str) -> Vec<(String, String)> {
    static LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"^\s*([A-Za-z0-9_]+)\s*=\s*"?([^"]*)"?\s*$"#).expect("a valid regex")
    });
    text.lines()
        .filter_map(|line| LINE.captures(line))
        .map(|found| (found[1].to_owned(), found[2].to_owned()))
        .collect()
}

/// The same, on raw bytes to preserve non-UTF-8 content byte-for-byte.
pub fn clean_bytes(raw: &[u8]) -> (Vec<String>, Vec<u8>) {
    let mut changes = Vec::new();
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());

    let mut reserved: BTreeMap<u32, String> = BTreeMap::new();
    for line in split_bytes(raw) {
        let Ok(content) = std::str::from_utf8(line.content) else {
            continue;
        };
        let Some(found) = SETTING.captures(content) else {
            continue;
        };
        if let Some(slot) = RESERVED_DEVICE.captures(&found["key"]) {
            if let Ok(player) = slot[1].parse::<u32>() {
                reserved.insert(player, setting_value(&found).to_owned());
            }
        }
    }

    for line in split_bytes(raw) {
        let verbatim = |out: &mut Vec<u8>| {
            out.extend_from_slice(line.content);
            out.extend_from_slice(line.ending);
        };
        let Ok(content) = std::str::from_utf8(line.content) else {
            verbatim(&mut out);
            continue;
        };
        let Some(found) = SETTING.captures(content) else {
            verbatim(&mut out);
            continue;
        };
        let key = &found["key"];
        let value = setting_value(&found);
        let Some(new) = cleaned_value(key, value, &reserved).filter(|new| new != value) else {
            verbatim(&mut out);
            continue;
        };
        changes.push(format!("{key}: \"{value}\" -> \"{new}\""));
        out.extend_from_slice(found["indent"].as_bytes());
        out.extend_from_slice(key.as_bytes());
        out.extend_from_slice(found["sep"].as_bytes());
        out.extend_from_slice(render_value(&new, found.name("quoted").is_some()).as_bytes());
        out.extend_from_slice(found["trail"].as_bytes());
        out.extend_from_slice(line.ending);
    }
    (changes, out)
}

struct Line<'a> {
    content: &'a [u8],
    ending: &'a [u8],
}

fn split_bytes(raw: &[u8]) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (at, byte) in raw.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(Line {
                content: &raw[start..at],
                ending: b"\n",
            });
            start = at + 1;
        }
    }
    if start < raw.len() {
        lines.push(Line {
            content: &raw[start..],
            ending: b"",
        });
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_left_by_padmap_is_cleaned_too() {
        let reserved = BTreeMap::new();
        for name in ["padmap Player 1", "danstick Player 1"] {
            assert_eq!(
                cleaned_value("input_player1_reserved_device", name, &reserved),
                Some(String::new()),
                "{name}"
            );
        }
        assert_eq!(
            cleaned_value(
                "input_player1_reserved_device",
                "Real Controller",
                &reserved
            ),
            None
        );
    }
}
