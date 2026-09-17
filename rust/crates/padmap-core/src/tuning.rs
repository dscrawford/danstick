//! What a user sets for a controller that misbehaves.
//!
//! Calibration measures where a stick rests and corrects for it. That is
//! enough for a stick that rests off-centre and stays put; it is not enough
//! for one that wanders, a button whose switch bounces, or a trigger that
//! reports noise it was never touched to make. Those need something the user
//! *sets*, because no measurement says how much drift a person is prepared to
//! put up with.
//!
//! Three knobs, all applied in transit so every consumer benefits at once:
//!
//! * **deadzone**, per axis, as a fraction of its travel. Around the middle
//!   for a stick and above the minimum for a trigger; inside it reads as
//!   untouched, and outside is rescaled so full deflection still reaches the
//!   end. Applied *after* calibration, so it is a band around the corrected
//!   centre.
//! * **debounce**, in milliseconds, for every button. A release is held
//!   back for that long, and a press arriving inside the window cancels it
//!   -- which is what a bouncing switch produces, a release-and-press pair
//!   a few milliseconds apart that a game reads as a double tap.
//! * **ignore**, for an axis or button that is simply broken. Its events are
//!   dropped, which beats a trigger that fires on its own.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::calibration::Declared;

/// The longest debounce worth having. A release held back for longer than
/// this is a button that feels broken in a different way.
pub const MAX_DEBOUNCE_MS: u32 = 500;

/// A user's settings for one controller.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tuning {
    /// ABS code -> fraction of travel, 0..=1, that reads as rest.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deadzone: BTreeMap<u16, f32>,
    /// Milliseconds a release is held back, for every button. 0 is off.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub debounce_ms: u32,
    /// ABS codes whose events are dropped.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub ignore_axes: BTreeSet<u16>,
    /// Key codes whose events are dropped.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub ignore_buttons: BTreeSet<u16>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl Tuning {
    /// Nothing set, which is what every controller starts with.
    pub fn is_default(&self) -> bool {
        *self == Tuning::default()
    }

    /// Whether this tuning changes anything about ABS events.
    pub fn shapes_axes(&self) -> bool {
        !self.deadzone.is_empty() || !self.ignore_axes.is_empty()
    }

    /// The deadzone for one axis, clamped to what makes sense.
    ///
    /// A fraction of 1 or more is an axis that can never leave rest, which is
    /// `ignore` spelled badly; it is capped just short so the axis still
    /// reaches its ends when slammed.
    pub fn deadzone_for(&self, code: u16) -> f32 {
        self.deadzone
            .get(&code)
            .copied()
            .filter(|fraction| fraction.is_finite())
            .map(|fraction| fraction.clamp(0.0, 0.95))
            .unwrap_or(0.0)
    }

    /// Set every knob that is `Some`, leaving the rest as they were.
    ///
    /// A front-end sends only the field a slider moved; a `None` is "not
    /// mentioned", not "back to default". Resetting is [`Tuning::default`].
    pub fn merged(&self, change: &Change) -> Tuning {
        let mut out = self.clone();
        for (code, fraction) in &change.deadzone {
            if *fraction <= 0.0 {
                out.deadzone.remove(code);
            } else {
                out.deadzone.insert(*code, fraction.clamp(0.0, 0.95));
            }
        }
        if let Some(debounce) = change.debounce_ms {
            out.debounce_ms = debounce.min(MAX_DEBOUNCE_MS);
        }
        if let Some(axes) = &change.ignore_axes {
            out.ignore_axes = axes.clone();
        }
        if let Some(buttons) = &change.ignore_buttons {
            out.ignore_buttons = buttons.clone();
        }
        out
    }

    /// One ABS value, with this axis's deadzone applied.
    ///
    /// `declared` says where the axis rests, which decides whether the band
    /// sits around the middle or above the minimum. `None` when the event
    /// should be dropped.
    pub fn shape_axis(&self, code: u16, value: i32, declared: Option<&Declared>) -> Option<i32> {
        if self.ignore_axes.contains(&code) {
            return None;
        }
        let fraction = self.deadzone_for(code);
        let Some(declared) = declared else {
            return Some(value);
        };
        if fraction <= 0.0 || declared.maximum <= declared.minimum {
            return Some(value);
        }
        if declared.calibratable(code) || rests_centred(declared, code) {
            Some(centred_band(value, declared, fraction))
        } else {
            Some(trigger_band(value, declared, fraction))
        }
    }

    /// Whether a key event should be dropped outright.
    pub fn ignores_button(&self, code: u16) -> bool {
        self.ignore_buttons.contains(&code)
    }
}

/// A stick axis: one that rests near the middle of its travel.
///
/// `Declared::calibratable` says so for the standard stick codes, but refuses
/// the trigger codes outright -- and the Mayflash GameCube adapter has its
/// C-stick on those. The rest position is what decides, whatever the code.
fn rests_centred(declared: &Declared, _code: u16) -> bool {
    crate::sdl::AxisSpan::new(declared.minimum, declared.maximum, declared.value).rests_centred()
}

/// A band around the middle, with the outside rescaled to keep the ends.
fn centred_band(value: i32, declared: &Declared, fraction: f32) -> i32 {
    let low = f64::from(declared.minimum);
    let high = f64::from(declared.maximum);
    let mid = (low + high) / 2.0;
    let half = (high - low) / 2.0;
    let dead = half * f64::from(fraction);
    let value = f64::from(value).clamp(low, high);
    let offset = value - mid;
    if offset.abs() <= dead {
        // The same middle `AxisCalibration::midpoint` lands on: integer
        // division, so -32768..32767 rests at 0 and not at -1.
        return ((i64::from(declared.minimum) + i64::from(declared.maximum)) / 2) as i32;
    }
    let live = half - dead;
    let scaled = (offset.abs() - dead) / live * half;
    let out = if offset < 0.0 {
        mid - scaled
    } else {
        mid + scaled
    };
    out.round().clamp(low, high) as i32
}

/// A band above the minimum, for a trigger that never quite lets go.
fn trigger_band(value: i32, declared: &Declared, fraction: f32) -> i32 {
    let low = f64::from(declared.minimum);
    let high = f64::from(declared.maximum);
    let span = high - low;
    let dead = span * f64::from(fraction);
    let value = f64::from(value).clamp(low, high);
    let travel = value - low;
    if travel <= dead {
        return declared.minimum;
    }
    let out = low + (travel - dead) / (span - dead) * span;
    out.round().clamp(low, high) as i32
}

/// A partial update: only what a caller mentioned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Change {
    /// Axis -> fraction. Zero or less removes that axis's band.
    pub deadzone: BTreeMap<u16, f32>,
    pub debounce_ms: Option<u32>,
    pub ignore_axes: Option<BTreeSet<u16>>,
    pub ignore_buttons: Option<BTreeSet<u16>>,
}

/// The axes a blanket deadzone applies to: `ABS_X` through `ABS_RZ`. Hats
/// are digital and everything above them is a throttle or a wheel.
pub const BLANKET_AXES: [u16; 6] = [0, 1, 2, 3, 4, 5];

/// A `tune` request, as it arrives over the socket or from the CLI.
///
/// Every field is optional and only what is present is changed, except
/// `reset`, which starts from nothing first. `deadzone` may be one number,
/// meaning every stick and trigger the pad has, or an object of axis code
/// to fraction.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Request {
    pub reset: bool,
    pub deadzone_all: Option<f32>,
    pub deadzone: BTreeMap<u16, f32>,
    pub debounce_ms: Option<u32>,
    pub ignore_axes: Option<BTreeSet<u16>>,
    pub ignore_buttons: Option<BTreeSet<u16>>,
}

impl Request {
    /// Whether this asks for anything at all.
    pub fn is_empty(&self) -> bool {
        *self == Request::default()
    }

    /// Read one off a JSON message, refusing anything that is not what it
    /// claims to be: a deadzone that is a string, a negative debounce, a code
    /// past what evdev has. Refused rather than guessed, because a setting
    /// misread is a controller that behaves strangely for no visible reason.
    pub fn from_json(message: &serde_json::Value) -> Result<Request, String> {
        use serde_json::Value;
        let mut out = Request {
            reset: matches!(message.get("reset"), Some(Value::Bool(true))),
            ..Request::default()
        };
        match message.get("deadzone") {
            None | Some(Value::Null) => {}
            Some(Value::Number(number)) => {
                out.deadzone_all = Some(fraction(number.as_f64(), "deadzone")?);
            }
            Some(Value::Object(axes)) => {
                for (code, value) in axes {
                    let code = axis_code(code)?;
                    let number = value.as_f64();
                    out.deadzone.insert(code, fraction(number, "deadzone")?);
                }
            }
            Some(_) => {
                return Err("deadzone must be a number or an object of axis to number".into())
            }
        }
        match message.get("debounce_ms") {
            None | Some(Value::Null) => {}
            Some(Value::Number(number)) => {
                let value = number
                    .as_u64()
                    .filter(|value| *value <= u64::from(MAX_DEBOUNCE_MS))
                    .ok_or_else(|| format!("debounce_ms must be 0..={MAX_DEBOUNCE_MS}"))?;
                out.debounce_ms = Some(value as u32);
            }
            Some(_) => return Err("debounce_ms must be a number".into()),
        }
        out.ignore_axes = codes(message.get("ignore_axes"), "ignore_axes")?;
        out.ignore_buttons = codes(message.get("ignore_buttons"), "ignore_buttons")?;
        Ok(out)
    }

    /// The same request as a message, for a CLI to send to the daemon.
    pub fn to_json(&self) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        if self.reset {
            out.insert("reset".into(), true.into());
        }
        if let Some(all) = self.deadzone_all {
            out.insert("deadzone".into(), all.into());
        } else if !self.deadzone.is_empty() {
            let axes: serde_json::Map<String, serde_json::Value> = self
                .deadzone
                .iter()
                .map(|(code, fraction)| (code.to_string(), (*fraction).into()))
                .collect();
            out.insert("deadzone".into(), axes.into());
        }
        if let Some(debounce) = self.debounce_ms {
            out.insert("debounce_ms".into(), debounce.into());
        }
        if let Some(axes) = &self.ignore_axes {
            out.insert(
                "ignore_axes".into(),
                axes.iter().copied().collect::<Vec<u16>>().into(),
            );
        }
        if let Some(buttons) = &self.ignore_buttons {
            out.insert(
                "ignore_buttons".into(),
                buttons.iter().copied().collect::<Vec<u16>>().into(),
            );
        }
        out.into()
    }

    /// What this asks to change, given the axes the pad declares.
    ///
    /// A blanket deadzone lands on every declared axis in [`BLANKET_AXES`];
    /// a per-axis one is taken as given, so a code the pad does not have is
    /// stored and simply never fires.
    pub fn change_for(&self, declared_axes: &[u16]) -> Change {
        let mut deadzone = BTreeMap::new();
        if let Some(all) = self.deadzone_all {
            for code in BLANKET_AXES {
                if declared_axes.contains(&code) {
                    deadzone.insert(code, all);
                }
            }
        }
        deadzone.extend(self.deadzone.iter().map(|(code, f)| (*code, *f)));
        Change {
            deadzone,
            debounce_ms: self.debounce_ms,
            ignore_axes: self.ignore_axes.clone(),
            ignore_buttons: self.ignore_buttons.clone(),
        }
    }

    /// The tuning after this request: from nothing if `reset`, else from
    /// what was there.
    pub fn apply(&self, existing: &Tuning, declared_axes: &[u16]) -> Tuning {
        let base = if self.reset {
            Tuning::default()
        } else {
            existing.clone()
        };
        base.merged(&self.change_for(declared_axes))
    }
}

fn fraction(number: Option<f64>, field: &str) -> Result<f32, String> {
    match number {
        Some(value) if value.is_finite() && (0.0..=1.0).contains(&value) => Ok(value as f32),
        _ => Err(format!("{field} must be a number from 0 to 1")),
    }
}

fn axis_code(text: &str) -> Result<u16, String> {
    text.trim()
        .parse::<u16>()
        .ok()
        .filter(|code| *code < 0x40)
        .ok_or_else(|| format!("{text:?} is not an ABS code"))
}

fn codes(raw: Option<&serde_json::Value>, field: &str) -> Result<Option<BTreeSet<u16>>, String> {
    use serde_json::Value;
    match raw {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_u64()
                    .filter(|code| *code <= u64::from(u16::MAX))
                    .map(|code| code as u16)
                    .ok_or_else(|| format!("{field} must be a list of event codes"))
            })
            .collect::<Result<BTreeSet<u16>, String>>()
            .map(Some),
        Some(_) => Err(format!("{field} must be a list of event codes")),
    }
}

/// Holds releases back so a bouncing switch reads as one press.
///
/// A press is passed through at once. A release is kept until `window_ms`
/// has passed; a press for the same key inside that window cancels it and
/// emits nothing, because the clone still shows the key held. The cost is
/// that every genuine release arrives late by the window, which is why the
/// window is a few tens of milliseconds and never more than
/// [`MAX_DEBOUNCE_MS`].
#[derive(Debug, Clone, Default)]
pub struct Debouncer {
    window_ms: u64,
    /// Key -> when its pending release is due, in the caller's milliseconds.
    pending: BTreeMap<u16, u64>,
}

impl Debouncer {
    pub fn new(window_ms: u32) -> Debouncer {
        Debouncer {
            window_ms: u64::from(window_ms.min(MAX_DEBOUNCE_MS)),
            pending: BTreeMap::new(),
        }
    }

    /// Whether anything is being held back.
    pub fn is_holding(&self) -> bool {
        !self.pending.is_empty()
    }

    /// One key event. `Some(value)` to pass it on now, `None` to swallow it.
    ///
    /// `now_ms` is any monotonic millisecond clock.
    pub fn key(&mut self, code: u16, value: i32, now_ms: u64) -> Option<i32> {
        if self.window_ms == 0 {
            return Some(value);
        }
        if value == 0 {
            // Held back. A second release for a key already pending -- a
            // driver repeating itself -- restarts nothing; the first deadline
            // stands, so a stream of releases cannot hold a key for ever.
            self.pending.entry(code).or_insert(now_ms + self.window_ms);
            return None;
        }
        // A press. If a release was pending, this is the bounce: the clone
        // never saw the release, so it must not see this press either.
        if self.pending.remove(&code).is_some() {
            return None;
        }
        Some(value)
    }

    /// Every release whose window has passed, in code order.
    pub fn due(&mut self, now_ms: u64) -> Vec<u16> {
        let due: Vec<u16> = self
            .pending
            .iter()
            .filter(|(_, deadline)| **deadline <= now_ms)
            .map(|(code, _)| *code)
            .collect();
        for code in &due {
            self.pending.remove(code);
        }
        due
    }

    /// Every release still pending, released now. For a pause, or a pad
    /// going away: a release held back with nothing left to deliver it is a
    /// key stuck down for ever.
    pub fn drain(&mut self) -> Vec<u16> {
        let all: Vec<u16> = self.pending.keys().copied().collect();
        self.pending.clear();
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stick() -> Declared {
        Declared {
            minimum: -32768,
            maximum: 32767,
            value: 0,
            flat: 0,
        }
    }

    fn trigger() -> Declared {
        Declared {
            minimum: 0,
            maximum: 255,
            value: 0,
            flat: 0,
        }
    }

    fn with_deadzone(code: u16, fraction: f32) -> Tuning {
        Tuning {
            deadzone: BTreeMap::from([(code, fraction)]),
            ..Tuning::default()
        }
    }

    #[test]
    fn inside_the_band_a_stick_reads_as_untouched() {
        let tuning = with_deadzone(0, 0.2);
        // 20% of 32768 is 6553.
        assert_eq!(tuning.shape_axis(0, 3000, Some(&stick())), Some(0));
        assert_eq!(tuning.shape_axis(0, -6000, Some(&stick())), Some(0));
        assert_eq!(tuning.shape_axis(0, 0, Some(&stick())), Some(0));
    }

    #[test]
    fn outside_the_band_a_stick_still_reaches_its_ends() {
        // A deadzone that shortened the travel would be a character that
        // cannot run. The live part is stretched back over the whole range.
        let tuning = with_deadzone(0, 0.2);
        assert_eq!(tuning.shape_axis(0, 32767, Some(&stick())), Some(32767));
        assert_eq!(tuning.shape_axis(0, -32768, Some(&stick())), Some(-32768));
        // Just past the band is just past the middle.
        let just_out = tuning.shape_axis(0, 6700, Some(&stick())).expect("a value");
        assert!(just_out > 0 && just_out < 1000, "{just_out}");
    }

    #[test]
    fn the_band_is_continuous_and_monotonic() {
        let tuning = with_deadzone(0, 0.15);
        let mut last = i32::MIN;
        for raw in (-32768..=32767).step_by(64) {
            let out = tuning.shape_axis(0, raw, Some(&stick())).expect("a value");
            assert!(out >= last, "{raw}: {out} after {last}");
            last = out;
        }
    }

    #[test]
    fn a_trigger_band_sits_above_the_minimum_not_around_the_middle() {
        // A trigger rests at its minimum. A band around the middle would
        // leave the noise it was set for and eat the middle of the pull.
        let tuning = with_deadzone(2, 0.1);
        assert_eq!(tuning.shape_axis(2, 20, Some(&trigger())), Some(0));
        assert_eq!(tuning.shape_axis(2, 0, Some(&trigger())), Some(0));
        assert_eq!(tuning.shape_axis(2, 255, Some(&trigger())), Some(255));
        let mid = tuning
            .shape_axis(2, 140, Some(&trigger()))
            .expect("a value");
        assert!(mid > 120 && mid < 140, "{mid}");
    }

    #[test]
    fn a_stick_on_the_trigger_codes_is_still_a_stick() {
        // The Mayflash C-stick: ABS_Z, resting at 131 of 0..255. The code says
        // trigger; the rest position says stick, and the rest position wins.
        let c_stick = Declared {
            minimum: 0,
            maximum: 255,
            value: 131,
            flat: 0,
        };
        let tuning = with_deadzone(2, 0.2);
        // 0..255 has no exact middle; 127 is where calibration puts it too.
        assert_eq!(tuning.shape_axis(2, 128, Some(&c_stick)), Some(127));
        assert_eq!(tuning.shape_axis(2, 110, Some(&c_stick)), Some(127));
        assert_eq!(tuning.shape_axis(2, 0, Some(&c_stick)), Some(0));
    }

    #[test]
    fn an_axis_with_no_band_passes_through_untouched() {
        let tuning = Tuning::default();
        assert_eq!(tuning.shape_axis(0, 1234, Some(&stick())), Some(1234));
        assert_eq!(tuning.shape_axis(0, 1234, None), Some(1234));
    }

    #[test]
    fn an_ignored_axis_is_dropped() {
        let tuning = Tuning {
            ignore_axes: BTreeSet::from([5]),
            ..Tuning::default()
        };
        assert_eq!(tuning.shape_axis(5, 200, Some(&trigger())), None);
        assert_eq!(tuning.shape_axis(2, 200, Some(&trigger())), Some(200));
    }

    #[test]
    fn a_band_of_one_or_more_is_capped_rather_than_eating_the_axis() {
        // A fraction of 1 is an axis that can never move; NaN is a division
        // that reaches the clone as garbage. Both come from a hand-edited
        // file, and both are capped.
        for silly in [1.0, 5.0, f32::NAN, f32::INFINITY] {
            let tuning = with_deadzone(0, silly);
            assert_eq!(tuning.shape_axis(0, 32767, Some(&stick())), Some(32767));
            assert_eq!(tuning.shape_axis(0, 0, Some(&stick())), Some(0));
        }
        assert_eq!(with_deadzone(0, -0.5).deadzone_for(0), 0.0);
    }

    #[test]
    fn a_degenerate_range_is_passed_through_rather_than_divided() {
        let flat = Declared {
            minimum: 7,
            maximum: 7,
            value: 7,
            flat: 0,
        };
        assert_eq!(with_deadzone(0, 0.5).shape_axis(0, 7, Some(&flat)), Some(7));
    }

    #[test]
    fn a_change_touches_only_what_it_mentions() {
        let start = Tuning {
            deadzone: BTreeMap::from([(0, 0.1), (1, 0.1)]),
            debounce_ms: 20,
            ignore_axes: BTreeSet::from([5]),
            ignore_buttons: BTreeSet::new(),
        };
        let change = Change {
            deadzone: BTreeMap::from([(1, 0.3), (0, 0.0)]),
            ..Change::default()
        };
        let out = start.merged(&change);
        assert_eq!(
            out.deadzone,
            BTreeMap::from([(1, 0.3)]),
            "0 removed, 1 changed"
        );
        assert_eq!(out.debounce_ms, 20, "not mentioned, not changed");
        assert_eq!(out.ignore_axes, BTreeSet::from([5]));
    }

    #[test]
    fn a_debounce_past_the_cap_is_capped() {
        let out = Tuning::default().merged(&Change {
            debounce_ms: Some(10_000),
            ..Change::default()
        });
        assert_eq!(out.debounce_ms, MAX_DEBOUNCE_MS);
    }

    #[test]
    fn the_file_shape_omits_what_is_not_set() {
        // A profile with nothing tuned should not grow four keys of noise.
        let text = serde_json::to_string(&Tuning::default()).expect("serialises");
        assert_eq!(text, "{}");
        let back: Tuning = serde_json::from_str("{}").expect("parses");
        assert!(back.is_default());
        let tuned: Tuning = serde_json::from_str(
            r#"{"deadzone":{"0":0.2},"debounce_ms":30,"ignore_axes":[2],"ignore_buttons":[311]}"#,
        )
        .expect("parses");
        assert_eq!(tuned.deadzone_for(0), 0.2);
        assert_eq!(tuned.debounce_ms, 30);
        assert!(tuned.ignores_button(311));
    }

    // --- debounce ---------------------------------------------------------

    #[test]
    fn a_press_passes_straight_through() {
        let mut debouncer = Debouncer::new(30);
        assert_eq!(debouncer.key(0x130, 1, 1000), Some(1));
        assert!(!debouncer.is_holding());
    }

    #[test]
    fn a_release_is_held_and_then_delivered() {
        let mut debouncer = Debouncer::new(30);
        assert_eq!(debouncer.key(0x130, 1, 1000), Some(1));
        assert_eq!(debouncer.key(0x130, 0, 1010), None);
        assert!(debouncer.is_holding());
        assert!(debouncer.due(1039).is_empty(), "not yet");
        assert_eq!(debouncer.due(1040), vec![0x130]);
        assert!(!debouncer.is_holding());
    }

    #[test]
    fn a_bounce_inside_the_window_collapses_to_one_press() {
        // Release then press 5ms apart is what a worn switch produces. The
        // clone saw the press and never the release, so the second press must
        // not reach it either -- a press on a key already down is a repeat.
        let mut debouncer = Debouncer::new(30);
        assert_eq!(debouncer.key(0x130, 1, 1000), Some(1));
        assert_eq!(debouncer.key(0x130, 0, 1100), None);
        assert_eq!(debouncer.key(0x130, 1, 1105), None, "the bounce");
        assert!(!debouncer.is_holding(), "the release was cancelled");
        assert!(debouncer.due(2000).is_empty());
        // And the real release later goes through as normal.
        assert_eq!(debouncer.key(0x130, 0, 1500), None);
        assert_eq!(debouncer.due(1530), vec![0x130]);
    }

    #[test]
    fn a_window_of_zero_holds_nothing() {
        let mut debouncer = Debouncer::new(0);
        assert_eq!(debouncer.key(0x130, 0, 1000), Some(0));
        assert!(!debouncer.is_holding());
    }

    #[test]
    fn repeated_releases_do_not_push_the_deadline_out() {
        // A driver that repeats a release every 8ms would otherwise hold the
        // key for as long as it kept repeating.
        let mut debouncer = Debouncer::new(30);
        debouncer.key(0x130, 0, 1000);
        debouncer.key(0x130, 0, 1020);
        debouncer.key(0x130, 0, 1028);
        assert_eq!(debouncer.due(1030), vec![0x130]);
    }

    #[test]
    fn keys_are_independent() {
        let mut debouncer = Debouncer::new(30);
        debouncer.key(0x130, 0, 1000);
        debouncer.key(0x131, 0, 1020);
        assert_eq!(debouncer.due(1030), vec![0x130]);
        assert_eq!(debouncer.due(1050), vec![0x131]);
    }

    #[test]
    fn draining_delivers_everything_at_once() {
        let mut debouncer = Debouncer::new(500);
        debouncer.key(0x131, 0, 1000);
        debouncer.key(0x130, 0, 1000);
        assert_eq!(debouncer.drain(), vec![0x130, 0x131]);
        assert!(!debouncer.is_holding());
    }

    // --- requests ---------------------------------------------------------

    #[test]
    fn a_blanket_deadzone_lands_on_every_stick_and_trigger_the_pad_has() {
        let request = Request::from_json(&serde_json::json!({"deadzone": 0.2})).expect("parses");
        let change = request.change_for(&[0, 1, 2, 5, 0x10, 0x11]);
        assert_eq!(
            change.deadzone,
            BTreeMap::from([(0, 0.2), (1, 0.2), (2, 0.2), (5, 0.2)]),
            "not the hat, not axes the pad lacks"
        );
    }

    #[test]
    fn a_per_axis_deadzone_is_taken_as_given() {
        let request = Request::from_json(&serde_json::json!({"deadzone": {"3": 0.3, "4": 0.1}}))
            .expect("parses");
        let change = request.change_for(&[0, 1]);
        assert_eq!(change.deadzone, BTreeMap::from([(3, 0.3), (4, 0.1)]));
    }

    #[test]
    fn a_request_that_mentions_nothing_changes_nothing() {
        let request =
            Request::from_json(&serde_json::json!({"cmd": "tune", "player": 1})).expect("parses");
        assert!(request.is_empty());
        let existing = Tuning {
            debounce_ms: 40,
            ..Tuning::default()
        };
        assert_eq!(request.apply(&existing, &[0, 1]), existing);
    }

    #[test]
    fn reset_starts_from_nothing_and_then_applies_the_rest() {
        let request = Request::from_json(&serde_json::json!({"reset": true, "debounce_ms": 10}))
            .expect("parses");
        let existing = Tuning {
            deadzone: BTreeMap::from([(0, 0.5)]),
            debounce_ms: 40,
            ..Tuning::default()
        };
        let out = request.apply(&existing, &[0]);
        assert!(out.deadzone.is_empty(), "reset dropped the deadzone");
        assert_eq!(out.debounce_ms, 10);
    }

    #[test]
    fn what_is_not_what_it_claims_is_refused_not_guessed() {
        for bad in [
            serde_json::json!({"deadzone": "lots"}),
            serde_json::json!({"deadzone": 1.5}),
            serde_json::json!({"deadzone": -0.1}),
            serde_json::json!({"deadzone": {"x": 0.1}}),
            serde_json::json!({"deadzone": {"0": "0.1"}}),
            serde_json::json!({"debounce_ms": -5}),
            serde_json::json!({"debounce_ms": 5000}),
            serde_json::json!({"debounce_ms": "30"}),
            serde_json::json!({"ignore_axes": 2}),
            serde_json::json!({"ignore_buttons": ["a"]}),
        ] {
            assert!(Request::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_request_survives_a_trip_through_its_own_json() {
        let request = Request {
            reset: true,
            deadzone_all: None,
            deadzone: BTreeMap::from([(0, 0.25)]),
            debounce_ms: Some(30),
            ignore_axes: Some(BTreeSet::from([5])),
            ignore_buttons: Some(BTreeSet::new()),
        };
        let back = Request::from_json(&request.to_json()).expect("parses");
        assert_eq!(back, request);
        let blanket = Request {
            deadzone_all: Some(0.1),
            ..Request::default()
        };
        assert_eq!(
            Request::from_json(&blanket.to_json()).expect("parses"),
            blanket
        );
    }
}
