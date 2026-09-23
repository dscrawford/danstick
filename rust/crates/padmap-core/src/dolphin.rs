//! Dolphin GameCube pad bindings via SDL backend.

use std::collections::BTreeMap;

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

/// A controller id, `<backend>/<slot>/<name>`, exactly as SDL would say it.
///
/// One line: a name arrives from a caller's JSON and a newline in it would
/// close the section and open whatever came next.
pub fn device(slot: u32, name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    format!("SDL/{slot}/{}", name.trim())
}

/// Each player's SDL slot: the rank of its clone's node among the clones'.
///
/// A slot of its own for every player, node or no node: two pads of one model
/// share a name, so a repeated slot is two ports claiming one controller.
pub fn sdl_slots(nodes: &BTreeMap<u32, String>) -> BTreeMap<u32, u32> {
    let mut order: Vec<(u32, Option<u64>)> = nodes
        .iter()
        .map(|(player, node)| (*player, event_number(node)))
        .collect();
    // Unreadable nodes last, in player order, which is the order clones are made.
    order.sort_by_key(|(player, number)| (number.is_none(), *number, *player));
    order
        .iter()
        .enumerate()
        .map(|(slot, (player, _))| (*player, slot as u32))
        .collect()
}

/// The number in `/dev/input/event12`, for ordering.
fn event_number(node: &str) -> Option<u64> {
    node.rsplit('/').next()?.strip_prefix("event")?.parse().ok()
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
pub fn sections(players: &[u32], seat: Option<u32>, device_for: impl Fn(u32) -> String) -> String {
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
                Some(section(port, &device_for(port)))
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
    fn a_name_cannot_close_the_section_it_is_written_into() {
        // The name comes from a caller's JSON; a newline in it would end the
        // port's section and everything under it would bind somewhere else.
        let forged = device(0, "X\n[GCPad2]\nDevice = SDL/0/X");
        assert!(!forged.contains('\n'), "{forged}");
        assert_eq!(forged, "SDL/0/X[GCPad2]Device = SDL/0/X");
        // One seated pad is one SDL device, whatever its name says; port 2 is
        // the keyboard's and is the only other section here.
        let text = sections(&[1], None, |_| forged.clone());
        let sdl = text
            .lines()
            .filter(|line| line.starts_with("Device = SDL/"))
            .count();
        assert_eq!(sdl, 1, "a name bound a second port:\n{text}");
        assert_eq!(device(0, "  spaced  "), "SDL/0/spaced");
        assert_eq!(
            device(0, "Bluetooth [x]"),
            "SDL/0/Bluetooth [x]",
            "a real name"
        );
    }

    #[test]
    fn a_device_is_the_backend_the_slot_and_the_name_sdl_uses() {
        assert_eq!(device(0, "padmap Player 3"), "SDL/0/padmap Player 3");
        assert_eq!(
            device(1, "Xbox 360 Controller"),
            "SDL/1/Xbox 360 Controller"
        );
    }

    #[test]
    fn a_slot_is_where_the_clones_node_sorts_among_the_clones() {
        // SDL numbers nodes in isolate's order, not player order.
        let nodes: BTreeMap<u32, String> = [
            (1, "/dev/input/event30".to_owned()),
            (2, "/dev/input/event9".to_owned()),
            (3, "/dev/input/event12".to_owned()),
        ]
        .into_iter()
        .collect();
        let slots = sdl_slots(&nodes);
        assert_eq!(slots[&2], 0, "event9 sorts first, not event30");
        assert_eq!(slots[&3], 1);
        assert_eq!(slots[&1], 2);
    }

    #[test]
    fn a_clone_whose_node_is_unknown_sorts_after_the_ones_that_are_known() {
        let nodes: BTreeMap<u32, String> =
            [(1, String::new()), (2, "/dev/input/event9".to_owned())]
                .into_iter()
                .collect();
        let slots = sdl_slots(&nodes);
        assert_eq!(slots[&2], 0, "the node there is sorts first");
        assert_eq!(slots[&1], 1);
    }

    #[test]
    fn every_player_gets_a_slot_of_its_own_even_with_no_node_at_all() {
        // A caller from before `node` existed sends none, and two pads of one
        // model share a name -- so one slot for both is one port for two pads.
        let nodes: BTreeMap<u32, String> =
            [(1, String::new()), (2, String::new()), (3, String::new())]
                .into_iter()
                .collect();
        let slots = sdl_slots(&nodes);
        assert_eq!(slots[&1], 0, "player order is the order clones are made");
        assert_eq!(slots[&2], 1);
        assert_eq!(slots[&3], 2);
    }

    #[test]
    fn a_node_that_is_not_an_event_node_is_treated_as_no_node() {
        let nodes: BTreeMap<u32, String> = [
            (1, "/dev/input/js0".to_owned()),
            (2, "/dev/input/event".to_owned()),
            (3, "/dev/input/event12x".to_owned()),
            (4, "event9".to_owned()),
        ]
        .into_iter()
        .collect();
        let slots = sdl_slots(&nodes);
        assert_eq!(slots[&4], 0, "a bare event9 with no directory still reads");
        let mut seen: Vec<u32> = slots.values().copied().collect();
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2, 3], "every player got its own slot");
    }

    #[test]
    fn two_nodes_of_the_same_number_still_get_a_slot_each() {
        // Nothing enforces uniqueness, and a collision is worse than a guess:
        // it is two ports claiming one controller.
        let nodes: BTreeMap<u32, String> = [
            (1, "/dev/input/event9".to_owned()),
            (2, "/dev/input/event9".to_owned()),
        ]
        .into_iter()
        .collect();
        let slots = sdl_slots(&nodes);
        assert_ne!(slots[&1], slots[&2]);
    }

    #[test]
    fn two_pads_of_one_model_are_told_apart_by_their_slots() {
        let nodes: BTreeMap<u32, String> = [
            (1, "/dev/input/event20".to_owned()),
            (2, "/dev/input/event21".to_owned()),
        ]
        .into_iter()
        .collect();
        let slots = sdl_slots(&nodes);
        let one = device(slots[&1], "Xbox 360 Controller");
        let two = device(slots[&2], "Xbox 360 Controller");
        assert_ne!(one, two, "one name, two ports, one device id");
        assert_eq!(one, "SDL/0/Xbox 360 Controller");
        assert_eq!(two, "SDL/1/Xbox 360 Controller");
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
        let text = sections(&[1], None, |p| device(0, &format!("padmap Player {p}")));
        assert!(text.contains("[GCPad1]\nDevice = SDL/0/padmap Player 1\n"));
        assert!(text.contains("[GCPad2]\nDevice = XInput2/0/Virtual core pointer\n"));
        assert!(text.contains("Buttons/A = `X`\n"), "{text}");
        assert!(text.contains("D-Pad/Up = `T`\n"), "{text}");
        assert!(!text.contains("[GCPad3]"));

        let none = sections(&[], None, |_| String::new());
        assert!(none.starts_with("[GCPad1]\nDevice = XInput2/0/Virtual core pointer\n"));

        let full = sections(&[1, 2, 3, 4], None, |p| device(0, &format!("p{p}")));
        assert!(!full.contains("XInput2"), "no port left for the keyboard");

        let seated = sections(&[2], Some(1), |p| device(0, &format!("p{p}")));
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
            device(0, &format!("padmap Player {player}"))
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
