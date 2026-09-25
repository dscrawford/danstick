//! A second input on one control: when the pad is mirrored, the clone still
//! has one button for that control, so a press on the second input comes out
//! as the first, and the control is down while either is.
//!
//! Everything downstream keeps its single binding -- the SDL line, every
//! emulator's config -- because the virtual pad still has one button. This is
//! the second `if` in the loop that reads the pad and writes the clone.

use std::collections::{BTreeMap, BTreeSet};

use crate::binding::{Binding, BindingKind, BTN_JOYSTICK, HAT_CODES};
use crate::capture::{ABS_HAT0X, ABS_HAT0Y, EV_ABS, EV_KEY};
use crate::control::Control;
use crate::sdl::AxisSpan;
pub use crate::xbox::Out;

/// One physical input, as a binding names it on this pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Member {
    Key(u16),
    /// An axis and the direction that means "pressed".
    Axis(u16, i8),
    /// A hat axis and the direction that means "pressed".
    Hat(u16, i8),
}

/// A control with more than one input: the first is what the clone has.
#[derive(Debug, Clone)]
struct Group {
    members: Vec<Member>,
    pressed: Vec<f64>,
    /// The primary's own last raw value, for when no twin overrides it.
    raw_primary: i32,
}

#[derive(Debug, Clone)]
pub struct Twins {
    groups: Vec<Group>,
    spans: BTreeMap<u16, AxisSpan>,
    last: BTreeMap<(u16, u16), i32>,
}

fn sdl_ordered(keys: &[u16]) -> Vec<u16> {
    let mut sorted: Vec<u16> = keys.to_vec();
    sorted.sort_unstable();
    sorted
        .iter()
        .copied()
        .filter(|c| *c >= BTN_JOYSTICK)
        .chain(sorted.iter().copied().filter(|c| *c < BTN_JOYSTICK))
        .collect()
}

fn member_of(binding: Binding, ordered: &[u16], axis_codes: &[u16]) -> Option<Member> {
    match binding.kind {
        BindingKind::Button => usize::try_from(binding.index)
            .ok()
            .and_then(|i| ordered.get(i))
            .map(|&code| Member::Key(code)),
        BindingKind::Axis => usize::try_from(binding.index)
            .ok()
            .and_then(|i| axis_codes.get(i))
            .map(|&code| Member::Axis(code, if binding.value < 0 { -1 } else { 1 })),
        BindingKind::Hat => match binding.value {
            1 => Some(Member::Hat(ABS_HAT0Y, -1)),
            2 => Some(Member::Hat(ABS_HAT0X, 1)),
            4 => Some(Member::Hat(ABS_HAT0Y, 1)),
            8 => Some(Member::Hat(ABS_HAT0X, -1)),
            _ => None,
        },
    }
}

fn position(span: AxisSpan, value: i32) -> f64 {
    if span.maximum <= span.minimum {
        return 0.0;
    }
    if span.rests_centred() {
        let travel = f64::from(value - span.rest);
        let reach = if travel >= 0.0 {
            f64::from(span.maximum - span.rest)
        } else {
            f64::from(span.rest - span.minimum)
        };
        if reach <= 0.0 {
            0.0
        } else {
            (travel / reach).clamp(-1.0, 1.0)
        }
    } else {
        (f64::from(value - span.minimum) / f64::from(span.maximum - span.minimum)).clamp(0.0, 1.0)
    }
}

impl Twins {
    /// None when no control has a second input: nothing to do, and the loop should know it.
    pub fn new(
        keys: &[u16],
        spans: &BTreeMap<u16, AxisSpan>,
        primaries: &BTreeMap<Control, Binding>,
        extras: &BTreeMap<Control, Vec<Binding>>,
    ) -> Option<Self> {
        let ordered = sdl_ordered(keys);
        let mut axis_codes: Vec<u16> = spans
            .keys()
            .copied()
            .filter(|c| !HAT_CODES.contains(c))
            .collect();
        axis_codes.sort_unstable();
        let mut groups = Vec::new();
        for (control, twins) in extras {
            let Some(primary) = primaries
                .get(control)
                .and_then(|b| member_of(*b, &ordered, &axis_codes))
            else {
                continue;
            };
            let mut members = vec![primary];
            members.extend(
                twins
                    .iter()
                    .filter_map(|b| member_of(*b, &ordered, &axis_codes))
                    .filter(|m| *m != primary),
            );
            if members.len() < 2 {
                continue;
            }
            let count = members.len();
            groups.push(Group {
                members,
                pressed: vec![0.0; count],
                raw_primary: 0,
            });
        }
        if groups.is_empty() {
            return None;
        }
        Some(Twins {
            groups,
            spans: spans.clone(),
            last: BTreeMap::new(),
        })
    }

    fn fraction(&self, member: Member, kind: u16, code: u16, value: i32) -> Option<f64> {
        match (kind, member) {
            (EV_KEY, Member::Key(c)) if c == code => Some(if value != 0 { 1.0 } else { 0.0 }),
            (EV_ABS, Member::Axis(c, sign)) if c == code => {
                let span = self.spans.get(&code)?;
                Some((position(*span, value) * f64::from(sign)).clamp(0.0, 1.0))
            }
            (EV_ABS, Member::Hat(c, sign)) if c == code => {
                Some(if value.signum() == i32::from(sign) {
                    1.0
                } else {
                    0.0
                })
            }
            _ => None,
        }
    }

    /// The clone's events for one source event: the event itself, unless it is
    /// a primary whose control is decided by the union, plus the union's.
    pub fn translate(&mut self, kind: u16, code: u16, value: i32) -> Vec<Out> {
        let mut touched: Vec<usize> = Vec::new();
        for g in 0..self.groups.len() {
            for m in 0..self.groups[g].members.len() {
                let member = self.groups[g].members[m];
                let Some(fraction) = self.fraction(member, kind, code, value) else {
                    continue;
                };
                self.groups[g].pressed[m] = fraction;
                if m == 0 {
                    self.groups[g].raw_primary = value;
                }
                if !touched.contains(&g) {
                    touched.push(g);
                }
            }
        }
        let mut out = Vec::new();
        let mut primaries: BTreeSet<(u16, u16)> = BTreeSet::new();
        for g in touched {
            let (kind, code, value) = self.union_of(g);
            primaries.insert((kind, code));
            self.push(&mut out, kind, code, value);
        }
        if !primaries.contains(&(kind, code)) {
            out.insert(0, Out { kind, code, value });
        }
        out
    }

    /// What the primary reads, from every input of its group.
    fn union_of(&self, g: usize) -> (u16, u16, i32) {
        let group = &self.groups[g];
        let any_twin = group.pressed.iter().skip(1).any(|p| *p > 0.5);
        match group.members[0] {
            Member::Key(code) => {
                let any = group.pressed.iter().any(|p| *p > 0.5);
                (EV_KEY, code, i32::from(any))
            }
            Member::Axis(code, sign) => {
                let value = if any_twin {
                    let span = self
                        .spans
                        .get(&code)
                        .copied()
                        .unwrap_or(AxisSpan::new(0, 1, 0));
                    if sign > 0 {
                        span.maximum
                    } else {
                        span.minimum
                    }
                } else {
                    group.raw_primary
                };
                (EV_ABS, code, value)
            }
            Member::Hat(code, sign) => {
                let value = if any_twin {
                    i32::from(sign)
                } else {
                    group.raw_primary
                };
                (EV_ABS, code, value)
            }
        }
    }

    fn push(&mut self, out: &mut Vec<Out>, kind: u16, code: u16, value: i32) {
        if self.last.insert((kind, code), value) == Some(value) {
            return;
        }
        out.push(Out { kind, code, value });
    }

    /// Every primary let go of, for a pause: what the clone reads when nothing is held.
    pub fn release_all(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        for g in 0..self.groups.len() {
            for p in &mut self.groups[g].pressed {
                *p = 0.0;
            }
            self.groups[g].raw_primary = match self.groups[g].members[0] {
                Member::Axis(code, _) => self.spans.get(&code).map(|s| s.rest).unwrap_or(0),
                _ => 0,
            };
            let (kind, code, value) = self.union_of(g);
            self.push(&mut out, kind, code, value);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pad where R (0x137, SDL ordinal 5) also answers to the right trigger axis (ABS_RZ, ordinal 5).
    fn twins() -> Twins {
        let keys = [0x130, 0x131, 0x133, 0x134, 0x136, 0x137];
        let mut spans = BTreeMap::new();
        for code in [0x00, 0x01, 0x02, 0x03, 0x04] {
            spans.insert(code, AxisSpan::new(-32768, 32767, 0));
        }
        spans.insert(0x05, AxisSpan::new(0, 255, 0));
        let mut primaries = BTreeMap::new();
        primaries.insert(Control::RightShoulder, Binding::button(5));
        primaries.insert(Control::A, Binding::button(0));
        let mut extras = BTreeMap::new();
        extras.insert(Control::RightShoulder, vec![Binding::axis(5, 1)]);
        Twins::new(&keys, &spans, &primaries, &extras).expect("a group")
    }

    #[test]
    fn the_second_input_comes_out_as_the_first_and_still_as_itself() {
        let mut t = twins();
        let out = t.translate(EV_ABS, 0x05, 255);
        assert_eq!(
            out,
            vec![
                Out {
                    kind: EV_ABS,
                    code: 0x05,
                    value: 255
                },
                Out {
                    kind: EV_KEY,
                    code: 0x137,
                    value: 1
                }
            ]
        );
        let out = t.translate(EV_ABS, 0x05, 0);
        assert_eq!(
            out,
            vec![
                Out {
                    kind: EV_ABS,
                    code: 0x05,
                    value: 0
                },
                Out {
                    kind: EV_KEY,
                    code: 0x137,
                    value: 0
                }
            ]
        );
    }

    #[test]
    fn the_control_is_down_while_either_input_is() {
        let mut t = twins();
        t.translate(EV_KEY, 0x137, 1);
        t.translate(EV_ABS, 0x05, 255);
        // Letting go of the bumper while the trigger is held: R stays down.
        assert!(
            t.translate(EV_KEY, 0x137, 0).is_empty(),
            "no release while the twin holds it"
        );
        assert_eq!(
            t.translate(EV_ABS, 0x05, 0),
            vec![
                Out {
                    kind: EV_ABS,
                    code: 0x05,
                    value: 0
                },
                Out {
                    kind: EV_KEY,
                    code: 0x137,
                    value: 0
                }
            ]
        );
    }

    #[test]
    fn a_control_with_one_input_passes_straight_through() {
        let mut t = twins();
        assert_eq!(
            t.translate(EV_KEY, 0x130, 1),
            vec![Out {
                kind: EV_KEY,
                code: 0x130,
                value: 1
            }]
        );
        assert_eq!(
            t.translate(EV_ABS, 0x00, 1000),
            vec![Out {
                kind: EV_ABS,
                code: 0x00,
                value: 1000
            }]
        );
    }

    #[test]
    fn nothing_doubled_means_nothing_to_do() {
        let keys = [0x130];
        let spans = BTreeMap::new();
        let mut primaries = BTreeMap::new();
        primaries.insert(Control::A, Binding::button(0));
        assert!(Twins::new(&keys, &spans, &primaries, &BTreeMap::new()).is_none());
        let mut extras = BTreeMap::new();
        extras.insert(Control::B, vec![Binding::button(0)]);
        assert!(
            Twins::new(&keys, &spans, &primaries, &extras).is_none(),
            "a twin with no primary is nothing"
        );
    }

    #[test]
    fn a_twin_on_an_axis_primary_drives_it_to_the_end() {
        let keys = [0x130, 0x131];
        let mut spans = BTreeMap::new();
        spans.insert(0x02, AxisSpan::new(0, 255, 0));
        let mut primaries = BTreeMap::new();
        primaries.insert(Control::LeftTrigger, Binding::axis(0, 1));
        let mut extras = BTreeMap::new();
        extras.insert(Control::LeftTrigger, vec![Binding::button(1)]);
        let mut t = Twins::new(&keys, &spans, &primaries, &extras).expect("group");
        assert_eq!(
            t.translate(EV_KEY, 0x131, 1),
            vec![
                Out {
                    kind: EV_KEY,
                    code: 0x131,
                    value: 1
                },
                Out {
                    kind: EV_ABS,
                    code: 0x02,
                    value: 255
                }
            ]
        );
        assert_eq!(
            t.translate(EV_KEY, 0x131, 0),
            vec![
                Out {
                    kind: EV_KEY,
                    code: 0x131,
                    value: 0
                },
                Out {
                    kind: EV_ABS,
                    code: 0x02,
                    value: 0
                }
            ]
        );
        let released = {
            t.translate(EV_KEY, 0x131, 1);
            t.release_all()
        };
        assert_eq!(
            released,
            vec![Out {
                kind: EV_ABS,
                code: 0x02,
                value: 0
            }]
        );
    }
}
