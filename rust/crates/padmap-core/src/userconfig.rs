//! Stripping padmap's settings back out of the user's `retroarch.cfg`.
//!
//! The only code in padmap that writes to a file RetroArch owns, and the
//! contract is narrow: **the lines it reports are the only lines that may
//! differ.** A CRLF ending, a latin-1 ROM path in `system_directory`, an
//! unquoted value, a file with no trailing newline -- all of it comes back out
//! exactly as it went in.
//!
//! Only needed for configs written before the launch override started
//! disabling `config_save_on_exit`; after that no new leakage occurs. It stays
//! a command the user asks for rather than something a launch does.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

/// The virtual pads' name prefix, which is how a leaked value is recognised.
pub const VIRTUAL_PREFIX: &str = "padmap Player ";

/// `input_playerN_*` binds RetroArch saves itself. "nul" is how it spells
/// unbound.
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

/// `(content, line ending)` pairs, splitting only where RetroArch splits.
///
/// On `\n` alone. Any `\r` stays as the last character of the content, which
/// is where RetroArch leaves it too: a quoted value stops at its closing quote
/// and the unquoted tokeniser counts `\r` as whitespace.
///
/// Reading through a universal-newline translation instead turned every
/// `\r\n` into `\n` before the cleaner saw the file, so a config carrying
/// Windows endings -- one off a dual-boot install, or hand-edited there -- was
/// rewritten end to end by a command reporting one changed line, and the
/// backup could not put the endings back.
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

/// Write a value back in the form the line already used.
///
/// An unquoted line stays unquoted: RetroArch reads it identically either way,
/// and adding quotes would edit a line the user can see beyond the change that
/// was reported. The exceptions are values that stop meaning the same thing
/// unquoted -- an empty one is no token at all to `strtok_r` and the setting
/// vanishes, and one holding whitespace or a quote is truncated.
pub fn render_value(value: &str, quoted: bool) -> String {
    if quoted || value.is_empty() || value.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{value}\"")
    } else {
        value.to_owned()
    }
}

/// The stock value for a key padmap may have leaked into, or `None` to leave
/// the line alone.
///
/// Only keys already present are rewritten; nothing is added. A key padmap
/// never wrote is left exactly as the user had it.
pub fn cleaned_value(key: &str, value: &str, reserved: &BTreeMap<u32, String>) -> Option<String> {
    if RESERVED_DEVICE.is_match(key) {
        return value.starts_with(VIRTUAL_PREFIX).then(String::new);
    }
    if let Some(found) = RESERVATION_TYPE.captures(key) {
        // Paired with the rule above: a type left at RESERVED while its device
        // name is empty holds the slot for nothing at all.
        let player: u32 = found[1].parse().ok()?;
        let name = reserved.get(&player).map_or("", String::as_str);
        let stale = name.starts_with(VIRTUAL_PREFIX);
        if stale || (name.is_empty() && value != RESERVATION_NONE.to_string()) {
            return Some(RESERVATION_NONE.to_string());
        }
        return None;
    }
    if PLAYER_BIND.is_match(key) {
        // These outrank autoconfig, and RetroArch saved them here itself.
        return (value != "nul").then(|| "nul".to_owned());
    }
    if let Some(found) = JOYPAD_INDEX.captures(key) {
        // RetroArch's own default is N-1, one pad per player in order. Any
        // other value is padmap's, or a hand-edit padmap overrides at every
        // launch anyway, so N-1 is the honest reset.
        let player: i64 = found[1].parse().ok()?;
        return Some((player - 1).to_string());
    }
    // Deliberately no rule for input_libretro_device_pN. RetroArch neither
    // loads nor saves that key in retroarch.cfg -- it lives only in `.rmp`
    // remap files -- so it cannot have leaked here, and a rule for it could
    // only damage a remap file someone pointed --config at.
    None
}

/// What a clean would change, and the text it would write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cleaned {
    /// `key: "old" -> "new"`, one per altered line.
    pub changes: Vec<String>,
    pub text: String,
}

/// Clean a config's text. Pure: the caller decides whether to write it.
pub fn clean(text: &str) -> Cleaned {
    let lines = split_lines(text);

    // Reservation type and device name are judged together, and the file is
    // sorted alphabetically so the type comes first. Collect names up front
    // rather than relying on the order.
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

/// The same parse, keeping the file's order and every repeated key.
///
/// For copying a profile: [`parse_profile_text`] answers "what does this key
/// say", and a `BTreeMap` is right for that; a copy has to come out in the
/// order it went in, or a diff against the original is noise.
pub fn parse_profile_pairs(text: &str) -> Vec<(String, String)> {
    static LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"^\s*([A-Za-z0-9_]+)\s*=\s*"?([^"]*)"?\s*$"#).expect("a valid regex")
    });
    text.lines()
        .filter_map(|line| LINE.captures(line))
        .map(|found| (found[1].to_owned(), found[2].to_owned()))
        .collect()
}

/// The same, on the bytes of a file rather than a `str`.
///
/// A `retroarch.cfg` is not guaranteed to be UTF-8 -- a latin-1 ROM path in
/// `system_directory` is the case that turns up -- and the contract here is
/// that an untouched line comes back byte for byte. The Python reads bytes and
/// decodes with `surrogateescape` for exactly this reason.
///
/// Per line rather than per file: a line that is not UTF-8 is passed through
/// unexamined. Nothing is lost by that, because every rule keys on an ASCII
/// setting name and compares against an ASCII value, so a line whose bytes
/// cannot be read is a line no rule would have changed.
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
