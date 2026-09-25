//! A clone that looks like a wired Xbox 360 pad: the identity, the layout the
//! `xpad` driver gives it, and the translation of a source's inputs onto it.
//!
//! Every SDL since 2.0 maps `030000005e0400008e02000010010000` out of the box,
//! draws Xbox prompts for it, and never needs a mapping handed to it. Steam
//! Input publishes exactly this for every game it launches. A source's inputs
//! are translated onto the layout through the profile padmap already has for
//! it -- control to source binding -- the way the SDL mapping string is; a
//! control the source lacks is simply never pressed.

use std::collections::{BTreeMap, BTreeSet};

use crate::binding::{Binding, BindingKind, HAT_CODES};
use crate::capture::{ABS_HAT0X, ABS_HAT0Y, EV_ABS, EV_KEY};
use crate::control::Control;
use crate::sdl::AxisSpan;

pub const VENDOR: u16 = 0x045e;
pub const PRODUCT: u16 = 0x028e;
pub const VERSION: u16 = 0x0110;
pub const BUS_USB: u16 = 0x03;

/// What `xpad` advertises for a wired 360 pad, in code order.
pub const KEYS: [u16; 11] = [
    0x130, // BTN_A     (south)
    0x131, // BTN_B     (east)
    0x133, // BTN_X     (west) -- the kernel also calls this BTN_NORTH; xpad sends it for X
    0x134, // BTN_Y     (north) -- and this, BTN_WEST, for Y
    0x136, // BTN_TL
    0x137, // BTN_TR
    0x13A, // BTN_SELECT (back)
    0x13B, // BTN_START
    0x13C, // BTN_MODE  (guide)
    0x13D, // BTN_THUMBL
    0x13E, // BTN_THUMBR
];

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;

pub const STICK_MIN: i32 = -32768;
pub const STICK_MAX: i32 = 32767;
pub const TRIGGER_MAX: i32 = 255;

/// One declared axis: code, minimum, maximum, fuzz, flat.
pub const AXES: [(u16, i32, i32, i32, i32); 8] = [
    (ABS_X, STICK_MIN, STICK_MAX, 16, 128),
    (ABS_Y, STICK_MIN, STICK_MAX, 16, 128),
    (ABS_Z, 0, TRIGGER_MAX, 0, 0),
    (ABS_RX, STICK_MIN, STICK_MAX, 16, 128),
    (ABS_RY, STICK_MIN, STICK_MAX, 16, 128),
    (ABS_RZ, 0, TRIGGER_MAX, 0, 0),
    (ABS_HAT0X, -1, 1, 0, 0),
    (ABS_HAT0Y, -1, 1, 0, 0),
];

/// The axis spans as a consumer sees them, keyed by code.
pub fn spans() -> BTreeMap<u16, AxisSpan> {
    AXES.iter()
        .map(|&(code, minimum, maximum, _, _)| {
            let rest = if minimum < 0 { 0 } else { minimum };
            (code, AxisSpan::new(minimum, maximum, rest))
        })
        .collect()
}

/// The axis codes, for callers that want them as a list.
pub fn axis_codes() -> Vec<u16> {
    AXES.iter().map(|&(code, ..)| code).collect()
}

/// Where each control lives on the layout, in padmap's own binding terms
/// (SDL ordinals over `KEYS`, axis ordinals skipping hats) -- so every
/// consumer's config for the clone derives from this, not from the source.
pub fn bindings() -> BTreeMap<Control, Binding> {
    let mut out = BTreeMap::new();
    let button =
        |control: Control, index: i32| (control, Binding::button(index).with_ra_index(Some(index)));
    for (control, binding) in [
        button(Control::A, 0),
        button(Control::B, 1),
        button(Control::X, 2),
        button(Control::Y, 3),
        button(Control::LeftShoulder, 4),
        button(Control::RightShoulder, 5),
        button(Control::Back, 6),
        button(Control::Start, 7),
        (Control::LeftTrigger, Binding::axis(2, 1)),
        (Control::RightTrigger, Binding::axis(5, 1)),
        (Control::DpadUp, Binding::hat(0, 1)),
        (Control::DpadRight, Binding::hat(0, 2)),
        (Control::DpadDown, Binding::hat(0, 4)),
        (Control::DpadLeft, Binding::hat(0, 8)),
        (Control::RightStickLeft, Binding::axis(3, -1)),
        (Control::RightStickRight, Binding::axis(3, 1)),
        (Control::RightStickUp, Binding::axis(4, -1)),
        (Control::RightStickDown, Binding::axis(4, 1)),
    ] {
        out.insert(control, binding);
    }
    out
}

/// An event for the clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Out {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

/// Where a source axis sits, as a fraction: -1..1 about its rest for a stick,
/// 0..1 from its minimum for a trigger.
fn position(span: AxisSpan, value: i32) -> f64 {
    if span.maximum <= span.minimum {
        return 0.0;
    }
    if span.rests_centred() {
        // Each side scaled on its own, so an even range still reaches both ends.
        let travel = f64::from(value - span.rest);
        let reach = if travel >= 0.0 {
            f64::from(span.maximum - span.rest)
        } else {
            f64::from(span.rest - span.minimum)
        };
        if reach <= 0.0 {
            return 0.0;
        }
        (travel / reach).clamp(-1.0, 1.0)
    } else {
        (f64::from(value - span.minimum) / f64::from(span.maximum - span.minimum)).clamp(0.0, 1.0)
    }
}

/// Source key codes in SDL's button order, the order a button binding's index counts in.
fn sdl_ordered(keys: &[u16]) -> Vec<u16> {
    let mut sorted: Vec<u16> = keys.to_vec();
    sorted.sort_unstable();
    sorted
        .iter()
        .copied()
        .filter(|c| *c >= crate::binding::BTN_JOYSTICK)
        .chain(
            sorted
                .iter()
                .copied()
                .filter(|c| *c < crate::binding::BTN_JOYSTICK),
        )
        .collect()
}

/// Translates one source's events onto the 360 layout.
#[derive(Debug, Clone)]
pub struct Translator {
    /// A source key and the controls it presses.
    keys: BTreeMap<u16, Vec<Control>>,
    /// A source axis and the controls each direction presses (+1 or -1).
    axes: BTreeMap<u16, Vec<(Control, i8)>>,
    spans: BTreeMap<u16, AxisSpan>,
    /// Source keys carried across by code: guide and the stick clicks, which no capture covers.
    carried_keys: BTreeSet<u16>,
    /// Source axes carried across by code with rescaling: the left stick, which no capture covers.
    carried_axes: BTreeSet<u16>,
    /// Per control, how far each of its sources (code, sign) is pressed; the control is the most.
    pressed: BTreeMap<Control, BTreeMap<(u16, i8), f64>>,
    last: BTreeMap<(u16, u16), i32>,
}

impl Translator {
    /// `keys` and `spans` describe the source; `bindings` is its stored capture,
    /// or empty for a pad that follows the kernel's gamepad convention, which
    /// is then carried across by code.
    pub fn new(
        keys: &[u16],
        spans: &BTreeMap<u16, AxisSpan>,
        bindings: &BTreeMap<Control, Binding>,
        extras: &BTreeMap<Control, Vec<Binding>>,
    ) -> Self {
        let ordered = sdl_ordered(keys);
        let mut axis_codes: Vec<u16> = spans
            .keys()
            .copied()
            .filter(|c| !HAT_CODES.contains(c))
            .collect();
        axis_codes.sort_unstable();
        let mut out = Translator {
            keys: BTreeMap::new(),
            axes: BTreeMap::new(),
            spans: spans.clone(),
            carried_keys: BTreeSet::new(),
            carried_axes: BTreeSet::new(),
            pressed: BTreeMap::new(),
            last: BTreeMap::new(),
        };
        if bindings.is_empty() {
            // The kernel's convention is the 360's, code for code, except that
            // xpad's X and Y are the kernel's north and west swapped.
            for &code in keys {
                let control = match code {
                    0x130 => Some(Control::A),
                    0x131 => Some(Control::B),
                    0x133 => Some(Control::Y),
                    0x134 => Some(Control::X),
                    0x136 => Some(Control::LeftShoulder),
                    0x137 => Some(Control::RightShoulder),
                    0x138 => Some(Control::LeftTrigger),
                    0x139 => Some(Control::RightTrigger),
                    0x13A => Some(Control::Back),
                    0x13B => Some(Control::Start),
                    0x13C..=0x13E => {
                        out.carried_keys.insert(code);
                        None
                    }
                    _ => None,
                };
                if let Some(control) = control {
                    out.keys.entry(code).or_default().push(control);
                }
            }
            for (&code, span) in spans {
                match code {
                    ABS_X | ABS_Y => {
                        out.carried_axes.insert(code);
                    }
                    ABS_Z if !span.rests_centred() => out
                        .axes
                        .entry(code)
                        .or_default()
                        .push((Control::LeftTrigger, 1)),
                    ABS_RZ if !span.rests_centred() => out
                        .axes
                        .entry(code)
                        .or_default()
                        .push((Control::RightTrigger, 1)),
                    ABS_RX => out
                        .axes
                        .entry(code)
                        .or_default()
                        .extend([(Control::RightStickLeft, -1), (Control::RightStickRight, 1)]),
                    ABS_RY => out
                        .axes
                        .entry(code)
                        .or_default()
                        .extend([(Control::RightStickUp, -1), (Control::RightStickDown, 1)]),
                    ABS_HAT0X => out
                        .axes
                        .entry(code)
                        .or_default()
                        .extend([(Control::DpadLeft, -1), (Control::DpadRight, 1)]),
                    ABS_HAT0Y => out
                        .axes
                        .entry(code)
                        .or_default()
                        .extend([(Control::DpadUp, -1), (Control::DpadDown, 1)]),
                    _ => {}
                }
            }
            return out;
        }
        let every = bindings.iter().map(|(c, b)| (*c, *b)).chain(
            extras
                .iter()
                .flat_map(|(c, twins)| twins.iter().map(move |b| (*c, *b))),
        );
        for (control, binding) in every {
            match binding.kind {
                BindingKind::Button => {
                    if let Some(&code) = usize::try_from(binding.index)
                        .ok()
                        .and_then(|i| ordered.get(i))
                    {
                        out.keys.entry(code).or_default().push(control);
                    }
                }
                BindingKind::Axis => {
                    if let Some(&code) = usize::try_from(binding.index)
                        .ok()
                        .and_then(|i| axis_codes.get(i))
                    {
                        let sign: i8 = if binding.value < 0 { -1 } else { 1 };
                        out.axes.entry(code).or_default().push((control, sign));
                    }
                }
                BindingKind::Hat => {
                    let (code, sign): (u16, i8) = match binding.value {
                        1 => (ABS_HAT0Y, -1),
                        2 => (ABS_HAT0X, 1),
                        4 => (ABS_HAT0Y, 1),
                        8 => (ABS_HAT0X, -1),
                        _ => continue,
                    };
                    out.axes.entry(code).or_default().push((control, sign));
                }
            }
        }
        // What no capture covers rides across by code where the source has it.
        for &code in keys {
            if (0x13C..=0x13E).contains(&code) && !out.keys.contains_key(&code) {
                out.carried_keys.insert(code);
            }
        }
        // An axis whose control the capture named is driven by the capture alone.
        let captured = |halves: [Control; 2]| {
            out.axes
                .values()
                .flatten()
                .any(|(control, _)| halves.contains(control))
                || out
                    .keys
                    .values()
                    .flatten()
                    .any(|control| halves.contains(control))
        };
        let left_x = captured([Control::LeftStickLeft, Control::LeftStickRight]);
        let left_y = captured([Control::LeftStickUp, Control::LeftStickDown]);
        for &code in spans.keys() {
            let spoken_for = match code {
                ABS_X => left_x,
                ABS_Y => left_y,
                _ => continue,
            };
            if !spoken_for && !out.axes.contains_key(&code) {
                out.carried_axes.insert(code);
            }
        }
        out
    }

    /// The clone's events for one source event, only those whose value changed.
    pub fn translate(&mut self, kind: u16, code: u16, value: i32) -> Vec<Out> {
        let mut touched: Vec<Control> = Vec::new();
        let mut out = Vec::new();
        match kind {
            EV_KEY => {
                if self.carried_keys.contains(&code) {
                    self.push(&mut out, EV_KEY, code, i32::from(value != 0));
                }
                if let Some(controls) = self.keys.get(&code).cloned() {
                    let pressed = if value != 0 { 1.0 } else { 0.0 };
                    for control in controls {
                        self.pressed
                            .entry(control)
                            .or_default()
                            .insert((code, 0), pressed);
                        touched.push(control);
                    }
                }
            }
            EV_ABS => {
                if self.carried_axes.contains(&code) {
                    if let Some(span) = self.spans.get(&code) {
                        let scaled = (position(*span, value) * f64::from(STICK_MAX)).round() as i32;
                        self.push(&mut out, EV_ABS, code, scaled.clamp(STICK_MIN, STICK_MAX));
                    }
                }
                if let Some(controls) = self.axes.get(&code).cloned() {
                    let fraction = if HAT_CODES.contains(&code) {
                        f64::from(value.signum())
                    } else {
                        self.spans
                            .get(&code)
                            .map(|span| position(*span, value))
                            .unwrap_or(0.0)
                    };
                    for (control, sign) in controls {
                        let along = (fraction * f64::from(sign)).clamp(0.0, 1.0);
                        self.pressed
                            .entry(control)
                            .or_default()
                            .insert((code, sign), along);
                        touched.push(control);
                    }
                }
            }
            _ => {}
        }
        for control in touched {
            let (kind, code, value) = self.output_for(control);
            self.push(&mut out, kind, code, value);
        }
        out
    }

    fn at(&self, control: Control) -> f64 {
        self.pressed
            .get(&control)
            .map(|sources| sources.values().copied().fold(0.0, f64::max))
            .unwrap_or(0.0)
    }

    /// What the clone's element for `control` reads now, from every control that feeds it.
    fn output_for(&self, control: Control) -> (u16, u16, i32) {
        let digital = |c: Control| i32::from(self.at(c) > 0.5);
        let stick = |neg: Control, pos: Control| {
            ((self.at(pos) - self.at(neg)) * f64::from(STICK_MAX)).round() as i32
        };
        match control {
            Control::A => (EV_KEY, 0x130, digital(control)),
            Control::B => (EV_KEY, 0x131, digital(control)),
            Control::X => (EV_KEY, 0x133, digital(control)),
            Control::Y => (EV_KEY, 0x134, digital(control)),
            Control::LeftShoulder => (EV_KEY, 0x136, digital(control)),
            Control::RightShoulder => (EV_KEY, 0x137, digital(control)),
            Control::Back => (EV_KEY, 0x13A, digital(control)),
            Control::Start => (EV_KEY, 0x13B, digital(control)),
            Control::LeftTrigger => (
                EV_ABS,
                ABS_Z,
                (self.at(control) * f64::from(TRIGGER_MAX)).round() as i32,
            ),
            Control::RightTrigger => (
                EV_ABS,
                ABS_RZ,
                (self.at(control) * f64::from(TRIGGER_MAX)).round() as i32,
            ),
            Control::DpadUp | Control::DpadDown => (
                EV_ABS,
                ABS_HAT0Y,
                digital(Control::DpadDown) - digital(Control::DpadUp),
            ),
            Control::DpadLeft | Control::DpadRight => (
                EV_ABS,
                ABS_HAT0X,
                digital(Control::DpadRight) - digital(Control::DpadLeft),
            ),
            Control::RightStickLeft | Control::RightStickRight => (
                EV_ABS,
                ABS_RX,
                stick(Control::RightStickLeft, Control::RightStickRight),
            ),
            Control::RightStickUp | Control::RightStickDown => (
                EV_ABS,
                ABS_RY,
                stick(Control::RightStickUp, Control::RightStickDown),
            ),
            Control::LeftStickLeft | Control::LeftStickRight => (
                EV_ABS,
                ABS_X,
                stick(Control::LeftStickLeft, Control::LeftStickRight),
            ),
            Control::LeftStickUp | Control::LeftStickDown => (
                EV_ABS,
                ABS_Y,
                stick(Control::LeftStickUp, Control::LeftStickDown),
            ),
        }
    }

    fn push(&mut self, out: &mut Vec<Out>, kind: u16, code: u16, value: i32) {
        if self.last.insert((kind, code), value) == Some(value) {
            return;
        }
        out.push(Out { kind, code, value });
    }

    /// Press, for every control in `faces`, the control it names instead: a
    /// pad kept by label rather than position.
    pub fn relabel(&mut self, faces: &BTreeMap<Control, Control>) {
        let moved = |control: &mut Control| {
            if let Some(to) = faces.get(control) {
                *control = *to;
            }
        };
        for controls in self.keys.values_mut() {
            controls.iter_mut().for_each(moved);
        }
        for controls in self.axes.values_mut() {
            controls.iter_mut().for_each(|(control, _)| moved(control));
        }
    }

    /// Everything released: what the clone should read when the source lets go of it all.
    pub fn release_all(&mut self) -> Vec<Out> {
        let held: Vec<Control> = self
            .pressed
            .iter()
            .filter(|(_, sources)| sources.values().any(|v| *v > 0.0))
            .map(|(c, _)| *c)
            .collect();
        let mut out = Vec::new();
        for control in held {
            self.pressed.remove(&control);
            let (kind, code, value) = self.output_for(control);
            self.push(&mut out, kind, code, value);
        }
        let carried: Vec<u16> = self.carried_keys.iter().copied().collect();
        for code in carried {
            if self.last.get(&(EV_KEY, code)).copied().unwrap_or(0) != 0 {
                self.push(&mut out, EV_KEY, code, 0);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stick() -> AxisSpan {
        AxisSpan::new(0, 255, 128)
    }

    fn trigger() -> AxisSpan {
        AxisSpan::new(0, 255, 0)
    }

    /// An N64-ish pad: no standard codes, a capture that says where things are.
    fn captured() -> Translator {
        // Keys in SDL order: 0x120 (0), 0x121 (1), 0x122 (2), 0x123 (3)
        let keys = [0x120, 0x121, 0x122, 0x123];
        let mut spans = BTreeMap::new();
        spans.insert(ABS_X, stick());
        spans.insert(ABS_Y, stick());
        spans.insert(ABS_Z, trigger()); // ordinal 2
        let mut bindings = BTreeMap::new();
        bindings.insert(Control::A, Binding::button(0));
        bindings.insert(Control::B, Binding::button(1));
        bindings.insert(Control::RightStickUp, Binding::button(2)); // a C-button
        bindings.insert(Control::RightStickDown, Binding::button(3));
        bindings.insert(Control::LeftTrigger, Binding::axis(2, 1)); // Z trigger on an axis
        Translator::new(&keys, &spans, &bindings, &BTreeMap::new())
    }

    #[test]
    fn a_pad_kept_by_label_presses_the_button_its_label_names() {
        // A pad on the kernel's convention: the bottom button is BTN_SOUTH.
        let keys = [0x130, 0x131, 0x133, 0x134];
        let mut t = Translator::new(&keys, &BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new());
        let bottom = t.translate(EV_KEY, 0x130, 1);
        t.translate(EV_KEY, 0x130, 0);
        t.relabel(&crate::layout::label_faces("switch"));
        // Nintendo's A is on the right; kept by label it is the 360's A.
        let right = t.translate(EV_KEY, 0x131, 1);
        assert_eq!(
            right, bottom,
            "the button labelled A did not press the 360's A"
        );
        assert_eq!(
            t.translate(EV_KEY, 0x130, 1).first().map(|out| out.code),
            Some(0x131),
            "the button labelled B did not press the 360's B"
        );
    }

    #[test]
    fn a_left_stick_bound_to_buttons_is_not_fought_by_the_carried_axis() {
        // A pad whose stick reports as four keys: the source still declares
        // ABS_X/ABS_Y, and both must not drive the clone's stick at once.
        let keys = [0x120, 0x121, 0x122, 0x123];
        let mut spans = BTreeMap::new();
        spans.insert(ABS_X, stick());
        spans.insert(ABS_Y, stick());
        let mut bindings = BTreeMap::new();
        bindings.insert(Control::LeftStickUp, Binding::button(0));
        bindings.insert(Control::LeftStickDown, Binding::button(1));
        bindings.insert(Control::LeftStickLeft, Binding::button(2));
        bindings.insert(Control::LeftStickRight, Binding::button(3));
        let mut t = Translator::new(&keys, &spans, &bindings, &BTreeMap::new());

        // Holding up drives the clone's stick to the top.
        assert_eq!(
            t.translate(EV_KEY, 0x120, 1),
            vec![Out {
                kind: EV_ABS,
                code: ABS_Y,
                value: -STICK_MAX
            }]
        );
        // The source's own ABS_Y is not what the capture named, so it must not
        // reach the clone and undo the press still being held.
        assert_eq!(
            t.translate(EV_ABS, ABS_Y, 128),
            Vec::new(),
            "the raw axis centred the stick a held button is pushing"
        );
    }

    #[test]
    fn a_stick_captured_onto_other_axes_is_the_only_thing_driving_the_clones() {
        // An adapter that puts the stick on ABS_RX/ABS_RY: the clone's left
        // stick must come from there, not from the source's own ABS_X/ABS_Y.
        let mut spans = BTreeMap::new();
        spans.insert(ABS_X, stick());
        spans.insert(ABS_Y, stick());
        spans.insert(ABS_RX, stick());
        spans.insert(ABS_RY, stick());
        let mut bindings = BTreeMap::new();
        bindings.insert(Control::LeftStickLeft, Binding::axis(2, -1));
        bindings.insert(Control::LeftStickRight, Binding::axis(2, 1));
        let mut t = Translator::new(&[], &spans, &bindings, &BTreeMap::new());

        assert_eq!(
            t.translate(EV_ABS, ABS_RX, 255),
            vec![Out {
                kind: EV_ABS,
                code: ABS_X,
                value: STICK_MAX
            }]
        );
        assert_eq!(
            t.translate(EV_ABS, ABS_X, 0),
            Vec::new(),
            "a second axis was driving the clone's left stick"
        );
    }

    #[test]
    fn a_stick_nobody_captured_still_rides_across_untouched() {
        let keys = [0x120];
        let mut spans = BTreeMap::new();
        spans.insert(ABS_X, stick());
        spans.insert(ABS_Y, stick());
        let mut bindings = BTreeMap::new();
        bindings.insert(Control::A, Binding::button(0));
        let mut t = Translator::new(&keys, &spans, &bindings, &BTreeMap::new());
        let out = t.translate(EV_ABS, ABS_X, 255);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].code, ABS_X);
    }

    #[test]
    fn a_captured_button_lands_on_its_360_button() {
        let mut t = captured();
        assert_eq!(
            t.translate(EV_KEY, 0x120, 1),
            vec![Out {
                kind: EV_KEY,
                code: 0x130,
                value: 1
            }]
        );
        assert_eq!(
            t.translate(EV_KEY, 0x120, 0),
            vec![Out {
                kind: EV_KEY,
                code: 0x130,
                value: 0
            }]
        );
        assert_eq!(
            t.translate(EV_KEY, 0x121, 1),
            vec![Out {
                kind: EV_KEY,
                code: 0x131,
                value: 1
            }]
        );
    }

    #[test]
    fn c_buttons_become_the_right_stick_and_let_go_returns_it_to_centre() {
        let mut t = captured();
        assert_eq!(
            t.translate(EV_KEY, 0x122, 1),
            vec![Out {
                kind: EV_ABS,
                code: ABS_RY,
                value: STICK_MIN + 1
            }]
        );
        assert_eq!(
            t.translate(EV_KEY, 0x123, 1),
            vec![Out {
                kind: EV_ABS,
                code: ABS_RY,
                value: 0
            }],
            "up and down cancel"
        );
        assert_eq!(
            t.translate(EV_KEY, 0x122, 0),
            vec![Out {
                kind: EV_ABS,
                code: ABS_RY,
                value: STICK_MAX
            }]
        );
        assert_eq!(
            t.translate(EV_KEY, 0x123, 0),
            vec![Out {
                kind: EV_ABS,
                code: ABS_RY,
                value: 0
            }]
        );
    }

    #[test]
    fn a_trigger_axis_is_rescaled_to_xpads_range_and_the_left_stick_rides_across() {
        let mut t = captured();
        assert_eq!(
            t.translate(EV_ABS, ABS_Z, 255),
            vec![Out {
                kind: EV_ABS,
                code: ABS_Z,
                value: 255
            }]
        );
        assert_eq!(
            t.translate(EV_ABS, ABS_Z, 128),
            vec![Out {
                kind: EV_ABS,
                code: ABS_Z,
                value: 128
            }]
        );
        assert_eq!(
            t.translate(EV_ABS, ABS_X, 255),
            vec![Out {
                kind: EV_ABS,
                code: ABS_X,
                value: STICK_MAX
            }]
        );
        assert_eq!(
            t.translate(EV_ABS, ABS_X, 128),
            vec![Out {
                kind: EV_ABS,
                code: ABS_X,
                value: 0
            }]
        );
        assert_eq!(
            t.translate(EV_ABS, ABS_X, 0),
            vec![Out {
                kind: EV_ABS,
                code: ABS_X,
                value: -32767
            }]
        );
    }

    #[test]
    fn a_control_the_source_lacks_is_never_pressed_and_an_unknown_key_does_nothing() {
        let mut t = captured();
        assert!(t.translate(EV_KEY, 0x1ff, 1).is_empty());
        assert!(
            t.translate(EV_KEY, 0x13C, 1).is_empty(),
            "no guide on this pad"
        );
    }

    #[test]
    fn the_same_value_twice_is_sent_once() {
        let mut t = captured();
        assert_eq!(t.translate(EV_KEY, 0x120, 1).len(), 1);
        assert!(
            t.translate(EV_KEY, 0x120, 1).is_empty(),
            "autorepeat is not a new press"
        );
    }

    #[test]
    fn a_standard_pad_is_carried_across_code_for_code_with_xpads_x_and_y() {
        let keys = [
            0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x13A, 0x13B, 0x13C, 0x13D, 0x13E,
        ];
        let mut spans = BTreeMap::new();
        for code in [ABS_X, ABS_Y, ABS_RX, ABS_RY] {
            spans.insert(code, AxisSpan::new(-32768, 32767, 0));
        }
        spans.insert(ABS_Z, AxisSpan::new(0, 1023, 0));
        spans.insert(ABS_HAT0X, AxisSpan::new(-1, 1, 0));
        spans.insert(ABS_HAT0Y, AxisSpan::new(-1, 1, 0));
        let mut t = Translator::new(&keys, &spans, &BTreeMap::new(), &BTreeMap::new());
        // The kernel's BTN_NORTH (0x133) is padmap's Y; xpad sends 0x133 for X. So Y
        // on a standard pad comes out as xpad's Y, which is 0x134.
        assert_eq!(
            t.translate(EV_KEY, 0x133, 1),
            vec![Out {
                kind: EV_KEY,
                code: 0x134,
                value: 1
            }]
        );
        assert_eq!(
            t.translate(EV_KEY, 0x134, 1),
            vec![Out {
                kind: EV_KEY,
                code: 0x133,
                value: 1
            }]
        );
        assert_eq!(
            t.translate(EV_KEY, 0x13C, 1),
            vec![Out {
                kind: EV_KEY,
                code: 0x13C,
                value: 1
            }],
            "guide rides across"
        );
        assert_eq!(
            t.translate(EV_ABS, ABS_Z, 1023),
            vec![Out {
                kind: EV_ABS,
                code: ABS_Z,
                value: 255
            }]
        );
        assert_eq!(
            t.translate(EV_ABS, ABS_HAT0X, -1),
            vec![Out {
                kind: EV_ABS,
                code: ABS_HAT0X,
                value: -1
            }]
        );
        assert_eq!(
            t.translate(EV_ABS, ABS_RX, 32767),
            vec![Out {
                kind: EV_ABS,
                code: ABS_RX,
                value: STICK_MAX
            }]
        );
        assert_eq!(
            t.translate(EV_ABS, ABS_RX, -32768),
            vec![Out {
                kind: EV_ABS,
                code: ABS_RX,
                value: -32767
            }]
        );
    }

    #[test]
    fn a_second_input_on_a_control_is_a_union_with_the_first() {
        let keys = [0x120, 0x121];
        let spans = BTreeMap::new();
        let mut bindings = BTreeMap::new();
        bindings.insert(Control::RightShoulder, Binding::button(0));
        let mut extras = BTreeMap::new();
        extras.insert(Control::RightShoulder, vec![Binding::button(1)]);
        let mut t = Translator::new(&keys, &spans, &bindings, &extras);
        assert_eq!(
            t.translate(EV_KEY, 0x121, 1),
            vec![Out {
                kind: EV_KEY,
                code: 0x137,
                value: 1
            }],
            "the twin presses R"
        );
        t.translate(EV_KEY, 0x120, 1);
        assert!(
            t.translate(EV_KEY, 0x121, 0).is_empty(),
            "R is still down on the first"
        );
        assert_eq!(
            t.translate(EV_KEY, 0x120, 0),
            vec![Out {
                kind: EV_KEY,
                code: 0x137,
                value: 0
            }]
        );
    }

    #[test]
    fn release_all_lets_go_of_everything_held() {
        let mut t = captured();
        t.translate(EV_KEY, 0x120, 1);
        t.translate(EV_KEY, 0x122, 1);
        let released = t.release_all();
        assert!(released.contains(&Out {
            kind: EV_KEY,
            code: 0x130,
            value: 0
        }));
        assert!(released.contains(&Out {
            kind: EV_ABS,
            code: ABS_RY,
            value: 0
        }));
        assert!(t.release_all().is_empty());
    }

    #[test]
    fn the_layouts_own_bindings_name_every_button_by_its_ordinal() {
        let b = bindings();
        assert_eq!(b[&Control::A], Binding::button(0).with_ra_index(Some(0)));
        assert_eq!(
            b[&Control::X],
            Binding::button(2).with_ra_index(Some(2)),
            "SDL's x:b2 is 0x133"
        );
        assert_eq!(b[&Control::Y], Binding::button(3).with_ra_index(Some(3)));
        assert_eq!(b[&Control::DpadUp], Binding::hat(0, 1));
        assert_eq!(b[&Control::LeftTrigger], Binding::axis(2, 1));
        assert_eq!(b[&Control::RightStickUp], Binding::axis(4, -1));
        assert_eq!(spans()[&ABS_Z], AxisSpan::new(0, 255, 0));
        assert_eq!(spans()[&ABS_X], AxisSpan::new(STICK_MIN, STICK_MAX, 0));
    }
}
