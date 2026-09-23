//! Showing a game padmap's pads and nothing else.
//!
//! padmap republishes every controller as a virtual pad, and every consumer is
//! meant to read those: the mapping, the player order, the motion and the
//! remapping all live there. Nothing stopped a game from reading the physical
//! pad *as well*. SDL opens whatever it finds, so a launch saw both -- the
//! clone and the controller it was cloned from -- and bound whichever came
//! first. Four Swords Adventures bound a raw Steam Controller to player one
//! while padmap had that seat pointed at an Xbox pad's clone.
//!
//! `padmap-rs exec` therefore starts the game in a sandbox where `/dev/input`
//! holds the clones and nothing that is a controller besides. The keyboard and
//! the mouse stay: they are not pads and something has to drive a menu.
//!
//! hidraw is the other half, and the half that is easy to forget. A Steam
//! Controller has no event node at all until something creates one -- SDL
//! reaches it through `/dev/hidraw*` -- so hiding event nodes alone leaves it
//! in full view. A clone is a uinput device and has no hidraw node, so every
//! one of them is covered over.
//!
//! Pure: what to bind and what to cover is decided here, from lists, and is
//! what the tests hold. Nothing in this module touches a device.

use std::path::{Path, PathBuf};

/// What padmap names its virtual pads. `emit::virtual_name` writes it.
pub const CLONE_PREFIX: &str = "padmap Player ";

/// Whether a device name is one of padmap's own pads.
pub fn is_clone(name: &str) -> bool {
    name.starts_with(CLONE_PREFIX)
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
    /// Whether there is any point sandboxing. With no clone published there is
    /// nothing to put in front of the game, and hiding its controllers would
    /// leave it with none at all -- worse than the problem.
    pub fn worth_it(&self) -> bool {
        self.keep.iter().any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
    }
}

/// Pure: which nodes a game may see.
///
/// `nodes` is every `/dev/input/event*` with the name its device reports;
/// `raw` is the physical pads padmap knows about, by node. A node is kept
/// unless it is one of those pads -- so the clones stay, and so does every
/// keyboard, mouse and touchpad, which are not pads and are not padmap's to
/// take away.
pub fn plan(nodes: &[(PathBuf, String)], raw: &[PathBuf], hidraw: &[PathBuf]) -> Plan {
    let mut keep: Vec<PathBuf> = Vec::new();
    for (path, name) in nodes {
        if is_clone(name) || !raw.iter().any(|pad| pad == path) {
            keep.push(path.clone());
        }
    }
    keep.sort();
    let mut cover = hidraw.to_vec();
    cover.sort();
    Plan { keep, cover }
}

/// The bwrap command that runs `argv` under that plan.
///
/// `--dev-bind` rather than `--bind` throughout: these are device nodes, and a
/// plain bind hands over a file the kernel will not talk through.
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

/// Every `/dev/input/event*` and the name its device reports.
pub fn event_nodes() -> Vec<(PathBuf, String)> {
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
        let reported =
            std::fs::read_to_string(Path::new("/sys/class/input").join(node).join("device/name"))
                .unwrap_or_default();
        out.push((entry.path(), reported.trim().to_owned()));
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

    fn node(path: &str, name: &str) -> (PathBuf, String) {
        (PathBuf::from(path), name.to_owned())
    }

    #[test]
    fn a_clone_is_kept_and_the_pad_it_was_cloned_from_is_not() {
        let nodes = [
            node("/dev/input/event3", "Xbox Wireless Controller"),
            node("/dev/input/event9", "padmap Player 1"),
        ];
        let raw = [PathBuf::from("/dev/input/event3")];
        let plan = plan(&nodes, &raw, &[]);
        assert_eq!(plan.keep, vec![PathBuf::from("/dev/input/event9")]);
    }

    #[test]
    fn a_keyboard_and_a_mouse_are_not_padmaps_to_take_away() {
        let nodes = [
            node("/dev/input/event0", "AT Translated Set 2 keyboard"),
            node("/dev/input/event1", "Logitech Mouse"),
            node("/dev/input/event3", "Steam Controller"),
            node("/dev/input/event9", "padmap Player 1"),
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
        // It has no event node at all: hiding event nodes alone leaves it in
        // full view of SDL's hidapi driver.
        let plan = plan(
            &[node("/dev/input/event9", "padmap Player 1")],
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
        // Hiding its controllers would leave it with none, which is worse than
        // the problem this solves.
        let plan = plan(
            &[node("/dev/input/event3", "Xbox Wireless Controller")],
            &[PathBuf::from("/dev/input/event3")],
            &[PathBuf::from("/dev/hidraw0")],
        );
        assert!(!plan.worth_it());
    }

    #[test]
    fn a_plan_with_a_clone_is_worth_running() {
        let plan = plan(&[node("/dev/input/event9", "padmap Player 1")], &[], &[]);
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
        // A clone made after a launch starts is not in its /dev/input, so this has to hold.
        let nodes = vec![
            (
                PathBuf::from("/dev/input/event20"),
                "padmap Player 1".to_owned(),
            ),
            (
                PathBuf::from("/dev/input/event21"),
                "padmap Player 2".to_owned(),
            ),
            (
                PathBuf::from("/dev/input/event9"),
                "Xbox 360 Controller".to_owned(),
            ),
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
