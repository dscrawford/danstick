//! Parse RetroArch command line: core path and ROM path.

/// RetroArch's spellings for "use this core" (-L or --libretro).
pub const CORE_FLAGS: [&str; 2] = ["-L", "--libretro"];

/// Flags that take a value (short list of actual flags, not complete RetroArch grammar).
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

/// Extract (core path, ROM path) from a RetroArch command line.
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
        if exists(item) {
            rom = item.to_owned();
        }
        index += 1;
    }
    (core, rom)
}

/// Human-readable title for scope picker: filename stem with underscores as spaces.
pub fn title_for(rom: &str) -> String {
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
        assert_eq!(title_for("Zelda, The (v1.2).z64"), "Zelda, The (v1.2)");
        assert_eq!(title_for("/roms/mame/10yard"), "10yard");
        assert_eq!(
            title_for("/roms/psx/Final Fantasy VII/"),
            "Final Fantasy VII"
        );
        assert_eq!(title_for(""), "");
    }
}
