//! Dolphin GameCube pad bindings via SDL backend.

/// Unmanaged ports emptied to avoid phantom controllers from prior sessions.
pub const SI_GC_CONTROLLER: u32 = 6;
pub const SI_NONE: u32 = 0;

/// Dolphin's GameCube pad has four ports and no more.
pub const MAX_PLAYERS: u32 = 4;

/// One `[GCPadN]` binding, as `key = value`.
pub const BINDINGS: [(&str, &str); 22] = [
    ("Buttons/A", "`Button S`"),
    ("Buttons/B", "`Button E`"),
    ("Buttons/X", "`Button N`"),
    ("Buttons/Y", "`Button W`"),
    ("Buttons/Z", "`Shoulder L`"), // Z on left bumper (triggers are L/R)
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

pub const DPAD_REST: [(&str, &str); 2] = [("D-Pad/Left", "`Pad W`"), ("D-Pad/Right", "`Pad E`")];

/// Dolphin's own default device on X11: the highest-priority one it finds.
pub const KEYBOARD_DEVICE: &str = "XInput2/0/Virtual core pointer";

/// Dolphin's own keyboard defaults (`GCPad::LoadDefaults`, Linux branch).
pub const KEYBOARD_BINDINGS: [(&str, &str); 24] = [
    ("Buttons/A", "`X`"),
    ("Buttons/B", "`Z`"),
    ("Buttons/X", "`C`"),
    ("Buttons/Y", "`S`"),
    ("Buttons/Z", "`D`"),
    ("Buttons/Start", "`Return`"),
    ("Main Stick/Up", "`Up`"),
    ("Main Stick/Down", "`Down`"),
    ("Main Stick/Left", "`Left`"),
    ("Main Stick/Right", "`Right`"),
    ("Main Stick/Modifier", "`Shift`"),
    (
        "Main Stick/Calibration",
        "100.00 141.42 100.00 141.42 100.00 141.42 100.00 141.42",
    ),
    ("C-Stick/Up", "`I`"),
    ("C-Stick/Down", "`K`"),
    ("C-Stick/Left", "`J`"),
    ("C-Stick/Right", "`L`"),
    ("C-Stick/Modifier", "`Ctrl`"),
    (
        "C-Stick/Calibration",
        "100.00 141.42 100.00 141.42 100.00 141.42 100.00 141.42",
    ),
    ("Triggers/L", "`Q`"),
    ("Triggers/R", "`W`"),
    ("D-Pad/Up", "`T`"),
    ("D-Pad/Down", "`G`"),
    ("D-Pad/Left", "`F`"),
    ("D-Pad/Right", "`H`"),
];

/// Dolphin has four Wii Remotes, as it has four GameCube ports.
pub const MAX_WIIMOTES: u32 = 4;

/// `[WiimoteN] Source`: 0 none, 1 emulated, 2 a real remote (`WiimoteSource`).
pub const WIIMOTE_NONE: u32 = 0;
pub const WIIMOTE_EMULATED: u32 = 1;

/// The square gate Dolphin sets when a stick is driven by keys, in its own
/// words: "Because our defaults use keyboard input, set calibration shapes to
/// squares."
const SQUARE_GATE: &str = "100.00 141.42 100.00 141.42 100.00 141.42 100.00 141.42";

/// One `[WiimoteN]` binding for a padmap clone, in the SDL backend's own
/// element names -- the same vocabulary as `BINDINGS`, so the per-model lookup
/// is still Dolphin's job. It points with the right stick, on the signs
/// `BINDINGS` already uses for the C-stick, because a pad has no pointer.
pub const WIIMOTE_BINDINGS: [(&str, &str); 19] = [
    ("Buttons/A", "`Button S`"),
    ("Buttons/B", "`Trigger R`"),
    ("Buttons/1", "`Button E`"),
    ("Buttons/2", "`Button W`"),
    ("Buttons/-", "`Select`"),
    ("Buttons/+", "`Start`"),
    ("Buttons/Home", "`Button N`"),
    ("D-Pad/Up", "`Pad N`"),
    ("D-Pad/Down", "`Pad S`"),
    ("D-Pad/Left", "`Pad W`"),
    ("D-Pad/Right", "`Pad E`"),
    ("IR/Up", "`Right Y+`"),
    ("IR/Down", "`Right Y-`"),
    ("IR/Left", "`Right X-`"),
    ("IR/Right", "`Right X+`"),
    ("Shake/X", "`Shoulder L`"),
    ("Shake/Y", "`Shoulder L`"),
    ("Shake/Z", "`Shoulder L`"),
    ("Extension", "Nunchuk"),
];

/// The Nunchuk on a clone: the left stick, with C and Z on the shoulder and
/// trigger the remote's own B does not use.
pub const WIIMOTE_NUNCHUK: [(&str, &str); 6] = [
    ("Nunchuk/Stick/Up", "`Left Y+`"),
    ("Nunchuk/Stick/Down", "`Left Y-`"),
    ("Nunchuk/Stick/Left", "`Left X-`"),
    ("Nunchuk/Stick/Right", "`Left X+`"),
    ("Nunchuk/Buttons/C", "`Shoulder R`"),
    ("Nunchuk/Buttons/Z", "`Trigger L`"),
];

/// Dolphin's own Wii Remote 1 defaults on the mouse and keyboard
/// (`WiimoteEmu::Wiimote::LoadDefaults`, `HAVE_X11`, `Nunchuk::LoadDefaults`);
/// padmap only moves the section to the seat that owns the mouse.
pub const WIIMOTE_KEYBOARD_BINDINGS: [(&str, &str); 25] = [
    ("Buttons/A", "`Click 1`"),
    ("Buttons/B", "`Click 3`"),
    ("Buttons/1", "`1`"),
    ("Buttons/2", "`2`"),
    ("Buttons/-", "`Q`"),
    ("Buttons/+", "`E`"),
    ("Buttons/Home", "`Return`"),
    ("Shake/X", "`Click 2`"),
    ("Shake/Y", "`Click 2`"),
    ("Shake/Z", "`Click 2`"),
    ("IR/Up", "`Cursor Y-`"),
    ("IR/Down", "`Cursor Y+`"),
    ("IR/Left", "`Cursor X-`"),
    ("IR/Right", "`Cursor X+`"),
    ("D-Pad/Up", "`Up`"),
    ("D-Pad/Down", "`Down`"),
    ("D-Pad/Left", "`Left`"),
    ("D-Pad/Right", "`Right`"),
    ("Extension", "Nunchuk"),
    ("Nunchuk/Stick/Up", "`W`"),
    ("Nunchuk/Stick/Down", "`S`"),
    ("Nunchuk/Stick/Left", "`A`"),
    ("Nunchuk/Stick/Right", "`D`"),
    ("Nunchuk/Buttons/C", "`Control_L`"),
    ("Nunchuk/Buttons/Z", "`Shift_L`"),
];

/// One `[WiimoteN]` section, ending in a newline. `Source` leads it: a remote
/// Dolphin is not sourcing is ignored however well it is bound.
fn wiimote_section<'a>(
    port: u32,
    device_line: &str,
    bindings: impl Iterator<Item = &'a (&'a str, &'a str)>,
) -> String {
    let mut out = format!("[Wiimote{port}]\nSource = {WIIMOTE_EMULATED}\nDevice = {device_line}\n");
    for (key, value) in bindings {
        out.push_str(&format!("{key} = {value}\n"));
    }
    out
}

/// A remote nobody holds: sourced from nothing, for the same reason an
/// unmanaged GameCube port is `SIDEVICE_NONE` -- a remote still declared from
/// a session with more players is a phantom in the next game.
fn wiimote_off(port: u32) -> String {
    format!("[Wiimote{port}]\nSource = {WIIMOTE_NONE}\n")
}

/// The remote the keyboard's seat takes: its seat, else the first of the four
/// no pad holds. The same rule as the GameCube port, so one person is one
/// player on both sides of Dolphin.
pub fn wiimote_port(players: &[u32], seat: Option<u32>) -> Option<u32> {
    crate::keyboard::port(seat, players, MAX_WIIMOTES)
}

/// Every remote, in port order. Dolphin emulates remote 1 on the mouse and
/// keyboard on a fresh install and leaves 2-4 off, so seating a pad there
/// first gave that player the mouse's remote and the keyboard's seat none.
pub fn wiimote_sections(
    players: &[u32],
    seat: Option<u32>,
    name_for: impl Fn(u32) -> String,
) -> String {
    let mut sorted: Vec<u32> = players
        .iter()
        .copied()
        .filter(|player| (1..=MAX_WIIMOTES).contains(player))
        .collect();
    sorted.sort_unstable();
    sorted.dedup();
    let keyboard = wiimote_port(&sorted, seat);
    (1..=MAX_WIIMOTES)
        .map(|port| {
            if sorted.contains(&port) {
                // No calibration: a stick's gate is round, and Dolphin's
                // default already says so.
                wiimote_section(
                    port,
                    &device(&name_for(port)),
                    WIIMOTE_BINDINGS.iter().chain(WIIMOTE_NUNCHUK.iter()),
                )
            } else if keyboard == Some(port) {
                let mut section =
                    wiimote_section(port, KEYBOARD_DEVICE, WIIMOTE_KEYBOARD_BINDINGS.iter());
                // Dolphin squares the gate itself when keys drive a stick.
                section.push_str("Nunchuk/Stick/Calibration = ");
                section.push_str(SQUARE_GATE);
                section.push('\n');
                section
            } else {
                wiimote_off(port)
            }
        })
        .collect()
}

/// Replace all `[Wiimote1..4]` sections, keeping everything else in the file,
/// `[BalanceBoard]` among it. A remote the user paired for real lives *in* one
/// of these sections as `Source = 2` and is replaced with the seat padmap has
/// given that port: padmap's answer to who player N is has to be the only one.
pub fn rewrite_wiimotes(existing: &str, body: &str) -> String {
    rewrite_sections(existing, body, is_wiimote_section)
}

fn is_wiimote_section(header: &str) -> bool {
    numbered_section(header, "[Wiimote", MAX_WIIMOTES)
}

fn numbered_section(header: &str, prefix: &str, max: u32) -> bool {
    header
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|number| number.parse::<u32>().ok())
        .is_some_and(|port| (1..=max).contains(&port))
}

/// The keyboard's `[GCPadN]` section: what Dolphin itself writes for port 1 on a fresh install.
pub fn keyboard_section(port: u32) -> String {
    let mut out = format!("[GCPad{port}]\nDevice = {KEYBOARD_DEVICE}\n");
    for (key, value) in KEYBOARD_BINDINGS {
        out.push_str(&format!("{key} = {value}\n"));
    }
    out
}

/// The port the keyboard takes: its seat, else the first of the four no pad holds.
pub fn keyboard_port(players: &[u32], seat: Option<u32>) -> Option<u32> {
    crate::keyboard::port(seat, players, MAX_PLAYERS)
}

/// Slot always 0 (it counts devices sharing a name; padmap pads are unique per
/// player), on one line so a name cannot close the section it is written into.
pub fn device(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    format!("SDL/0/{}", name.trim())
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
pub fn sections(players: &[u32], seat: Option<u32>, name_for: impl Fn(u32) -> String) -> String {
    let mut sorted: Vec<u32> = players
        .iter()
        .copied()
        .filter(|player| (1..=MAX_PLAYERS).contains(player))
        .collect();
    sorted.sort_unstable();
    sorted.dedup();
    let keyboard = keyboard_port(&sorted, seat);
    (1..=MAX_PLAYERS)
        .filter_map(|port| {
            if sorted.contains(&port) {
                Some(section(port, &device(&name_for(port))))
            } else if keyboard == Some(port) {
                Some(keyboard_section(port))
            } else {
                None
            }
        })
        .collect()
}

/// Replace all `[GCPad1..4]` sections to clear unplugged controller ports.
pub fn rewrite_bindings(existing: &str, body: &str) -> String {
    rewrite_sections(existing, body, is_pad_section)
}

/// Drop every section `ours` claims and append `body`, keeping the rest of
/// the file: neither ini is only padmap's.
fn rewrite_sections(existing: &str, body: &str, ours: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(existing.len() + body.len());
    let mut dropping = false;
    for line in existing.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if bare.starts_with('[') {
            dropping = ours(bare);
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
    numbered_section(header, "[GCPad", MAX_PLAYERS)
}

/// `SIDeviceN` zero-based; `GCPadN` one-based. The keyboard's port counts as a controller.
pub fn si_devices(players: &[u32], seat: Option<u32>) -> Vec<(String, u32)> {
    let keyboard = keyboard_port(players, seat);
    (1..=MAX_PLAYERS)
        .map(|port| {
            let kind = if players.contains(&port) || keyboard == Some(port) {
                SI_GC_CONTROLLER
            } else {
                SI_NONE
            };
            (format!("SIDevice{}", port - 1), kind)
        })
        .collect()
}

/// Edit ini in place, adding section or key if missing.
pub fn set_ini(existing: &str, section_name: &str, key: &str, value: &str) -> String {
    let header = format!("[{section_name}]");
    let line = format!("{key} = {value}\n");
    let mut out = String::with_capacity(existing.len() + line.len());
    let mut in_section = false;
    let mut written = false;

    for raw in existing.split_inclusive('\n') {
        let bare = raw.trim_end_matches(['\n', '\r']);
        if bare.starts_with('[') {
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

/// Appears as `DSUClient/<n>/padmap` in Dolphin.
pub const DSU_DESCRIPTION: &str = "padmap";

/// DSU server entry: `[Server]` with `Enabled` and `Entries`.
pub fn dsu_client_ini(existing: &str) -> String {
    let ours_at = format!("{}:{}", crate::dsu::HOST, crate::dsu::PORT);
    let mut entries = String::new();
    for entry in get_ini(existing, "Server", "Entries")
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let is_ours = entry.split_once(':').is_some_and(|(description, address)| {
            description == DSU_DESCRIPTION || address == ours_at
        });
        if !is_ours {
            entries.push_str(entry);
            entries.push(';');
        }
    }
    entries.push_str(&format!("{DSU_DESCRIPTION}:{ours_at};"));
    let text = set_ini(existing, "Server", "Enabled", "True");
    set_ini(&text, "Server", "Entries", &entries)
}

/// One key of one section, if it is set.
pub fn get_ini(existing: &str, section_name: &str, key: &str) -> Option<String> {
    let header = format!("[{section_name}]");
    let mut in_section = false;
    for raw in existing.lines() {
        let bare = raw.trim_end_matches('\r');
        if bare.starts_with('[') {
            in_section = bare == header;
            continue;
        }
        if in_section && key_of(bare) == Some(key) {
            return bare
                .split_once('=')
                .map(|(_, value)| value.trim().to_owned());
        }
    }
    None
}

fn key_of(line: &str) -> Option<&str> {
    let (key, _) = line.split_once('=')?;
    Some(key.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(text.contains("`Left Y+`|`Pad N`"));
    }

    #[test]
    fn a_name_cannot_close_the_remote_it_is_written_into() {
        // The sibling of the GCPad test below: a remote's device line comes
        // from the same caller-supplied name, so it gets the same scrubbing.
        let forged = "X\n[Wiimote3]\nSource = 1\nDevice = SDL/0/X";
        let text = wiimote_sections(&[1, 2, 3, 4], None, |_| forged.to_owned());
        let headers: Vec<&str> = text.lines().filter(|line| line.starts_with('[')).collect();
        assert_eq!(
            headers,
            ["[Wiimote1]", "[Wiimote2]", "[Wiimote3]", "[Wiimote4]"],
            "a name opened a section of its own:\n{text}"
        );
        let sourced = text
            .lines()
            .filter(|line| line.starts_with("Source"))
            .count();
        assert_eq!(sourced, 4, "a name sourced a remote of its own:\n{text}");
        let devices = text
            .lines()
            .filter(|line| line.starts_with("Device"))
            .count();
        assert_eq!(devices, 4, "a name bound a device of its own:\n{text}");
    }

    #[test]
    fn a_padmap_pad_is_always_slot_zero() {
        assert_eq!(device("padmap Player 3"), "SDL/0/padmap Player 3");
    }

    #[test]
    fn a_name_cannot_close_the_section_it_is_written_into() {
        // The name comes from a caller's JSON; a newline in it would end the
        // port's section and everything under it would bind somewhere else.
        let forged = device("X\n[GCPad2]\nDevice = SDL/0/X");
        assert_eq!(forged, "SDL/0/X[GCPad2]Device = SDL/0/X");
        let text = sections(&[1], None, |_| "X\n[GCPad2]\nDevice = SDL/0/X".to_owned());
        let sdl = text
            .lines()
            .filter(|line| line.starts_with("Device = SDL/"))
            .count();
        assert_eq!(sdl, 1, "a name bound a second port:\n{text}");
        assert_eq!(device("  spaced  "), "SDL/0/spaced");
        assert_eq!(
            device("Bluetooth [x]"),
            "SDL/0/Bluetooth [x]",
            "a real name"
        );
    }

    #[test]
    fn every_pad_section_is_replaced_and_everything_else_is_kept() {
        let existing = "[Core]\nSIDevice0 = 6\n[GCPad1]\nDevice = SDL/0/old\nButtons/A = `x`\n\
                        [GCPad4]\nDevice = SDL/0/gone\n[DSUClient]\nServer = 127.0.0.1\n";
        let body = section(1, "SDL/0/padmap Player 1");
        let out = rewrite_bindings(existing, &body);
        assert!(out.contains("[Core]\nSIDevice0 = 6\n"), "{out}");
        assert!(out.contains("[DSUClient]\nServer = 127.0.0.1"), "{out}");
        assert!(!out.contains("SDL/0/gone"), "{out}");
        assert!(!out.contains("SDL/0/old"), "{out}");
        assert!(out.contains("Device = SDL/0/padmap Player 1"), "{out}");
    }

    #[test]
    fn a_section_that_is_not_a_pad_is_left_alone() {
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
        // Port 3 is the keyboard's; port 4 is nobody's.
        let devices = si_devices(&[1, 2], None);
        assert_eq!(
            devices,
            vec![
                ("SIDevice0".to_owned(), SI_GC_CONTROLLER),
                ("SIDevice1".to_owned(), SI_GC_CONTROLLER),
                ("SIDevice2".to_owned(), SI_GC_CONTROLLER),
                ("SIDevice3".to_owned(), SI_NONE),
            ]
        );
        assert_eq!(si_devices(&[1, 2, 3, 4], None)[3].1, SI_GC_CONTROLLER);
        assert_eq!(
            si_devices(&[2], Some(1)),
            vec![
                ("SIDevice0".to_owned(), SI_GC_CONTROLLER),
                ("SIDevice1".to_owned(), SI_GC_CONTROLLER),
                ("SIDevice2".to_owned(), SI_NONE),
                ("SIDevice3".to_owned(), SI_NONE),
            ],
            "a seated keyboard is port 1 ahead of a pad seated after it"
        );
    }

    #[test]
    fn the_keyboard_takes_the_first_free_port_in_dolphins_own_keys() {
        let text = sections(&[1], None, |p| format!("padmap Player {p}"));
        assert!(text.contains("[GCPad1]\nDevice = SDL/0/padmap Player 1\n"));
        assert!(text.contains("[GCPad2]\nDevice = XInput2/0/Virtual core pointer\n"));
        assert!(text.contains("Buttons/A = `X`\n"), "{text}");
        assert!(text.contains("D-Pad/Up = `T`\n"), "{text}");
        assert!(!text.contains("[GCPad3]"));

        let none = sections(&[], None, |_| String::new());
        assert!(none.starts_with("[GCPad1]\nDevice = XInput2/0/Virtual core pointer\n"));

        let full = sections(&[1, 2, 3, 4], None, |p| format!("p{p}"));
        assert!(!full.contains("XInput2"), "no port left for the keyboard");

        let seated = sections(&[2], Some(1), |p| format!("p{p}"));
        assert!(seated.starts_with("[GCPad1]\nDevice = XInput2/0/Virtual core pointer\n"));
        assert!(seated.contains("[GCPad2]\nDevice = SDL/0/p2\n"));
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
        let text = sections(&[1, 2, 5, 0], None, |player| {
            format!("padmap Player {player}")
        });
        assert!(text.contains("[GCPad1]") && text.contains("[GCPad2]"));
        assert!(!text.contains("[GCPad5]"), "Dolphin has four ports");
        assert!(!text.contains("[GCPad0]"));
    }

    #[test]
    fn a_fresh_dsu_client_ini_enables_padmap() {
        let text = dsu_client_ini("");
        assert_eq!(get_ini(&text, "Server", "Enabled").as_deref(), Some("True"));
        assert_eq!(
            get_ini(&text, "Server", "Entries").as_deref(),
            Some("padmap:127.0.0.1:26760;")
        );
    }

    #[test]
    fn a_users_other_dsu_servers_are_kept() {
        // padmap owns its entry, not the list.
        let existing = "[Server]\nEnabled = False\nEntries = phone:192.168.1.5:26760;\n";
        let text = dsu_client_ini(existing);
        assert_eq!(
            get_ini(&text, "Server", "Entries").as_deref(),
            Some("phone:192.168.1.5:26760;padmap:127.0.0.1:26760;")
        );
        assert_eq!(get_ini(&text, "Server", "Enabled").as_deref(), Some("True"));
    }

    #[test]
    fn writing_twice_does_not_list_padmap_twice() {
        let once = dsu_client_ini("");
        let twice = dsu_client_ini(&once);
        assert_eq!(once, twice);
        let renamed = "[Server]\nEntries = mine:127.0.0.1:26760;\n";
        assert_eq!(
            get_ini(&dsu_client_ini(renamed), "Server", "Entries").as_deref(),
            Some("padmap:127.0.0.1:26760;")
        );
    }

    #[test]
    fn other_sections_of_the_dsu_file_are_left_alone() {
        let existing = "[Other]\nKey = 1\n";
        let text = dsu_client_ini(existing);
        assert_eq!(get_ini(&text, "Other", "Key").as_deref(), Some("1"));
    }
}
