//! Integration tests for the capture wizard.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use padmap_core::binding::{Binding, RA_INVISIBLE};
use padmap_core::capture::{
    deflection, game_scope_options, layout_options, scope_options, ChoiceKind, Chooser, Claim,
    Event, MappingRun, Outcome, ABS_HAT0X, ABS_HAT0Y, ABS_X, AXIS_AS_BUTTON_THRESHOLD,
    AXIS_RELEASE, AXIS_THRESHOLD, CAPTURE_GAP_SECONDS, HAT_DOWN, HAT_LEFT, HAT_RIGHT, HAT_UP,
    SKIP_HOLD_SECONDS,
};
use padmap_core::control::Control;
use padmap_core::layout::Layout;
use padmap_core::sdl::AxisSpan;
use padmap_core::{layout, scope};

const TAP: f64 = 0.05;
const AFTER_GAP: f64 = CAPTURE_GAP_SECONDS + 0.01;
const SKIP_BUTTON: u16 = 0x13f;

/// Clock is a parameter so tests don't need to sleep; all decisions are "how long ago".
fn play(run: &mut MappingRun, script: &[(Event, f64)]) -> Vec<Outcome> {
    script
        .iter()
        .map(|(event, now)| run.feed(*event, *now))
        .collect()
}

fn last(outcomes: Vec<Outcome>) -> Outcome {
    outcomes
        .into_iter()
        .last()
        .expect("a script does something")
}

fn tap(run: &mut MappingRun, code: u16, at: f64) -> Outcome {
    last(play(
        run,
        &[(Event::key(code, 1), at), (Event::key(code, 0), at + TAP)],
    ))
}

fn hold(run: &mut MappingRun, code: u16, at: f64) -> Outcome {
    last(play(
        run,
        &[
            (Event::key(code, 1), at),
            (Event::key(code, 0), at + SKIP_HOLD_SECONDS + 0.01),
        ],
    ))
}

fn skip_to(run: &mut MappingRun, target: usize) -> f64 {
    let mut clock = 0.0;
    while run.index() < target {
        let outcome = hold(run, SKIP_BUTTON, clock);
        assert!(
            matches!(outcome, Outcome::Skipped { .. }),
            "a hold past the threshold must skip, not {outcome:?}"
        );
        clock += SKIP_HOLD_SECONDS + 0.01 + AFTER_GAP;
    }
    clock
}

fn recorded(outcome: &Outcome) -> (Control, Binding) {
    match outcome {
        Outcome::Recorded { control, binding } => (*control, *binding),
        other => panic!("expected a capture, got {other:?}"),
    }
}

fn stick() -> AxisSpan {
    AxisSpan::new(-100, 100, 0)
}

fn trigger() -> AxisSpan {
    AxisSpan::new(0, 255, 0)
}

fn axes(entries: &[(u16, AxisSpan)]) -> BTreeMap<u16, AxisSpan> {
    entries.iter().copied().collect()
}

fn joystick_keys() -> Vec<u16> {
    (0x130..0x13c).collect()
}

fn snes_run(keys: Vec<u16>, axes: BTreeMap<u16, AxisSpan>, held: BTreeSet<u16>) -> MappingRun {
    MappingRun::new(1, layout::get("snes"), keys, String::new(), axes, held)
}

fn run() -> MappingRun {
    snes_run(joystick_keys(), BTreeMap::new(), BTreeSet::new())
}

fn first_dpad() -> usize {
    layout::get("snes")
        .controls
        .iter()
        .position(|control| control.kind == "dpad")
        .expect("the SNES layout has a d-pad")
}

/// Edge case: layout with no controls must not panic.
fn empty_layout() -> &'static Layout {
    static EMPTY: OnceLock<Layout> = OnceLock::new();
    EMPTY.get_or_init(|| Layout {
        id: "nothing".to_owned(),
        label: "A pad with no controls".to_owned(),
        console_label: String::new(),
        image: String::new(),
        shapes: Vec::new(),
        controls: Vec::new(),
    })
}

#[test]
fn a_tap_records_the_current_control_and_moves_on() {
    let mut run = run();
    let first = run.current().expect("a first prompt");
    assert_eq!(run.index(), 0);
    assert_eq!(run.total(), 12);

    assert_eq!(run.feed(Event::key(0x130, 1), 0.0), Outcome::Ignored);
    let outcome = run.feed(Event::key(0x130, 0), TAP);

    let (control, binding) = recorded(&outcome);
    assert_eq!(control, first);
    assert!(outcome.advanced());
    assert_eq!(binding, Binding::button(0).with_ra_index(Some(0)));
    assert_eq!(run.index(), 1);
    assert_eq!(run.bindings().len(), 1);
    assert_eq!(run.bindings().get(&first), Some(&binding));
    assert_eq!(run.conflict(), None);
}

#[test]
fn a_whole_layout_can_be_walked_to_completion() {
    let mut run = run();
    let order = run.layout.order();
    for (position, code) in joystick_keys().into_iter().enumerate() {
        let expected = run.current().expect("a prompt for every control");
        let outcome = tap(&mut run, code, position as f64);
        let (control, binding) = recorded(&outcome);
        assert_eq!(control, expected);
        assert_eq!(control, order[position]);
        assert_eq!(
            binding,
            Binding::button(position as i32).with_ra_index(Some(position as i32))
        );
    }
    assert!(run.finished());
    assert_eq!(run.current(), None, "a finished run asks nothing");
    assert_eq!(run.index(), run.total());
    for control in order {
        assert!(
            run.bindings().contains_key(&control),
            "{control} was walked past without a binding"
        );
    }
}

#[test]
fn a_finished_run_ignores_everything_afterwards() {
    let mut run = run();
    for (position, code) in joystick_keys().into_iter().enumerate() {
        tap(&mut run, code, position as f64);
    }
    assert!(run.finished());
    let recorded_count = run.bindings().len();

    // A pad does not stop reporting because the wizard is done, and whatever.
    for event in [
        Event::key(0x130, 1),
        Event::key(0x130, 0),
        Event::key(0x130, 2),
        Event::abs(ABS_HAT0X, 1),
        Event::abs(ABS_X, -100),
    ] {
        assert_eq!(run.feed(event, 100.0), Outcome::Ignored);
    }
    assert_eq!(run.bindings().len(), recorded_count);
    assert_eq!(run.index(), run.total());
    assert_eq!(run.skip(100.0), None, "there is nothing left to skip");
}

#[test]
fn a_press_held_from_before_the_run_cannot_answer_the_first_prompt() {
    let mut run = snes_run(joystick_keys(), BTreeMap::new(), [0x130].into());
    assert!(run.settling());

    assert_eq!(run.feed(Event::key(0x130, 0), 0.1), Outcome::Ignored);

    assert_eq!(run.index(), 0, "the opening press answered a prompt");
    assert!(run.bindings().is_empty());
    assert!(!run.settling());
}

#[test]
fn settling_lasts_until_every_opening_button_is_released() {
    let mut run = snes_run(joystick_keys(), BTreeMap::new(), [0x130, 0x131].into());
    assert!(run.settling());

    run.feed(Event::key(0x130, 0), 0.1);
    assert!(run.settling(), "one of the two is still down");

    run.feed(Event::key(0x131, 0), 0.2);
    assert!(!run.settling());
    assert_eq!(run.index(), 0, "neither release may answer a prompt");
}

#[test]
fn a_settling_run_refuses_a_button_that_was_not_held_at_the_start() {
    let mut run = snes_run(joystick_keys(), BTreeMap::new(), [0x130].into());

    assert_eq!(tap(&mut run, 0x131, 0.1), Outcome::Ignored);

    assert_eq!(run.index(), 0);
    assert!(run.bindings().is_empty());
}

#[test]
fn a_settling_run_refuses_an_axis_too() {
    let mut run = snes_run(Vec::new(), axes(&[(0x02, stick())]), [0x130].into());

    assert_eq!(run.feed(Event::abs(0x02, 100), 0.1), Outcome::Ignored);

    assert_eq!(run.index(), 0);
}

#[test]
fn a_button_pressed_after_the_run_started_is_not_a_settling_button() {
    let mut run = snes_run(joystick_keys(), BTreeMap::new(), [0x130].into());
    let first = run.current().expect("a first prompt");

    run.feed(Event::key(0x130, 0), 0.1);
    assert!(!run.settling());
    let outcome = tap(&mut run, 0x130, 0.2);

    let (control, _) = recorded(&outcome);
    assert_eq!(control, first);
    assert_eq!(run.index(), 1);
}

#[test]
fn a_hold_past_the_skip_threshold_skips_the_control_instead_of_recording_it() {
    let mut run = run();
    let first = run.current().expect("a first prompt");

    let outcome = hold(&mut run, 0x130, 0.0);

    assert_eq!(outcome, Outcome::Skipped { control: first });
    assert!(outcome.advanced());
    assert_eq!(run.index(), 1);
    assert!(run.bindings().is_empty(), "a skip must record nothing");
}

#[test]
fn a_hold_of_exactly_the_skip_threshold_skips() {
    let mut run = run();
    let first = run.current().expect("a first prompt");

    run.feed(Event::key(0x130, 1), 0.0);
    let outcome = run.feed(Event::key(0x130, 0), SKIP_HOLD_SECONDS);

    assert_eq!(outcome, Outcome::Skipped { control: first });
}

#[test]
fn a_hold_just_under_the_skip_threshold_records() {
    let mut run = run();
    let first = run.current().expect("a first prompt");

    run.feed(Event::key(0x130, 1), 0.0);
    let outcome = run.feed(Event::key(0x130, 0), SKIP_HOLD_SECONDS - 0.01);

    let (control, _) = recorded(&outcome);
    assert_eq!(control, first);
    assert_eq!(run.bindings().len(), 1);
}

#[test]
fn skipping_the_last_control_finishes_the_run() {
    let mut run = run();
    let last_index = run.total() - 1;
    let clock = skip_to(&mut run, last_index);
    let last_control = run.current().expect("one prompt left");

    let outcome = hold(&mut run, SKIP_BUTTON, clock);

    assert_eq!(
        outcome,
        Outcome::Skipped {
            control: last_control
        }
    );
    assert!(
        run.finished(),
        "a pad that has none of these controls must still finish"
    );
    assert!(run.bindings().is_empty());
}

#[test]
fn a_skip_starts_the_same_gap_a_capture_does() {
    // The button released after a skip-hold must not answer the control the.
    let mut run = run();
    let released = SKIP_HOLD_SECONDS + 0.01;
    hold(&mut run, 0x130, 0.0);

    assert_eq!(tap(&mut run, 0x131, released + 0.05), Outcome::Ignored);

    assert_eq!(
        run.index(),
        1,
        "an input inside the post-skip gap answered a prompt"
    );
    assert!(tap(&mut run, 0x131, released + AFTER_GAP).advanced());
}

#[test]
fn skip_called_directly_advances_clears_the_conflict_and_blocks() {
    let mut run = run();
    let first = run.current().expect("a first prompt");

    assert_eq!(run.skip(0.0), Some(first));

    assert_eq!(run.index(), 1);
    assert_eq!(run.conflict(), None);
    assert_eq!(tap(&mut run, 0x130, 0.1), Outcome::Ignored);
}

#[test]
fn nothing_is_accepted_during_the_gap_even_on_a_different_code() {
    let mut run = run();
    tap(&mut run, 0x130, 0.0);
    let at = run.index();

    assert_eq!(tap(&mut run, 0x131, TAP + 0.01), Outcome::Ignored);

    assert_eq!(run.index(), at, "an input inside the gap answered a prompt");
    assert!(
        tap(&mut run, 0x131, TAP + AFTER_GAP).advanced(),
        "and works once it has passed"
    );
}

#[test]
fn an_axis_inside_the_gap_cannot_answer_either() {
    let mut run = snes_run(Vec::new(), axes(&[(0x02, stick())]), BTreeSet::new());
    assert!(run.feed(Event::abs(0x02, 100), 0.0).advanced());

    assert_eq!(run.feed(Event::abs(0x02, -100), 0.05), Outcome::Ignored);
    assert_eq!(run.feed(Event::abs(ABS_HAT0X, 1), 0.10), Outcome::Ignored);

    assert_eq!(run.index(), 1);
}

#[test]
fn a_button_pressed_inside_the_gap_and_released_after_it_records_nothing() {
    let mut run = run();
    tap(&mut run, 0x130, 0.0);

    run.feed(Event::key(0x131, 1), 0.10);
    assert_eq!(run.feed(Event::key(0x131, 0), 0.50), Outcome::Ignored);

    assert_eq!(run.index(), 1);
    assert!(
        tap(&mut run, 0x131, 1.0).advanced(),
        "the button is dead afterwards"
    );
}

#[test]
fn a_release_inside_the_gap_clears_the_press_it_belonged_to() {
    let mut run = run();
    play(
        &mut run,
        &[
            (Event::key(0x131, 1), 0.00),
            (Event::key(0x130, 1), 0.10),
            (Event::key(0x130, 0), 0.15),
            (Event::key(0x131, 0), 0.20),
        ],
    );
    assert_eq!(run.index(), 1);

    assert_eq!(run.feed(Event::key(0x131, 0), 0.60), Outcome::Ignored);

    assert_eq!(
        run.index(),
        1,
        "a release with no press behind it answered a prompt"
    );
    assert_eq!(run.bindings().len(), 1);
}

#[test]
fn an_axis_released_inside_the_gap_is_still_re_armed() {
    // Release during gap must still re-arm; drop it and trigger gets stuck.
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    assert!(
        run.feed(Event::abs(ABS_X, -100), clock).advanced(),
        "full left"
    );
    run.feed(Event::abs(ABS_X, 0), clock + 0.05);

    assert!(
        run.feed(Event::abs(ABS_X, 100), clock + AFTER_GAP)
            .advanced(),
        "the axis never re-armed"
    );
}

#[test]
fn a_hat_released_inside_the_gap_is_still_re_armed() {
    let mut run = snes_run(Vec::new(), BTreeMap::new(), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    assert!(run.feed(Event::abs(ABS_HAT0X, -1), clock).advanced());
    run.feed(Event::abs(ABS_HAT0X, 0), clock + 0.05);

    assert!(
        run.feed(Event::abs(ABS_HAT0X, 1), clock + AFTER_GAP)
            .advanced(),
        "a cheap adapter reports the opposite direction on release; the hat must still re-arm"
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
    assert!(run.bindings().is_empty());
}

#[test]
fn an_autorepeat_is_not_mistaken_for_a_release_and_does_not_end_settling() {
    let mut run = snes_run(joystick_keys(), BTreeMap::new(), [0x130].into());

    run.feed(Event::key(0x130, 2), 0.5);

    assert!(run.settling(), "a repeat is not a release");
}

#[test]
fn one_button_cannot_answer_two_prompts() {
    let mut run = run();
    let first = run.current().expect("a first prompt");
    tap(&mut run, 0x130, 0.0);
    let second = run.current().expect("a second prompt");

    let outcome = tap(&mut run, 0x130, 1.0);

    assert_eq!(
        outcome,
        Outcome::Refused {
            claim: Claim::Button { code: 0x130 },
            held_by: first
        }
    );
    assert!(!outcome.advanced());
    assert_eq!(
        run.conflict(),
        Some(first),
        "the refusal must name its holder"
    );
    assert_eq!(run.current(), Some(second), "a refusal does not advance");
    assert_eq!(run.bindings().len(), 1);
}

#[test]
fn one_hat_direction_cannot_answer_two_prompts() {
    let mut run = snes_run(Vec::new(), BTreeMap::new(), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());
    let first = run.current().expect("a d-pad prompt");

    run.feed(Event::abs(ABS_HAT0X, 1), clock);
    run.feed(Event::abs(ABS_HAT0X, 0), clock + 0.05);
    let outcome = run.feed(Event::abs(ABS_HAT0X, 1), clock + AFTER_GAP);

    assert_eq!(
        outcome,
        Outcome::Refused {
            claim: Claim::Hat {
                index: 0,
                value: HAT_RIGHT
            },
            held_by: first
        }
    );
    assert_eq!(run.conflict(), Some(first));
}

#[test]
fn one_axis_half_cannot_answer_two_prompts() {
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());
    let first = run.current().expect("a d-pad prompt");

    run.feed(Event::abs(ABS_X, -100), clock);
    run.feed(Event::abs(ABS_X, 0), clock + 0.05);
    let outcome = run.feed(Event::abs(ABS_X, -100), clock + AFTER_GAP);

    assert_eq!(
        outcome,
        Outcome::Refused {
            claim: Claim::Axis {
                code: ABS_X,
                sign: -1
            },
            held_by: first
        }
    );
    assert_eq!(run.conflict(), Some(first));
}

#[test]
fn the_other_half_of_an_axis_is_a_separate_claim() {
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    let left = recorded(&run.feed(Event::abs(ABS_X, -100), clock));
    run.feed(Event::abs(ABS_X, 0), clock + 0.05);
    let right = recorded(&run.feed(Event::abs(ABS_X, 100), clock + AFTER_GAP));

    assert_eq!(left.1, Binding::axis(0, -1));
    assert_eq!(right.1, Binding::axis(0, 1));
    assert_ne!(left.0, right.0, "both halves answered the same control");
}

#[test]
fn a_later_capture_clears_the_conflict() {
    // A stale conflict pinned under a later control names a clash that is not.
    let mut run = run();
    tap(&mut run, 0x130, 0.0);
    tap(&mut run, 0x130, 1.0);
    assert!(run.conflict().is_some());

    tap(&mut run, 0x131, 2.0);

    assert_eq!(run.conflict(), None);
}

#[test]
fn a_skip_clears_the_conflict_too() {
    let mut run = run();
    tap(&mut run, 0x130, 0.0);
    tap(&mut run, 0x130, 1.0);
    assert!(run.conflict().is_some());

    hold(&mut run, 0x131, 2.0);

    assert_eq!(run.conflict(), None);
}

#[test]
fn an_axis_springing_back_through_centre_does_not_answer_the_next_prompt() {
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());
    let up = run.current().expect("a d-pad prompt");

    let left = recorded(&run.feed(Event::abs(ABS_X, -100), clock));
    assert_eq!(left.0, up);
    let down = run.current().expect("the next d-pad prompt");

    assert_eq!(
        run.feed(Event::abs(ABS_X, 100), clock + 0.05),
        Outcome::Ignored
    );
    assert_eq!(
        run.feed(Event::abs(ABS_X, 100), clock + 1.0),
        Outcome::Ignored
    );
    assert_eq!(run.current(), Some(down));

    run.feed(Event::abs(ABS_X, 0), clock + 1.1);
    let right = recorded(&run.feed(Event::abs(ABS_X, 100), clock + 1.2));

    assert_eq!(right.0, down);
    assert_eq!(right.1, Binding::axis(0, 1));
    assert_eq!(
        run.bindings().len(),
        2,
        "one control per push, no more and no fewer"
    );
}

#[test]
fn an_axis_that_only_comes_back_part_way_is_not_re_armed() {
    let part_way = (AXIS_RELEASE * 100.0) as i32 + 10;
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    run.feed(Event::abs(ABS_X, -100), clock);
    run.feed(Event::abs(ABS_X, -part_way), clock + 0.05);

    assert_eq!(
        run.feed(Event::abs(ABS_X, 100), clock + AFTER_GAP),
        Outcome::Ignored
    );
    assert_eq!(run.bindings().len(), 1);
    run.feed(Event::abs(ABS_X, 0), clock + 1.0);
    assert!(run.feed(Event::abs(ABS_X, 100), clock + 1.1).advanced());
}

#[test]
fn an_axis_never_touched_is_armed_from_the_start() {
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    assert!(run.feed(Event::abs(ABS_X, -100), clock).advanced());
}

#[test]
fn a_trigger_resting_at_its_minimum_still_re_arms_after_it_is_let_go() {
    let triggers = axes(&[(0x02, trigger()), (0x05, trigger())]);
    let mut run = snes_run(Vec::new(), triggers, BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    let (_, first) = recorded(&run.feed(Event::abs(0x02, 255), clock));
    run.feed(Event::abs(0x02, 0), clock + 0.05);
    let again = run.feed(Event::abs(0x02, 255), clock + AFTER_GAP);

    assert_eq!(first, Binding::axis(0, 1), "a trigger only travels one way");
    assert!(
        matches!(again, Outcome::Refused { .. }),
        "the trigger went dead after one press: {again:?}"
    );
    let (_, second) = recorded(&run.feed(Event::abs(0x05, 255), clock + AFTER_GAP + 0.1));
    assert_eq!(second, Binding::axis(1, 1));
}

#[test]
fn a_resting_axis_reports_nothing_at_all() {
    // Continuous resting axis events must not bury session.
    let mut run = snes_run(Vec::new(), axes(&[(0x02, stick())]), BTreeSet::new());
    assert_eq!(run.layout.controls[0].kind, "button");

    for value in [-5, -1, 0, 1, 5, 40, 54] {
        assert_eq!(
            run.feed(Event::abs(0x02, value), 0.0),
            Outcome::Ignored,
            "{value}% of deflection was reported"
        );
    }
}

#[test]
fn an_axis_between_the_two_thresholds_reports_how_far_short_it_fell() {
    let mut run = snes_run(Vec::new(), axes(&[(0x02, stick())]), BTreeSet::new());

    let outcome = run.feed(Event::abs(0x02, 70), 0.0);

    match outcome {
        Outcome::TooGentle {
            claim,
            travel,
            needed,
        } => {
            assert_eq!(
                claim,
                Claim::Axis {
                    code: 0x02,
                    sign: 1
                }
            );
            assert!((travel - 0.70).abs() < 1e-9, "travel was {travel}");
            assert_eq!(needed, AXIS_AS_BUTTON_THRESHOLD);
        }
        other => panic!("expected a too-gentle report, got {other:?}"),
    }
    assert_eq!(run.index(), 0);
}

#[test]
fn the_too_gentle_report_carries_the_direction_it_was_pushed() {
    let mut run = snes_run(Vec::new(), axes(&[(0x02, stick())]), BTreeSet::new());

    let outcome = run.feed(Event::abs(0x02, -70), 0.0);

    assert!(matches!(
        outcome,
        Outcome::TooGentle {
            claim: Claim::Axis { sign: -1, .. },
            ..
        }
    ));
}

#[test]
fn an_axis_exactly_at_the_face_button_threshold_answers_the_prompt() {
    let mut run = snes_run(Vec::new(), axes(&[(0x02, stick())]), BTreeSet::new());
    let first = run.current().expect("a face-button prompt");

    let outcome = run.feed(
        Event::abs(0x02, (AXIS_AS_BUTTON_THRESHOLD * 100.0) as i32),
        0.0,
    );

    let (control, binding) = recorded(&outcome);
    assert_eq!(control, first);
    assert_eq!(binding, Binding::axis(0, 1));
}

#[test]
fn an_axis_one_step_below_the_face_button_threshold_does_not() {
    let mut run = snes_run(Vec::new(), axes(&[(0x02, stick())]), BTreeSet::new());

    let outcome = run.feed(
        Event::abs(0x02, (AXIS_AS_BUTTON_THRESHOLD * 100.0) as i32 - 1),
        0.0,
    );

    assert!(matches!(outcome, Outcome::TooGentle { .. }));
    assert_eq!(run.index(), 0);
}

#[test]
fn an_axis_the_pad_never_declared_cannot_answer_a_face_button() {
    let mut run = snes_run(Vec::new(), BTreeMap::new(), BTreeSet::new());

    assert_eq!(run.feed(Event::abs(0x02, 32767), 0.0), Outcome::Ignored);

    assert_eq!(run.index(), 0);
}

#[test]
fn a_hat_never_answers_a_face_button_prompt_at_any_value() {
    // Hat answering face button is never correct; undeclared hat must not fill face buttons.
    let mut run = run();
    assert_eq!(run.layout.controls[0].kind, "button");

    for code in [ABS_HAT0X, ABS_HAT0Y] {
        for value in [-32767, -1, 0, 1, 32767] {
            assert_eq!(
                run.feed(Event::abs(code, value), 0.0),
                Outcome::Ignored,
                "hat code {code:#x} value {value} answered a face button"
            );
        }
    }
    assert_eq!(run.index(), 0);
}

#[test]
fn an_axis_answers_a_shoulder_prompt_on_the_ordinary_threshold() {
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());
    assert_eq!(run.layout.controls[first_dpad()].kind, "dpad");

    let outcome = run.feed(
        Event::abs(ABS_X, (AXIS_THRESHOLD * 100.0) as i32 + 1),
        clock,
    );

    assert!(
        outcome.advanced(),
        "a d-pad prompt refused an ordinary push: {outcome:?}"
    );
}

#[test]
fn an_axis_short_of_the_ordinary_threshold_never_answers_a_dpad_prompt() {
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    let outcome = run.feed(
        Event::abs(ABS_X, (AXIS_THRESHOLD * 100.0) as i32 - 1),
        clock,
    );

    assert_eq!(outcome, Outcome::Ignored);
}

#[test]
fn deflection_is_measured_from_rest_not_from_the_declared_middle() {
    let trigger = trigger();

    assert_eq!(
        deflection(trigger, 0),
        0.0,
        "an untouched trigger must read zero"
    );
    assert!(
        deflection(trigger, 255) > 1.9,
        "a pressed trigger reads about 2.0"
    );
    assert!(
        deflection(trigger, 128) > 0.9,
        "the midpoint is a real push, not rest"
    );
}

#[test]
fn deflection_is_signed_because_direction_is_what_a_binding_records() {
    let stick = stick();

    assert!(deflection(stick, -100) < 0.0);
    assert!(deflection(stick, 100) > 0.0);
    assert_eq!(deflection(stick, 0), 0.0);
    assert_eq!(deflection(stick, 50), 0.5);
    assert_eq!(deflection(stick, -50), -0.5);
}

#[test]
fn deflection_scales_by_half_the_declared_range_not_the_travel_available() {
    let lopsided = AxisSpan::new(0, 100, 20);

    assert_eq!(deflection(lopsided, 100), 1.6);
    assert_eq!(deflection(lopsided, 0), -0.4);
}

#[test]
fn deflection_of_a_degenerate_span_is_zero_rather_than_a_division_by_zero() {
    assert_eq!(deflection(AxisSpan::new(0, 0, 0), 50), 0.0);
    assert_eq!(deflection(AxisSpan::new(10, 5, 7), 50), 0.0);
    assert_eq!(deflection(AxisSpan::new(-1, -1, -1), 0), 0.0);
}

#[test]
fn each_hat_direction_records_the_sdl_bit_that_names_it() {
    for (code, value, bit) in [
        (ABS_HAT0X, 1, HAT_RIGHT),
        (ABS_HAT0X, -1, HAT_LEFT),
        (ABS_HAT0Y, 1, HAT_DOWN),
        (ABS_HAT0Y, -1, HAT_UP),
    ] {
        let mut run = snes_run(Vec::new(), BTreeMap::new(), BTreeSet::new());
        let clock = skip_to(&mut run, first_dpad());

        let (_, binding) = recorded(&run.feed(Event::abs(code, value), clock));

        assert_eq!(
            binding,
            Binding::hat(0, bit),
            "hat code {code:#x} value {value} recorded the wrong direction"
        );
    }
}

#[test]
fn a_hat_at_full_scale_is_still_just_one_direction() {
    let mut run = snes_run(Vec::new(), BTreeMap::new(), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    let (_, binding) = recorded(&run.feed(Event::abs(ABS_HAT0X, 32767), clock));

    assert_eq!(binding, Binding::hat(0, HAT_RIGHT));
}

#[test]
fn a_hat_release_records_nothing() {
    let mut run = snes_run(Vec::new(), BTreeMap::new(), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    assert_eq!(run.feed(Event::abs(ABS_HAT0X, 0), clock), Outcome::Ignored);
    assert_eq!(run.feed(Event::abs(ABS_HAT0Y, 0), clock), Outcome::Ignored);

    assert!(run.bindings().is_empty());
}

#[test]
fn a_hat_binding_is_always_hat_zero_whichever_code_it_arrived_on() {
    let mut run = snes_run(Vec::new(), BTreeMap::new(), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    let (_, x) = recorded(&run.feed(Event::abs(ABS_HAT0X, 1), clock));
    run.feed(Event::abs(ABS_HAT0X, 0), clock + 0.05);
    let (_, y) = recorded(&run.feed(Event::abs(ABS_HAT0Y, -1), clock + AFTER_GAP));

    assert_eq!(x.index, 0);
    assert_eq!(y.index, 0);
}

#[test]
fn an_axis_records_its_index_among_the_pads_axes_not_its_evdev_code() {
    // Code 0x05 may not be axis 5 in the map.
    let mut run = snes_run(
        Vec::new(),
        axes(&[(0x00, stick()), (0x01, stick()), (0x05, stick())]),
        BTreeSet::new(),
    );
    let clock = skip_to(&mut run, first_dpad());

    let (_, binding) = recorded(&run.feed(Event::abs(0x05, 100), clock));

    assert_eq!(
        binding,
        Binding::axis(2, 1),
        "code 0x05 is the third axis, not the sixth"
    );
}

#[test]
fn hat_codes_do_not_count_towards_the_axis_numbering() {
    let mut run = snes_run(
        Vec::new(),
        axes(&[(0x00, stick()), (ABS_HAT0X, stick()), (0x05, stick())]),
        BTreeSet::new(),
    );
    let clock = skip_to(&mut run, first_dpad());

    let (_, binding) = recorded(&run.feed(Event::abs(0x05, 100), clock));

    assert_eq!(binding, Binding::axis(1, 1));
}

#[test]
fn a_key_the_pad_never_declared_records_nothing() {
    let mut run = snes_run(vec![0x130, 0x131], BTreeMap::new(), BTreeSet::new());

    assert_eq!(tap(&mut run, 0x2ff, 0.0), Outcome::Ignored);

    assert_eq!(run.index(), 0);
    assert!(run.bindings().is_empty());
}

#[test]
fn a_key_below_btn_misc_records_as_invisible_to_retroarch() {
    let key_a: u16 = 0x1e;
    let mut run = snes_run(vec![key_a, 0x130, 0x131], BTreeMap::new(), BTreeSet::new());

    let outcome = tap(&mut run, key_a, 0.0);

    let (_, binding) = recorded(&outcome);
    assert_eq!(
        binding,
        Binding::button(2).with_ra_index(Some(RA_INVISIBLE))
    );
    assert!(binding.sdl_visible());
    assert!(
        !binding.retroarch_visible(),
        "a button RetroArch cannot see was given a number"
    );
}

#[test]
fn a_layout_with_no_controls_is_finished_before_it_starts() {
    let mut run = MappingRun::new(
        1,
        empty_layout(),
        joystick_keys(),
        String::new(),
        axes(&[(ABS_X, stick())]),
        BTreeSet::new(),
    );

    assert!(run.finished());
    assert_eq!(run.current(), None);
    assert_eq!(run.total(), 0);
    assert_eq!(run.index(), 0);
    assert_eq!(run.skip(0.0), None);
    assert_eq!(tap(&mut run, 0x130, 0.0), Outcome::Ignored);
    assert_eq!(hold(&mut run, 0x130, 1.0), Outcome::Ignored);
    assert_eq!(run.feed(Event::abs(ABS_X, 100), 3.0), Outcome::Ignored);
    assert_eq!(run.feed(Event::abs(ABS_HAT0X, 1), 4.0), Outcome::Ignored);
    assert!(run.bindings().is_empty());
}

#[test]
fn an_event_that_is_neither_a_key_nor_an_axis_is_ignored() {
    let mut run = run();

    assert_eq!(
        run.feed(
            Event {
                kind: 0x00,
                code: 0,
                value: 0
            },
            0.0
        ),
        Outcome::Ignored
    );
    assert_eq!(
        run.feed(
            Event {
                kind: 0x15,
                code: 0,
                value: 1
            },
            0.1
        ),
        Outcome::Ignored
    );

    assert_eq!(run.index(), 0);
}

#[test]
fn the_scope_is_carried_on_the_run_because_it_is_needed_at_the_end() {
    // Scope must persist across wizard abandon/restart.
    let run = snes_run(joystick_keys(), BTreeMap::new(), BTreeSet::new());
    assert_eq!(run.scope, "");

    let filed = MappingRun::new(
        2,
        layout::get("n64"),
        Vec::new(),
        scope::console("n64"),
        BTreeMap::new(),
        BTreeSet::new(),
    );

    assert_eq!(filed.player, 2);
    assert_eq!(filed.scope, "console:n64");
    assert_eq!(filed.layout.id, "n64");
}

#[test]
fn a_claim_describes_itself_in_the_terms_a_log_reader_has_to_match() {
    assert_eq!(Claim::Button { code: 0x130 }.to_string(), "button code 304");
    assert_eq!(
        Claim::Hat {
            index: 0,
            value: HAT_UP
        }
        .to_string(),
        "hat up"
    );
    assert_eq!(
        Claim::Hat {
            index: 0,
            value: HAT_RIGHT
        }
        .to_string(),
        "hat right"
    );
    assert_eq!(
        Claim::Hat {
            index: 0,
            value: HAT_DOWN
        }
        .to_string(),
        "hat down"
    );
    assert_eq!(
        Claim::Hat {
            index: 0,
            value: HAT_LEFT
        }
        .to_string(),
        "hat left"
    );
    assert_eq!(
        Claim::Axis { code: 5, sign: 1 }.to_string(),
        "axis code 5 +"
    );
    assert_eq!(
        Claim::Axis { code: 5, sign: -1 }.to_string(),
        "axis code 5 -"
    );
}

#[test]
fn a_claim_on_a_hat_value_that_is_not_a_direction_still_describes_itself() {
    // A diagonal reads 3 and is not a control, but a log line that raised.
    assert_eq!(Claim::Hat { index: 0, value: 3 }.to_string(), "hat 3");
    assert_eq!(Claim::Hat { index: 0, value: 0 }.to_string(), "hat 0");
}

#[test]
fn only_a_capture_or_a_skip_counts_as_answering_the_prompt() {
    assert!(Outcome::Recorded {
        control: Control::A,
        binding: Binding::button(0)
    }
    .advanced());
    assert!(Outcome::Skipped {
        control: Control::A
    }
    .advanced());
    assert!(!Outcome::Ignored.advanced());
    assert!(!Outcome::Refused {
        claim: Claim::Button { code: 1 },
        held_by: Control::A
    }
    .advanced());
    assert!(!Outcome::TooGentle {
        claim: Claim::Axis { code: 1, sign: 1 },
        travel: 0.7,
        needed: AXIS_AS_BUTTON_THRESHOLD,
    }
    .advanced());
}

fn chooser(axes: BTreeMap<u16, AxisSpan>, held: BTreeSet<u16>) -> Chooser {
    Chooser::new(
        1,
        layout_options(&BTreeSet::new()),
        ChoiceKind::Layout,
        "Which controller is this?".to_owned(),
        axes,
        held,
    )
}

fn play_chooser(chooser: &mut Chooser, script: &[(Event, f64)]) -> Vec<bool> {
    script
        .iter()
        .map(|(event, now)| chooser.feed(*event, *now))
        .collect()
}

#[test]
fn a_chooser_moves_on_the_transition_into_a_direction_only() {
    // Move on transition only; held stick would spin selection past entry.
    let mut chooser = chooser(BTreeMap::new(), BTreeSet::new());
    assert_eq!(chooser.index(), 0);

    let moved = play_chooser(
        &mut chooser,
        &[
            (Event::abs(ABS_HAT0X, 1), 0.0),
            (Event::abs(ABS_HAT0X, 1), 0.1),
            (Event::abs(ABS_HAT0X, 0), 0.2),
        ],
    );

    assert_eq!(
        moved,
        vec![true, false, false],
        "the push moved, the hold and release must not"
    );
    assert_eq!(chooser.index(), 1);
}

#[test]
fn a_chooser_moves_from_the_analogue_stick_too() {
    let mut chooser = chooser(axes(&[(ABS_X, stick())]), BTreeSet::new());

    let moved = play_chooser(
        &mut chooser,
        &[
            (Event::abs(ABS_X, 100), 0.0),
            (Event::abs(ABS_X, 100), 0.1),
            (Event::abs(ABS_X, 0), 0.2),
            (Event::abs(ABS_X, -100), 0.3),
        ],
    );

    assert_eq!(moved, vec![true, false, false, true]);
    assert_eq!(
        chooser.index(),
        0,
        "right then left returns to where it started"
    );
}

#[test]
fn a_chooser_ignores_a_stick_push_that_does_not_reach_the_threshold() {
    let mut chooser = chooser(axes(&[(ABS_X, stick())]), BTreeSet::new());

    assert!(!chooser.feed(Event::abs(ABS_X, (AXIS_THRESHOLD * 100.0) as i32 - 1), 0.0));

    assert_eq!(chooser.index(), 0);
}

#[test]
fn a_chooser_ignores_an_axis_that_is_not_the_horizontal_one() {
    let mut chooser = chooser(axes(&[(0x01, stick()), (0x05, stick())]), BTreeSet::new());

    assert!(!chooser.feed(Event::abs(0x01, 100), 0.0));
    assert!(!chooser.feed(Event::abs(0x05, -100), 0.1));
    assert!(!chooser.feed(Event::abs(ABS_HAT0Y, 1), 0.2));

    assert_eq!(chooser.index(), 0);
}

#[test]
fn a_chooser_ignores_an_axis_it_has_no_span_for() {
    let mut chooser = chooser(BTreeMap::new(), BTreeSet::new());

    assert!(!chooser.feed(Event::abs(ABS_X, 32767), 0.0));

    assert_eq!(chooser.index(), 0);
}

#[test]
fn a_chooser_ignores_an_axis_whose_span_is_degenerate() {
    let mut chooser = chooser(axes(&[(ABS_X, AxisSpan::new(0, 0, 0))]), BTreeSet::new());

    assert!(!chooser.feed(Event::abs(ABS_X, 32767), 0.0));

    assert_eq!(chooser.index(), 0);
}

#[test]
fn a_chooser_wraps_at_both_ends() {
    // Strip is a loop; wrapping keeps all consoles reachable.
    let count = layout_options(&BTreeSet::new()).len();
    let mut chooser = chooser(BTreeMap::new(), BTreeSet::new());

    assert!(chooser.move_by(-1));
    assert_eq!(chooser.index(), count - 1);
    assert!(chooser.move_by(1));
    assert_eq!(chooser.index(), 0);

    play_chooser(
        &mut chooser,
        &[
            (Event::abs(ABS_HAT0X, -1), 0.0),
            (Event::abs(ABS_HAT0X, 0), 0.1),
        ],
    );
    assert_eq!(chooser.index(), count - 1);
    play_chooser(&mut chooser, &[(Event::abs(ABS_HAT0X, 1), 0.2)]);
    assert_eq!(chooser.index(), 0);
}

#[test]
fn an_empty_chooser_reports_no_choice_rather_than_indexing_past_the_end() {
    // game_scope_options returns empty for unknown console; daemon must not crash.
    let mut chooser = Chooser::new(
        1,
        Vec::new(),
        ChoiceKind::Scope,
        String::new(),
        axes(&[(ABS_X, stick())]),
        BTreeSet::new(),
    );

    assert_eq!(chooser.chosen(), "");
    assert_eq!(chooser.chosen_layout(), "");
    assert!(!chooser.move_by(1));
    assert!(!chooser.move_by(-1));
    assert!(!chooser.feed(Event::abs(ABS_HAT0X, 1), 0.0));
    assert!(!chooser.feed(Event::abs(ABS_X, -100), 0.1));
    assert_eq!(chooser.index(), 0);
    assert_eq!(chooser.chosen(), "");
}

#[test]
fn a_chooser_reports_what_is_selected_and_what_to_draw_beside_it() {
    let mut chooser = chooser(BTreeMap::new(), BTreeSet::new());
    let options = layout_options(&BTreeSet::new());

    assert_eq!(chooser.chosen(), options[0].id);
    assert_eq!(chooser.chosen_layout(), options[0].layout);
    chooser.move_by(2);
    assert_eq!(chooser.chosen(), options[2].id);
}

#[test]
fn a_chooser_accepts_a_hold_and_ignores_a_tap() {
    let mut chooser = chooser(BTreeMap::new(), BTreeSet::new());

    chooser.feed(Event::key(0x130, 1), 0.0);
    assert!(!chooser.feed(Event::key(0x130, 0), 0.1), "a tap confirmed");
    assert!(!chooser.confirmed());

    chooser.feed(Event::key(0x130, 1), 1.0);
    assert!(chooser.feed(Event::key(0x130, 0), 1.0 + SKIP_HOLD_SECONDS + 0.01));
    assert!(chooser.confirmed());
}

#[test]
fn a_chooser_confirms_at_exactly_the_hold_threshold() {
    let mut chooser = chooser(BTreeMap::new(), BTreeSet::new());

    chooser.feed(Event::key(0x130, 1), 0.0);
    assert!(chooser.feed(Event::key(0x130, 0), SKIP_HOLD_SECONDS));

    assert!(chooser.confirmed());
}

#[test]
fn a_chooser_ignores_an_autorepeat_and_a_release_with_no_press() {
    let mut chooser = chooser(BTreeMap::new(), BTreeSet::new());

    assert!(!chooser.feed(Event::key(0x130, 2), 0.0));
    assert!(
        !chooser.feed(Event::key(0x130, 0), 5.0),
        "a release with no press confirmed"
    );

    assert!(!chooser.confirmed());
}

#[test]
fn a_confirmed_chooser_ignores_everything_afterwards() {
    let mut chooser = chooser(axes(&[(ABS_X, stick())]), BTreeSet::new());
    chooser.feed(Event::key(0x130, 1), 0.0);
    chooser.feed(Event::key(0x130, 0), SKIP_HOLD_SECONDS + 0.01);
    assert!(chooser.confirmed());
    let chosen = chooser.chosen().to_owned();

    assert!(!chooser.feed(Event::abs(ABS_HAT0X, 1), 3.0));
    assert!(!chooser.feed(Event::abs(ABS_X, 100), 3.1));
    assert!(!chooser.feed(Event::key(0x131, 1), 3.2));
    assert!(!chooser.feed(Event::key(0x131, 0), 5.0));

    assert_eq!(chooser.index(), 0);
    assert_eq!(chooser.chosen(), chosen);
}

#[test]
fn a_chooser_settles_before_it_accepts_anything() {
    // Opening press held = *hold*, would confirm first entry.
    let mut chooser = chooser(axes(&[(ABS_X, stick())]), [0x130].into());
    assert!(chooser.settling());

    chooser.feed(Event::key(0x131, 1), 0.0);
    assert!(!chooser.feed(Event::key(0x131, 0), 1.0));
    assert!(!chooser.confirmed());
    assert!(!chooser.feed(Event::abs(ABS_X, 100), 1.1));
    assert_eq!(chooser.index(), 0);

    assert!(!chooser.feed(Event::key(0x130, 0), 1.2));
    assert!(!chooser.settling());
    assert!(!chooser.confirmed());

    chooser.feed(Event::key(0x131, 1), 1.3);
    assert!(chooser.feed(Event::key(0x131, 0), 1.3 + SKIP_HOLD_SECONDS));
    assert!(chooser.confirmed());
}

#[test]
fn a_chooser_settles_only_once_every_opening_button_is_released() {
    let mut chooser = chooser(BTreeMap::new(), [0x130, 0x131].into());

    chooser.feed(Event::key(0x130, 0), 0.1);
    assert!(chooser.settling(), "one of the two is still down");

    chooser.feed(Event::key(0x131, 0), 0.2);
    assert!(!chooser.settling());
    assert!(!chooser.confirmed());
}

#[test]
fn a_chooser_re_arms_at_the_push_threshold_not_at_the_wizards_release_threshold() {
    let resting = 40;
    let mut chooser = chooser(axes(&[(ABS_X, stick())]), BTreeSet::new());

    assert!(chooser.feed(Event::abs(ABS_X, 100), 0.0));
    assert!(
        !chooser.feed(Event::abs(ABS_X, resting), 0.1),
        "not far enough to be a push"
    );
    assert!(
        chooser.feed(Event::abs(ABS_X, 100), 0.2),
        "the picker stopped responding"
    );

    assert_eq!(chooser.index(), 2);
}

#[test]
fn the_wizard_would_not_have_re_armed_where_the_chooser_does() {
    let resting = 40;
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    run.feed(Event::abs(ABS_X, 100), clock);
    run.feed(Event::abs(ABS_X, resting), clock + 0.05);

    assert_eq!(
        run.feed(Event::abs(ABS_X, 100), clock + AFTER_GAP),
        Outcome::Ignored
    );
    assert_eq!(run.bindings().len(), 1);
}

#[test]
fn a_chooser_says_which_question_it_is_asking() {
    assert_eq!(ChoiceKind::Layout.as_str(), "layout");
    assert_eq!(ChoiceKind::Scope.as_str(), "scope");

    let chooser = chooser(BTreeMap::new(), BTreeSet::new());
    assert_eq!(chooser.kind, ChoiceKind::Layout);
    assert_eq!(chooser.title, "Which controller is this?");
    assert_eq!(chooser.player, 1);
}

#[test]
fn layout_options_offers_every_shipped_layout_in_catalogue_order() {
    let options = layout_options(&BTreeSet::new());
    let catalogue = layout::all();

    assert_eq!(options.len(), catalogue.len());
    for (option, shipped) in options.iter().zip(catalogue) {
        assert_eq!(option.id, shipped.id);
        assert_eq!(option.label, shipped.label);
        assert_eq!(option.layout, option.id);
        assert!(!option.mapped);
    }
}

#[test]
fn layout_options_marks_the_layouts_already_captured() {
    let mapped: BTreeSet<String> = ["n64".to_owned(), "snes".to_owned()].into();

    let options = layout_options(&mapped);

    for option in &options {
        assert_eq!(
            option.mapped,
            mapped.contains(&option.id),
            "{} is marked wrongly",
            option.id
        );
    }
    assert!(
        options.iter().any(|option| option.mapped),
        "nothing was marked at all"
    );
}

#[test]
fn layout_options_ignores_a_mapped_name_that_is_not_a_layout() {
    let mapped: BTreeSet<String> = ["dreamcast".to_owned()].into();

    let options = layout_options(&mapped);

    assert_eq!(options.len(), layout::all().len());
    assert!(options.iter().all(|option| !option.mapped));
}

#[test]
fn a_game_scope_strip_offers_the_console_first() {
    let options = game_scope_options("n64", "n64/goldeneye", "GoldenEye 007", &BTreeSet::new());

    assert_eq!(options.len(), 2);
    assert_eq!(options[0].id, "console:n64");
    assert_eq!(options[0].label, "Nintendo 64 games");
    assert_eq!(options[1].id, "game:n64/goldeneye");
    assert_eq!(options[1].label, "GoldenEye 007");
}

#[test]
fn both_game_scope_entries_draw_the_consoles_pad() {
    // Game mapping is still N64 control set; pad shown must match wizard's.
    let options = game_scope_options("n64", "n64/goldeneye", "GoldenEye 007", &BTreeSet::new());

    assert_eq!(options[0].layout, "n64");
    assert_eq!(options[1].layout, "n64");
}

#[test]
fn a_game_with_no_console_is_offered_nothing_rather_than_the_generic_pad() {
    // Exporter omits console for unrecognized cores; strip would promise wrong controller.
    assert!(game_scope_options("", "x/y", "Title", &BTreeSet::new()).is_empty());
    assert!(game_scope_options("", "", "", &BTreeSet::new()).is_empty());
}

#[test]
fn a_game_scope_strip_with_no_key_offers_only_the_console() {
    let options = game_scope_options("snes", "", "Super Mario World", &BTreeSet::new());

    assert_eq!(options.len(), 1);
    assert_eq!(options[0].id, "console:snes");
    assert_eq!(options[0].label, "SNES games");
}

#[test]
fn a_game_with_no_title_is_offered_under_its_key() {
    let options = game_scope_options("snes", "snes/smw", "", &BTreeSet::new());

    assert_eq!(options[1].label, "snes/smw");
}

#[test]
fn a_console_label_is_used_in_place_of_the_controllers_name() {
    let options = game_scope_options("arcade", "", "", &BTreeSet::new());

    assert_eq!(options[0].label, "Arcade games");
}

#[test]
fn a_game_scope_strip_marks_each_entry_separately() {
    let scopes: BTreeSet<String> = ["game:n64/goldeneye".to_owned()].into();

    let options = game_scope_options("n64", "n64/goldeneye", "GoldenEye 007", &scopes);

    assert!(!options[0].mapped, "the console scope is not captured");
    assert!(options[1].mapped, "the game scope is");
}

#[test]
fn a_scope_strip_offers_any_game_first_then_every_console() {
    let options = scope_options(&BTreeSet::new(), "gamecube", &[]);
    let consoles = layout::consoles();

    assert_eq!(options[0].id, scope::UNIVERSAL);
    assert_eq!(options[0].label, "Any game");
    assert_eq!(options[0].layout, "gamecube");
    assert_eq!(options.len(), 1 + consoles.len());
    for (option, console) in options[1..].iter().zip(&consoles) {
        assert_eq!(option.id, scope::console(console));
        assert_eq!(option.layout, *console);
    }
}

#[test]
fn a_scope_strip_does_not_offer_the_generic_layout_as_a_console() {
    let options = scope_options(&BTreeSet::new(), "generic", &[]);

    assert!(options.iter().all(|option| option.id != "console:generic"));
    assert!(options.iter().any(|option| option.id == "console:snes"));
}

#[test]
fn a_scope_strip_with_no_recent_games_is_just_any_game_and_the_consoles() {
    let options = scope_options(&BTreeSet::new(), "generic", &[]);

    assert_eq!(options.len(), 1 + layout::consoles().len());
    assert!(options.iter().all(|option| !option.id.starts_with("game:")));
}

#[test]
fn a_scope_strip_lists_recent_games_after_the_consoles() {
    let recent = vec![
        (
            "n64".to_owned(),
            "n64/mario".to_owned(),
            "Mario 64".to_owned(),
        ),
        ("snes".to_owned(), "snes/smw".to_owned(), String::new()),
    ];

    let options = scope_options(&BTreeSet::new(), "generic", &recent);

    let tail = &options[1 + layout::consoles().len()..];
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].id, "game:n64/mario");
    assert_eq!(tail[0].label, "Mario 64");
    assert_eq!(
        tail[0].layout, "n64",
        "a game entry draws its console's pad"
    );
    assert_eq!(tail[1].id, "game:snes/smw");
    assert_eq!(
        tail[1].label, "snes/smw",
        "an empty title falls back to the key"
    );
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

    assert!(options.iter().any(|option| option.id == "game:n64/mario"));
    assert!(options.iter().all(|option| option.id != "game:x/unknown"));
}

#[test]
fn a_scope_strip_skips_a_recent_game_with_no_key() {
    let recent = vec![("n64".to_owned(), String::new(), "Nameless".to_owned())];

    let options = scope_options(&BTreeSet::new(), "generic", &recent);

    assert_eq!(options.len(), 1 + layout::consoles().len());
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
        (
            "snes".to_owned(),
            "snes/smw".to_owned(),
            "Mario World".to_owned(),
        ),
    ];

    let options = scope_options(&BTreeSet::new(), "generic", &recent);

    assert_eq!(
        options
            .iter()
            .filter(|option| option.id == "game:n64/mario")
            .count(),
        1
    );
    assert_eq!(
        options
            .iter()
            .filter(|option| option.id == "game:snes/smw")
            .count(),
        1
    );
}

#[test]
fn a_console_less_sighting_does_not_hide_the_same_game_s_usable_entry() {
    let recent = vec![
        (String::new(), "n64/mario".to_owned(), "Mario 64".to_owned()),
        (
            "n64".to_owned(),
            "n64/mario".to_owned(),
            "Mario 64".to_owned(),
        ),
    ];

    let options = scope_options(&BTreeSet::new(), "generic", &recent);

    let offered: Vec<_> = options
        .iter()
        .filter(|option| option.id == "game:n64/mario")
        .collect();
    assert_eq!(
        offered.len(),
        1,
        "offered once, from the entry that names a console"
    );
    assert_eq!(
        offered[0].layout, "n64",
        "and drawn as the pad the wizard will ask about"
    );
}

#[test]
fn a_scope_strip_marks_what_is_already_captured() {
    // Mark captured scopes so user knows what re-mapping would destroy.
    let scopes: BTreeSet<String> = [
        scope::UNIVERSAL.to_owned(),
        "console:n64".to_owned(),
        "game:snes/smw".to_owned(),
    ]
    .into();
    let recent = vec![
        (
            "snes".to_owned(),
            "snes/smw".to_owned(),
            "Mario World".to_owned(),
        ),
        (
            "n64".to_owned(),
            "n64/mario".to_owned(),
            "Mario 64".to_owned(),
        ),
    ];

    let options = scope_options(&scopes, "gamecube", &recent);

    for option in &options {
        assert_eq!(
            option.mapped,
            scopes.contains(&option.id),
            "{} is marked wrongly",
            option.id
        );
    }
    assert_eq!(options.iter().filter(|option| option.mapped).count(), 3);
}

// ---- Finishing from the pad, and runs that start from a stored capture ----

use padmap_core::capture::FINISH_HOLD_SECONDS;

fn seeded_run(stored: &[(Control, Binding)]) -> MappingRun {
    let axes = axes(&[(ABS_X, stick())]);
    snes_run(joystick_keys(), axes, BTreeSet::new())
        .seeded(&stored.iter().copied().collect::<BTreeMap<_, _>>())
}

fn control_at(index: usize) -> Control {
    layout::get("snes").controls[index].canonical
}

#[test]
fn a_hold_past_the_finish_tier_ends_the_run_and_keeps_what_was_bound() {
    let mut run = run();
    let first = control_at(0);
    assert!(tap(&mut run, 0x130, 0.0).advanced());
    let outcome = last(play(
        &mut run,
        &[
            (Event::key(0x131, 1), 1.0),
            (Event::key(0x131, 0), 1.0 + FINISH_HOLD_SECONDS + 0.01),
        ],
    ));
    assert_eq!(outcome, Outcome::Finished);
    assert!(run.finished(), "a finish must end the run");
    assert!(
        run.index() < run.total(),
        "finishing is not walking to the end"
    );
    assert!(
        run.bindings().contains_key(&first),
        "what was bound stays bound"
    );
    assert!(
        !run.bindings().contains_key(&control_at(1)),
        "the finishing button must not bind the prompt it ended on"
    );
    assert_eq!(
        run.feed(Event::key(0x132, 1), 5.0),
        Outcome::Ignored,
        "a finished run is deaf"
    );
}

#[test]
fn a_hold_between_the_two_tiers_still_skips() {
    let mut run = run();
    let first = control_at(0);
    let outcome = last(play(
        &mut run,
        &[
            (Event::key(0x130, 1), 0.0),
            (
                Event::key(0x130, 0),
                (SKIP_HOLD_SECONDS + FINISH_HOLD_SECONDS) / 2.0,
            ),
        ],
    ));
    assert_eq!(outcome, Outcome::Skipped { control: first });
    assert!(!run.finished());
    // The tiers must be far enough apart that a slow skip cannot finish.
    const {
        assert!(FINISH_HOLD_SECONDS >= 2.0 * SKIP_HOLD_SECONDS);
    }
}

#[test]
fn the_finish_ring_fills_while_a_button_is_down_and_time_alone_ends_the_run() {
    let mut run = run();
    assert_eq!(run.finish_hold(0.0), None, "nothing held, nothing to draw");
    run.feed(Event::key(0x130, 1), 10.0);
    assert_eq!(run.tick(10.0), Outcome::Ignored);
    let half = run
        .finish_hold(10.0 + FINISH_HOLD_SECONDS / 2.0)
        .expect("a held button fills the ring");
    assert!((half - 0.5).abs() < 1e-9, "{half}");
    assert_eq!(
        run.tick(10.0 + FINISH_HOLD_SECONDS - 0.01),
        Outcome::Ignored
    );
    assert!(!run.finished());
    assert_eq!(run.tick(10.0 + FINISH_HOLD_SECONDS), Outcome::Finished);
    assert!(run.finished());
    assert_eq!(run.finish_hold(20.0), None, "a finished run draws nothing");
    assert_eq!(
        run.feed(Event::key(0x130, 0), 10.0 + FINISH_HOLD_SECONDS + 0.5),
        Outcome::Ignored,
        "the release after a timed finish must not skip or record anything"
    );
}

#[test]
fn a_release_ends_the_ring_and_a_tap_never_fills_it() {
    let mut run = run();
    run.feed(Event::key(0x130, 1), 0.0);
    assert!(run.finish_hold(0.5).is_some());
    run.feed(Event::key(0x130, 0), 0.6);
    assert_eq!(run.finish_hold(0.7), None, "released: nothing to draw");
    assert!(run.bindings().contains_key(&control_at(0)), "0.6s is a tap");
    let mut fresh = snes_run(joystick_keys(), BTreeMap::new(), BTreeSet::new());
    assert!(tap(&mut fresh, 0x130, 0.0).advanced());
    assert_eq!(fresh.finish_hold(TAP + 0.01), None);
}

#[test]
fn a_press_held_from_before_the_run_does_not_fill_the_finish_ring() {
    let mut run = snes_run(
        joystick_keys(),
        BTreeMap::new(),
        [0x130].into_iter().collect(),
    );
    assert_eq!(
        run.finish_hold(5.0),
        None,
        "an opening hold is not a gesture"
    );
    assert_eq!(run.tick(5.0), Outcome::Ignored);
    assert!(!run.finished());
    run.feed(Event::key(0x131, 1), 0.0);
    assert_eq!(
        run.finish_hold(1.0),
        None,
        "nothing counts while the opening press is still settling"
    );
}

#[test]
fn a_seeded_run_starts_with_the_stored_capture_and_guards_it() {
    let a = control_at(0);
    let b = control_at(1);
    let mut run = seeded_run(&[(a, Binding::button(0)), (b, Binding::button(1))]);
    assert_eq!(run.bindings().len(), 2, "seeded before the first prompt");
    assert_eq!(run.index(), 0, "seeding does not advance the prompt");

    let retaken = tap(&mut run, 0x130, 0.0);
    assert_eq!(
        retaken,
        Outcome::Recorded {
            control: a,
            binding: Binding::button(0).with_ra_index(Some(0)),
        },
        "the control being asked for may take its own stored input again"
    );

    let clash = tap(&mut run, 0x130, TAP + AFTER_GAP);
    assert_eq!(
        clash,
        Outcome::Refused {
            claim: Claim::Button { code: 0x130 },
            held_by: a,
        },
        "an input a stored control holds is refused, naming the holder"
    );
    assert_eq!(run.conflict(), Some(a));
}

#[test]
fn rebinding_a_seeded_control_frees_the_input_it_used_to_hold() {
    let a = control_at(0);
    let b = control_at(1);
    let mut run = seeded_run(&[(a, Binding::button(0))]);
    let moved = tap(&mut run, 0x135, 0.0);
    assert!(matches!(moved, Outcome::Recorded { control, .. } if control == a));
    let taken = tap(&mut run, 0x130, TAP + AFTER_GAP);
    assert!(
        matches!(taken, Outcome::Recorded { control, .. } if control == b),
        "button 0 belonged to A a moment ago and must be free now, not {taken:?}"
    );
    assert_eq!(run.bindings()[&a].index, 5);
    assert_eq!(run.bindings()[&b].index, 0);
}

#[test]
fn an_early_finish_on_a_seeded_run_leaves_a_whole_mapping() {
    let stored: Vec<(Control, Binding)> = layout::get("snes")
        .controls
        .iter()
        .enumerate()
        .take(6)
        .map(|(index, control)| (control.canonical, Binding::button(index as i32)))
        .collect();
    let mut run = seeded_run(&stored);
    assert!(tap(&mut run, 0x137, 0.0).advanced(), "A moves to button 7");
    let outcome = last(play(
        &mut run,
        &[
            (Event::key(0x138, 1), 1.0),
            (Event::key(0x138, 0), 1.0 + FINISH_HOLD_SECONDS),
        ],
    ));
    assert_eq!(outcome, Outcome::Finished);
    assert_eq!(run.bindings().len(), 6, "one moved, five kept, none lost");
    assert_eq!(run.bindings()[&control_at(0)].index, 7);
    for (control, binding) in &stored[1..] {
        assert_eq!(run.bindings()[control], *binding);
    }
}

#[test]
fn a_skip_on_a_seeded_run_leaves_that_control_as_it_was() {
    let a = control_at(0);
    let mut run = seeded_run(&[(a, Binding::button(3))]);
    assert_eq!(
        hold(&mut run, SKIP_BUTTON, 0.0),
        Outcome::Skipped { control: a }
    );
    assert_eq!(run.bindings()[&a], Binding::button(3));
}

#[test]
fn seeded_hats_and_axes_are_guarded_too() {
    let first_dpad = first_dpad();
    let up = layout::get("snes").controls[first_dpad].canonical;
    let mut run = seeded_run(&[
        (up, Binding::hat(0, HAT_UP)),
        (control_at(0), Binding::axis(0, 1)),
    ]);
    let clock = skip_to(&mut run, first_dpad + 1);
    assert_eq!(
        run.feed(Event::abs(ABS_HAT0Y, -1), clock),
        Outcome::Refused {
            claim: Claim::Hat {
                index: 0,
                value: HAT_UP
            },
            held_by: up,
        }
    );
    run.feed(Event::abs(ABS_HAT0Y, 0), clock + 0.1);
    assert_eq!(
        run.feed(Event::abs(ABS_X, 100), clock + 0.2),
        Outcome::Refused {
            claim: Claim::Axis {
                code: ABS_X,
                sign: 1
            },
            held_by: control_at(0),
        },
        "the stored axis binding is resolved back to its ABS code"
    );
}

#[test]
fn a_stored_binding_the_pad_no_longer_has_is_kept_but_guards_nothing() {
    let a = control_at(0);
    let mut run = seeded_run(&[(a, Binding::button(40))]);
    assert_eq!(run.bindings()[&a], Binding::button(40));
    let clock = skip_to(&mut run, 1);
    assert!(
        tap(&mut run, 0x130, clock).advanced(),
        "no input on this pad answers to index 40, so nothing is refused"
    );
}

#[test]
fn an_empty_seed_is_the_run_as_it_always_was() {
    let mut run = seeded_run(&[]);
    assert!(run.bindings().is_empty());
    assert!(tap(&mut run, 0x130, 0.0).advanced());
    assert_eq!(run.index(), 1);
}

mod describing_presses {
    use std::collections::{BTreeMap, BTreeSet};

    use padmap_core::binding::BindingKind;
    use padmap_core::capture::{Event, MappingRun};
    use padmap_core::layout;
    use padmap_core::sdl::AxisSpan;

    fn run() -> MappingRun {
        let mut axes = BTreeMap::new();
        axes.insert(0x00, AxisSpan::new(0, 255, 128)); // ABS_X, ordinal 0
        axes.insert(0x01, AxisSpan::new(0, 255, 128)); // ABS_Y, ordinal 1
        axes.insert(0x10, AxisSpan::new(-1, 1, 0)); // hat, not an axis ordinal
        axes.insert(0x11, AxisSpan::new(-1, 1, 0));
        MappingRun::new(
            1,
            layout::get("snes"),
            vec![0x130, 0x131, 0x133, 0x134],
            "console:snes".to_owned(),
            axes,
            BTreeSet::new(),
        )
    }

    #[test]
    fn a_button_is_its_sdl_ordinal_and_one_or_zero() {
        let run = run();
        let down = run.describe(Event::key(0x133, 1)).expect("known");
        assert_eq!(
            (down.kind, down.index, down.value),
            (BindingKind::Button, 2, 1.0)
        );
        let up = run.describe(Event::key(0x133, 0)).expect("known");
        assert_eq!(up.value, 0.0);
        assert!(
            run.describe(Event::key(0x1ff, 1)).is_none(),
            "not on this pad"
        );
    }

    #[test]
    fn a_hat_is_a_direction_bit_and_centred_is_zero() {
        let run = run();
        assert_eq!(
            run.describe(Event::abs(0x11, -1)).map(|p| p.value),
            Some(1.0),
            "up"
        );
        assert_eq!(
            run.describe(Event::abs(0x10, 1)).map(|p| p.value),
            Some(2.0),
            "right"
        );
        assert_eq!(
            run.describe(Event::abs(0x11, 1)).map(|p| p.value),
            Some(4.0),
            "down"
        );
        assert_eq!(
            run.describe(Event::abs(0x10, -1)).map(|p| p.value),
            Some(8.0),
            "left"
        );
        let centred = run.describe(Event::abs(0x10, 0)).expect("hat");
        assert_eq!(
            (centred.kind, centred.index, centred.value),
            (BindingKind::Hat, 0, 0.0)
        );
    }

    #[test]
    fn an_axis_is_its_ordinal_skipping_hats_and_a_deflection() {
        let run = run();
        let full = run.describe(Event::abs(0x01, 255)).expect("axis");
        assert_eq!((full.kind, full.index), (BindingKind::Axis, 1));
        assert!((full.value - 1.0).abs() < 0.01, "{}", full.value);
        let rest = run.describe(Event::abs(0x00, 128)).expect("axis");
        assert!(rest.value.abs() < 0.01, "{}", rest.value);
    }
}
