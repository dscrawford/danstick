//! Controller event message: self-sufficient roster for live binding.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

pub const ACTION_ADDED: &str = "added";
pub const ACTION_REMOVED: &str = "removed"; // Slot deliberately not reused
pub const ACTION_UNCONFIGURED: &str = "unconfigured";

pub const EVENT: &str = "controller";

pub const REASON_UNMAPPED: &str = "unmapped";
pub const REASON_UNREADABLE: &str = "unreadable";

/// Physical controller (lsusb/udev matching).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Controller {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub path: String,
    pub phys: String,
    pub uniq: String,
    pub signature: String,
    pub retroarch_visible: bool,
}

/// Clone identity (mirror mode: pad itself; danstick mode: 1209:0001).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Virtual {
    pub name: String,
    pub node: String,
    pub phys: String,
    pub vid: u16,
    pub pid: u16,
    pub bustype: u16,
    pub guid: String,
    pub identity_mode: String,
}

/// RetroArch side: port, index, binds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Retroarch {
    pub index: Option<usize>,
    pub profile: String,
    pub binds: BTreeMap<String, String>,
}

/// Controller and its clone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attached {
    pub player: u32,
    pub controller: Controller,
    pub virtual_pad: Option<Virtual>,
    pub retroarch: Option<Retroarch>,
    pub sdl_mapping: String,
}

/// Lowest free 1-based slot (no gaps for positional ports).
pub fn next_player(taken: &[u32]) -> u32 {
    let mut player = 1;
    while taken.contains(&player) {
        player += 1;
    }
    player
}

/// The next `count` free seats, in order: who gets what when several hold at once.
pub fn next_players(taken: &[u32], count: usize) -> Vec<u32> {
    let mut taken = taken.to_vec();
    (0..count)
        .map(|_| {
            let player = next_player(&taken);
            taken.push(player);
            player
        })
        .collect()
}

fn controller_fields(controller: &Controller, configured: bool) -> Value {
    json!({
        "name": controller.name,
        "vid": format!("{:04x}", controller.vid), // Hex strings (USB id standard)
        "pid": format!("{:04x}", controller.pid),
        "path": controller.path,
        "phys": controller.phys,
        "uniq": controller.uniq,
        "signature": controller.signature,
        "configured": configured,
        "retroarch_visible": controller.retroarch_visible,
    })
}

fn virtual_fields(virtual_pad: &Virtual) -> Value {
    json!({
        "name": virtual_pad.name,
        "node": virtual_pad.node,
        "phys": virtual_pad.phys,
        "vid": format!("{:04x}", virtual_pad.vid),
        "pid": format!("{:04x}", virtual_pad.pid),
        "bustype": virtual_pad.bustype,
        "guid": virtual_pad.guid,
        "identity_mode": virtual_pad.identity_mode,
    })
}

fn retroarch_fields(player: u32, retroarch: &Retroarch) -> Value {
    json!({
        "port": player, // 1-based, matches input_playerN_*
        "index": retroarch.index.map(|index| index as i64).unwrap_or(-1),
        "profile": retroarch.profile,
        "binds": retroarch.binds,
    })
}

pub fn entry_fields(entry: &Attached, configured: bool) -> Value {
    let mut fields = Map::new();
    fields.insert("player".to_owned(), json!(entry.player));
    fields.insert(
        "controller".to_owned(),
        controller_fields(&entry.controller, configured),
    );
    if !configured {
        return Value::Object(fields);
    }
    fields.insert(
        "virtual".to_owned(),
        entry
            .virtual_pad
            .as_ref()
            .map(virtual_fields)
            .unwrap_or(Value::Null),
    );
    fields.insert(
        "retroarch".to_owned(),
        entry
            .retroarch
            .as_ref()
            .map(|retroarch| retroarch_fields(entry.player, retroarch))
            .unwrap_or(Value::Null),
    );
    fields.insert("sdl_mapping".to_owned(), json!(entry.sdl_mapping));
    Value::Object(fields)
}

/// Complete message: subject (change), roster (after).
#[allow(clippy::too_many_arguments)]
pub fn controller_event(
    action: &str,
    subject: &Attached,
    roster: &[Attached],
    console: &str,
    game: &str,
    build: &str,
    reason: &str,
) -> Value {
    let configured = action != ACTION_UNCONFIGURED;
    let mut sorted: Vec<&Attached> = roster.iter().collect();
    sorted.sort_by_key(|entry| entry.player);
    let mut event = Map::new();
    event.insert("event".to_owned(), json!(EVENT));
    event.insert("action".to_owned(), json!(action));
    event.insert("player".to_owned(), json!(subject.player));
    event.insert("changed".to_owned(), entry_fields(subject, configured));
    event.insert(
        "roster".to_owned(),
        Value::Array(
            sorted
                .iter()
                .map(|entry| entry_fields(entry, true))
                .collect(),
        ),
    );
    event.insert(
        "scope".to_owned(),
        json!({ "console": console, "game": game }),
    );
    event.insert("build".to_owned(), json!(build));
    if !reason.is_empty() {
        event.insert("reason".to_owned(), json!(reason));
    }
    Value::Object(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attached(player: u32) -> Attached {
        Attached {
            player,
            controller: Controller {
                name: "Test Pad".to_owned(),
                vid: 0x057E,
                pid: 0x2009,
                path: format!("/dev/input/event{player}"),
                signature: format!("057e:2009:Test Pad {player}"),
                retroarch_visible: true,
                ..Controller::default()
            },
            virtual_pad: Some(Virtual {
                name: format!("danstick Player {player}"),
                node: format!("/dev/input/event{}", 90 + player),
                vid: 0x057E,
                pid: 0x2009,
                bustype: 3,
                guid: "guid".to_owned(),
                identity_mode: "mirror".to_owned(),
                ..Virtual::default()
            }),
            retroarch: Some(Retroarch {
                index: Some((player as usize).saturating_sub(1)),
                profile: "/run/danstick/autoconfig/udev/x.cfg".to_owned(),
                binds: [("input_a_btn".to_owned(), "0".to_owned())]
                    .into_iter()
                    .collect(),
            }),
            sdl_mapping: "guid,danstick Player 1,a:b0,".to_owned(),
        }
    }

    #[test]
    fn the_lowest_free_slot_is_taken_not_the_next_one_up() {
        assert_eq!(next_player(&[]), 1);
        assert_eq!(next_player(&[1, 3]), 2);
        assert_eq!(next_player(&[1, 2, 3]), 4);
        assert_eq!(next_player(&[2]), 1);
    }

    #[test]
    fn an_addition_carries_the_whole_roster_sorted() {
        let event = controller_event(
            ACTION_ADDED,
            &attached(2),
            &[attached(2), attached(1)],
            "n64",
            "n64/goldeneye",
            "build-1",
            "",
        );
        assert_eq!(event["player"], 2);
        assert_eq!(event["action"], "added");
        let roster: Vec<u64> = event["roster"]
            .as_array()
            .expect("roster")
            .iter()
            .map(|entry| entry["player"].as_u64().expect("player"))
            .collect();
        assert_eq!(roster, vec![1, 2]);
        assert_eq!(event["scope"]["console"], "n64");
        assert_eq!(event["build"], "build-1");
        assert!(event.get("reason").is_none(), "no reason unless given");
        assert_eq!(event["changed"]["virtual"]["vid"], "057e");
        assert_eq!(event["changed"]["retroarch"]["port"], 2);
        assert_eq!(event["changed"]["retroarch"]["index"], 1);
    }

    #[test]
    fn an_unconfigured_controller_has_no_virtual_or_retroarch_half() {
        let mut subject = attached(0);
        subject.virtual_pad = None;
        subject.retroarch = None;
        let event = controller_event(
            ACTION_UNCONFIGURED,
            &subject,
            &[],
            "",
            "",
            "b",
            REASON_UNMAPPED,
        );
        let changed = event["changed"].as_object().expect("changed");
        assert!(!changed.contains_key("virtual"));
        assert!(!changed.contains_key("retroarch"));
        assert!(!changed.contains_key("sdl_mapping"));
        assert_eq!(changed["controller"]["configured"], false);
        assert_eq!(event["reason"], "unmapped");
    }

    #[test]
    fn a_player_retroarch_cannot_see_reports_minus_one() {
        let mut subject = attached(1);
        subject.retroarch.as_mut().expect("retroarch").index = None;
        let event = controller_event(ACTION_ADDED, &subject, &[subject.clone()], "", "", "b", "");
        assert_eq!(event["changed"]["retroarch"]["index"], -1);
    }
}
