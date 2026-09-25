//! Button mapping wizard.

use std::collections::{BTreeMap, BTreeSet};

use crate::binding::{axis_index, retroarch_button_index, sdl_button_index, Binding, BindingKind};
use crate::control::Control;
use crate::layout::Layout;
use crate::sdl::AxisSpan;

pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

pub const AXIS_THRESHOLD: f64 = 0.55;

/// Axis must settle within this of rest before re-arming (prevents overshoot re-triggering).
pub const AXIS_RELEASE: f64 = 0.30;

/// Axis must move far to answer a face-button prompt (axis binding is expensive).
pub const AXIS_AS_BUTTON_THRESHOLD: f64 = 0.90;

pub const HAT_UP: i32 = 1;
pub const HAT_RIGHT: i32 = 2;
pub const HAT_DOWN: i32 = 4;
pub const HAT_LEFT: i32 = 8;

pub const SKIP_HOLD_SECONDS: f64 = 0.8;

/// One tier above skip: hold this long to end the run and keep what is bound.
pub const FINISH_HOLD_SECONDS: f64 = 2.0;

pub const CAPTURE_GAP_SECONDS: f64 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl Event {
    pub const fn key(code: u16, value: i32) -> Self {
        Event {
            kind: EV_KEY,
            code,
            value,
        }
    }

    pub const fn abs(code: u16, value: i32) -> Self {
        Event {
            kind: EV_ABS,
            code,
            value,
        }
    }
}

pub fn deflection(span: AxisSpan, value: i32) -> f64 {
    if span.maximum <= span.minimum {
        return 0.0;
    }
    let travel = i64::from(value) - i64::from(span.rest);
    let span = i64::from(span.maximum) - i64::from(span.minimum);
    travel as f64 / (span as f64 / 2.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Claim {
    Button { code: u16 },
    Hat { index: i32, value: i32 },
    Axis { code: u16, sign: i32 },
}

impl std::fmt::Display for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Claim::Button { code } => write!(f, "button code {code}"),
            Claim::Hat { value, .. } => {
                let word = match *value {
                    HAT_UP => "up",
                    HAT_RIGHT => "right",
                    HAT_DOWN => "down",
                    HAT_LEFT => "left",
                    other => return write!(f, "hat {other}"),
                };
                write!(f, "hat {word}")
            }
            Claim::Axis { code, sign } => {
                write!(f, "axis code {code} {}", if *sign > 0 { '+' } else { '-' })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Ignored,
    Recorded {
        control: Control,
        binding: Binding,
    },
    Skipped {
        control: Control,
    },
    Refused {
        claim: Claim,
        held_by: Control,
    },
    TooGentle {
        claim: Claim,
        travel: f64,
        needed: f64,
    },
    /// A hold reached [`FINISH_HOLD_SECONDS`]: the run is over, keep what is bound.
    Finished,
}

impl Outcome {
    pub fn advanced(&self) -> bool {
        matches!(self, Outcome::Recorded { .. } | Outcome::Skipped { .. })
    }
}

/// A raw input in the terms a profile uses: kind, ordinal, and a value that is
/// 0 or 1 for a button, a direction bit for a hat, -1..1 for an axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pressed {
    pub kind: BindingKind,
    pub index: i32,
    pub value: f64,
}

#[derive(Debug, Clone)]
pub struct MappingRun {
    pub player: u32,
    pub layout: &'static Layout,
    pub keys: Vec<u16>,
    pub scope: String,
    pub axes: BTreeMap<u16, AxisSpan>,

    index: usize,
    bindings: BTreeMap<Control, Binding>,
    claimed: BTreeMap<Claim, Control>,
    opening_held: BTreeSet<u16>,
    down: BTreeSet<u16>,
    down_at: BTreeMap<u16, f64>,
    blocked_until: f64,
    conflict: Option<Control>,
    axis_armed: BTreeMap<u16, bool>,
    ended: bool,
    /// Set when the run is for one control only: the last index it walks.
    last: Option<usize>,
    /// The press captured is a second input for the control, not a replacement.
    pub add: bool,
}

impl MappingRun {
    pub fn new(
        player: u32,
        layout: &'static Layout,
        keys: Vec<u16>,
        scope: String,
        axes: BTreeMap<u16, AxisSpan>,
        held: BTreeSet<u16>,
    ) -> Self {
        MappingRun {
            player,
            layout,
            keys,
            scope,
            axes,
            index: 0,
            bindings: BTreeMap::new(),
            claimed: BTreeMap::new(),
            down: held.clone(),
            opening_held: held,
            down_at: BTreeMap::new(),
            blocked_until: 0.0,
            conflict: None,
            axis_armed: BTreeMap::new(),
            ended: false,
            last: None,
            add: false,
        }
    }

    /// Walk one control only: start on it and finish after it. None if the
    /// layout has no such control.
    pub fn only(mut self, control: Control) -> Option<Self> {
        let at = self
            .layout
            .controls
            .iter()
            .position(|entry| entry.canonical == control)?;
        self.index = at;
        self.last = Some(at);
        Some(self)
    }

    /// The press captured adds an input to the control rather than replacing it.
    pub fn adding(mut self) -> Self {
        self.add = true;
        self
    }

    /// The control this run is for, when it is for one.
    pub fn single(&self) -> Option<Control> {
        self.last
            .and_then(|at| self.layout.controls.get(at))
            .map(|entry| entry.canonical)
    }

    /// Start from a stored capture, so the conflict guard sees it and an early
    /// finish leaves a whole mapping.
    pub fn seeded(mut self, stored: &BTreeMap<Control, Binding>) -> Self {
        for (control, binding) in stored {
            if let Some(claim) = self.claim_of(*binding) {
                self.claimed.insert(claim, *control);
            }
            self.bindings.insert(*control, *binding);
        }
        self
    }

    /// The input a stored binding names, on this pad; None if the pad no longer has it.
    fn claim_of(&self, binding: Binding) -> Option<Claim> {
        match binding.kind {
            BindingKind::Button => self
                .keys
                .iter()
                .copied()
                .find(|code| sdl_button_index(&self.keys, *code) == Some(binding.index))
                .map(|code| Claim::Button { code }),
            BindingKind::Hat => Some(Claim::Hat {
                index: binding.index,
                value: binding.value,
            }),
            BindingKind::Axis => {
                let codes: Vec<u16> = self.axes.keys().copied().collect();
                codes
                    .iter()
                    .copied()
                    .find(|code| axis_index(&codes, *code) == Some(binding.index))
                    .map(|code| Claim::Axis {
                        code,
                        sign: if binding.value >= 0 { 1 } else { -1 },
                    })
            }
        }
    }

    pub fn settling(&self) -> bool {
        self.opening_held
            .iter()
            .any(|code| self.down.contains(code))
    }

    pub fn finished(&self) -> bool {
        self.ended
            || self.index >= self.layout.controls.len()
            || self.last.is_some_and(|last| self.index > last)
    }

    /// How far the longest live hold is towards finishing, or None with nothing held.
    pub fn finish_hold(&self, now: f64) -> Option<f64> {
        if self.finished() || self.settling() {
            return None;
        }
        self.down_at
            .iter()
            .filter(|(code, _)| !self.opening_held.contains(code))
            .map(|(_, started)| now - started)
            .fold(None, |best: Option<f64>, elapsed| {
                Some(best.map_or(elapsed, |b| b.max(elapsed)))
            })
            .map(|elapsed| (elapsed / FINISH_HOLD_SECONDS).clamp(0.0, 1.0))
    }

    /// Time passing: a hold that reaches the finish tier ends the run without a release.
    pub fn tick(&mut self, now: f64) -> Outcome {
        match self.finish_hold(now) {
            Some(fraction) if fraction >= 1.0 => self.finish(),
            _ => Outcome::Ignored,
        }
    }

    fn finish(&mut self) -> Outcome {
        self.ended = true;
        self.conflict = None;
        self.down_at.clear();
        Outcome::Finished
    }

    /// Who else holds this input; the control being asked for may re-take its own.
    fn taken_by_other(&self, claim: Claim) -> Option<Control> {
        self.claimed
            .get(&claim)
            .copied()
            .filter(|holder| Some(*holder) != self.current())
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn total(&self) -> usize {
        self.layout.controls.len()
    }

    pub fn conflict(&self) -> Option<Control> {
        self.conflict
    }

    pub fn bindings(&self) -> &BTreeMap<Control, Binding> {
        &self.bindings
    }

    pub fn current(&self) -> Option<Control> {
        self.layout
            .controls
            .get(self.index)
            .map(|control| control.canonical)
    }

    pub fn skip(&mut self, now: f64) -> Option<Control> {
        let control = self.current()?;
        self.index += 1;
        self.conflict = None;
        self.blocked_until = now + CAPTURE_GAP_SECONDS;
        Some(control)
    }

    fn record(&mut self, binding: Binding, claim: Claim, now: f64) -> Outcome {
        let Some(control) = self.current() else {
            return Outcome::Ignored;
        };
        self.claimed.retain(|_, holder| *holder != control);
        self.bindings.insert(control, binding);
        self.claimed.insert(claim, control);
        self.index += 1;
        self.conflict = None;
        self.blocked_until = now + CAPTURE_GAP_SECONDS;
        Outcome::Recorded { control, binding }
    }

    fn refuse(&mut self, claim: Claim, held_by: Control) -> Outcome {
        self.conflict = Some(held_by);
        Outcome::Refused { claim, held_by }
    }

    /// What `event` is on this pad, whether or not it binds anything -- so a
    /// front-end can show the button under the thumb while the wizard runs.
    pub fn describe(&self, event: Event) -> Option<Pressed> {
        match event.kind {
            EV_KEY => Some(Pressed {
                kind: BindingKind::Button,
                index: sdl_button_index(&self.keys, event.code)?,
                value: if event.value != 0 { 1.0 } else { 0.0 },
            }),
            EV_ABS if event.code == ABS_HAT0X || event.code == ABS_HAT0Y => {
                let bit = match (event.code, event.value.signum()) {
                    (ABS_HAT0Y, -1) => 1,
                    (ABS_HAT0X, 1) => 2,
                    (ABS_HAT0Y, 1) => 4,
                    (ABS_HAT0X, -1) => 8,
                    _ => 0,
                };
                Some(Pressed {
                    kind: BindingKind::Hat,
                    index: 0,
                    value: f64::from(bit),
                })
            }
            EV_ABS => {
                let span = *self.axes.get(&event.code)?;
                let codes: Vec<u16> = self.axes.keys().copied().collect();
                Some(Pressed {
                    kind: BindingKind::Axis,
                    index: axis_index(&codes, event.code)?,
                    value: deflection(span, event.value),
                })
            }
            _ => None,
        }
    }

    pub fn feed(&mut self, event: Event, now: f64) -> Outcome {
        if self.finished() {
            return Outcome::Ignored;
        }

        if now < self.blocked_until {
            if event.kind == EV_KEY && event.value == 0 {
                self.down.remove(&event.code);
                self.down_at.remove(&event.code);
                self.opening_held.remove(&event.code);
            } else if event.kind == EV_ABS {
                self.rearm(event);
            }
            return Outcome::Ignored;
        }

        match event.kind {
            EV_KEY => self.feed_key(event, now),
            EV_ABS => self.feed_abs(event, now),
            _ => Outcome::Ignored,
        }
    }

    fn feed_key(&mut self, event: Event, now: f64) -> Outcome {
        if event.value == 1 {
            self.down.insert(event.code);
            self.down_at.insert(event.code, now);
            return Outcome::Ignored;
        }
        if event.value != 0 {
            return Outcome::Ignored; // Autorepeat
        }

        self.down.remove(&event.code);
        let started = self.down_at.remove(&event.code);
        if self.opening_held.remove(&event.code) {
            return Outcome::Ignored;
        }
        let Some(started) = started else {
            return Outcome::Ignored;
        };
        if self.settling() {
            return Outcome::Ignored;
        }

        let held = now - started;
        if held >= FINISH_HOLD_SECONDS {
            return self.finish();
        }
        if held >= SKIP_HOLD_SECONDS {
            return match self.skip(now) {
                Some(control) => Outcome::Skipped { control },
                None => Outcome::Ignored,
            };
        }

        let claim = Claim::Button { code: event.code };
        if let Some(holder) = self.taken_by_other(claim) {
            return self.refuse(claim, holder);
        }

        let Some(index) = sdl_button_index(&self.keys, event.code) else {
            return Outcome::Ignored;
        };
        let binding =
            Binding::button(index).with_ra_index(retroarch_button_index(&self.keys, event.code));
        self.record(binding, claim, now)
    }

    fn rearm(&mut self, event: Event) {
        if event.code == ABS_HAT0X || event.code == ABS_HAT0Y {
            if event.value == 0 {
                self.axis_armed.insert(event.code, true);
            }
            return;
        }
        let Some(span) = self.axes.get(&event.code).copied() else {
            return;
        };
        if deflection(span, event.value).abs() < AXIS_RELEASE {
            self.axis_armed.insert(event.code, true);
        }
    }

    fn armed(&self, code: u16) -> bool {
        self.axis_armed.get(&code).copied().unwrap_or(true)
    }

    fn feed_abs(&mut self, event: Event, now: f64) -> Outcome {
        self.rearm(event);
        if self.settling() {
            return Outcome::Ignored;
        }

        let Some(current) = self.layout.controls.get(self.index) else {
            return Outcome::Ignored;
        };
        if current.kind == "button" {
            if event.code == ABS_HAT0X || event.code == ABS_HAT0Y {
                return Outcome::Ignored;
            }
            let Some(span) = self.axes.get(&event.code).copied() else {
                return Outcome::Ignored;
            };
            let position = deflection(span, event.value);
            let travel = position.abs();
            if travel < AXIS_AS_BUTTON_THRESHOLD {
                if travel >= AXIS_THRESHOLD {
                    let sign = if position > 0.0 { 1 } else { -1 };
                    return Outcome::TooGentle {
                        claim: Claim::Axis {
                            code: event.code,
                            sign,
                        },
                        travel,
                        needed: AXIS_AS_BUTTON_THRESHOLD,
                    };
                }
                return Outcome::Ignored;
            }
        }

        if event.code == ABS_HAT0X || event.code == ABS_HAT0Y {
            if event.value == 0 {
                return Outcome::Ignored;
            }
            let bit = if event.code == ABS_HAT0X {
                if event.value > 0 {
                    HAT_RIGHT
                } else {
                    HAT_LEFT
                }
            } else if event.value > 0 {
                HAT_DOWN
            } else {
                HAT_UP
            };
            if !self.armed(event.code) {
                return Outcome::Ignored;
            }
            let claim = Claim::Hat {
                index: 0,
                value: bit,
            };
            if let Some(holder) = self.taken_by_other(claim) {
                return self.refuse(claim, holder);
            }
            self.axis_armed.insert(event.code, false);
            return self.record(Binding::hat(0, bit), claim, now);
        }

        let Some(span) = self.axes.get(&event.code).copied() else {
            return Outcome::Ignored;
        };
        let position = deflection(span, event.value);
        if position.abs() < AXIS_THRESHOLD {
            return Outcome::Ignored;
        }
        if !self.armed(event.code) {
            return Outcome::Ignored;
        }

        let sign = if position > 0.0 { 1 } else { -1 };
        let claim = Claim::Axis {
            code: event.code,
            sign,
        };
        if let Some(holder) = self.taken_by_other(claim) {
            return self.refuse(claim, holder);
        }
        let codes: Vec<u16> = self.axes.keys().copied().collect();
        let Some(index) = axis_index(&codes, event.code) else {
            return Outcome::Ignored;
        };
        self.axis_armed.insert(event.code, false);
        self.record(Binding::axis(index, sign), claim, now)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Option_ {
    pub id: String,
    pub label: String,
    pub layout: String,
    pub mapped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceKind {
    Layout,
    Scope,
}

impl ChoiceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ChoiceKind::Layout => "layout",
            ChoiceKind::Scope => "scope",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Chooser {
    pub player: u32,
    pub options: Vec<Option_>,
    pub kind: ChoiceKind,
    pub title: String,
    pub axes: BTreeMap<u16, AxisSpan>,

    index: usize,
    confirmed: bool,
    opening_held: BTreeSet<u16>,
    down: BTreeSet<u16>,
    down_at: BTreeMap<u16, f64>,
    pushed: BTreeMap<u16, i32>,
}

impl Chooser {
    pub fn new(
        player: u32,
        options: Vec<Option_>,
        kind: ChoiceKind,
        title: String,
        axes: BTreeMap<u16, AxisSpan>,
        held: BTreeSet<u16>,
    ) -> Self {
        Chooser {
            player,
            options,
            kind,
            title,
            axes,
            index: 0,
            confirmed: false,
            down: held.clone(),
            opening_held: held,
            down_at: BTreeMap::new(),
            pushed: BTreeMap::new(),
        }
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn confirmed(&self) -> bool {
        self.confirmed
    }

    pub fn settling(&self) -> bool {
        self.opening_held
            .iter()
            .any(|code| self.down.contains(code))
    }

    pub fn chosen(&self) -> &str {
        self.options
            .get(self.index)
            .map(|option| option.id.as_str())
            .unwrap_or("")
    }

    pub fn chosen_layout(&self) -> &str {
        self.options
            .get(self.index)
            .map(|option| option.layout.as_str())
            .unwrap_or("")
    }

    pub fn move_by(&mut self, delta: i32) -> bool {
        if self.options.is_empty() {
            return false;
        }
        let len = self.options.len() as i32;
        self.index = (self.index as i32 + delta).rem_euclid(len) as usize;
        true
    }

    pub fn feed(&mut self, event: Event, now: f64) -> bool {
        if self.confirmed {
            return false;
        }
        match event.kind {
            EV_KEY => self.feed_key(event, now),
            EV_ABS => self.feed_abs(event),
            _ => false,
        }
    }

    fn feed_key(&mut self, event: Event, now: f64) -> bool {
        if event.value == 1 {
            self.down.insert(event.code);
            self.down_at.insert(event.code, now);
            return false;
        }
        if event.value != 0 {
            return false;
        }

        self.down.remove(&event.code);
        let started = self.down_at.remove(&event.code);
        if self.opening_held.remove(&event.code) {
            return false;
        }
        let Some(started) = started else { return false };
        if self.settling() {
            return false;
        }
        if now - started >= SKIP_HOLD_SECONDS {
            self.confirmed = true;
            return true;
        }
        false
    }

    fn feed_abs(&mut self, event: Event) -> bool {
        if self.settling() {
            return false;
        }

        let direction = if event.code == ABS_HAT0X {
            match event.value.cmp(&0) {
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Less => -1,
            }
        } else {
            let Some(span) = self.axes.get(&event.code).copied() else {
                return false;
            };
            if event.code != ABS_X {
                return false;
            }
            let position = deflection(span, event.value);
            if position.abs() < AXIS_THRESHOLD {
                0
            } else if position > 0.0 {
                1
            } else {
                -1
            }
        };

        let previous = self.pushed.get(&event.code).copied().unwrap_or(0);
        self.pushed.insert(event.code, direction);
        if direction == 0 || direction == previous {
            return false;
        }
        self.move_by(direction)
    }
}

pub fn layout_options(mapped: &BTreeSet<String>) -> Vec<Option_> {
    crate::layout::all()
        .iter()
        .map(|layout| Option_ {
            id: layout.id.clone(),
            label: layout.label.clone(),
            layout: layout.id.clone(),
            mapped: mapped.contains(&layout.id),
        })
        .collect()
}

fn console_label(layout_id: &str) -> String {
    let layout = crate::layout::get(layout_id);
    let name = if layout.console_label.is_empty() {
        &layout.label
    } else {
        &layout.console_label
    };
    format!("{name} games")
}

pub fn game_scope_options(
    console: &str,
    key: &str,
    title: &str,
    scopes: &BTreeSet<String>,
) -> Vec<Option_> {
    if console.is_empty() {
        return Vec::new();
    }
    let mut options = Vec::with_capacity(2);
    let scope = crate::scope::console(console);
    options.push(Option_ {
        label: console_label(console),
        mapped: scopes.contains(&scope),
        id: scope,
        layout: console.to_owned(),
    });
    if !key.is_empty() {
        let scope = crate::scope::game(key);
        options.push(Option_ {
            label: if title.is_empty() {
                key.to_owned()
            } else {
                title.to_owned()
            },
            mapped: scopes.contains(&scope),
            id: scope,
            layout: console.to_owned(),
        });
    }
    options
}

pub type RecentGame = (String, String, String);

pub fn scope_options(
    scopes: &BTreeSet<String>,
    default_layout: &str,
    recent: &[RecentGame],
) -> Vec<Option_> {
    let mut options = vec![Option_ {
        id: crate::scope::UNIVERSAL.to_owned(),
        label: "Any game".to_owned(),
        layout: default_layout.to_owned(),
        mapped: scopes.contains(crate::scope::UNIVERSAL),
    }];
    for layout_id in crate::layout::consoles() {
        let scope = crate::scope::console(layout_id);
        options.push(Option_ {
            label: console_label(layout_id),
            mapped: scopes.contains(&scope),
            id: scope,
            layout: layout_id.to_owned(),
        });
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (game_console, key, title) in recent {
        if key.is_empty() {
            continue;
        }
        if game_console.is_empty() {
            continue;
        }
        if !seen.insert(key.as_str()) {
            continue;
        }
        let scope = crate::scope::game(key);
        options.push(Option_ {
            label: if title.is_empty() {
                key.clone()
            } else {
                title.clone()
            },
            mapped: scopes.contains(&scope),
            id: scope,
            layout: game_console.clone(),
        });
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;

    fn span(minimum: i32, maximum: i32, rest: i32) -> AxisSpan {
        AxisSpan::new(minimum, maximum, rest)
    }

    fn run() -> MappingRun {
        MappingRun::new(
            1,
            layout::get("snes"),
            (0x130..0x13a).collect(),
            String::new(),
            BTreeMap::new(),
            BTreeSet::new(),
        )
    }

    #[test]
    fn a_tap_records_the_current_control_and_moves_on() {
        let mut run = run();
        let first = run.current().expect("a first prompt");
        assert_eq!(run.feed(Event::key(0x130, 1), 0.0), Outcome::Ignored);
        let outcome = run.feed(Event::key(0x130, 0), 0.05);
        assert!(outcome.advanced());
        assert!(matches!(outcome, Outcome::Recorded { control, .. } if control == first));
        assert_eq!(run.index(), 1);
    }

    #[test]
    fn a_press_still_held_from_before_the_run_cannot_answer_the_first_prompt() {
        let mut run = MappingRun::new(
            1,
            layout::get("snes"),
            (0x130..0x13a).collect(),
            String::new(),
            BTreeMap::new(),
            [0x130].into_iter().collect(),
        );
        assert!(run.settling());
        assert_eq!(run.feed(Event::key(0x130, 0), 0.1), Outcome::Ignored);
        assert_eq!(run.index(), 0, "the opening press answered a prompt");
        assert!(!run.settling());
    }

    #[test]
    fn a_long_hold_skips_the_control_instead_of_recording_it() {
        let mut run = run();
        let first = run.current().expect("a first prompt");
        run.feed(Event::key(0x130, 1), 0.0);
        let outcome = run.feed(Event::key(0x130, 0), SKIP_HOLD_SECONDS + 0.01);
        assert_eq!(outcome, Outcome::Skipped { control: first });
        assert_eq!(run.index(), 1);
        assert!(
            !run.bindings().contains_key(&first),
            "a skip must record nothing"
        );
    }

    #[test]
    fn nothing_is_accepted_during_the_gap_after_a_capture() {
        let mut run = run();
        run.feed(Event::key(0x130, 1), 0.0);
        run.feed(Event::key(0x130, 0), 0.05);
        let at = run.index();
        run.feed(Event::key(0x131, 1), 0.06);
        assert_eq!(run.feed(Event::key(0x131, 0), 0.10), Outcome::Ignored);
        assert_eq!(run.index(), at, "an input inside the gap answered a prompt");
        run.feed(Event::key(0x131, 1), 0.5);
        assert!(run.feed(Event::key(0x131, 0), 0.55).advanced());
    }

    #[test]
    fn one_button_cannot_answer_two_prompts() {
        let mut run = run();
        run.feed(Event::key(0x130, 1), 0.0);
        let first = run.current().expect("a first prompt");
        run.feed(Event::key(0x130, 0), 0.05);
        run.feed(Event::key(0x130, 1), 1.0);
        let outcome = run.feed(Event::key(0x130, 0), 1.05);
        assert_eq!(
            outcome,
            Outcome::Refused {
                claim: Claim::Button { code: 0x130 },
                held_by: first
            }
        );
        assert_eq!(
            run.conflict(),
            Some(first),
            "the refusal must name its holder"
        );
    }

    #[test]
    fn an_autorepeat_does_not_walk_the_wizard() {
        let mut run = run();
        run.feed(Event::key(0x130, 1), 0.0);
        for tick in 1..20 {
            assert_eq!(
                run.feed(Event::key(0x130, 2), f64::from(tick) * 0.03),
                Outcome::Ignored
            );
        }
        assert_eq!(run.index(), 0);
    }

    #[test]
    fn a_nudged_axis_cannot_answer_a_face_button_but_a_push_to_the_stop_can() {
        let axes: BTreeMap<u16, AxisSpan> = [(0x02, span(0, 255, 128))].into_iter().collect();
        let mut run = MappingRun::new(
            1,
            layout::get("snes"),
            Vec::new(),
            String::new(),
            axes,
            BTreeSet::new(),
        );
        assert_eq!(run.layout.controls[0].kind, "button");
        let nudge = 128 + (0.70 * 127.5) as i32;
        assert!(matches!(
            run.feed(Event::abs(0x02, nudge), 0.0),
            Outcome::TooGentle { .. }
        ));
        assert_eq!(run.index(), 0);
        run.feed(Event::abs(0x02, 128), 0.1);
        assert!(run.feed(Event::abs(0x02, 255), 0.2).advanced());
    }

    #[test]
    fn a_resting_axis_does_not_report_anything_at_all() {
        let axes: BTreeMap<u16, AxisSpan> = [(0x02, span(0, 255, 128))].into_iter().collect();
        let mut run = MappingRun::new(
            1,
            layout::get("snes"),
            Vec::new(),
            String::new(),
            axes,
            BTreeSet::new(),
        );
        for value in 126..=130 {
            assert_eq!(run.feed(Event::abs(0x02, value), 0.0), Outcome::Ignored);
        }
    }

    #[test]
    fn a_hat_never_answers_a_face_button_prompt() {
        let mut run = run();
        assert_eq!(run.layout.controls[0].kind, "button");
        assert_eq!(run.feed(Event::abs(ABS_HAT0X, 1), 0.0), Outcome::Ignored);
        assert_eq!(run.index(), 0);
    }

    #[test]
    fn an_axis_springing_back_through_centre_does_not_answer_the_next_prompt() {
        // Axis overshoots centre when released, must not answer next control.
        let axes: BTreeMap<u16, AxisSpan> = [(ABS_X, span(0, 255, 128))].into_iter().collect();
        let dpad = layout::get("snes");
        let start = dpad
            .controls
            .iter()
            .position(|c| c.kind == "dpad")
            .expect("a d-pad");
        let mut run = MappingRun::new(1, dpad, Vec::new(), String::new(), axes, BTreeSet::new());
        run.index = start;

        assert!(run.feed(Event::abs(ABS_X, 0), 0.0).advanced(), "full left");
        assert_eq!(run.feed(Event::abs(ABS_X, 255), 0.1), Outcome::Ignored);
        assert_eq!(run.feed(Event::abs(ABS_X, 255), 1.0), Outcome::Ignored);
        assert_eq!(run.index(), start + 1);
    }

    #[test]
    fn an_axis_released_inside_the_gap_is_still_re_armed() {
        // Release usually lands inside gap; must still re-arm.
        let axes: BTreeMap<u16, AxisSpan> = [(ABS_X, span(0, 255, 128))].into_iter().collect();
        let dpad = layout::get("snes");
        let start = dpad
            .controls
            .iter()
            .position(|c| c.kind == "dpad")
            .expect("a d-pad");
        let mut run = MappingRun::new(1, dpad, Vec::new(), String::new(), axes, BTreeSet::new());
        run.index = start;

        assert!(run.feed(Event::abs(ABS_X, 0), 0.0).advanced());
        run.feed(Event::abs(ABS_X, 128), 0.05); // inside the gap
        assert!(
            run.feed(Event::abs(ABS_X, 255), 0.5).advanced(),
            "the axis never re-armed"
        );
    }

    #[test]
    fn a_chooser_moves_on_the_push_not_on_the_hold() {
        let mut chooser = Chooser::new(
            1,
            layout_options(&BTreeSet::new()),
            ChoiceKind::Layout,
            "Which controller is this?".to_owned(),
            BTreeMap::new(),
            BTreeSet::new(),
        );
        assert_eq!(chooser.index(), 0);
        assert!(
            chooser.feed(Event::abs(ABS_HAT0X, 1), 0.0),
            "the first push moves"
        );
        assert_eq!(chooser.index(), 1);
        assert!(
            !chooser.feed(Event::abs(ABS_HAT0X, 1), 0.1),
            "a held hat must not spin"
        );
        assert!(
            !chooser.feed(Event::abs(ABS_HAT0X, 0), 0.2),
            "a release must not move back"
        );
        assert_eq!(chooser.index(), 1);
    }

    #[test]
    fn a_chooser_wraps_at_both_ends() {
        let options = layout_options(&BTreeSet::new());
        let count = options.len();
        let mut chooser = Chooser::new(
            1,
            options,
            ChoiceKind::Layout,
            String::new(),
            BTreeMap::new(),
            BTreeSet::new(),
        );
        chooser.move_by(-1);
        assert_eq!(chooser.index(), count - 1);
        chooser.move_by(1);
        assert_eq!(chooser.index(), 0);
    }

    #[test]
    fn a_chooser_accepts_a_hold_and_ignores_a_tap() {
        let mut chooser = Chooser::new(
            1,
            layout_options(&BTreeSet::new()),
            ChoiceKind::Layout,
            String::new(),
            BTreeMap::new(),
            BTreeSet::new(),
        );
        chooser.feed(Event::key(0x130, 1), 0.0);
        assert!(!chooser.feed(Event::key(0x130, 0), 0.1), "a tap confirmed");
        assert!(!chooser.confirmed());
        chooser.feed(Event::key(0x130, 1), 1.0);
        assert!(chooser.feed(Event::key(0x130, 0), 1.0 + SKIP_HOLD_SECONDS));
        assert!(chooser.confirmed());
        assert!(!chooser.feed(Event::abs(ABS_HAT0X, 1), 3.0));
    }

    #[test]
    fn an_empty_chooser_reports_no_choice_rather_than_indexing_past_the_end() {
        let mut chooser = Chooser::new(
            1,
            Vec::new(),
            ChoiceKind::Scope,
            String::new(),
            BTreeMap::new(),
            BTreeSet::new(),
        );
        assert_eq!(chooser.chosen(), "");
        assert_eq!(chooser.chosen_layout(), "");
        assert!(!chooser.move_by(1));
    }

    #[test]
    fn a_game_scope_strip_offers_the_console_first() {
        let options = game_scope_options("n64", "n64/goldeneye", "GoldenEye 007", &BTreeSet::new());
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].id, "console:n64");
        assert_eq!(options[1].id, "game:n64/goldeneye");
        assert_eq!(options[1].label, "GoldenEye 007");
        assert_eq!(options[0].layout, "n64");
        assert_eq!(options[1].layout, "n64");
    }

    #[test]
    fn a_game_with_no_console_is_offered_nothing_rather_than_the_generic_pad() {
        assert!(game_scope_options("", "x/y", "Title", &BTreeSet::new()).is_empty());
    }

    #[test]
    fn a_scope_strip_marks_what_is_already_captured() {
        let scopes: BTreeSet<String> = ["".to_owned(), "console:n64".to_owned()].into();
        let options = scope_options(&scopes, "gamecube", &[]);
        assert!(options[0].mapped, "the universal scope is captured");
        let n64 = options.iter().find(|o| o.id == "console:n64").expect("n64");
        assert!(n64.mapped);
        let snes = options
            .iter()
            .find(|o| o.id == "console:snes")
            .expect("snes");
        assert!(!snes.mapped);
    }

    #[test]
    fn a_scope_strip_skips_a_recent_game_with_no_console() {
        let recent = vec![
            (
                "n64".to_owned(),
                "n64/mario".to_owned(),
                "Mario 64".to_owned(),
            ),
            (String::new(), "x/unknown".to_owned(), "Unknown".to_owned()),
        ];
        let options = scope_options(&BTreeSet::new(), "generic", &recent);
        assert!(options.iter().any(|o| o.id == "game:n64/mario"));
        assert!(!options.iter().any(|o| o.id == "game:x/unknown"));
    }

    #[test]
    fn a_scope_strip_does_not_offer_the_same_game_twice() {
        let recent = vec![
            (
                "n64".to_owned(),
                "n64/mario".to_owned(),
                "Mario 64".to_owned(),
            ),
            (
                "n64".to_owned(),
                "n64/mario".to_owned(),
                "Mario 64".to_owned(),
            ),
        ];
        let options = scope_options(&BTreeSet::new(), "generic", &recent);
        assert_eq!(
            options.iter().filter(|o| o.id == "game:n64/mario").count(),
            1
        );
    }

    #[test]
    fn deflection_is_measured_from_rest_not_from_the_declared_middle() {
        // Trigger at rest must read 0, not fully deflected.
        let trigger = span(0, 255, 0);
        assert_eq!(
            deflection(trigger, 0),
            0.0,
            "a resting trigger must read zero"
        );
        assert!(
            deflection(trigger, 255) > 1.9,
            "a pressed trigger reads about 2.0"
        );
        let stick = span(0, 255, 128);
        assert!(deflection(stick, 128).abs() < 0.01);
        assert!(deflection(stick, 0) < -0.9);
        assert!(deflection(stick, 255) > 0.9);
    }

    #[test]
    fn deflection_refuses_a_degenerate_span_rather_than_dividing_by_zero() {
        assert_eq!(deflection(span(0, 0, 0), 50), 0.0);
        assert_eq!(deflection(span(10, 5, 7), 50), 0.0);
    }
}
