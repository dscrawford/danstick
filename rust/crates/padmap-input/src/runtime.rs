//! Where padmap keeps state that lasts a login session. Paths must agree with
//! the Python launcher's for as long as both are installed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A non-empty environment variable.
pub(crate) fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

pub(crate) fn env_path(name: &str) -> Option<PathBuf> {
    env_nonempty(name).map(PathBuf::from)
}

/// `$HOME`, or `/`.
pub(crate) fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// `$XDG_RUNTIME_DIR/padmap`, falling back to `/tmp/padmap`.
pub fn dir() -> PathBuf {
    dir_under(std::env::var("XDG_RUNTIME_DIR").ok().as_deref())
}

/// Takes the base as an argument so tests never touch the process environment.
pub fn dir_under(base: Option<&str>) -> PathBuf {
    PathBuf::from(base.unwrap_or("/tmp")).join("padmap")
}

pub fn assignments_path() -> PathBuf {
    dir().join("assignments.json")
}

pub fn socket_path() -> PathBuf {
    dir().join("padmap.sock")
}

/// Holds the running game's pid, so a launcher killed outright cannot wedge it.
pub fn playing_marker() -> PathBuf {
    dir().join("playing")
}

pub fn daemon_log_path() -> PathBuf {
    dir().join("padmap.log")
}

/// Models already offered a setup screen this login session.
pub fn prompted_path() -> PathBuf {
    dir().join("prompted")
}

pub fn last_game_path() -> PathBuf {
    dir().join("lastgame.json")
}

pub fn game_is_running() -> bool {
    game_is_running_at(&playing_marker())
}

pub fn game_is_running_at(marker: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(marker) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return false;
    };
    if PathBuf::from(format!("/proc/{pid}")).exists() {
        return true;
    }
    let _ = std::fs::remove_file(marker);
    false
}

/// Lossy: anything may have written this file, and a bad byte must not stop startup.
pub fn read_prompted(path: &Path) -> BTreeSet<String> {
    let Ok(raw) = std::fs::read(path) else {
        return Default::default();
    };
    String::from_utf8_lossy(&raw)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn write_prompted(path: &Path, prompted: &BTreeSet<String>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body: String = prompted.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(path, body)
}

pub fn config_home() -> PathBuf {
    env_path("XDG_CONFIG_HOME").unwrap_or_else(|| {
        std::env::var("HOME")
            .map(|home| PathBuf::from(home).join(".config"))
            .unwrap_or_else(|_| PathBuf::from(".config"))
    })
}

/// The user's icon overrides, `{"0079:1879": "n64"}`.
pub fn icon_overrides_path() -> PathBuf {
    config_home().join("padmap").join("icons.json")
}

pub fn load_icon_overrides() -> BTreeMap<String, String> {
    std::fs::read_to_string(icon_overrides_path())
        .map(|text| padmap_core::icons::parse_overrides(&text))
        .unwrap_or_default()
}

fn mtime_nanos(meta: std::fs::Metadata) -> Option<u128> {
    let since = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(since.as_nanos())
}

fn build_id_from(label: &Path, newest: Option<u128>) -> String {
    if let Some(store) = env_nonempty("PADMAP_BUILD_ID") {
        return store;
    }
    match newest {
        Some(nanos) => format!("mtime:{}:{nanos}", label.display()),
        None => "unknown".to_owned(),
    }
}

/// Identity of the code this binary is running; `PADMAP_BUILD_ID` (the Nix store path) wins.
pub fn build_id_of_binary() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return build_id_from(Path::new(""), None);
    };
    let newest = std::fs::metadata(&exe).ok().and_then(mtime_nanos);
    build_id_from(&exe, newest)
}

/// Identity of a Python source tree: the newest `.py` mtime, unless `PADMAP_BUILD_ID` is set.
pub fn build_id(source: &Path) -> String {
    let newest = std::fs::read_dir(source)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "py"))
        .filter_map(|entry| entry.metadata().ok())
        .filter_map(mtime_nanos)
        .max();
    build_id_from(source, newest)
}

fn proc_field(pid: u32, name: &str) -> Vec<String> {
    let Ok(raw) = std::fs::read(format!("/proc/{pid}/{name}")) else {
        return Vec::new();
    };
    raw.split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect()
}

/// Structural, not a substring match: `pgrep -f` would also kill the shell mentioning it.
pub fn is_daemon_argv(argv: &[String]) -> bool {
    let [.., before, last] = argv else {
        return false;
    };
    if last != "serve" {
        return false;
    }
    if before == "padmap.cli" {
        return argv.iter().any(|arg| arg == "-m");
    }
    let program = Path::new(before)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    program == "padmap" || program == "padmap-rs"
}

/// Pids of padmap daemons serving a given `XDG_RUNTIME_DIR` (which decides the socket).
pub fn daemon_pids(runtime: Option<&str>) -> Vec<u32> {
    use std::os::unix::fs::MetadataExt;
    let wanted = runtime
        .map(str::to_owned)
        .unwrap_or_else(|| std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned()));
    let uid = rustix::process::getuid().as_raw();
    let mut pids: Vec<u32> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let meta = entry.metadata().ok()?;
            (meta.uid() == uid).then_some(pid)
        })
        .filter(|&pid| is_daemon_argv(&proc_field(pid, "cmdline")))
        .filter(|&pid| {
            let theirs = proc_field(pid, "environ")
                .into_iter()
                .find_map(|item| item.strip_prefix("XDG_RUNTIME_DIR=").map(str::to_owned))
                .unwrap_or_else(|| "/tmp".to_owned());
            same_runtime(&theirs, &wanted)
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// Lexically normalised, never resolved: `/run/user/1000/` and `/run/user/1000` are one daemon.
pub fn same_runtime(one: &str, other: &str) -> bool {
    normalise(one) == normalise(other)
}

/// `os.path.normpath`: unlike `Path::components`, pops `..` and keeps exactly two leading slashes.
fn normalise(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let absolute = path.starts_with('/');
    let double = path.starts_with("//") && !path.starts_with("///");
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => match parts.last() {
                Some(&last) if last != ".." => {
                    parts.pop();
                }
                _ if absolute => {}
                _ => parts.push(".."),
            },
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("{}{joined}", if double { "//" } else { "/" })
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

/// How many recently played games the scope picker may offer.
pub const RECENT_GAMES: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Game {
    #[serde(default)]
    pub console: String,
    pub key: String,
    #[serde(default)]
    pub title: String,
}

/// Recently launched games, newest first. Never fails.
pub fn read_recent_games() -> Vec<Game> {
    recent_games_from(&std::fs::read_to_string(last_game_path()).unwrap_or_default())
}

/// Reads the pre-list format (a bare object) too, so an upgrade mid-session loses nothing.
pub fn recent_games_from(text: &str) -> Vec<Game> {
    let Ok(raw) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    match raw.get("games").and_then(|value| value.as_array()) {
        Some(games) => games
            .iter()
            .filter_map(game_entry)
            .take(RECENT_GAMES)
            .collect(),
        None => game_entry(&raw).into_iter().collect(),
    }
}

/// Non-string values are dropped, not stringified: `str(None)` would read "None" in a picker.
fn game_entry(raw: &serde_json::Value) -> Option<Game> {
    let object = raw.as_object()?;
    let key = object.get("key")?.as_str()?;
    if key.is_empty() {
        return None;
    }
    let text = |name: &str| {
        object
            .get(name)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    Some(Game {
        console: text("console"),
        key: key.to_owned(),
        title: text("title"),
    })
}

/// Record a launch: newest first, deduplicated by key, truncated to [`RECENT_GAMES`].
pub fn write_last_game(game: &Game) -> std::io::Result<PathBuf> {
    let mut kept: Vec<Game> = read_recent_games()
        .into_iter()
        .filter(|previous| previous.key != game.key)
        .collect();
    kept.insert(0, game.clone());
    kept.truncate(RECENT_GAMES);
    let path = last_game_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::json!({ "games": kept });
    std::fs::write(&path, serde_json::to_string_pretty(&body)? + "\n")?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_is_recognised_by_its_argv_shape_not_a_substring() {
        let owned =
            |parts: &[&str]| -> Vec<String> { parts.iter().map(|p| (*p).to_owned()).collect() };
        assert!(is_daemon_argv(&owned(&[
            "/nix/store/x/bin/padmap",
            "serve"
        ])));
        assert!(is_daemon_argv(&owned(&["padmap-rs", "serve"])));
        assert!(is_daemon_argv(&owned(&[
            "python3",
            "-m",
            "padmap.cli",
            "serve"
        ])));
        assert!(!is_daemon_argv(&owned(&["bash", "-c", "padmap serve"])));
        assert!(!is_daemon_argv(&owned(&["padmap", "list"])));
        assert!(!is_daemon_argv(&owned(&["padmap.cli", "serve"])), "no -m");
        assert!(!is_daemon_argv(&owned(&["serve"])));
        assert!(!is_daemon_argv(&owned(&[])));
    }

    #[test]
    fn the_prompted_file_round_trips_and_survives_bytes_that_are_not_utf8() {
        let dir = std::env::temp_dir().join(format!("padmap-prompted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("prompted");
        let mut set = BTreeSet::new();
        set.insert("1234:0001:Pad".to_owned());
        set.insert("abcd:0002:Other".to_owned());
        write_prompted(&path, &set).expect("creates the directory");
        assert_eq!(read_prompted(&path), set);
        std::fs::write(&path, b"1234:0001:Pad\n\xff\xfe\n\n").expect("seed");
        let read = read_prompted(&path);
        assert!(read.contains("1234:0001:Pad"));
        assert_eq!(read.len(), 2, "the bad line is kept lossily, not fatal");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_path_hangs_off_one_directory() {
        let base = dir();
        for path in [assignments_path(), socket_path(), playing_marker()] {
            assert_eq!(path.parent(), Some(base.as_path()), "{path:?}");
        }
    }

    #[test]
    fn the_filenames_are_the_ones_the_python_writes() {
        assert_eq!(
            assignments_path().file_name().and_then(|n| n.to_str()),
            Some("assignments.json")
        );
        assert_eq!(
            socket_path().file_name().and_then(|n| n.to_str()),
            Some("padmap.sock")
        );
        assert_eq!(
            playing_marker().file_name().and_then(|n| n.to_str()),
            Some("playing")
        );
    }

    #[test]
    fn a_missing_marker_is_not_a_running_game() {
        let missing = dir_under(Some("/nonexistent-padmap-test")).join("playing");
        assert!(!game_is_running_at(&missing));
    }

    #[test]
    fn the_base_comes_from_the_environment_but_defaults_to_tmp() {
        assert_eq!(
            dir_under(Some("/run/user/1000")),
            PathBuf::from("/run/user/1000/padmap")
        );
        assert_eq!(dir_under(None), PathBuf::from("/tmp/padmap"));
    }
}
