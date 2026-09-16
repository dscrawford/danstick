//! Which physical pad is which player, as written to disk.
//!
//! The format is the Python's, field for field, because both daemons will be
//! installed for the whole of this port and a rollback has to keep the user's
//! controller order.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::pad::Pad;

/// One line of `assignments.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    /// 1-based.
    pub player: u32,
    pub path: PathBuf,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub phys: String,
    #[serde(default)]
    pub vid: u16,
    #[serde(default)]
    pub pid: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum AssignmentsError {
    #[error("reading {0}: {1}")]
    Read(PathBuf, #[source] std::io::Error),
    #[error("parsing {0}: {1}")]
    Parse(PathBuf, #[source] serde_json::Error),
}

/// Read the saved order, or an empty list if nothing has been assigned.
///
/// A missing file is not an error: it is what "nobody has run setup yet" looks
/// like, and the caller says so in words the user can act on.
pub fn load(path: &Path) -> Result<Vec<Assignment>, AssignmentsError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(AssignmentsError::Read(path.to_path_buf(), error)),
    };
    serde_json::from_str(&text).map_err(|error| AssignmentsError::Parse(path.to_path_buf(), error))
}

pub fn save(path: &Path, assignments: &[Assignment]) -> Result<(), AssignmentsError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| AssignmentsError::Read(parent.to_path_buf(), error))?;
    }
    let text = serde_json::to_string_pretty(assignments)
        .map_err(|error| AssignmentsError::Parse(path.to_path_buf(), error))?;
    std::fs::write(path, text + "\n")
        .map_err(|error| AssignmentsError::Read(path.to_path_buf(), error))
}

/// Pair each assignment with the pad currently at its node.
///
/// A pad that has gone is reported by name rather than skipped silently: from
/// the user's side an absent controller and a controller padmap declined to
/// republish look identical, and only one of them is their fault.
pub fn resolve<'a>(
    assignments: &'a [Assignment],
    pads: &'a [Pad],
) -> (Vec<(u32, &'a Pad)>, Vec<&'a Assignment>) {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for assignment in assignments {
        match pads.iter().find(|pad| pad.path == assignment.path) {
            Some(pad) => found.push((assignment.player, pad)),
            None => missing.push(assignment),
        }
    }
    (found, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(event: &str) -> Pad {
        Pad {
            path: PathBuf::from(format!("/dev/input/{event}")),
            name: "Pad".to_owned(),
            phys: String::new(),
            uniq: String::new(),
            vid: 1,
            pid: 2,
            syspath: PathBuf::from("/sys"),
            retroarch_visible: true,
            motion: None,
        }
    }

    #[test]
    fn the_python_file_shape_round_trips() {
        let raw = r#"[{"player": 1, "path": "/dev/input/event5", "name": "Pad",
                       "phys": "usb-3/input0", "vid": 121, "pid": 6211}]"#;
        let parsed: Vec<Assignment> = serde_json::from_str(raw).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].player, 1);
        assert_eq!(parsed[0].path, PathBuf::from("/dev/input/event5"));
        assert_eq!(parsed[0].vid, 121);
        let back = serde_json::to_value(&parsed[0]).expect("serialize");
        assert_eq!(back["player"], 1);
        assert_eq!(back["path"], "/dev/input/event5");
    }

    #[test]
    fn a_record_missing_everything_but_the_essentials_still_reads() {
        // A file written by an older version, or by hand.
        let parsed: Vec<Assignment> =
            serde_json::from_str(r#"[{"player": 2, "path": "/dev/input/event0"}]"#).expect("parse");
        assert_eq!(parsed[0].player, 2);
        assert_eq!(parsed[0].name, "");
        assert_eq!(parsed[0].vid, 0);
    }

    #[test]
    fn a_missing_file_is_an_empty_order_rather_than_a_failure() {
        let path = Path::new("/nonexistent-padmap-test/assignments.json");
        assert_eq!(load(path).expect("a missing file is not an error").len(), 0);
    }

    #[test]
    fn a_pad_that_has_gone_is_reported_rather_than_skipped() {
        let assignments = vec![
            Assignment {
                player: 1,
                path: PathBuf::from("/dev/input/event5"),
                name: "Here".to_owned(),
                phys: String::new(),
                vid: 0,
                pid: 0,
            },
            Assignment {
                player: 2,
                path: PathBuf::from("/dev/input/event9"),
                name: "Gone".to_owned(),
                phys: String::new(),
                vid: 0,
                pid: 0,
            },
        ];
        let pads = vec![pad("event5")];
        let (found, missing) = resolve(&assignments, &pads);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 1);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].name, "Gone");
    }

    #[test]
    fn player_numbers_survive_a_gap_in_the_middle() {
        // Player 2 unplugged does not renumber player 3 into its slot -- the
        // whole point of the assignment is that the order is fixed.
        let assignments: Vec<Assignment> = (1..=3)
            .map(|player| Assignment {
                player,
                path: PathBuf::from(format!("/dev/input/event{player}")),
                name: String::new(),
                phys: String::new(),
                vid: 0,
                pid: 0,
            })
            .collect();
        let pads = vec![pad("event1"), pad("event3")];
        let (found, missing) = resolve(&assignments, &pads);
        assert_eq!(
            found.iter().map(|(player, _)| *player).collect::<Vec<_>>(),
            [1, 3]
        );
        assert_eq!(missing.len(), 1);
    }

    #[test]
    fn a_saved_order_reads_back_as_itself() {
        let dir = std::env::temp_dir().join("padmap-assignment-test");
        let path = dir.join("assignments.json");
        let _ = std::fs::remove_dir_all(&dir);
        let assignments = vec![Assignment {
            player: 1,
            path: PathBuf::from("/dev/input/event7"),
            name: "MAYFLASH GameCube Adapter".to_owned(),
            phys: "usb-0000:00:14.0-3/input0".to_owned(),
            vid: 0x0079,
            pid: 0x1843,
        }];
        save(&path, &assignments).expect("save");
        assert_eq!(load(&path).expect("load"), assignments);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
