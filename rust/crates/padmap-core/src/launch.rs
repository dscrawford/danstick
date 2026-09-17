//! Parse RetroArch command line: core path and ROM path.
//! ROM identified as existing file, not by position (allows prepended/appended flags).

/// RetroArch's own spellings for "use this core".
///
/// `-L` is what every front-end emits; the long form is accepted because a
/// hand-written launch command is a perfectly ordinary thing to point
/// `padmap-play` at.
pub const CORE_FLAGS: [&str; 2] = ["-L", "--libretro"];

/// Flags that take a value, so the value is never mistaken for the ROM.
///
/// Deliberately a short list of the ones padmap or a front-end actually emits
/// rather than an attempt at RetroArch's whole grammar: an unlisted flag's
/// value can only be misread as a ROM if it also happens to be an existing
/// file, and the fallback for "no ROM identified" is the console mapping,
/// which is still better than the default.
pub const VALUE_FLAGS: [&str; 26] = [
    "-L",
    "--libretro",
    "-c",
    "--config",
    "--appendconfig",
    "-s",
    "--save",
    "-S",
    "--savestate",
    "-N",
    "--nodevice",
    "-A",
    "--dualanalog",
    "-d",
    "--device",
    "-P",
    "--bsvplay",
    "-R",
    "--bsvrecord",
    "--record",
    "--recordconfig",
    "--size",
    "--log-file",
    "--subsystem",
    "--eof-exit",
    "--max-frames",
];

/// `(core path, ROM path)` out of a RetroArch command line.
///
/// The ROM is identified by *being a file that exists* rather than by
/// position. RetroArch takes it as a positional argument, but `padmap-play`
/// prepends its own flags and a front-end may append more, so counting from
/// either end is wrong sooner or later -- and a ROM that does not exist is not
/// a game whose mapping is worth resolving.
pub fn split_args(argv: &[String], exists: impl Fn(&str) -> bool) -> (String, String) {
    let mut core = String::new();
    let mut rom = String::new();
    let mut index = 0;
    while index < argv.len() {
        let item = argv[index].as_str();
        if CORE_FLAGS.contains(&item) && index + 1 < argv.len() {
            core = argv[index + 1].clone();
            index += 2;
            continue;
        }
        if VALUE_FLAGS.contains(&item) {
            index += 2;
            continue;
        }
        if item.starts_with('-') {
            index += 1;
            continue;
        }
        // Last one wins: a launch naming several files is a subsystem load,
        // where the last is still a game and any of them identifies it about
        // equally well.
        if exists(item) {
            rom = item.to_owned();
        }
        index += 1;
    }
    (core, rom)
}

/// A human-readable name for the scope picker's entry.
///
/// Derived from the filename rather than looked up. The daemon shows this on a
/// strip beside four console names, where "close enough to recognise" is the
/// whole requirement, and a lookup would make the picker depend on a metadata
/// table that may not have this game in it.
pub fn title_for(rom: &str) -> String {
    // Trailing separators first, as `pathlib.Path(...).name` does and as
    // `scope::game_key` and `layout::for_core` already do. A directory-shaped
    // "ROM" is normal -- a PlayStation disc folder, a MAME set -- and the
    // three have to read a path the same way or they disagree about which
    // game a launch is.
    let name = rom.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let stem = match name.rfind('.') {
        Some(dot) => &name[..dot],
        None => name,
    };
    stem.replace('_', " ").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_owned()).collect()
    }

    /// Everything the test says exists.
    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn the_core_comes_from_the_flag_and_the_rom_from_the_filesystem() {
        let (core, rom) = split_args(&argv(["-L", "n64.so", "mario.z64"].as_slice()), all);
        assert_eq!(core, "n64.so");
        assert_eq!(rom, "mario.z64");
    }

    #[test]
    fn a_flags_value_is_never_mistaken_for_the_rom() {
        // --appendconfig's value is a real file. Counting positionally, or
        // taking the first path that exists, would launch the *config* as the
        // game and resolve a mapping for it.
        let (_, rom) = split_args(
            &argv(["--appendconfig", "launch.cfg", "-L", "n64.so", "mario.z64"].as_slice()),
            all,
        );
        assert_eq!(rom, "mario.z64");
    }

    #[test]
    fn a_rom_that_is_not_there_is_not_a_game() {
        let (core, rom) = split_args(&argv(["-L", "n64.so", "gone.z64"].as_slice()), |_| false);
        assert_eq!(core, "n64.so", "the core is named, not looked for");
        assert_eq!(rom, "", "nothing to resolve a per-game mapping against");
    }

    #[test]
    fn the_last_file_wins_for_a_subsystem_load() {
        let (_, rom) = split_args(&argv(["-L", "c.so", "a.sfc", "b.sfc"].as_slice()), all);
        assert_eq!(rom, "b.sfc");
    }

    #[test]
    fn unknown_flags_are_skipped_without_eating_the_rom() {
        let (_, rom) = split_args(&argv(["--fullscreen", "-v", "mario.z64"].as_slice()), all);
        assert_eq!(rom, "mario.z64");
    }

    #[test]
    fn a_core_flag_with_nothing_after_it_names_no_core() {
        let (core, rom) = split_args(&argv(["-L"].as_slice()), all);
        assert_eq!(core, "");
        assert_eq!(rom, "");
    }

    #[test]
    fn a_title_is_the_stem_with_underscores_opened_up() {
        assert_eq!(title_for("/roms/n64/Super_Mario_64.z64"), "Super Mario 64");
        assert_eq!(title_for("GoldenEye 007 (USA).z64"), "GoldenEye 007 (USA)");
        // Only the last suffix, so a name full of dots keeps them.
        assert_eq!(title_for("Zelda, The (v1.2).z64"), "Zelda, The (v1.2)");
        assert_eq!(title_for("/roms/mame/10yard"), "10yard");
        // A directory-shaped ROM keeps its name, as Path(...).name does.
        assert_eq!(
            title_for("/roms/psx/Final Fantasy VII/"),
            "Final Fantasy VII"
        );
        assert_eq!(title_for(""), "");
    }
}
