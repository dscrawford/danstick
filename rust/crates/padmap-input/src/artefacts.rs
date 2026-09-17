//! Writing the files other programs read.
//!
//! The decisions are in `padmap_core::emit`; this is where they meet a disk.
//! Both destinations are shared with somebody: the SDL database has the user's
//! own hand-written lines in it, and the autoconfig directory is scanned whole
//! by RetroArch. So both are rewritten rather than appended, and both clear
//! what padmap wrote last time.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use log::warn;
use padmap_core::emit::{self, Identity};

/// Where padmap writes the SDL database it generates.
///
/// padmap's own directory, and overridable, because any SDL program can be
/// pointed at it:
///
/// ```text
/// SDL_GAMECONTROLLERCONFIG_FILE=~/.config/padmap/sdl_controllers.txt
/// ```
pub const ENV_SDL_DB: &str = "PADMAP_SDL_DB";

pub fn sdl_database_path() -> PathBuf {
    if let Ok(path) = std::env::var(ENV_SDL_DB) {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    config_dir().join("padmap").join("sdl_controllers.txt")
}

fn config_dir() -> PathBuf {
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => home().join(".config"),
    }
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// The autoconfig directory padmap hands RetroArch for a launch.
///
/// The `udev` subdirectory matters: RetroArch looks in `<dir>/<driver>` first
/// and only falls back to the base directory when that is empty.
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

/// Replace padmap's lines in the SDL database, keeping every other.
///
/// Refuses if the file exists and cannot be read. A read that failed cannot
/// tell "the file is empty" from "the file is there and I could not see it",
/// and publishing that difference is how a user's hand-written mapping is lost
/// on the next wizard run -- silently, which is the part that matters.
pub fn write_sdl_database(
    lines: &BTreeMap<u32, String>,
    notes: &BTreeMap<u32, String>,
    identity_for: impl Fn(u32) -> Identity,
    path: Option<&Path>,
) -> Result<PathBuf, WriteError> {
    let target = path
        .map(Path::to_path_buf)
        .unwrap_or_else(sdl_database_path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| WriteError::Io(parent.to_path_buf(), error))?;
    }

    let existing = match std::fs::read(&target) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        // No database yet -- a first run, or a symlink whose target has still
        // to be created. Nothing to preserve, so nothing to lose.
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
    std::fs::write(&target, body).map_err(|error| WriteError::Io(target.clone(), error))?;
    Ok(target)
}

/// Write one autoconfig profile per virtual pad, clearing previous ones.
///
/// A profile for a player who no longer exists would still be scanned, and
/// could match a pad it was never meant for.
pub fn write_autoconfig(
    profiles: &BTreeMap<u32, String>,
    dir: Option<&Path>,
) -> Result<Vec<PathBuf>, WriteError> {
    let target = dir.map(Path::to_path_buf).unwrap_or_else(autoconfig_dir);
    std::fs::create_dir_all(&target).map_err(|error| WriteError::Io(target.clone(), error))?;

    if let Ok(entries) = std::fs::read_dir(&target) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(emit::VIRTUAL_PREFIX) && name.ends_with(".cfg") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    let mut written = Vec::new();
    for (player, text) in profiles {
        let path = target.join(format!("{}.cfg", emit::virtual_name(*player)));
        std::fs::write(&path, text).map_err(|error| WriteError::Io(path.clone(), error))?;
        written.push(path);
    }
    Ok(written)
}

/// Where Cemu keeps its per-player controller profiles.
pub fn cemu_profile_dir() -> PathBuf {
    if let Ok(path) = std::env::var("PADMAP_CEMU_DIR") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    config_home().join("Cemu").join("controllerProfiles")
}

fn config_home() -> PathBuf {
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => match std::env::var("HOME") {
            Ok(home) if !home.is_empty() => PathBuf::from(home).join(".config"),
            _ => PathBuf::from(".config"),
        },
    }
}

/// Write a Cemu profile per player, and report the paths written.
///
/// Only up to Cemu's own eight slots. A ninth player gets no profile rather
/// than one wrapped round to `controller0.xml`, which would take player one's
/// pad away.
///
/// **Not sufficient on its own.** Cemu reads no mapping database, so a pad SDL
/// does not already recognise as a gamepad never appears in its list at all --
/// padmap's mapping has to reach it through `SDL_GAMECONTROLLERCONFIG` in the
/// environment Cemu is launched with. See `padmap_core::cemu::CONFIG_ENV`.
///
/// Existing profiles for players padmap is not managing are left alone: they
/// are the user's, and a session with two players has no business clearing the
/// profile someone set up by hand for player three.
pub fn write_cemu_profiles(
    players: &[u32],
    guid_for: impl Fn(u32) -> String,
    name_for: impl Fn(u32) -> String,
    dir: Option<&Path>,
) -> Result<Vec<PathBuf>, WriteError> {
    let target = dir.map(Path::to_path_buf).unwrap_or_else(cemu_profile_dir);
    std::fs::create_dir_all(&target).map_err(|error| WriteError::Io(target.clone(), error))?;

    let mut written = Vec::new();
    for &player in players {
        if player == 0 || player > padmap_core::cemu::MAX_PLAYERS {
            continue;
        }
        let path = target.join(padmap_core::cemu::profile_filename(player));
        let body = padmap_core::cemu::profile(player, &guid_for(player), &name_for(player));
        std::fs::write(&path, body).map_err(|error| WriteError::Io(path.clone(), error))?;
        written.push(path);
    }
    Ok(written)
}

/// Where Dolphin keeps its configuration.
pub fn dolphin_config_dir() -> PathBuf {
    if let Ok(path) = std::env::var("PADMAP_DOLPHIN_DIR") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    config_home().join("dolphin-emu")
}

/// Write Dolphin's GameCube bindings and declare a controller in each port.
///
/// Two files, and both matter: `GCPadNew.ini` binds the pads, and a port with
/// no controller declared in `Dolphin.ini` is ignored however well its pad is
/// bound.
///
/// Unlike ares and Ryujinx, this *does* write into a directory that does not
/// exist yet. Dolphin creates its config on first run, but it reads these
/// files at startup either way, so bindings are worth having on the first run
/// too -- and neither file is the whole of Dolphin's settings the way
/// `settings.bml` is the whole of ares'. `Dolphin.ini` is edited key by key;
/// `GCPadNew.ini` keeps every section that is not a GameCube port.
pub fn write_dolphin_config(
    players: &[u32],
    name_for: impl Fn(u32) -> String,
    dir: Option<&Path>,
) -> Result<Vec<PathBuf>, WriteError> {
    use padmap_core::dolphin;

    let target = dir
        .map(Path::to_path_buf)
        .unwrap_or_else(dolphin_config_dir);
    std::fs::create_dir_all(&target).map_err(|error| WriteError::Io(target.clone(), error))?;

    let bindings = target.join("GCPadNew.ini");
    let existing = std::fs::read(&bindings)
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
        .unwrap_or_default();
    let body = dolphin::sections(players, name_for);
    std::fs::write(&bindings, dolphin::rewrite_bindings(&existing, &body))
        .map_err(|error| WriteError::Io(bindings.clone(), error))?;

    let core = target.join("Dolphin.ini");
    let mut text = std::fs::read(&core)
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
        .unwrap_or_default();
    for (key, kind) in dolphin::si_devices(players) {
        text = dolphin::set_ini(&text, "Core", &key, &kind.to_string());
    }
    std::fs::write(&core, text).map_err(|error| WriteError::Io(core.clone(), error))?;

    Ok(vec![bindings, core])
}

/// Where ares keeps its settings.
pub fn ares_settings_path() -> PathBuf {
    if let Ok(path) = std::env::var("PADMAP_ARES_SETTINGS") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    data_home().join("ares").join("settings.bml")
}

/// Where Ryujinx keeps its configuration.
pub fn ryujinx_config_path() -> PathBuf {
    if let Ok(path) = std::env::var("PADMAP_RYUJINX_CONFIG") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    config_home().join("Ryujinx").join("Config.json")
}

fn data_home() -> PathBuf {
    match std::env::var("XDG_DATA_HOME") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => home().join(".local").join("share"),
    }
}

/// Replace the `VirtualPadN` blocks padmap manages, leaving the rest of the
/// file alone.
///
/// settings.bml holds every setting ares has -- video, audio, per-system
/// paths, hotkeys -- so rewriting it from scratch would throw away everything
/// the user configured. Only the blocks for the players padmap is binding are
/// replaced; a port it is not managing keeps whatever was there.
pub fn rewrite_ares_settings(existing: &str, blocks: &BTreeMap<u32, String>) -> String {
    let mut out = String::with_capacity(existing.len());
    let mut skipping: Option<u32> = None;
    let mut replaced: BTreeSet<u32> = BTreeSet::new();
    for line in existing.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if let Some(rest) = bare.strip_prefix("VirtualPad") {
            // A new top-level block ends whatever was being skipped.
            let player: Option<u32> = rest.parse().ok();
            skipping = player.filter(|player| blocks.contains_key(player));
            if let Some(player) = skipping {
                out.push_str(&blocks[&player]);
                replaced.insert(player);
                continue;
            }
        } else if skipping.is_some() && !bare.starts_with("  ") {
            // Any line that is not indented under the block we are dropping.
            skipping = None;
        }
        if skipping.is_none() {
            out.push_str(line);
        }
    }

    // A port ares has never written has no block to replace, and a binding
    // that is simply absent is indistinguishable from one that failed: ares
    // starts, the pad is listed, and nothing is bound. Append the ones that
    // were not found rather than dropping them.
    let missing = blocks
        .iter()
        .filter(|(player, _)| !replaced.contains(player));
    for (_, block) in missing {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(block);
    }
    out
}

/// Write ares' settings, replacing only padmap's ports.
///
/// Refuses rather than writes if ares has never run: its settings file holds
/// every other setting too, and inventing one from nothing would leave ares
/// with padmap's ports and defaults for everything else.
pub fn write_ares_settings(
    blocks: &BTreeMap<u32, String>,
    path: Option<&Path>,
) -> Result<PathBuf, WriteError> {
    let target = path
        .map(Path::to_path_buf)
        .unwrap_or_else(ares_settings_path);
    let existing =
        std::fs::read_to_string(&target).map_err(|error| WriteError::Io(target.clone(), error))?;
    std::fs::write(&target, rewrite_ares_settings(&existing, blocks))
        .map_err(|error| WriteError::Io(target.clone(), error))?;
    Ok(target)
}

/// Write Ryujinx's `input_config`, keeping every other setting in the file.
///
/// Same reasoning as ares: Config.json is the whole of Ryujinx's
/// configuration, so only the one key is replaced, and only the players padmap
/// is binding within it.
pub fn write_ryujinx_config(
    entries: Vec<serde_json::Value>,
    path: Option<&Path>,
) -> Result<PathBuf, WriteError> {
    let target = path
        .map(Path::to_path_buf)
        .unwrap_or_else(ryujinx_config_path);
    let text =
        std::fs::read_to_string(&target).map_err(|error| WriteError::Io(target.clone(), error))?;
    let mut config: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        WriteError::Io(
            target.clone(),
            std::io::Error::new(std::io::ErrorKind::InvalidData, error),
        )
    })?;
    let merged = padmap_core::ryujinx::merge(
        config
            .get("input_config")
            .unwrap_or(&serde_json::Value::Null),
        entries,
    );
    config["input_config"] = merged;
    // Ryujinx writes this pretty-printed; matching it keeps the diff a user
    // sees to the lines padmap actually changed.
    let body = serde_json::to_string_pretty(&config).map_err(|error| {
        WriteError::Io(
            target.clone(),
            std::io::Error::new(std::io::ErrorKind::InvalidData, error),
        )
    })?;
    std::fs::write(&target, body + "\n").map_err(|error| WriteError::Io(target.clone(), error))?;
    Ok(target)
}

/// RetroArch's own config directory.
pub fn retroarch_config_dir() -> PathBuf {
    match std::env::var("RETROARCH_CONFIG_DIR") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => home().join(".config").join("retroarch"),
    }
}

/// Where to look for existing joypad profiles, most specific first.
///
/// `PADMAP_AUTOCONFIG_DIRS` is what the flake sets; without it the module
/// falls back to globbing the Nix store, which can pick an older autoconfig
/// package at random.
pub fn autoconfig_dirs() -> Vec<PathBuf> {
    // Once per process. Listing /nix/store is tens of thousands of entries,
    // and this is reached from the daemon's tick -- on the thread that
    // forwards controller events. The set of installed databases does not
    // change while the daemon runs; a new one arrives with a new daemon.
    static DIRS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    DIRS.get_or_init(scan_autoconfig_dirs).clone()
}

fn scan_autoconfig_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(value) = std::env::var("PADMAP_AUTOCONFIG_DIRS") {
        dirs.extend(
            value
                .split(':')
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
        );
    }
    dirs.push(retroarch_config_dir().join("autoconfig"));
    dirs.push(PathBuf::from(
        "/run/current-system/sw/share/libretro/autoconfig",
    ));
    // Nix has no global share dir; fall back to the store paths directly,
    // newest name first.
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

/// Every `.cfg` under a directory, in sorted order.
fn profiles_under(base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
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

/// The best existing libretro profile for a physical pad.
///
/// Exact name match wins; a vid/pid match is the fallback, mirroring how
/// RetroArch scores autoconfig candidates. Returns the file's name and its
/// settings in file order, which is what [`padmap_core::retroarch::derive_profile`]
/// copies.
pub fn find_profile(name: &str, vid: u16, pid: u16) -> Option<(String, Vec<(String, String)>)> {
    // Memoised for the same reason the directory list is: a lookup reads
    // every profile in every database, a thousand files and more, and it is
    // asked on every hotplug and every republish for every pad. The answer
    // for a given controller does not change while the daemon runs.
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
            let Ok(raw) = std::fs::read(&path) else {
                continue;
            };
            let text = String::from_utf8_lossy(&raw);
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

/// Files SDL itself would read a mapping out of, the user's own first.
///
/// A line in the user's file was either written by a Gamepad Editor or typed
/// by hand, and either way it is a statement about this machine rather than
/// a database's guess about a product line. The legacy location is where the
/// database lived while padmap shipped a particular front-end; dropping it
/// would orphan exactly the mappings this exists to carry over.
pub fn sdl_database_paths() -> Vec<PathBuf> {
    let mut paths = vec![sdl_database_path()];
    let legacy = config_home()
        .join("pegasus-frontend")
        .join("sdl_controllers.txt");
    if !paths.contains(&legacy) {
        paths.push(legacy);
    }
    if let Ok(value) = std::env::var("SDL_GAMECONTROLLERCONFIG_FILE") {
        paths.extend(
            value
                .split(':')
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
        );
    }
    paths
}

/// A mapping for this GUID already on disk, and where it came from.
///
/// For a pad nobody has mapped through padmap: a line the user wrote for the
/// physical controller is worth more than a guess, and it is carried over to
/// the clone. padmap's own lines are skipped -- one written for a virtual pad
/// cannot also be the physical pad's, and matching one would let a stale
/// generation feed itself back in.
pub fn carried_fields(guid: &str) -> Option<(padmap_core::fields::Fields, String)> {
    if guid.is_empty() {
        return None;
    }
    for path in sdl_database_paths() {
        let text = match std::fs::read(&path) {
            Ok(raw) => String::from_utf8_lossy(&raw).into_owned(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                // A database that is there and unreadable is not the same
                // thing as absent, and skipping it quietly means a user's own
                // mapping is passed over in favour of a guess.
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
            if found != guid || name.starts_with(emit::VIRTUAL_PREFIX) {
                continue;
            }
            return Some((binding_fields(&fields), path.display().to_string()));
        }
    }
    None
}

/// Fields that describe the pad rather than bind it, dropped when a line is
/// carried over: the clone has its own identity.
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

/// Write the launch override, creating its directory.
pub fn write_launch_config(path: &Path, text: &str) -> Result<(), WriteError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| WriteError::Io(parent.to_path_buf(), error))?;
    }
    std::fs::write(path, text).map_err(|error| WriteError::Io(path.to_path_buf(), error))
}

/// Persist the `--nodevice` flags, one token per line, for the launch wrapper.
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
        // Through the environment the flake sets, without touching the
        // process's own: a temporary child sees it.
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

    /// The other half of the test above, run in a child with the environment
    /// set. Prints rather than asserts; the parent checks.
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
        // "Could not read" became "was empty" once, and a user lost their
        // hand-written mappings on the next wizard run.
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
        // It would still be scanned, and could match a pad it was never for.
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
        // RetroArch looks in <dir>/<driver> before the base directory.
        assert!(autoconfig_dir().ends_with("autoconfig/udev"));
    }
}
