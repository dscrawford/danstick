//! Showing a game danstick's pads and nothing else.

use std::path::{Path, PathBuf};

/// What danstick names its virtual pads, as `emit::virtual_name` writes it.
pub const CLONE_PREFIX: &str = "danstick Player ";

/// Whether a device name is one of danstick's own pads.
pub fn is_clone(name: &str) -> bool {
    name.starts_with(CLONE_PREFIX)
}

/// One `/dev/input/event*` node as the plan sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub path: PathBuf,
    /// The name its device reports.
    pub name: String,
    /// Its device's vendor and product.
    pub id: (u16, u16),
}

/// The bind plan for one launch.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Input nodes the game may open, `/dev/input/...`.
    pub keep: Vec<PathBuf>,
    /// Nodes covered with /dev/null: the hidraw a raw pad is reachable through.
    pub cover: Vec<PathBuf>,
}

impl Plan {
    /// Whether there is any point sandboxing.
    pub fn worth_it(&self) -> bool {
        self.keep.iter().any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
    }
}

/// Pure: which nodes a game may see; Steam's copies of pads never, since the game reads clones.
pub fn plan(nodes: &[Node], raw: &[PathBuf], hidraw: &[PathBuf]) -> Plan {
    let mut keep: Vec<PathBuf> = Vec::new();
    for node in nodes {
        let pad = raw.contains(&node.path) || node.id == danstick_core::icons::STEAM_VIRTUAL_ID;
        if is_clone(&node.name) || !pad {
            keep.push(node.path.clone());
        }
    }
    keep.sort();
    let mut cover = hidraw.to_vec();
    cover.sort();
    Plan { keep, cover }
}

/// The bwrap command that runs `argv` under that plan.
pub fn bwrap_argv(plan: &Plan, argv: &[String], bwrap: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![
        bwrap.to_owned(),
        "--die-with-parent".into(),
        "--dev-bind".into(),
        "/".into(),
        "/".into(),
        "--tmpfs".into(),
        "/dev/input".into(),
    ];
    // --dev-bind, not --bind: a plain bind hands over a file the kernel will not talk through.
    for path in &plan.keep {
        out.push("--dev-bind".into());
        out.push(path.display().to_string());
        out.push(path.display().to_string());
    }
    for path in &plan.cover {
        out.push("--bind".into());
        out.push("/dev/null".into());
        out.push(path.display().to_string());
    }
    out.push("--".into());
    out.extend(argv.iter().cloned());
    out
}

/// Every `/dev/input/event*`, with the name and id its device reports.
pub fn event_nodes() -> Vec<Node> {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(node) = name.to_str() else { continue };
        if !node.starts_with("event") {
            continue;
        }
        let device = Path::new("/sys/class/input").join(node).join("device");
        let read = |file: &str| std::fs::read_to_string(device.join(file)).unwrap_or_default();
        let hex = |file: &str| u16::from_str_radix(read(file).trim(), 16).unwrap_or(0);
        out.push(Node {
            path: entry.path(),
            name: read("name").trim().to_owned(),
            id: (hex("id/vendor"), hex("id/product")),
        });
    }
    out
}

/// Every `/dev/hidraw*`.
pub fn hidraw_nodes() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("hidraw"))
        })
        .map(|entry| entry.path())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(path: &str, name: &str) -> Node {
        Node {
            path: PathBuf::from(path),
            name: name.to_owned(),
            id: (0x045e, 0x028e),
        }
    }

    fn steams_copy(path: &str) -> Node {
        Node {
            path: PathBuf::from(path),
            name: "Microsoft X-Box 360 pad 1".to_owned(),
            id: danstick_core::icons::STEAM_VIRTUAL_ID,
        }
    }

    #[test]
    fn steams_copy_of_a_pad_is_never_the_games_to_open() {
        let nodes = [
            steams_copy("/dev/input/event256"),
            node("/dev/input/event257", "danstick Player 1"),
        ];
        let plan = plan(&nodes, &[], &[PathBuf::from("/dev/hidraw3")]);
        assert_eq!(plan.keep, vec![PathBuf::from("/dev/input/event257")]);
    }

    #[test]
    fn a_clone_is_kept_whatever_identity_it_mirrors() {
        let mut clone = steams_copy("/dev/input/event257");
        clone.name = "danstick Player 1".to_owned();
        let plan = plan(&[clone], &[], &[]);
        assert_eq!(plan.keep, vec![PathBuf::from("/dev/input/event257")]);
    }

    #[test]
    fn a_clone_is_kept_and_the_pad_it_was_cloned_from_is_not() {
        let nodes = [
            node("/dev/input/event3", "Xbox Wireless Controller"),
            node("/dev/input/event9", "danstick Player 1"),
        ];
        let raw = [PathBuf::from("/dev/input/event3")];
        let plan = plan(&nodes, &raw, &[]);
        assert_eq!(plan.keep, vec![PathBuf::from("/dev/input/event9")]);
    }

    #[test]
    fn a_keyboard_and_a_mouse_are_not_dansticks_to_take_away() {
        let nodes = [
            node("/dev/input/event0", "AT Translated Set 2 keyboard"),
            node("/dev/input/event1", "Logitech Mouse"),
            node("/dev/input/event3", "Steam Controller"),
            node("/dev/input/event9", "danstick Player 1"),
        ];
        let raw = [PathBuf::from("/dev/input/event3")];
        let plan = plan(&nodes, &raw, &[]);
        assert_eq!(
            plan.keep,
            vec![
                PathBuf::from("/dev/input/event0"),
                PathBuf::from("/dev/input/event1"),
                PathBuf::from("/dev/input/event9"),
            ]
        );
    }

    #[test]
    fn every_hidraw_is_covered_because_a_steam_controller_lives_there() {
        // It has no event node at all, so hiding event nodes leaves it visible to hidapi.
        let plan = plan(
            &[node("/dev/input/event9", "danstick Player 1")],
            &[],
            &[PathBuf::from("/dev/hidraw2"), PathBuf::from("/dev/hidraw0")],
        );
        assert_eq!(
            plan.cover,
            vec![PathBuf::from("/dev/hidraw0"), PathBuf::from("/dev/hidraw2")]
        );
    }

    #[test]
    fn with_no_clone_published_there_is_nothing_to_put_in_front_of_the_game() {
        let plan = plan(
            &[node("/dev/input/event3", "Xbox Wireless Controller")],
            &[PathBuf::from("/dev/input/event3")],
            &[PathBuf::from("/dev/hidraw0")],
        );
        assert!(!plan.worth_it());
    }

    #[test]
    fn a_plan_with_a_clone_is_worth_running() {
        let plan = plan(&[node("/dev/input/event9", "danstick Player 1")], &[], &[]);
        assert!(plan.worth_it());
    }

    #[test]
    fn the_command_binds_the_kept_nodes_and_covers_the_rest() {
        let plan = Plan {
            keep: vec![PathBuf::from("/dev/input/event9")],
            cover: vec![PathBuf::from("/dev/hidraw2")],
        };
        let argv = bwrap_argv(
            &plan,
            &["dolphin-emu".to_owned(), "game.rvz".to_owned()],
            "bwrap",
        );
        let line = argv.join(" ");
        assert!(line.starts_with("bwrap --die-with-parent --dev-bind / / --tmpfs /dev/input"));
        assert!(line.contains("--dev-bind /dev/input/event9 /dev/input/event9"));
        assert!(line.contains("--bind /dev/null /dev/hidraw2"));
        assert!(line.ends_with("-- dolphin-emu game.rvz"));
    }

    #[test]
    fn a_seat_nobody_has_taken_is_still_bound_into_the_launch() {
        let nodes = vec![
            node("/dev/input/event20", "danstick Player 1"),
            node("/dev/input/event21", "danstick Player 2"),
            node("/dev/input/event9", "Xbox 360 Controller"),
        ];
        let raw = vec![PathBuf::from("/dev/input/event9")];
        let plan = plan(&nodes, &raw, &[]);
        assert!(
            plan.keep.contains(&PathBuf::from("/dev/input/event21")),
            "an empty seat was left outside the launch: {plan:?}"
        );
        assert!(!plan.keep.contains(&PathBuf::from("/dev/input/event9")));
        assert!(plan.worth_it());
    }
}
