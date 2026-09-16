//! Dolphin's GameCube controller bindings.
//!
//! The easiest of the four emulator targets, and worth saying why, because it
//! looks like it should be the hardest: **Dolphin's SDL backend names inputs by
//! standard gamepad element** -- `Button S`, `Left Y+`, `Pad N` -- and does the
//! per-model lookup itself. So there is no capture to translate and no table
//! per controller, which is the opposite of ares.
//!
//! That works because by the time Dolphin sees a padmap pad, the mapping
//! padmap wrote has already made it a standard SDL gamepad. `Button S` is
//! whichever physical button the user pressed when the wizard asked for A.
//!
//! Two things have to be right, and neither is the bindings:
//!
//! * **The device line.** `SDL/<n>/<name>`, where `n` counts devices already
//!   sharing that *name*. Each padmap pad's name is unique per player, so `n`
//!   is always 0 -- simpler than counting devices that share a GUID, which is
//!   what a caller binding physical pads has to do. A binding naming a device
//!   Dolphin cannot see is silently inert.
//! * **The port's device type.** A port with no controller declared in it is
//!   ignored however well its pad is bound, so `SIDevice0..3` in `Dolphin.ini`
//!   is as load-bearing as the bindings themselves.
//!
//! The strings below were copied from a `GCPadNew.ini` Dolphin itself wrote
//! for a real pad -- the same ground-truth rule padmap uses for Cemu and ares.

/// Dolphin's `SIDevices` enum, `Core/HW/SI/SI_Device.h`.
///
/// A managed port holds a standard controller; an unmanaged one is emptied
/// rather than left alone, for the same reason padmap clears an unused
/// RetroArch reservation -- a port left declared from a session with more
/// players is a phantom controller in the next game.
pub const SI_GC_CONTROLLER: u32 = 6;
pub const SI_NONE: u32 = 0;

/// Dolphin's GameCube pad has four ports and no more.
pub const MAX_PLAYERS: u32 = 4;

/// One `[GCPadN]` binding, as `key = value`.
///
/// The main stick answers to the d-pad as well -- `|` is Dolphin's "or" -- so
/// the two are interchangeable. A game reading both gets both, which is the
/// price of the d-pad working in a game that only reads the stick.
pub const BINDINGS: [(&str, &str); 22] = [
    ("Buttons/A", "`Button S`"),
    ("Buttons/B", "`Button E`"),
    ("Buttons/X", "`Button N`"),
    ("Buttons/Y", "`Button W`"),
    // GameCube Z, on the left bumper: the analogue triggers are L and R, so
    // Z has nowhere else to sit on a standard pad.
    ("Buttons/Z", "`Shoulder L`"),
    ("Buttons/Start", "`Start`"),
    ("Main Stick/Up", "`Left Y+`|`Pad N`"),
    ("Main Stick/Down", "`Left Y-`|`Pad S`"),
    ("Main Stick/Left", "`Left X-`|`Pad W`"),
    ("Main Stick/Right", "`Left X+`|`Pad E`"),
    (
        "Main Stick/Calibration",
        "100.00 141.42 100.00 141.42 100.00 141.42 100.00 141.42",
    ),
    ("C-Stick/Up", "`Right Y+`"),
    ("C-Stick/Down", "`Right Y-`"),
    ("C-Stick/Left", "`Right X-`"),
    ("C-Stick/Right", "`Right X+`"),
    (
        "C-Stick/Calibration",
        "100.00 141.42 100.00 141.42 100.00 141.42 100.00 141.42",
    ),
    ("Triggers/L", "`Trigger L`"),
    ("Triggers/R", "`Trigger R`"),
    ("Triggers/L-Analog", "`Trigger L`"),
    ("Triggers/R-Analog", "`Trigger R`"),
    ("D-Pad/Up", "`Pad N`"),
    ("D-Pad/Down", "`Pad S`"),
];

/// The d-pad bindings the table above runs out of room for.
///
/// Split only because a fixed-size array reads better than a growing one; they
/// are written together and mean nothing apart.
pub const DPAD_REST: [(&str, &str); 2] = [("D-Pad/Left", "`Pad W`"), ("D-Pad/Right", "`Pad E`")];

/// How Dolphin addresses one of padmap's pads.
///
/// The slot is always 0: it counts devices already sharing the *name*, and
/// every padmap pad is named for its player.
pub fn device(name: &str) -> String {
    format!("SDL/0/{name}")
}

/// One `[GCPadN]` section, ending in a newline.
pub fn section(port: u32, device_line: &str) -> String {
    let mut out = format!("[GCPad{port}]\nDevice = {device_line}\n");
    for (key, value) in BINDINGS.iter().chain(DPAD_REST.iter()) {
        out.push_str(&format!("{key} = {value}\n"));
    }
    out
}

/// Every managed port's section, in player order.
pub fn sections(players: &[u32], name_for: impl Fn(u32) -> String) -> String {
    let mut sorted: Vec<u32> = players
        .iter()
        .copied()
        .filter(|player| (1..=MAX_PLAYERS).contains(player))
        .collect();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
        .iter()
        .map(|player| section(*player, &device(&name_for(*player))))
        .collect()
}

/// Replace the `[GCPad1..4]` sections, leaving everything else exactly as it
/// was.
///
/// All four are dropped rather than only the ones being written: a controller
/// unplugged since the last run would otherwise keep its port.
pub fn rewrite_bindings(existing: &str, body: &str) -> String {
    let mut out = String::with_capacity(existing.len() + body.len());
    let mut dropping = false;
    for line in existing.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if bare.starts_with('[') {
            dropping = is_pad_section(bare);
        }
        if !dropping {
            out.push_str(line);
        }
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(body);
    out
}

fn is_pad_section(header: &str) -> bool {
    let Some(rest) = header.strip_prefix("[GCPad") else {
        return false;
    };
    let Some(number) = rest.strip_suffix(']') else {
        return false;
    };
    number
        .parse::<u32>()
        .is_ok_and(|port| (1..=MAX_PLAYERS).contains(&port))
}

/// What each `SIDevice` key should say, ports 1..=4 in order.
///
/// `SIDeviceN` is **zero-based** where `GCPadN` is one-based, which is the
/// kind of off-by-one that binds player one's pad and then ignores it.
pub fn si_devices(players: &[u32]) -> Vec<(String, u32)> {
    (1..=MAX_PLAYERS)
        .map(|port| {
            let kind = if players.contains(&port) {
                SI_GC_CONTROLLER
            } else {
                SI_NONE
            };
            (format!("SIDevice{}", port - 1), kind)
        })
        .collect()
}

/// Set one key of one section of an ini, adding either if it is missing.
///
/// `Dolphin.ini` holds every setting Dolphin has, so this edits rather than
/// rewrites -- the same rule as ares' `settings.bml` and Ryujinx's
/// `Config.json`.
pub fn set_ini(existing: &str, section_name: &str, key: &str, value: &str) -> String {
    let header = format!("[{section_name}]");
    let line = format!("{key} = {value}\n");
    let mut out = String::with_capacity(existing.len() + line.len());
    let mut in_section = false;
    let mut written = false;

    for raw in existing.split_inclusive('\n') {
        let bare = raw.trim_end_matches(['\n', '\r']);
        if bare.starts_with('[') {
            // Leaving the section without having written the key: write it
            // now, before the header that ends the section.
            if in_section && !written {
                out.push_str(&line);
                written = true;
            }
            in_section = bare == header;
            out.push_str(raw);
            continue;
        }
        if in_section && !written && key_of(bare) == Some(key) {
            out.push_str(&line);
            written = true;
            continue;
        }
        out.push_str(raw);
    }

    if !written {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !in_section {
            out.push_str(&header);
            out.push('\n');
        }
        out.push_str(&line);
    }
    out
}

/// The key an ini line sets, or `None` if it sets nothing.
fn key_of(line: &str) -> Option<&str> {
    let (key, _) = line.split_once('=')?;
    Some(key.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte for byte what GOTG's implementation writes, which was itself
    /// copied from a `GCPadNew.ini` Dolphin wrote for a real pad. If these
    /// drift, a GameCube game binds the wrong buttons and nothing says so.
    #[test]
    fn a_section_is_what_dolphin_itself_wrote() {
        let text = section(1, "SDL/0/padmap Player 1");
        assert!(text.starts_with("[GCPad1]\nDevice = SDL/0/padmap Player 1\n"));
        for expected in [
            "Buttons/A = `Button S`",
            "Buttons/B = `Button E`",
            "Buttons/X = `Button N`",
            "Buttons/Y = `Button W`",
            "Buttons/Z = `Shoulder L`",
            "Buttons/Start = `Start`",
            "Main Stick/Up = `Left Y+`|`Pad N`",
            "Main Stick/Down = `Left Y-`|`Pad S`",
            "Main Stick/Left = `Left X-`|`Pad W`",
            "Main Stick/Right = `Left X+`|`Pad E`",
            "C-Stick/Up = `Right Y+`",
            "Triggers/L = `Trigger L`",
            "Triggers/R-Analog = `Trigger R`",
            "D-Pad/Up = `Pad N`",
            "D-Pad/Left = `Pad W`",
            "D-Pad/Right = `Pad E`",
        ] {
            assert!(
                text.contains(expected),
                "missing {expected:?} from:\n{text}"
            );
        }
        // The stick answers to the d-pad as well, so a game that only reads
        // the stick still works from the d-pad.
        assert!(text.contains("`Left Y+`|`Pad N`"));
    }

    #[test]
    fn a_padmap_pad_is_always_slot_zero() {
        // Each pad's name is unique per player, so nothing ever shares one.
        assert_eq!(device("padmap Player 3"), "SDL/0/padmap Player 3");
    }

    #[test]
    fn every_pad_section_is_replaced_and_everything_else_is_kept() {
        let existing = "[Core]\nSIDevice0 = 6\n[GCPad1]\nDevice = SDL/0/old\nButtons/A = `x`\n\
                        [GCPad4]\nDevice = SDL/0/gone\n[DSUClient]\nServer = 127.0.0.1\n";
        let body = section(1, "SDL/0/padmap Player 1");
        let out = rewrite_bindings(existing, &body);
        assert!(out.contains("[Core]\nSIDevice0 = 6\n"), "{out}");
        assert!(out.contains("[DSUClient]\nServer = 127.0.0.1"), "{out}");
        // The stale port went, not just the one being rewritten: a controller
        // unplugged since last time would otherwise keep its port.
        assert!(!out.contains("SDL/0/gone"), "{out}");
        assert!(!out.contains("SDL/0/old"), "{out}");
        assert!(out.contains("Device = SDL/0/padmap Player 1"), "{out}");
    }

    #[test]
    fn a_section_that_is_not_a_pad_is_left_alone() {
        // `[GCPad5]` is not a port Dolphin has, and `[GCPadWii]` is somebody
        // else's section entirely.
        assert!(is_pad_section("[GCPad1]"));
        assert!(is_pad_section("[GCPad4]"));
        assert!(!is_pad_section("[GCPad5]"));
        assert!(!is_pad_section("[GCPad0]"));
        assert!(!is_pad_section("[GCPadWii]"));
        assert!(!is_pad_section("[Core]"));
    }

    #[test]
    fn writing_into_nothing_at_all_still_produces_a_file() {
        let out = rewrite_bindings("", &section(2, "SDL/0/padmap Player 2"));
        assert!(out.starts_with("[GCPad2]"), "{out}");
    }

    #[test]
    fn an_unmanaged_port_is_emptied_rather_than_left_alone() {
        // Ports are zero-based here and one-based in the section names, which
        // is exactly the off-by-one that binds a pad and then ignores it.
        let devices = si_devices(&[1, 2]);
        assert_eq!(
            devices,
            vec![
                ("SIDevice0".to_owned(), SI_GC_CONTROLLER),
                ("SIDevice1".to_owned(), SI_GC_CONTROLLER),
                ("SIDevice2".to_owned(), SI_NONE),
                ("SIDevice3".to_owned(), SI_NONE),
            ]
        );
    }

    #[test]
    fn setting_a_key_that_is_there_changes_only_that_line() {
        let existing = "[Core]\nSIDevice0 = 0\nCPUCore = 1\n[Display]\nFullscreen = True\n";
        let out = set_ini(existing, "Core", "SIDevice0", "6");
        assert_eq!(
            out,
            "[Core]\nSIDevice0 = 6\nCPUCore = 1\n[Display]\nFullscreen = True\n"
        );
    }

    #[test]
    fn a_missing_key_lands_in_its_own_section_rather_than_the_next_one() {
        let existing = "[Core]\nCPUCore = 1\n[Display]\nFullscreen = True\n";
        let out = set_ini(existing, "Core", "SIDevice0", "6");
        assert_eq!(
            out,
            "[Core]\nCPUCore = 1\nSIDevice0 = 6\n[Display]\nFullscreen = True\n"
        );
    }

    #[test]
    fn a_missing_section_is_created() {
        let out = set_ini("[Display]\nFullscreen = True\n", "Core", "SIDevice0", "6");
        assert!(out.contains("[Display]\nFullscreen = True\n"), "{out}");
        assert!(out.ends_with("[Core]\nSIDevice0 = 6\n"), "{out}");
    }

    #[test]
    fn setting_the_same_key_twice_changes_nothing_the_second_time() {
        let once = set_ini("", "Core", "SIDevice0", "6");
        let twice = set_ini(&once, "Core", "SIDevice0", "6");
        assert_eq!(once, twice);
    }

    #[test]
    fn a_key_whose_name_is_a_prefix_of_another_is_not_confused_for_it() {
        // SIDevice1 must not be mistaken for SIDevice10, nor the reverse.
        let existing = "[Core]\nSIDevice10 = 1\nSIDevice1 = 0\n";
        let out = set_ini(existing, "Core", "SIDevice1", "6");
        assert_eq!(out, "[Core]\nSIDevice10 = 1\nSIDevice1 = 6\n");
    }

    #[test]
    fn only_the_four_real_ports_get_a_section() {
        let text = sections(&[1, 2, 5, 0], |player| format!("padmap Player {player}"));
        assert!(text.contains("[GCPad1]") && text.contains("[GCPad2]"));
        assert!(!text.contains("[GCPad5]"), "Dolphin has four ports");
        assert!(!text.contains("[GCPad0]"));
    }
}
