//! What the daemon says to a client, as JSON.
//! One place to avoid multiple out-of-sync definitions.

use std::collections::BTreeMap;

use padmap_core::capture::{Chooser, MappingRun};
use padmap_core::state::{PlayerState, StateEvent};
use serde_json::{json, Value};

pub fn round3(value: f64) -> f64 {
    format!("{value:.3}").parse().unwrap_or(value)
}

pub fn error(message: impl Into<String>) -> Value {
    json!({ "event": "error", "message": message.into() })
}

pub fn state(
    state: &str,
    slots: u32,
    players: Vec<PlayerState>,
    build: String,
    identity: &str,
) -> Value {
    let event = StateEvent::new(state, slots, players, build, std::process::id(), identity);
    serde_json::to_value(event).unwrap_or_else(|_| json!({ "event": "state" }))
}

pub fn pads(count: usize) -> Value {
    json!({ "event": "pads", "count": count })
}

pub fn progress(fraction: f64) -> Value {
    json!({ "event": "progress", "frac": round3(fraction) })
}

pub fn confirm(fraction: f64) -> Value {
    json!({ "event": "confirm", "frac": round3(fraction) })
}

pub fn claim(player: u32, name: &str, node: &str, icon: &str, configured: bool) -> Value {
    json!({
        "event": "claim",
        "player": player,
        "name": name,
        "node": node,
        "icon": icon,
        "configured": configured,
    })
}

pub fn newpad(names: &[String]) -> Value {
    json!({ "event": "newpad", "names": names })
}

pub fn calibration(
    phase: &str,
    fraction: f64,
    player: u32,
    name: Option<&str>,
    axes: Option<usize>,
) -> Value {
    let mut event = json!({
        "event": "calibration",
        "phase": phase,
        "frac": round3(fraction),
        "player": player,
    });
    if let Some(name) = name {
        event["name"] = json!(name);
    }
    if let Some(axes) = axes {
        event["axes"] = json!(axes);
    }
    event
}

fn layout_json(layout_id: &str) -> Value {
    serde_json::to_value(padmap_core::layout::get(layout_id)).unwrap_or(Value::Null)
}

pub fn layout_choice(chooser: &Chooser) -> Value {
    json!({
        "event": "layout_choice",
        "active": !chooser.confirmed(),
        "player": chooser.player,
        "kind": chooser.kind.as_str(),
        "title": chooser.title,
        "index": chooser.index(),
        "chosen": chooser.chosen(),
        "choices": chooser.options.iter().map(|option| json!({
            "id": option.id,
            "label": option.label,
            "mapped": option.mapped,
            "layout": layout_json(&option.layout),
        })).collect::<Vec<_>>(),
    })
}

pub fn layout_choice_ended(chooser: Option<&Chooser>) -> Value {
    json!({
        "event": "layout_choice",
        "active": false,
        "player": chooser.map(|c| c.player).unwrap_or(0),
        "index": chooser.map(|c| c.index()).unwrap_or(0),
        "kind": chooser.map(|c| c.kind.as_str()).unwrap_or("layout"),
        "title": chooser.map(|c| c.title.as_str()).unwrap_or(""),
        "chosen": chooser.map(|c| c.chosen()).unwrap_or(""),
        "choices": [],
    })
}

pub fn mapping(run: &MappingRun) -> Value {
    let control = run.current();
    let label = control
        .and_then(|c| {
            run.layout
                .controls
                .iter()
                .find(|entry| entry.canonical == c)
        })
        .map(|entry| entry.label.clone())
        .unwrap_or_default();
    let captured: BTreeMap<String, String> = run
        .bindings()
        .iter()
        .filter_map(|(control, binding)| binding.sdl().ok().map(|sdl| (control.to_string(), sdl)))
        .collect();
    json!({
        "event": "mapping",
        "player": run.player,
        "layout": serde_json::to_value(run.layout).unwrap_or(Value::Null),
        "index": run.index(),
        "total": run.total(),
        "control": control.map(|c| c.to_string()).unwrap_or_default(),
        "label": label,
        "done": run.finished(),
        "conflict": run.conflict().map(|c| c.to_string()).unwrap_or_default(),
        "captured": captured,
    })
}

pub fn mapping_done(run: Option<&MappingRun>, stored: bool) -> Value {
    json!({
        "event": "mapping",
        "done": true,
        "stored": stored,
        "player": run.map(|r| r.player).unwrap_or(0),
        "index": run.map(|r| r.index()).unwrap_or(0),
        "total": run.map(|r| r.total()).unwrap_or(0),
        "layout": run.map(|r| serde_json::to_value(r.layout).unwrap_or(Value::Null)).unwrap_or(json!({})),
        "control": "",
        "label": "",
        "captured": {},
    })
}

pub fn accepted(players: Vec<PlayerState>, launch_config: &str) -> Value {
    json!({
        "event": "accepted",
        "players": players,
        "launch_config": launch_config,
    })
}

pub fn sdl_mapping(lines: &[String]) -> Value {
    json!({ "event": "sdl_mapping", "lines": lines })
}

pub fn tuned(player: u32, signature: &str, tuning: &padmap_core::tuning::Tuning) -> Value {
    json!({
        "event": "tuned",
        "player": player,
        "signature": signature,
        "tuning": serde_json::to_value(tuning).unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions_are_rounded_to_three_places() {
        assert_eq!(progress(0.123456)["frac"], 0.123);
        assert_eq!(confirm(1.0)["frac"], 1.0);
        assert_eq!(round3(0.0005), 0.001);
    }

    #[test]
    fn a_calibration_event_only_names_what_it_was_given() {
        let bare = calibration("rest", 0.5, 1, None, None);
        assert!(bare.get("name").is_none());
        assert!(bare.get("axes").is_none());
        let full = calibration("icon", 1.0, 2, Some("Pad"), Some(3));
        assert_eq!(full["name"], "Pad");
        assert_eq!(full["axes"], 3);
    }

    #[test]
    fn a_closed_picker_has_no_choices_and_is_inactive() {
        let event = layout_choice_ended(None);
        assert_eq!(event["active"], false);
        assert_eq!(event["kind"], "layout");
        assert_eq!(event["choices"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn a_finished_wizard_says_whether_it_kept_anything() {
        let event = mapping_done(None, false);
        assert_eq!(event["done"], true);
        assert_eq!(event["stored"], false);
        assert_eq!(event["captured"], json!({}));
    }
}
