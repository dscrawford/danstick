//! MAME set names, turned into titles a person would recognise.
//!
//! RetroArch's arcade playlists label entries by ROM set name -- `10yard`,
//! `ckongjeu`, `1941j` -- because that is the filename, and a library built on
//! those reads like a directory listing. MAME's own XML carries the real
//! description per set, so this maps one to the other.
//!
//! Matching is by **set name**, not CRC. The playlist records
//! `crc32 = "00000000|crc"` for all 8302 entries, a single distinct value, so
//! checksum matching against libretro-database cannot work at all.
//!
//! Use the XML that matches the *core*, not the newest available. Set names
//! drift between MAME versions and the playlist was scanned for MAME 2010
//! (0.139): measured against that XML, 8215/8302 = 99.0% resolve, and the
//! remainder are clone and bootleg sets absent from that build, which fall
//! back to the raw name.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// MAME grades each driver. "preliminary" is the status behind its own red
/// "THIS GAME DOES NOT WORK" warning.
pub const STATUS_PRELIMINARY: &str = "preliminary";

/// A run of whitespace containing a line break, anywhere in a field.
static WRAPPED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]*[\r\n\u{2028}\u{2029}]+[ \t]*").expect("a valid regex"));
static GAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<game name="([^"]+)""#).expect("a valid regex"));
static DESCRIPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<description>(.*?)</description>").expect("a valid regex"));
static YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<year>(.*?)</year>").expect("a valid regex"));
static MANUFACTURER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<manufacturer>(.*?)</manufacturer>").expect("a valid regex"));
/// Matched on `<driver ` and not on `status="..."` anywhere: ROM elements
/// carry their own status ("baddump", "nodump") and there are thousands of
/// those per file.
static DRIVER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<driver\s+status="([a-z]+)""#).expect("a valid regex"));

/// One line, with a wrapped description joined the way a reader would.
///
/// The description patterns are dot-matches-newline, so a MAME description
/// that the dump wrapped across two lines arrives here as
/// `"Puck\n            Man"`. That newline is not cosmetic: a title reaches
/// line-oriented files and line-oriented reports, so one holding a newline can
/// write a line of its own.
///
/// Only whitespace runs that span a line break collapse. Spaces inside a
/// single line are left exactly as they are, because "Double  Dragon" is a
/// real MAME description and rewriting it would lose the match this table
/// exists to make.
pub fn flatten(value: &str) -> String {
    WRAPPED.replace_all(value, " ").trim().to_owned()
}

/// A set's real name, and what MAME says about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Title {
    pub title: String,
    pub year: String,
    pub manufacturer: String,
    /// "good", "imperfect", "preliminary", or "" when the set is not in the
    /// table at all. Only meaningful for arcade.
    pub status: String,
}

impl Title {
    /// False only when MAME says the driver does not work.
    ///
    /// An unknown status counts as working: most of a library is not MAME, and
    /// marking every console game broken because it has no driver grade would
    /// be worse than saying nothing.
    pub fn working(&self) -> bool {
        self.status != STATUS_PRELIMINARY
    }
}

/// Extract set name -> Title from a MAME `-listxml` dump.
///
/// Scanned with regexes rather than parsed as a DOM: the MAME 2010 XML is 43MB
/// and a DOM of it costs several hundred MB of RSS for three fields per entry.
pub fn parse_mame_xml(text: &str) -> BTreeMap<String, Title> {
    let mut out = BTreeMap::new();
    for found in GAME.captures_iter(text) {
        let name = &found[1];
        let whole = found.get(0).expect("the whole match");
        // From the end of this <game ...> to the next </game>. A dump whose
        // tag is never closed therefore reads to the *following* game's close
        // and absorbs its fields -- faithfully reproduced, because the Python
        // does it and the two have to agree while both are installed.
        let Some(end) = text[whole.end()..].find("</game>") else {
            continue;
        };
        let block = &text[whole.end()..whole.end() + end];

        let Some(description) = DESCRIPTION.captures(block) else {
            continue;
        };
        let title = flatten(&description[1]);
        if title.is_empty() {
            // Same reason an entry with no <description> is skipped: a set
            // named "" is a blank row in the arcade tab, which is worse than
            // falling back to the raw set name.
            continue;
        }
        out.insert(
            name.to_owned(),
            Title {
                title,
                year: YEAR
                    .captures(block)
                    .map_or_else(String::new, |m| flatten(&m[1])),
                manufacturer: MANUFACTURER
                    .captures(block)
                    .map_or_else(String::new, |m| flatten(&m[1])),
                status: DRIVER
                    .captures(block)
                    .map_or_else(String::new, |m| m[1].to_owned()),
            },
        );
    }
    out
}

/// The compact form the runtime loads: `{name: [title, year, maker, status]}`.
pub fn to_json(titles: &BTreeMap<String, Title>) -> String {
    let rows: BTreeMap<&String, [&str; 4]> = titles
        .iter()
        .map(|(name, title)| {
            (
                name,
                [
                    title.title.as_str(),
                    title.year.as_str(),
                    title.manufacturer.as_str(),
                    title.status.as_str(),
                ],
            )
        })
        .collect();
    serde_json::to_string(&rows).unwrap_or_else(|_| "{}".to_owned())
}

/// One cell of a row, or "" when it is not something to show a player.
///
/// A JSON `true` would otherwise print as the year "True" -- the same trap
/// `protocol._game_entry` had, and the reason this is a function rather than
/// a cast.
pub fn column(value: &Value) -> String {
    match value {
        Value::String(text) => flatten(text),
        Value::Bool(_) => String::new(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

/// What went wrong reading a table, when nothing could be read at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("title table is a {0}, not an object of set name -> row")]
pub struct NotATable(pub &'static str);

/// A dumped table, and the rows that could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Loaded {
    pub titles: BTreeMap<String, Title>,
    /// Set names whose row was the wrong shape. Reported, not raised: the rest
    /// of the table still names its games, and those sets keep their raw
    /// names, which is what RetroArch shows anyway.
    pub dropped: Vec<String>,
}

/// Read a dumped table, dropping any row that is not one.
///
/// Every row is checked rather than trusted. This is a build product read at
/// runtime, so a truncated write, a half-copied file and a hand-edited one all
/// arrive here -- and the shapes are not theoretical. A row that is a plain
/// string (`{"pacman": "Pac-Man"}`) used to load as `Title(title="P",
/// year="a")` in the Python, because a string subscripts exactly like a list:
/// every one-character arcade name, with nothing saying so. That shape is now
/// read as the one thing it can only mean, a title with no year.
pub fn load_json(text: &str) -> Result<Loaded, NotATable> {
    let raw: Value = serde_json::from_str(text).map_err(|_| NotATable("str"))?;
    let object = raw.as_object().ok_or(NotATable(match raw {
        Value::Array(_) => "list",
        Value::String(_) => "str",
        Value::Number(_) => "int",
        Value::Bool(_) => "bool",
        _ => "NoneType",
    }))?;

    let mut loaded = Loaded::default();
    for (name, value) in object {
        // Tested for explicitly, never by "is it subscriptable": str and dict
        // are subscriptable too, and both produced a wrong Title rather than
        // an error.
        let row: Vec<Value> = match value {
            Value::String(text) => vec![Value::String(text.clone())],
            Value::Array(items) => items.clone(),
            _ => {
                loaded.dropped.push(name.clone());
                continue;
            }
        };
        // Short and long rows read the same way: missing columns are empty,
        // extra ones from a future format are ignored. Three-element rows are
        // what was written before driver status existed, and an already-built
        // table keeps working rather than making every arcade entry fall back.
        let mut columns: Vec<String> = row.iter().take(4).map(column).collect();
        columns.resize(4, String::new());
        if columns[0].is_empty() {
            // No title is not a title. Keeping the row replaces the set name
            // in the arcade tab with a blank line.
            loaded.dropped.push(name.clone());
            continue;
        }
        loaded.titles.insert(
            name.clone(),
            Title {
                title: columns[0].clone(),
                year: columns[1].clone(),
                manufacturer: columns[2].clone(),
                status: columns[3].clone(),
            },
        );
    }
    Ok(loaded)
}

/// The title for one playlist entry.
///
/// Keyed on the ROM basename rather than the playlist label, because the label
/// is only incidentally the same string and users can edit it.
pub fn resolve(label: &str, path: &str, titles: &BTreeMap<String, Title>) -> Title {
    let stem = std::path::Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(found) = titles.get(&stem) {
        return found.clone();
    }
    Title {
        title: if label.is_empty() {
            stem
        } else {
            label.to_owned()
        },
        ..Title::default()
    }
}
