//! Writing the files other programs read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use log::warn;
use padmap_core::emit::{self, Identity};

use crate::runtime::{config_home, env_path, home};

/// Overrides where the generated SDL database goes.
pub const ENV_SDL_DB: &str = "PADMAP_SDL_DB";

pub fn sdl_database_path() -> PathBuf {
    env_path(ENV_SDL_DB).unwrap_or_else(|| config_dir().join("padmap").join("sdl_controllers.txt"))
}

fn config_dir() -> PathBuf {
    env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"))
}

fn data_home() -> PathBuf {
    env_path("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local").join("share"))
}

/// `<dir>/udev`: RetroArch looks in `<dir>/<driver>` before the base directory.
pub fn autoconfig_dir() -> PathBuf {
    crate::runtime::dir().join("autoconfig").join("udev")
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error(
        "{0} is there and could not be read, so rewriting it would lose \
             the mappings already in it"
    )]
    Unreadable(PathBuf),
    #[error("writing {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
}

fn io_at(path: &Path) -> impl Fn(std::io::Error) -> WriteError + '_ {
    move |error| WriteError::Io(path.to_path_buf(), error)
}

fn invalid(path: &Path, error: serde_json::Error) -> WriteError {
    let error = std::io::Error::new(std::io::ErrorKind::InvalidData, error);
    WriteError::Io(path.to_path_buf(), error)
}

fn read_lossy(path: &Path) -> std::io::Result<String> {
    std::fs::read(path).map(|raw| String::from_utf8_lossy(&raw).into_owned())
}

fn env_paths(name: &str) -> Vec<PathBuf> {
    std::env::var(name)
        .map(|value| {
            value
                .split(':')
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

fn or_default(path: Option<&Path>, default: fn() -> PathBuf) -> PathBuf {
    path.map(Path::to_path_buf).unwrap_or_else(default)
}

/// Replace padmap's lines, keeping every other; refuses an existing file it cannot read.
pub fn write_sdl_database(
    lines: &BTreeMap<u32, String>,
    notes: &BTreeMap<u32, String>,
    identity_for: impl Fn(u32) -> Identity,
    path: Option<&Path>,
) -> Result<PathBuf, WriteError> {
    let target = or_default(path, sdl_database_path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(io_at(parent))?;
    }
    let existing = match read_lossy(&target) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => {
            warn!(
                "cannot read {} -- refusing to rewrite it, because the mappings \
                 already in it would be lost",
                target.display()
            );
            return Err(WriteError::Unreadable(target));
        }
    };
    let body = emit::rewrite_sdl_database(&existing, lines, notes, identity_for);
    std::fs::write(&target, body).map_err(io_at(&target))?;
    Ok(target)
}

/// One autoconfig profile per virtual pad; stale ones are cleared so they cannot match.
pub fn write_autoconfig(
    profiles: &BTreeMap<u32, String>,
    dir: Option<&Path>,
) -> Result<Vec<PathBuf>, WriteError> {
    let target = or_default(dir, autoconfig_dir);
    std::fs::create_dir_all(&target).map_err(io_at(&target))?;
    for entry in std::fs::read_dir(&target).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(emit::VIRTUAL_PREFIX) && name.ends_with(".cfg") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let mut written = Vec::new();
    for (player, text) in profiles {
        let path = target.join(format!("{}.cfg", emit::virtual_name(*player)));
        std::fs::write(&path, text).map_err(io_at(&path))?;
        written.push(path);
    }
    Ok(written)
}

pub fn cemu_profile_dir() -> PathBuf {
    env_path("PADMAP_CEMU_DIR")
        .unwrap_or_else(|| config_home().join("Cemu").join("controllerProfiles"))
}

/// Up to Cemu's eight slots; profiles for players padmap is not managing are left alone.
pub fn write_cemu_profiles(
    players: &[u32],
    seat: Option<u32>,
    guid_for: impl Fn(u32) -> String,
    name_for: impl Fn(u32) -> String,
    dir: Option<&Path>,
) -> Result<Vec<PathBuf>, WriteError> {
    use padmap_core::cemu;

    let target = or_default(dir, cemu_profile_dir);
    std::fs::create_dir_all(&target).map_err(io_at(&target))?;
    let mut written = Vec::new();
    for &player in players {
        if player == 0 || player > cemu::MAX_PLAYERS {
            continue;
        }
        let path = target.join(cemu::profile_filename(player));
        let body = cemu::profile(player, &guid_for(player), &name_for(player));
        std::fs::write(&path, body).map_err(io_at(&path))?;
        written.push(path);
    }
    // The keyboard takes the first free port. A keyboard profile padmap left
    // at another port last time would make the keyboard two players at once,
    // so those go; a keyboard profile the user made themselves is not ours to
    // touch.
    let keyboard = padmap_core::keyboard::port(seat, players, cemu::MAX_PLAYERS);
    for port in 1..=cemu::MAX_PLAYERS {
        if players.contains(&port) || keyboard == Some(port) {
            continue;
        }
        let path = target.join(cemu::profile_filename(port));
        if read_lossy(&path).is_ok_and(|text| cemu::is_keyboard_profile(&text)) {
            std::fs::remove_file(&path).map_err(io_at(&path))?;
        }
    }
    if let Some(port) = keyboard {
        let path = target.join(cemu::profile_filename(port));
        std::fs::write(&path, cemu::keyboard_profile(port)).map_err(io_at(&path))?;
        written.push(path);
    }
    Ok(written)
}

pub fn dolphin_config_dir() -> PathBuf {
    env_path("PADMAP_DOLPHIN_DIR").unwrap_or_else(|| config_home().join("dolphin-emu"))
}

/// Writes `GCPadNew.ini`, `Dolphin.ini` (port device types) and `DSUClient.ini`, key by key.
pub fn write_dolphin_config(
    players: &[u32],
    seat: Option<u32>,
    device_for: impl Fn(u32) -> String,
    dir: Option<&Path>,
) -> Result<Vec<PathBuf>, WriteError> {
    use padmap_core::dolphin;

    let target = or_default(dir, dolphin_config_dir);
    std::fs::create_dir_all(&target).map_err(io_at(&target))?;

    let bindings = target.join("GCPadNew.ini");
    let existing = read_lossy(&bindings).unwrap_or_default();
    let body = dolphin::sections(players, seat, device_for);
    std::fs::write(&bindings, dolphin::rewrite_bindings(&existing, &body))
        .map_err(io_at(&bindings))?;

    let core = target.join("Dolphin.ini");
    let mut text = read_lossy(&core).unwrap_or_default();
    for (key, kind) in dolphin::si_devices(players, seat) {
        text = dolphin::set_ini(&text, "Core", &key, &kind.to_string());
    }
    std::fs::write(&core, text).map_err(io_at(&core))?;

    let dsu = target.join("DSUClient.ini");
    let existing = read_lossy(&dsu).unwrap_or_default();
    std::fs::write(&dsu, dolphin::dsu_client_ini(&existing)).map_err(io_at(&dsu))?;

    Ok(vec![bindings, core, dsu])
}

pub fn ares_settings_path() -> PathBuf {
    env_path("PADMAP_ARES_SETTINGS")
        .unwrap_or_else(|| data_home().join("ares").join("settings.bml"))
}

pub fn ryujinx_config_path() -> PathBuf {
    env_path("PADMAP_RYUJINX_CONFIG")
        .unwrap_or_else(|| config_home().join("Ryujinx").join("Config.json"))
}

/// Replace the `VirtualPadN` blocks padmap manages; blocks ares never wrote are appended.
pub fn rewrite_ares_settings(existing: &str, blocks: &BTreeMap<u32, String>) -> String {
    let mut out = String::with_capacity(existing.len());
    let mut skipping: Option<u32> = None;
    let mut replaced: BTreeSet<u32> = BTreeSet::new();
    for line in existing.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if let Some(rest) = bare.strip_prefix("VirtualPad") {
            skipping = rest
                .parse()
                .ok()
                .filter(|player| blocks.contains_key(player));
            if let Some(player) = skipping {
                out.push_str(&blocks[&player]);
                replaced.insert(player);
                continue;
            }
        } else if skipping.is_some() && !bare.starts_with("  ") {
            skipping = None;
        }
        if skipping.is_none() {
            out.push_str(line);
        }
    }
    for (_, block) in blocks
        .iter()
        .filter(|(player, _)| !replaced.contains(player))
    {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(block);
    }
    out
}

/// Refuses if ares has never run: settings.bml holds every other setting too.
pub fn write_ares_settings(
    blocks: &BTreeMap<u32, String>,
    path: Option<&Path>,
) -> Result<PathBuf, WriteError> {
    let target = or_default(path, ares_settings_path);
    let existing = std::fs::read_to_string(&target).map_err(io_at(&target))?;
    std::fs::write(&target, rewrite_ares_settings(&existing, blocks)).map_err(io_at(&target))?;
    Ok(target)
}

/// Replaces only `input_config`, and only padmap's players within it.
pub fn write_ryujinx_config(
    entries: Vec<serde_json::Value>,
    seat: Option<u32>,
    path: Option<&Path>,
) -> Result<PathBuf, WriteError> {
    let target = or_default(path, ryujinx_config_path);
    let text = std::fs::read_to_string(&target).map_err(io_at(&target))?;
    let mut config: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| invalid(&target, error))?;
    let merged = padmap_core::ryujinx::merge(
        config
            .get("input_config")
            .unwrap_or(&serde_json::Value::Null),
        entries,
        seat,
    );
    config["input_config"] = merged;
    let body = serde_json::to_string_pretty(&config).map_err(|error| invalid(&target, error))?;
    std::fs::write(&target, body + "\n").map_err(io_at(&target))?;
    Ok(target)
}

pub fn retroarch_config_dir() -> PathBuf {
    env_path("RETROARCH_CONFIG_DIR").unwrap_or_else(|| home().join(".config").join("retroarch"))
}

/// Existing joypad profile directories, most specific first.
pub fn autoconfig_dirs() -> Vec<PathBuf> {
    static DIRS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    DIRS.get_or_init(scan_autoconfig_dirs).clone()
}

fn scan_autoconfig_dirs() -> Vec<PathBuf> {
    let mut dirs = env_paths("PADMAP_AUTOCONFIG_DIRS");
    dirs.push(retroarch_config_dir().join("autoconfig"));
    dirs.push(PathBuf::from(
        "/run/current-system/sw/share/libretro/autoconfig",
    ));
    if let Ok(entries) = std::fs::read_dir("/nix/store") {
        let mut stores: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.contains("retroarch-joypad-autoconfig-"))
            })
            .map(|entry| entry.path().join("share/libretro/autoconfig"))
            .collect();
        stores.sort();
        stores.reverse();
        dirs.extend(stores);
    }
    dirs.into_iter().filter(|dir| dir.is_dir()).collect()
}

/// Every `.cfg` under a directory, sorted.
fn profiles_under(base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "cfg") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Best libretro profile for a pad: exact name first, vid/pid fallback.
pub fn find_profile(name: &str, vid: u16, pid: u16) -> Option<(String, Vec<(String, String)>)> {
    type Found = Option<(String, Vec<(String, String)>)>;
    type Cache = std::sync::OnceLock<std::sync::Mutex<BTreeMap<(String, u16, u16), Found>>>;
    static CACHE: Cache = std::sync::OnceLock::new();
    let key = (name.to_owned(), vid, pid);
    if let Ok(cache) = CACHE.get_or_init(Default::default).lock() {
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
    }
    let found = scan_for_profile(name, vid, pid);
    if let Ok(mut cache) = CACHE.get_or_init(Default::default).lock() {
        cache.insert(key, found.clone());
    }
    found
}

fn scan_for_profile(name: &str, vid: u16, pid: u16) -> Option<(String, Vec<(String, String)>)> {
    let mut by_ids: Option<(String, Vec<(String, String)>)> = None;
    for base in autoconfig_dirs() {
        for path in profiles_under(&base) {
            let Ok(text) = read_lossy(&path) else {
                continue;
            };
            let pairs = padmap_core::userconfig::parse_profile_pairs(&text);
            let file = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_owned();
            let value = |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.as_str())
            };
            if value("input_device") == Some(name) {
                return Some((file, pairs));
            }
            if by_ids.is_none() && vid != 0 && pid != 0 {
                let matches = value("input_vendor_id")
                    .and_then(|v| v.parse::<u32>().ok())
                    .zip(value("input_product_id").and_then(|v| v.parse::<u32>().ok()))
                    .is_some_and(|(v, p)| v == u32::from(vid) && p == u32::from(pid));
                if matches {
                    by_ids = Some((file, pairs));
                }
            }
        }
    }
    by_ids
}

/// Files SDL itself would read a mapping from, the user's own first, then the legacy location.
pub fn sdl_database_paths() -> Vec<PathBuf> {
    let mut paths = vec![sdl_database_path()];
    let legacy = config_home()
        .join("pegasus-frontend")
        .join("sdl_controllers.txt");
    if !paths.contains(&legacy) {
        paths.push(legacy);
    }
    paths.extend(env_paths("SDL_GAMECONTROLLERCONFIG_FILE"));
    paths
}

/// A mapping for this GUID already on disk, and where it came from.
pub fn carried_fields(guid: &str) -> Option<(padmap_core::fields::Fields, String)> {
    if guid.is_empty() {
        return None;
    }
    for path in sdl_database_paths() {
        let text = match read_lossy(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                warn!(
                    "cannot read {}, so a mapping in it will not be carried over: {error}",
                    path.display()
                );
                continue;
            }
        };
        for line in text.lines() {
            let Some((found, name, fields)) = padmap_core::sdl::parse_line(line) else {
                continue;
            };
            if found == guid && !name.starts_with(emit::VIRTUAL_PREFIX) {
                return Some((binding_fields(&fields), path.display().to_string()));
            }
        }
    }
    None
}

/// Fields that describe the pad rather than bind it; the clone has its own identity.
const IDENTITY_FIELDS: [&str; 5] = ["platform", "crc", "hint", "sdk", "type"];

pub fn binding_fields(fields: &padmap_core::fields::Fields) -> padmap_core::fields::Fields {
    let mut out = padmap_core::fields::Fields::new();
    for (field, target) in fields.iter() {
        if !IDENTITY_FIELDS.contains(&field.as_str()) {
            out.insert(field.clone(), target.clone());
        }
    }
    out
}

pub fn write_launch_config(path: &Path, text: &str) -> Result<(), WriteError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_at(parent))?;
    }
    std::fs::write(path, text).map_err(io_at(path))
}

/// One `--nodevice` token per line, for the launch wrapper.
pub fn write_launch_args(path: &Path, args: &[String]) -> Result<(), WriteError> {
    let body: String = args.iter().map(|arg| format!("{arg}\n")).collect();
    write_launch_config(path, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PADMAP: Identity = Identity {
        bustype: 0x06,
        vendor: 0x1209,
        product: 0x0001,
        version: 0x0001,
    };

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("padmap-artefact-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn one_line() -> BTreeMap<u32, String> {
        [(1, "aaa,padmap Player 1,a:b0,".to_owned())]
            .into_iter()
            .collect()
    }

    #[test]
    fn a_libretro_profile_is_found_by_name_before_ids() {
        let dir = scratch("find");
        std::fs::create_dir_all(dir.join("udev")).expect("mkdir");
        std::fs::write(
            dir.join("udev").join("ids.cfg"),
            "input_driver = \"udev\"\ninput_device = \"Other\"\ninput_vendor_id = \"4660\"\ninput_product_id = \"1\"\ninput_b_btn = \"1\"\n",
        )
        .expect("seed");
        std::fs::write(
            dir.join("udev").join("name.cfg"),
            "input_device = \"Mine\"\ninput_vendor_id = \"0\"\ninput_product_id = \"0\"\ninput_a_btn = \"0\"\n",
        )
        .expect("seed");
        let out = std::process::Command::new(std::env::current_exe().expect("exe"))
            .args([
                "--exact",
                "artefacts::tests::find_profile_child",
                "--nocapture",
                "--quiet",
            ])
            .env("PADMAP_AUTOCONFIG_DIRS", &dir)
            .env("PADMAP_FIND_CHILD", "1")
            .output()
            .expect("run");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("by-name=name.cfg"), "{text}");
        assert!(text.contains("by-ids=ids.cfg"), "{text}");
        assert!(text.contains("none=-"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The child half of the test above; prints, and the parent checks.
    #[test]
    fn find_profile_child() {
        if std::env::var("PADMAP_FIND_CHILD").is_err() {
            return;
        }
        println!(
            "by-name={}",
            find_profile("Mine", 0x1234, 1)
                .map(|(f, _)| f)
                .unwrap_or("-".into())
        );
        println!(
            "by-ids={}",
            find_profile("Nobody", 0x1234, 1)
                .map(|(f, _)| f)
                .unwrap_or("-".into())
        );
        println!(
            "none={}",
            find_profile("Nobody", 0, 0)
                .map(|(f, _)| f)
                .unwrap_or("-".into())
        );
    }

    #[test]
    fn a_carried_line_drops_the_identity_fields_and_skips_our_own() {
        let mut fields = padmap_core::fields::Fields::new();
        fields.insert("a", "b0");
        fields.insert("platform", "Linux");
        fields.insert("crc", "abcd");
        let kept = binding_fields(&fields);
        assert_eq!(kept.get("a"), Some("b0"));
        assert!(!kept.contains_key("platform"));
        assert!(!kept.contains_key("crc"));
        assert!(carried_fields("").is_none(), "no GUID, nothing to look up");
    }

    #[test]
    fn a_first_run_with_no_database_still_writes_one() {
        let dir = scratch("first");
        let path = dir.join("nested").join("sdl_controllers.txt");
        let written = write_sdl_database(&one_line(), &BTreeMap::new(), |_| PADMAP, Some(&path))
            .expect("a missing file is nothing to lose");
        assert!(std::fs::read_to_string(&written)
            .expect("read")
            .contains("Player 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hand_written_line_survives_a_rewrite() {
        let dir = scratch("keep");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("sdl_controllers.txt");
        std::fs::write(&path, "030000005e040000e002000000000000,Their Pad,a:b0,\n").expect("seed");
        write_sdl_database(&one_line(), &BTreeMap::new(), |_| PADMAP, Some(&path)).expect("write");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(
            text.contains("Their Pad"),
            "the reason this rewrites at all"
        );
        assert!(text.contains("padmap Player 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_database_is_refused_rather_than_overwritten() {
        if rustix::process::geteuid().is_root() {
            return; // root can read anything; the check cannot be staged
        }
        let dir = scratch("unreadable");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("sdl_controllers.txt");
        std::fs::write(&path, "030000005e040000e002000000000000,Their Pad,a:b0,\n").expect("seed");
        let mut perms = std::fs::metadata(&path).expect("stat").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o222);
        std::fs::set_permissions(&path, perms).expect("chmod");

        let result = write_sdl_database(&one_line(), &BTreeMap::new(), |_| PADMAP, Some(&path));
        assert!(
            matches!(result, Err(WriteError::Unreadable(_))),
            "{result:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_profile_for_a_player_who_no_longer_exists_is_cleared() {
        let dir = scratch("autoconfig");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("padmap Player 4.cfg"), "stale").expect("seed");
        std::fs::write(dir.join("Someone Else.cfg"), "theirs").expect("seed");

        let profiles: BTreeMap<u32, String> = [(1, "fresh\n".to_owned())].into_iter().collect();
        let written = write_autoconfig(&profiles, Some(&dir)).expect("write");

        assert_eq!(written.len(), 1);
        assert!(
            !dir.join("padmap Player 4.cfg").exists(),
            "a stale slot survived"
        );
        assert!(
            dir.join("Someone Else.cfg").exists(),
            "someone else's file was eaten"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("padmap Player 1.cfg")).expect("read"),
            "fresh\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_database_path_is_overridable_for_anything_that_wants_its_own() {
        let previous = std::env::var(ENV_SDL_DB).ok();
        std::env::set_var(ENV_SDL_DB, "/tmp/somewhere-else.txt");
        assert_eq!(
            sdl_database_path(),
            PathBuf::from("/tmp/somewhere-else.txt")
        );
        match previous {
            Some(value) => std::env::set_var(ENV_SDL_DB, value),
            None => std::env::remove_var(ENV_SDL_DB),
        }
    }

    #[test]
    fn the_autoconfig_directory_is_the_one_retroarch_scans_first() {
        assert!(autoconfig_dir().ends_with("autoconfig/udev"));
    }
}
