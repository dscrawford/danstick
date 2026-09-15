//! Watching a pad to find out where each control lives.
//!
//! Deliberately separate from the daemon's socket handling so the interesting
//! part -- deciding what an incoming event means -- can be exercised without a
//! session, a client or a controller.
//!
//! The hard part is not reading events, it is refusing most of them. A pad
//! streams axis noise continuously, an analogue trigger reports a resting value
//! that is not zero on some hardware, and the button someone pressed to reach
//! this screen is often still travelling when the first prompt appears. Every
//! guard below exists because one of those otherwise fills several controls in
//! with the same accidental input.
//!
//! The clock is a parameter rather than a field. The Python took an injectable
//! `now` callable for exactly this reason; passing the reading in at the call
//! site says the same thing without a boxed closure on a per-event path.

use std::collections::{BTreeMap, BTreeSet};

use crate::binding::{axis_index, retroarch_button_index, sdl_button_index, Binding};
use crate::control::Control;
use crate::layout::Layout;
use crate::sdl::AxisSpan;

/// evdev constants, spelled out rather than imported: this crate is pure logic
/// and a device library for the sake of five numbers is not a trade.
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const ABS_X: u16 = 0x00;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

/// How far an axis must travel from rest before it counts as deliberate.
/// Generous, because the alternative -- catching drift -- silently binds a
/// control to a stick that merely leans.
pub const AXIS_THRESHOLD: f64 = 0.55;

/// How close to rest an axis must come back before it may answer another
/// prompt.
///
/// Without this, one push answers two controls: release a d-pad wired to an
/// analogue axis and it springs back *through* centre, overshooting far enough
/// to read as a deliberate push the other way. Pressing left then filled in
/// both left and right, which is exactly what it looked like.
pub const AXIS_RELEASE: f64 = 0.30;

/// How far an axis must travel to answer a prompt for a *face button*.
///
/// Near the stop, and much higher than [`AXIS_THRESHOLD`], because the two
/// cases have opposite failure modes. For a stick or a shoulder an axis is the
/// expected answer and the only risk is drift; for a face button an axis is the
/// unusual answer, and binding one by accident is expensive in a way no other
/// misbinding is -- the axis is typically also the stick, so every later stick
/// movement presses that button for the rest of the session. That is how a
/// mapping ended up with cancel on `-a3`.
///
/// This used to be a flat refusal, which is right for a pad that has buttons to
/// spare and wrong for one that does not: an N64 pad mapped against the
/// GameCube layout has to answer X and Y from its C cluster, which that pad
/// reports as axes. A push past this is not something drift produces.
pub const AXIS_AS_BUTTON_THRESHOLD: f64 = 0.90;

/// SDL hat bits, which is also how a hat binding is written.
pub const HAT_UP: i32 = 1;
pub const HAT_RIGHT: i32 = 2;
pub const HAT_DOWN: i32 = 4;
pub const HAT_LEFT: i32 = 8;

/// Hold any button this long to skip a control the pad does not have.
///
/// It cannot be a *particular* button: the daemon holds EVIOCGRAB for the whole
/// session and republishing is stopped, so a "press Select to skip" prompt
/// could never have worked from a controller, and Select is not mapped until
/// halfway through anyway.
pub const SKIP_HOLD_SECONDS: f64 = 0.8;

/// Nothing is accepted for this long after a control is recorded.
///
/// Advancing instantly means a single continuous input can answer two prompts.
/// The per-axis arming rule catches one axis springing back; this catches the
/// general case, including inputs arriving on a different code entirely.
pub const CAPTURE_GAP_SECONDS: f64 = 0.35;

/// One evdev event, reduced to the three fields any of this reads.
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

/// How far an axis has moved from rest, as a fraction of half its range.
///
/// Signed, because the direction of travel is what a binding records.
///
/// Measured from *rest* rather than from the middle of the declared range, and
/// that distinction is the entire point. An analogue trigger rests at its
/// minimum, so measuring from the midpoint reports an untouched trigger as
/// fully deflected -- on a GameCube pad that made L and R unusable in the
/// wizard, since the axis could never come back near enough to the midpoint to
/// be re-armed and every later press was dropped.
///
/// Scaled by half the *declared* range rather than by the travel actually
/// available in the direction of movement, so a trigger reads 0 at rest and 2.0
/// fully pressed. Every threshold here is a floor, so reading high is harmless;
/// normalising by available travel would instead make an off-centre stick need
/// a bigger push on its long side than its short one.
pub fn deflection(span: AxisSpan, value: i32) -> f64 {
    if span.maximum <= span.minimum {
        return 0.0;
    }
    // Widened before subtracting. A driver reporting an absinfo near the ends
    // of i32 -- which is nonsense, and which padmap has already met once in the
    // shape of a profile declaring a range of 2^40 -- would otherwise overflow
    // here, on the path a wizard runs per event.
    let travel = i64::from(value) - i64::from(span.rest);
    let span = i64::from(span.maximum) - i64::from(span.minimum);
    travel as f64 / (span as f64 / 2.0)
}

/// A raw input already spoken for, in the terms a log reader has to match it
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Claim {
    Button {
        code: u16,
    },
    Hat {
        index: i32,
        value: i32,
    },
    /// `sign` is -1 or 1; an axis half is a separate control from its other half.
    Axis {
        code: u16,
        sign: i32,
    },
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

/// What one offered event did.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The overwhelming majority of what arrives.
    Ignored,
    /// The prompt was answered and the wizard moved on.
    Recorded { control: Control, binding: Binding },
    /// The user held a button past [`SKIP_HOLD_SECONDS`]; this pad has no such
    /// control.
    Skipped { control: Control },
    /// The input is already answering an earlier control.
    ///
    /// One input must not answer two prompts, and that rule is also invisible:
    /// the press simply does nothing, which from the outside is
    /// indistinguishable from a dead button or a wizard that has hung. Naming
    /// the control that holds it is the only actionable remedy -- restart and
    /// answer the earlier prompt differently.
    Refused { claim: Claim, held_by: Control },
    /// An axis moved, but not far enough to answer a *face button* prompt. Only
    /// reported once it has actually moved past [`AXIS_THRESHOLD`], since a
    /// resting axis streams events continuously.
    TooGentle {
        claim: Claim,
        travel: f64,
        needed: f64,
    },
}

impl Outcome {
    /// Whether this answered the current prompt -- what the Python's `feed`
    /// returned as a bare bool.
    pub fn advanced(&self) -> bool {
        matches!(self, Outcome::Recorded { .. } | Outcome::Skipped { .. })
    }
}

/// One pass through a layout, recording what the user presses.
#[derive(Debug, Clone)]
pub struct MappingRun {
    pub player: u32,
    pub layout: &'static Layout,
    /// The pad's evdev key codes, for turning a press into each consumer's
    /// button number.
    pub keys: Vec<u16>,
    /// Which scope the result will be filed under.
    ///
    /// Carried on the run rather than remembered beside it, because the answer
    /// is needed at the *end*, and a wizard that can be abandoned, restarted,
    /// or opened for a different player in between is exactly the shape of
    /// thing that loses a value parked elsewhere.
    pub scope: String,
    /// Absolute axis travel, for deciding when an axis has been pushed rather
    /// than nudged. Rest is measured, not assumed to be the centre.
    pub axes: BTreeMap<u16, AxisSpan>,

    index: usize,
    bindings: BTreeMap<Control, Binding>,
    /// Raw inputs already used, so one button cannot answer two prompts.
    claimed: BTreeMap<Claim, Control>,
    /// Buttons down when the run started, read from the device rather than
    /// guessed: the press that opened the wizard is usually still held, and its
    /// release must not answer the first prompt.
    opening_held: BTreeSet<u16>,
    down: BTreeSet<u16>,
    down_at: BTreeMap<u16, f64>,
    blocked_until: f64,
    /// The last refusal, for the front-end to show. Cleared once the prompt
    /// moves on, because a stale conflict pinned under a later control names a
    /// clash that is not happening.
    conflict: Option<Control>,
    /// Axes that have returned near centre since they last answered a prompt.
    /// Absent means armed: an axis never touched is ready.
    axis_armed: BTreeMap<u16, bool>,
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
        }
    }

    /// True while something held from before the run is still down.
    pub fn settling(&self) -> bool {
        self.opening_held
            .iter()
            .any(|code| self.down.contains(code))
    }

    pub fn finished(&self) -> bool {
        self.index >= self.layout.controls.len()
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

    /// The control being asked about, or `None` once every one is answered.
    pub fn current(&self) -> Option<Control> {
        self.layout
            .controls
            .get(self.index)
            .map(|control| control.canonical)
    }

    /// Move past a control this pad does not have.
    pub fn skip(&mut self, now: f64) -> Option<Control> {
        let control = self.current()?;
        self.index += 1;
        self.conflict = None;
        // Same gap as a capture: the button released after a skip-hold must not
        // answer the control it moved on to.
        self.blocked_until = now + CAPTURE_GAP_SECONDS;
        Some(control)
    }

    fn record(&mut self, binding: Binding, claim: Claim, now: f64) -> Outcome {
        let Some(control) = self.current() else {
            return Outcome::Ignored;
        };
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

    /// Offer one evdev event.
    pub fn feed(&mut self, event: Event, now: f64) -> Outcome {
        if self.finished() {
            return Outcome::Ignored;
        }

        if now < self.blocked_until {
            // Just recorded something. Accept nothing -- but go on tracking
            // what is *released*, or the gap leaves state behind that nothing
            // afterwards can correct.
            if event.kind == EV_KEY && event.value == 0 {
                self.down.remove(&event.code);
                self.down_at.remove(&event.code);
                self.opening_held.remove(&event.code);
            } else if event.kind == EV_ABS {
                // An axis let go inside the gap has genuinely been let go, and
                // a release takes about a tenth of the time the gap lasts, so
                // this is where nearly every one of them lands. Dropping it
                // leaves the axis disarmed with nothing left to re-arm it: a
                // trigger settles at rest and stops reporting entirely, and the
                // wizard then ignores it for good.
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
            // Autorepeat. Holding a button must not walk the whole wizard.
            return Outcome::Ignored;
        }

        // Release. Binding happens here rather than on the press, because how
        // long it was held is what separates "this is the button" from "skip
        // this control", and that is only known once it comes back up.
        self.down.remove(&event.code);
        let started = self.down_at.remove(&event.code);
        if self.opening_held.remove(&event.code) {
            // Whatever opened the wizard has now been let go.
            return Outcome::Ignored;
        }
        let Some(started) = started else {
            return Outcome::Ignored;
        };
        if self.settling() {
            return Outcome::Ignored;
        }

        if now - started >= SKIP_HOLD_SECONDS {
            return match self.skip(now) {
                Some(control) => Outcome::Skipped { control },
                None => Outcome::Ignored,
            };
        }

        let claim = Claim::Button { code: event.code };
        if let Some(holder) = self.claimed.get(&claim).copied() {
            return self.refuse(claim, holder);
        }

        let Some(index) = sdl_button_index(&self.keys, event.code) else {
            return Outcome::Ignored;
        };
        // Both numberings are stored. Recomputing RetroArch's at emission time
        // would need the pad's key list to still be around, and would silently
        // shift every binding on a pad carrying sub-0x120 codes.
        let binding =
            Binding::button(index).with_ra_index(retroarch_button_index(&self.keys, event.code));
        self.record(binding, claim, now)
    }

    /// Note an axis that has come back to rest, so it may answer again.
    ///
    /// Separate from [`Self::feed_abs`] because it has to run in places that
    /// accept nothing at all -- notably inside the capture gap, where the
    /// release of whatever was just recorded arrives.
    fn rearm(&mut self, event: Event) {
        if event.code == ABS_HAT0X || event.code == ABS_HAT0Y {
            // A hat only reads 0 at rest, so this is unambiguous.
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
            // The hat stays refused outright. A d-pad direction answering a
            // face button is a mistake in every case anyone has had, and a
            // device that reports a hat it does not have -- or sends ABS events
            // while declaring no axes at all -- would otherwise fill face
            // buttons in from noise.
            if event.code == ABS_HAT0X || event.code == ABS_HAT0Y {
                return Outcome::Ignored;
            }
            let Some(span) = self.axes.get(&event.code).copied() else {
                return Outcome::Ignored;
            };
            // An axis may answer, but only if it is meant. A nudge is refused,
            // a push held against the stop is taken.
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
                return Outcome::Ignored; // released; rearm has already noted it
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
            if let Some(holder) = self.claimed.get(&claim).copied() {
                return self.refuse(claim, holder);
            }
            self.axis_armed.insert(event.code, false);
            return self.record(Binding::hat(0, bit), claim, now);
        }

        let Some(span) = self.axes.get(&event.code).copied() else {
            return Outcome::Ignored;
        };
        // Signed travel away from where this axis sat when the wizard opened --
        // from rest, not from the middle of the declared range.
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
        if let Some(holder) = self.claimed.get(&claim).copied() {
            return self.refuse(claim, holder);
        }
        // By axis *index*, not evdev code: ABS_RZ is code 5 but may be axis 3.
        let codes: Vec<u16> = self.axes.keys().copied().collect();
        let Some(index) = axis_index(&codes, event.code) else {
            return Outcome::Ignored;
        };
        // Disarmed until it settles again, so the spring-back does not answer
        // the next prompt too.
        self.axis_armed.insert(event.code, false);
        self.record(Binding::axis(index, sign), claim, now)
    }
}

/// One entry on a chooser's strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Option_ {
    /// What the daemon acts on: a layout id for the layout picker, a scope
    /// string for the scope picker.
    pub id: String,
    /// What the user reads.
    pub label: String,
    /// Layout id to *draw*. May differ from `id` -- a scope option's id is
    /// `console:n64` while the picture is the N64 pad. This is the entire
    /// reason the two pickers share one overlay: the pad shown is the pad the
    /// wizard will then ask about, from one set of coordinates.
    pub layout: String,
    /// Whether something is already recorded here. Shown, because re-mapping a
    /// scope replaces it and without a mark there is no way to tell which ones
    /// that would destroy.
    pub mapped: bool,
}

/// Which question a [`Chooser`] is asking. Sent to the front-end rather than
/// inferred: a theme guessing from the option ids would be a third place that
/// has to know what a scope string looks like.
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

/// A strip of options worked from the pad, before mapping its buttons.
///
/// It has to be driven from the pad itself, and that is the whole difficulty.
/// The daemon holds EVIOCGRAB for the duration of a session and republishing is
/// stopped, so the front-end receives no controller input at all; a picker the
/// theme navigates could only ever be worked from a keyboard. And nothing is
/// mapped yet, so no gesture may name a button. Both constraints are answered
/// the same way the wizard's skip is: push left/right on the raw axis to move,
/// hold any button to accept.
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
    /// Per axis: which way it is currently pushed, -1, 0 or 1. Moving happens
    /// on the transition *into* a direction, so a stick held over does not spin
    /// the selection and a released one does not move it back.
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

    /// Offer one evdev event. True if the selection or state changed.
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
            // The press that opened the picker, finally released.
            return false;
        }
        let Some(started) = started else { return false };
        if self.settling() {
            return false;
        }
        // A tap does nothing on purpose. The button that claimed the slot is
        // often still travelling when this appears, and a picker that accepts
        // the first press anyone makes is a picker nobody gets to use.
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
            // Deliberately *not* the wizard's re-arming rule, which needs the
            // axis back inside AXIS_RELEASE of centre. An uncalibrated stick
            // can rest at 36% deflection -- measured on the N64 adapter here --
            // which never re-arms, and a picker that stops responding after one
            // move is worse than one that occasionally moves twice.
            return false;
        }
        self.move_by(direction)
    }
}

/// "Which controller is this?", as a strip of every layout padmap knows.
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

/// "Console or just this game?", asked with both answers already known.
///
/// The strip [`scope_options`] builds has to offer every console and a handful
/// of recently played games, because it is reached from the controller setup
/// screen, which knows nothing about what the user wants to play. Reached from
/// a game in the library instead, both facts are in hand, so the question
/// collapses to two entries.
///
/// Console first: it is the answer that is right more often, and the first
/// entry is the one a hurried user confirms.
///
/// Empty when the console is unknown. Both entries are captured against the
/// console's control set, so without one there is nothing coherent to offer --
/// not even the game. Reachable in practice: the exporter writes a game key for
/// every game but omits the console when the collection's core is not one
/// padmap recognises.
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

/// A game offered by [`scope_options`]: console layout id, key, title.
pub type RecentGame = (String, String, String);

/// "What is this mapping for?", as a strip of scopes.
///
/// `default_layout` is the pad's best guess, drawn beside the "any game" entry
/// only so the strip has a picture there; it is not a promise about which
/// layout the wizard will walk, because that entry leads to the layout picker.
///
/// `recent` is newest first. That is the only way a per-game scope can be
/// offered at all: the setup screen is reached from the front-end, never from
/// inside a game, so nothing else here knows which game the user means.
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
        // Same rule as game_scope_options: padmap records every launch,
        // including one whose core cannot be named -- deliberately, since a
        // launch with an unknown core is exactly the one whose controls are
        // most likely to have felt wrong. Offering it would draw the generic
        // pad beside the entry and then walk whatever the pad's icon guesses,
        // so the strip promises one controller and the wizard asks about
        // another. That is precisely how a mapping ended up with cancel on an
        // axis.
        if game_console.is_empty() {
            continue;
        }
        // Marked seen only once the entry is actually offered. The same ROM
        // appears in the recent list both with and without a console -- the
        // launcher records every launch, including one whose core it cannot
        // name -- and burning the key on the console-less sighting drops the
        // usable one behind it.
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
        // ...and the same press works once the gap has passed.
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
        // 0.70 of the way over: past AXIS_THRESHOLD, short of the stop.
        let nudge = 128 + (0.70 * 127.5) as i32;
        assert!(matches!(
            run.feed(Event::abs(0x02, nudge), 0.0),
            Outcome::TooGentle { .. }
        ));
        assert_eq!(run.index(), 0);
        // Back to rest to re-arm, then all the way over.
        run.feed(Event::abs(0x02, 128), 0.1);
        assert!(run.feed(Event::abs(0x02, 255), 0.2).advanced());
    }

    #[test]
    fn a_resting_axis_does_not_report_anything_at_all() {
        // A resting axis streams events continuously; reporting them would bury
        // the session in noise.
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
        // Release a d-pad wired to an analogue axis and it overshoots far
        // enough to read as a deliberate push the other way. Pressing left then
        // filled in both left and right.
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
        // The overshoot arrives inside the capture gap and after it; neither
        // may answer, because the axis has not been back to rest.
        assert_eq!(run.feed(Event::abs(ABS_X, 255), 0.1), Outcome::Ignored);
        assert_eq!(run.feed(Event::abs(ABS_X, 255), 1.0), Outcome::Ignored);
        assert_eq!(run.index(), start + 1);
    }

    #[test]
    fn an_axis_released_inside_the_gap_is_still_re_armed() {
        // A release takes about a tenth of the time the gap lasts, so this is
        // where nearly every one of them lands. Dropping it leaves the axis
        // disarmed with nothing left to re-arm it.
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
        // Nothing gets through afterwards.
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
        // Both draw the console's pad: a mapping for one N64 game is still a
        // mapping of the N64 control set.
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
        // An analogue trigger rests at its minimum. Measuring from the midpoint
        // reports an untouched trigger as fully deflected.
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
