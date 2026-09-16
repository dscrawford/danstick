//! The `controller` event: everything needed to bind a pad, in one message.
//!
//! padmap's premise is that a program attaches to it and gets stable virtual
//! gamepads instead of configuring controllers itself. That works at launch,
//! when the launcher reads the files padmap wrote. It did not work *during* a
//! game: a controller plugged in mid-session produced no event a running
//! program could act on, so the only way to pick it up was to quit.
//!
//! This builds the message that closes that gap. A consumer receiving one has
//! enough to bind the new pad live -- the virtual node, the SDL GUID and
//! mapping line, the RetroArch port index and its bind lines -- without
//! reading a file, asking padmap anything further, or knowing how padmap
//! works.
//!
//! **Self-sufficient on purpose.** Every event carries the whole roster, not
//! just the controller that changed. A consumer that connected a moment ago,
//! or missed an event while it was busy, can apply the latest message it holds
//! and be correct; there is no log to replay and no way to be subtly out of
//! step. The cost is a bigger message on a socket that carries a few per hour.
//!
//! Pure: the daemon gathers the facts, this decides what the announcement
//! says, and a test can check the second without standing up the first.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

/// A controller arrived and is live.
pub const ACTION_ADDED: &str = "added";
/// A controller that was live has gone. Its slot is deliberately *not* reused
/// -- see [`next_player`] -- so a consumer may keep the port bound and expect
/// the same controller back.
pub const ACTION_REMOVED: &str = "removed";
/// A controller arrived that padmap cannot bind. Announced anyway: "a
/// controller appeared and does nothing" is the exact situation a user needs
/// told, and silence is what makes it baffling.
pub const ACTION_UNCONFIGURED: &str = "unconfigured";

pub const EVENT: &str = "controller";

/// Nobody has ever mapped this model.
pub const REASON_UNMAPPED: &str = "unmapped";
/// It has a mapping, but its device node could not be opened -- usually a
/// permission that never arrived.
pub const REASON_UNREADABLE: &str = "unreadable";

/// The physical controller, as a consumer matching against lsusb or a udev
/// rule needs it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Controller {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub path: String,
    pub phys: String,
    pub uniq: String,
    /// The key padmap stores a profile under. Not the name, which two
    /// identical pads share.
    pub signature: String,
    /// False once `padmap hide` has cleared ID_INPUT_JOYSTICK.
    pub retroarch_visible: bool,
}

/// The clone, as it advertises itself. In mirror mode the pad's own identity;
/// in padmap mode 1209:0001. Either way it is what a consumer matching on
/// vid/pid has to be told, because the two differ exactly when it matters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Virtual {
    pub name: String,
    /// `/dev/input/eventN` of the clone, or empty if it does not exist.
    pub node: String,
    pub phys: String,
    pub vid: u16,
    pub pid: u16,
    pub bustype: u16,
    /// Computed from the identity above, never re-derived from the pad: the
    /// two can disagree, and did, and a consumer keying on the GUID would
    /// register its mapping under one SDL never looks up.
    pub guid: String,
    pub identity_mode: String,
}

/// The RetroArch side: port, index and the actual bind lines.
///
/// The binds are included rather than only the profile path because a
/// consumer may not be RetroArch, and a path is only useful to something
/// willing to parse RetroArch's config format.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Retroarch {
    /// 0-based joypad index, or `None` for a player whose clone RetroArch
    /// cannot see -- reported as -1, the honest answer rather than a number
    /// that would point at someone else's pad.
    pub index: Option<usize>,
    pub profile: String,
    pub binds: BTreeMap<String, String>,
}

/// One controller padmap is republishing, and the clone it publishes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attached {
    pub player: u32,
    pub controller: Controller,
    /// `None` for an unconfigured controller, which has no clone.
    pub virtual_pad: Option<Virtual>,
    pub retroarch: Option<Retroarch>,
    /// The SDL database line, or empty when the controller has no capture --
    /// a line built from no bindings claims a pad with no buttons.
    pub sdl_mapping: String,
}

/// The lowest free 1-based slot.
///
/// Lowest free rather than highest-plus-one, so a controller arriving after
/// another was unplugged takes the empty slot instead of opening a fifth one
/// beyond three live pads. RetroArch ports are positional; leaving a hole
/// means a four-player game with a gap at player 2.
pub fn next_player(taken: &[u32]) -> u32 {
    let mut player = 1;
    while taken.contains(&player) {
        player += 1;
    }
    player
}

fn controller_fields(controller: &Controller, configured: bool) -> Value {
    json!({
        "name": controller.name,
        // Hex strings rather than ints: this is how every other tool on the
        // machine writes a USB id.
        "vid": format!("{:04x}", controller.vid),
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
        // 1-based, as RetroArch's own `input_playerN_*` settings count.
        "port": player,
        "index": retroarch.index.map(|index| index as i64).unwrap_or(-1),
        "profile": retroarch.profile,
        "binds": retroarch.binds,
    })
}

/// One controller, complete.
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

/// The whole message.
///
/// `subject` is what changed; `roster` is everything live afterwards. For a
/// removal the subject is not in the roster, which is the only way a consumer
/// can tell which port to release.
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
    // Repeated at the top level so the common case -- "which port do I
    // rebind?" -- is one lookup rather than a nested one.
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
    // Which game the mappings were resolved for: a scoped mapping differs per
    // console and per game, so a consumer caching binds needs to know what
    // they were scoped to.
    event.insert(
        "scope".to_owned(),
        json!({ "console": console, "game": game }),
    );
    // Same reason `ensure-daemon` compares it: a consumer that reconnects to
    // a daemon it did not start can tell whether the code changed under it.
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
                name: format!("padmap Player {player}"),
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
                profile: "/run/padmap/autoconfig/udev/x.cfg".to_owned(),
                binds: [("input_a_btn".to_owned(), "0".to_owned())]
                    .into_iter()
                    .collect(),
            }),
            sdl_mapping: "guid,padmap Player 1,a:b0,".to_owned(),
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
