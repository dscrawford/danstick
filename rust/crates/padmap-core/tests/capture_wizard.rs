//! The capture wizard, driven the only way it can be: from the pad.
//!
//! The daemon holds EVIOCGRAB for the whole session and republishing is
//! stopped, so the front-end receives no controller input at all while any of
//! this runs. Every gesture here therefore has to be recognisable by the daemon
//! with *nothing mapped yet* -- no prompt may name a button, because no button
//! has a name until the run that is asking finishes.
//!
//! What follows is almost entirely about refusal. A pad streams axis noise
//! continuously, an analogue trigger reports a resting value that is not zero,
//! a d-pad wired to an analogue axis springs back through centre far enough to
//! read as a deliberate push the other way, and the button someone pressed to
//! reach this screen is usually still travelling when the first prompt appears.
//! Each of those filled several controls in with one accidental input on real
//! hardware; each has a guard, and each guard has a test below carrying the
//! wound it came from.
//!
//! `MappingRun::index` is private, so a test that needs to start partway
//! through a layout gets there by holding a button past the skip threshold --
//! which is what the user would do, and keeps these tests honest about what is
//! reachable through the public API.

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

// ---------------------------------------------------------------------------
// Scripting
// ---------------------------------------------------------------------------

/// How long an ordinary press lasts. Well under [`SKIP_HOLD_SECONDS`].
const TAP: f64 = 0.05;

/// Long enough that the gap opened by the previous capture has closed.
const AFTER_GAP: f64 = CAPTURE_GAP_SECONDS + 0.01;

/// A key code no test puts in a pad's `keys` list, so holding it can only ever
/// mean "skip" and never accidentally record a binding.
const SKIP_BUTTON: u16 = 0x13f;

/// Drive a scripted `(event, clock reading)` sequence and return what each one
/// did, so a test reads as a scenario rather than as a column of calls.
///
/// The clock is a parameter rather than a field precisely so this is possible:
/// the interesting decisions here are all "how long ago", and a test that had
/// to sleep for them would be both slow and flaky.
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

/// Press and release inside the skip threshold: the ordinary answer to a
/// prompt. Binding happens on the *release*, because how long a button was held
/// is what separates "this is the button" from "skip this control".
fn tap(run: &mut MappingRun, code: u16, at: f64) -> Outcome {
    last(play(
        run,
        &[(Event::key(code, 1), at), (Event::key(code, 0), at + TAP)],
    ))
}

/// Press and hold past the skip threshold.
fn hold(run: &mut MappingRun, code: u16, at: f64) -> Outcome {
    last(play(
        run,
        &[
            (Event::key(code, 1), at),
            (Event::key(code, 0), at + SKIP_HOLD_SECONDS + 0.01),
        ],
    ))
}

/// Walk the run forward to `target` by skipping, and answer with the clock
/// reading at which it is free to accept again.
///
/// `index` is private and deliberately so; a skip is the only way in from
/// outside the crate, and it is also what a user with a pad that lacks the
/// first few controls actually does.
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

// ---------------------------------------------------------------------------
// Pads
// ---------------------------------------------------------------------------

/// A stick, scaled so a raw value *is* its deflection in percent: half the
/// declared range is 100, and rest is the middle.
fn stick() -> AxisSpan {
    AxisSpan::new(-100, 100, 0)
}

/// An analogue trigger, which rests at its *minimum* rather than in the middle.
/// The Mayflash GameCube adapter reports exactly this shape, and measuring from
/// the midpoint instead made its L and R unusable.
fn trigger() -> AxisSpan {
    AxisSpan::new(0, 255, 0)
}

fn axes(entries: &[(u16, AxisSpan)]) -> BTreeMap<u16, AxisSpan> {
    entries.iter().copied().collect()
}

/// Twelve ordinary joystick buttons, one per SNES control, all at or above
/// BTN_JOYSTICK so SDL and RetroArch number them identically.
fn joystick_keys() -> Vec<u16> {
    (0x130..0x13c).collect()
}

fn snes_run(keys: Vec<u16>, axes: BTreeMap<u16, AxisSpan>, held: BTreeSet<u16>) -> MappingRun {
    MappingRun::new(1, layout::get("snes"), keys, String::new(), axes, held)
}

/// A plain run over the shortest shipped layout, answering with buttons.
fn run() -> MappingRun {
    snes_run(joystick_keys(), BTreeMap::new(), BTreeSet::new())
}

/// Where the first d-pad control sits. A d-pad prompt is the one that accepts a
/// hat or an ordinary axis push; a face-button prompt is far stricter.
fn first_dpad() -> usize {
    layout::get("snes")
        .controls
        .iter()
        .position(|control| control.kind == "dpad")
        .expect("the SNES layout has a d-pad")
}

/// A layout with nothing to ask about, which no shipped file is -- built here
/// because "the run is over before it began" still has to not panic.
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

// ---------------------------------------------------------------------------
// MappingRun: the happy path
// ---------------------------------------------------------------------------

#[test]
fn a_tap_records_the_current_control_and_moves_on() {
    let mut run = run();
    let first = run.current().expect("a first prompt");
    assert_eq!(run.index(), 0);
    assert_eq!(run.total(), 12);

    // The press alone does nothing: the release is where the decision is made.
    assert_eq!(run.feed(Event::key(0x130, 1), 0.0), Outcome::Ignored);
    let outcome = run.feed(Event::key(0x130, 0), TAP);

    let (control, binding) = recorded(&outcome);
    assert_eq!(control, first);
    assert!(outcome.advanced());
    // Both numberings are stored at capture time. Recomputing RetroArch's at
    // emission would need the pad's key list to still be around, and would
    // silently shift every binding on a pad carrying sub-0x120 codes.
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
        // Buttons are numbered by position among the pad's sorted key codes,
        // not by the code itself.
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

    // A pad does not stop reporting because the wizard is done, and whatever
    // arrives next must not land on a control that no longer exists.
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

// ---------------------------------------------------------------------------
// MappingRun: settling
// ---------------------------------------------------------------------------

#[test]
fn a_press_held_from_before_the_run_cannot_answer_the_first_prompt() {
    // The press that opened the wizard is usually still travelling when the
    // first prompt appears. Its release must clear the hold and nothing else,
    // or the first control is filled in by the gesture that asked for it.
    let mut run = snes_run(joystick_keys(), BTreeMap::new(), [0x130].into());
    assert!(run.settling());

    assert_eq!(run.feed(Event::key(0x130, 0), 0.1), Outcome::Ignored);

    assert_eq!(run.index(), 0, "the opening press answered a prompt");
    assert!(run.bindings().is_empty());
    assert!(!run.settling());
}

#[test]
fn settling_lasts_until_every_opening_button_is_released() {
    // Two buttons down at the start is ordinary: the slot is claimed with one
    // hand while the other is already on the pad. Clearing on the first release
    // would eat one real press.
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
    // Not only the opening button is suspect. Anything pressed while the pad is
    // still being let go of is part of the same gesture.
    let mut run = snes_run(joystick_keys(), BTreeMap::new(), [0x130].into());

    assert_eq!(tap(&mut run, 0x131, 0.1), Outcome::Ignored);

    assert_eq!(run.index(), 0);
    assert!(run.bindings().is_empty());
}

#[test]
fn a_settling_run_refuses_an_axis_too() {
    // Pushed all the way to the stop, which would otherwise be enough to answer
    // even a face-button prompt.
    let mut run = snes_run(Vec::new(), axes(&[(0x02, stick())]), [0x130].into());

    assert_eq!(run.feed(Event::abs(0x02, 100), 0.1), Outcome::Ignored);

    assert_eq!(run.index(), 0);
}

#[test]
fn a_button_pressed_after_the_run_started_is_not_a_settling_button() {
    // Sharing a code with the button that opened the wizard must not make a
    // later, deliberate press invisible -- on a pad with few buttons that same
    // button is very often the answer to the first prompt too.
    let mut run = snes_run(joystick_keys(), BTreeMap::new(), [0x130].into());
    let first = run.current().expect("a first prompt");

    run.feed(Event::key(0x130, 0), 0.1);
    assert!(!run.settling());
    let outcome = tap(&mut run, 0x130, 0.2);

    let (control, _) = recorded(&outcome);
    assert_eq!(control, first);
    assert_eq!(run.index(), 1);
}

// ---------------------------------------------------------------------------
// MappingRun: skip
// ---------------------------------------------------------------------------

#[test]
fn a_hold_past_the_skip_threshold_skips_the_control_instead_of_recording_it() {
    // Skip cannot be a *particular* button: nothing is mapped yet and the
    // front-end sees no controller input at all, so "press Select to skip"
    // could never have worked. Holding any button needs no prior mapping.
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

    // The comparison is `>=`. A threshold that needed to be exceeded would make
    // a hold timed exactly right bind the control it was meant to pass over.
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
    // The button released after a skip-hold must not answer the control the
    // skip moved on to; it is one continuous gesture from the pad's side.
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
    // Blocked, on the same terms as a capture.
    assert_eq!(tap(&mut run, 0x130, 0.1), Outcome::Ignored);
}

// ---------------------------------------------------------------------------
// MappingRun: the capture gap
// ---------------------------------------------------------------------------

#[test]
fn nothing_is_accepted_during_the_gap_even_on_a_different_code() {
    // "There is no delay between buttons being set -- if I hold the d-pad too
    // long it registers as two." Recording used to advance instantly, leaving
    // whatever the user was still holding pointed at a fresh prompt. The
    // per-axis arming rule catches one axis springing back; this catches the
    // general case, including an input arriving on a different code entirely.
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
    // Answer the face-button prompt with a push to the stop, then oscillate
    // while still holding it -- which is what an analogue axis does.
    assert!(run.feed(Event::abs(0x02, 100), 0.0).advanced());

    assert_eq!(run.feed(Event::abs(0x02, -100), 0.05), Outcome::Ignored);
    assert_eq!(run.feed(Event::abs(ABS_HAT0X, 1), 0.10), Outcome::Ignored);

    assert_eq!(run.index(), 1);
}

#[test]
fn a_button_pressed_inside_the_gap_and_released_after_it_records_nothing() {
    // The press never happened as far as the run is concerned, so there is no
    // hold duration to judge and nothing to bind. It must still work next time.
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
    // Releases are tracked during the gap even though nothing is accepted,
    // "so a button held across the gap is not still considered down". Without
    // it the press stays on the books, and a stray second release -- which a
    // pad that reconnects mid-run really does produce -- binds a control from
    // a press nobody made.
    let mut run = run();
    play(
        &mut run,
        &[
            (Event::key(0x131, 1), 0.00), // down before anything is captured
            (Event::key(0x130, 1), 0.10),
            (Event::key(0x130, 0), 0.15), // captures; blocks until 0.50
            (Event::key(0x131, 0), 0.20), // released inside the gap
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
    // A release takes about a tenth of the time the gap lasts, so this is where
    // nearly every one of them lands. Dropping it leaves the axis disarmed with
    // nothing left to re-arm it: a trigger settles at rest and stops reporting
    // entirely, and the wizard then ignores it for good. That is the "pressing
    // R causes it to stay stuck in the interface" report.
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    assert!(
        run.feed(Event::abs(ABS_X, -100), clock).advanced(),
        "full left"
    );
    run.feed(Event::abs(ABS_X, 0), clock + 0.05); // the release, inside the gap

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
    run.feed(Event::abs(ABS_HAT0X, 0), clock + 0.05); // released inside the gap

    assert!(
        run.feed(Event::abs(ABS_HAT0X, 1), clock + AFTER_GAP)
            .advanced(),
        "a cheap adapter reports the opposite direction on release; the hat must still re-arm"
    );
}

// ---------------------------------------------------------------------------
// MappingRun: autorepeat
// ---------------------------------------------------------------------------

#[test]
fn an_autorepeat_does_not_walk_the_wizard() {
    // value 2 is the kernel repeating a key that is still down. Binding on the
    // release rather than the press is what made this stop needing a special
    // case; it is pinned because the special case is easy to reintroduce.
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

// ---------------------------------------------------------------------------
// MappingRun: double claims
// ---------------------------------------------------------------------------

#[test]
fn one_button_cannot_answer_two_prompts() {
    // The refusal is invisible from the outside -- the press simply does
    // nothing, which looks exactly like a dead button or a hung wizard. Naming
    // the control that holds it is the only actionable remedy: restart and
    // answer the earlier prompt differently.
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
    run.feed(Event::abs(ABS_HAT0X, 0), clock + 0.05); // re-arms inside the gap
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
    // This bites hardest where a pad has fewer inputs than the layout has
    // controls: mapping an N64 pad to the GameCube layout, the C directions are
    // the only spare axis halves.
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());
    let first = run.current().expect("a d-pad prompt");

    run.feed(Event::abs(ABS_X, -100), clock);
    run.feed(Event::abs(ABS_X, 0), clock + 0.05); // re-arms inside the gap
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
    // One physical stick answers two prompts, and must: left and right are
    // different controls even though they are one axis.
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
    // A stale conflict pinned under a later control names a clash that is not
    // happening, and the front-end would go on showing it.
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

// ---------------------------------------------------------------------------
// MappingRun: axis arming
// ---------------------------------------------------------------------------

#[test]
fn an_axis_springing_back_through_centre_does_not_answer_the_next_prompt() {
    // Reported from a real N64 adapter: pressing left filled in both left *and*
    // right. The d-pad there is an analogue axis; releasing it lets the stick
    // spring back through centre and overshoot far enough to pass the capture
    // threshold in the opposite direction, which is indistinguishable event for
    // event from a deliberate push the other way. The threshold alone cannot
    // fix it -- the overshoot is a genuine full deflection. What makes it not a
    // press is that the axis never went back to rest in between.
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());
    let up = run.current().expect("a d-pad prompt");

    let left = recorded(&run.feed(Event::abs(ABS_X, -100), clock));
    assert_eq!(left.0, up);
    let down = run.current().expect("the next d-pad prompt");

    // The overshoot, inside the gap and then after it. Neither may answer,
    // because the axis has not been back to rest.
    assert_eq!(
        run.feed(Event::abs(ABS_X, 100), clock + 0.05),
        Outcome::Ignored
    );
    assert_eq!(
        run.feed(Event::abs(ABS_X, 100), clock + 1.0),
        Outcome::Ignored
    );
    assert_eq!(run.current(), Some(down));

    // Settle, then a deliberate push the other way, which must be taken.
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
    // Re-arming needs the axis back inside AXIS_RELEASE of rest. A stick that
    // stops 40% out has not been let go of, and the events it is still sending
    // are the tail of the push that already answered a prompt.
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
    // ...and all the way back does re-arm it.
    run.feed(Event::abs(ABS_X, 0), clock + 1.0);
    assert!(run.feed(Event::abs(ABS_X, 100), clock + 1.1).advanced());
}

#[test]
fn an_axis_never_touched_is_armed_from_the_start() {
    // Absent means armed. An axis that has to be pushed and released once
    // before it counts would lose the first answer of every run.
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    assert!(run.feed(Event::abs(ABS_X, -100), clock).advanced());
}

#[test]
fn a_trigger_resting_at_its_minimum_still_re_arms_after_it_is_let_go() {
    // The whole GameCube L/R story in one sequence: rest reads 0, a press reads
    // about 2.0, and the release passes back through rest so the trigger
    // re-arms. Measured from the midpoint instead, the untouched trigger reads
    // fully deflected and can never come back near enough to be re-armed --
    // after one press the trigger was dead and every later press was dropped in
    // silence. That is the "stuck".
    let triggers = axes(&[(0x02, trigger()), (0x05, trigger())]);
    let mut run = snes_run(Vec::new(), triggers, BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    let (_, first) = recorded(&run.feed(Event::abs(0x02, 255), clock));
    run.feed(Event::abs(0x02, 0), clock + 0.05); // settles back to rest
    let again = run.feed(Event::abs(0x02, 255), clock + AFTER_GAP);

    assert_eq!(first, Binding::axis(0, 1), "a trigger only travels one way");
    // A refusal is proof it re-armed: a disarmed axis never reaches the claim
    // check at all, and answers Ignored.
    assert!(
        matches!(again, Outcome::Refused { .. }),
        "the trigger went dead after one press: {again:?}"
    );
    // The *other* trigger is a different claim and answers the next prompt.
    let (_, second) = recorded(&run.feed(Event::abs(0x05, 255), clock + AFTER_GAP + 0.1));
    assert_eq!(second, Binding::axis(1, 1));
}

// ---------------------------------------------------------------------------
// MappingRun: an axis answering a face button
// ---------------------------------------------------------------------------

#[test]
fn a_resting_axis_reports_nothing_at_all() {
    // A resting axis streams events continuously. Reporting them would bury the
    // session, and the front-end would flash a refusal with nothing touched.
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
    // Past AXIS_THRESHOLD it is deliberate enough to be worth telling the user
    // about, and short of AXIS_AS_BUTTON_THRESHOLD it is not enough to bind.
    // Without the message, pressing C-up for Y did nothing at all: no log line
    // and no change on screen.
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
    // An N64 pad mapped against the GameCube layout has to answer X and Y from
    // its C cluster, which that pad reports as axes. A flat refusal made those
    // prompts unanswerable.
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
    // Binding a stick to a face button by accident is expensive in a way no
    // other misbinding is: every later stick movement presses that button for
    // the rest of the session. That is how a mapping ended up with cancel on
    // `-a3`.
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
    // A device sending ABS events while declaring no axes at all would
    // otherwise fill face buttons in from noise.
    let mut run = snes_run(Vec::new(), BTreeMap::new(), BTreeSet::new());

    assert_eq!(run.feed(Event::abs(0x02, 32767), 0.0), Outcome::Ignored);

    assert_eq!(run.index(), 0);
}

#[test]
fn a_hat_never_answers_a_face_button_prompt_at_any_value() {
    // A d-pad direction answering a face button is a mistake in every case
    // anyone has had, and a device reporting a hat it does not have would
    // otherwise fill face buttons in from noise.
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
    // Only a *button* prompt demands a push to the stop. For a shoulder or a
    // d-pad an axis is the expected answer and the only risk is drift.
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
    // Generous on purpose: the alternative -- catching drift -- silently binds
    // a control to a stick that merely leans.
    let mut run = snes_run(Vec::new(), axes(&[(ABS_X, stick())]), BTreeSet::new());
    let clock = skip_to(&mut run, first_dpad());

    let outcome = run.feed(
        Event::abs(ABS_X, (AXIS_THRESHOLD * 100.0) as i32 - 1),
        clock,
    );

    assert_eq!(outcome, Outcome::Ignored);
}

// ---------------------------------------------------------------------------
// deflection
// ---------------------------------------------------------------------------

#[test]
fn deflection_is_measured_from_rest_not_from_the_declared_middle() {
    // "When I registered a GameCube controller, pressing R causes it to stay
    // stuck in the interface." The untouched triggers on that adapter read 81%
    // deflected when measured from the midpoint, so the first event of a press
    // captured the direction the trigger was travelling *away* from, a resting
    // report could answer a prompt with nothing touched, and the axis could
    // never re-arm.
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
    // What the midpoint measurement would have said about an untouched trigger.
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
    // Reading high is harmless -- every threshold here is a floor. Normalising
    // by the travel available in the direction of movement would instead make
    // an off-centre stick need a bigger push on its long side than its short.
    let lopsided = AxisSpan::new(0, 100, 20);

    assert_eq!(deflection(lopsided, 100), 1.6);
    assert_eq!(deflection(lopsided, 0), -0.4);
}

#[test]
fn deflection_of_a_degenerate_span_is_zero_rather_than_a_division_by_zero() {
    // A span comes off a device, and a device can say anything.
    assert_eq!(deflection(AxisSpan::new(0, 0, 0), 50), 0.0);
    assert_eq!(deflection(AxisSpan::new(10, 5, 7), 50), 0.0);
    assert_eq!(deflection(AxisSpan::new(-1, -1, -1), 0), 0.0);
}

// ---------------------------------------------------------------------------
// MappingRun: hats and axis numbering
// ---------------------------------------------------------------------------

#[test]
fn each_hat_direction_records_the_sdl_bit_that_names_it() {
    // Y is positive downwards, which is the one that is easy to get backwards
    // and produces a d-pad that works upside down in every game.
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
    // Some pads report a hat as a full-range axis rather than as -1/0/1.
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
    // Both hat codes are the same physical hat, and SDL and RetroArch both
    // write it as hat 0 with a direction bit.
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
    // ABS_RZ is code 5 but may be axis 3. Storing the raw code and hoping it is
    // the index works right up to the first pad whose axes are not 0,1,2,...
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
    // Neither consumer counts a hat as an axis, so an axis map that happens to
    // carry hat codes must not shift the real axes along.
    let mut run = snes_run(
        Vec::new(),
        axes(&[(0x00, stick()), (ABS_HAT0X, stick()), (0x05, stick())]),
        BTreeSet::new(),
    );
    let clock = skip_to(&mut run, first_dpad());

    let (_, binding) = recorded(&run.feed(Event::abs(0x05, 100), clock));

    assert_eq!(binding, Binding::axis(1, 1));
}

// ---------------------------------------------------------------------------
// MappingRun: buttons the pad does not report
// ---------------------------------------------------------------------------

#[test]
fn a_key_the_pad_never_declared_records_nothing() {
    // There is no number to give it, and inventing one binds a button that does
    // not exist -- which both consumers accept without complaining.
    let mut run = snes_run(vec![0x130, 0x131], BTreeMap::new(), BTreeSet::new());

    assert_eq!(tap(&mut run, 0x2ff, 0.0), Outcome::Ignored);

    assert_eq!(run.index(), 0);
    assert!(run.bindings().is_empty());
}

#[test]
fn a_key_below_btn_misc_records_as_invisible_to_retroarch() {
    // A combo adapter reporting KEY_A alongside its twelve buttons used to
    // store SDL's index for RetroArch too, binding a button RetroArch's udev
    // driver never enumerates: the control works in the front-end and is dead
    // in every game. SDL still numbers it, and it goes *after* the joystick
    // codes because SDL walks BTN_JOYSTICK..KEY_MAX before 0..BTN_JOYSTICK.
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

// ---------------------------------------------------------------------------
// MappingRun: degenerate runs
// ---------------------------------------------------------------------------

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
    // EV_SYN arrives after every report and must not be mistaken for input.
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
    // A wizard that can be abandoned, restarted, or opened for a different
    // player in between is exactly the shape of thing that loses a value parked
    // elsewhere.
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

// ---------------------------------------------------------------------------
// Claim and Outcome
// ---------------------------------------------------------------------------

#[test]
fn a_claim_describes_itself_in_the_terms_a_log_reader_has_to_match() {
    // Raw evdev codes for buttons and axes, because that is what a `padmon`
    // trace and an `evtest` dump show; the hat is named, since its value is a
    // direction bit and "8" means nothing to anyone.
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
    // A diagonal reads 3 and is not a control, but a log line that raised
    // instead of printing it would be worse than one that prints the number.
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

// ---------------------------------------------------------------------------
// Chooser
// ---------------------------------------------------------------------------

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

/// The chooser's equivalent of [`play`]: a scripted sequence, and whether each
/// event changed anything.
fn play_chooser(chooser: &mut Chooser, script: &[(Event, f64)]) -> Vec<bool> {
    script
        .iter()
        .map(|(event, now)| chooser.feed(*event, *now))
        .collect()
}

#[test]
fn a_chooser_moves_on_the_transition_into_a_direction_only() {
    // A held stick would otherwise spin the selection past whatever the user
    // was looking at, and a release would move it back off the entry they had
    // just reached.
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
    // ABS_X and ABS_HAT0X are horizontal on every pad, which is why they can be
    // read with nothing mapped.
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
    // Vertical movement, triggers and the right stick all stream while the
    // picker is open, and none of them chooses anything.
    let mut chooser = chooser(axes(&[(0x01, stick()), (0x05, stick())]), BTreeSet::new());

    assert!(!chooser.feed(Event::abs(0x01, 100), 0.0));
    assert!(!chooser.feed(Event::abs(0x05, -100), 0.1));
    assert!(!chooser.feed(Event::abs(ABS_HAT0Y, 1), 0.2));

    assert_eq!(chooser.index(), 0);
}

#[test]
fn a_chooser_ignores_an_axis_it_has_no_span_for() {
    // Without a span there is no way to say how far "far enough" is, and
    // guessing from the raw value binds the picker to one pad's scale.
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
    // The strip is a loop. Walking off the end and stopping would make the last
    // console unreachable to anyone who pushed the wrong way first.
    let count = layout_options(&BTreeSet::new()).len();
    let mut chooser = chooser(BTreeMap::new(), BTreeSet::new());

    assert!(chooser.move_by(-1));
    assert_eq!(chooser.index(), count - 1);
    assert!(chooser.move_by(1));
    assert_eq!(chooser.index(), 0);

    // ...and from the pad, which is the only way a user reaches it.
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
    // Reachable: game_scope_options answers an empty list for a game whose
    // console is unknown, and the daemon must not take the picker down with it.
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
    // The button that claimed the slot is often still travelling when this
    // appears, and a picker that accepts the first press anyone makes is a
    // picker nobody gets to use.
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
    // The daemon is already building the run the answer named; a later push
    // would change the selection under it.
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
    // Same reason as the wizard's: the press that opened the picker is still
    // held, and it is a *hold*, so it would confirm the very first entry.
    let mut chooser = chooser(axes(&[(ABS_X, stick())]), [0x130].into());
    assert!(chooser.settling());

    // A long hold on another button while settling must not confirm...
    chooser.feed(Event::key(0x131, 1), 0.0);
    assert!(!chooser.feed(Event::key(0x131, 0), 1.0));
    assert!(!chooser.confirmed());
    // ...nor may a stick push move the selection.
    assert!(!chooser.feed(Event::abs(ABS_X, 100), 1.1));
    assert_eq!(chooser.index(), 0);

    // The opening press is released: that clears the hold and nothing else.
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
    // Deliberately *not* the wizard's rule. An uncalibrated stick can rest at
    // 36% deflection -- measured on the N64 adapter here -- which never comes
    // back inside AXIS_RELEASE, and a picker that stops responding after one
    // move is worse than one that occasionally moves twice. A wrong step here
    // costs a nudge back; a wrong step in the wizard costs a mis-recorded
    // binding.
    let resting = 40; // between AXIS_RELEASE (0.30) and AXIS_THRESHOLD (0.55)
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
    // The other half of the pair above: the same 40% rest leaves the wizard's
    // axis disarmed, and that is correct there, because the cost of a wrong
    // step is a binding nobody can see is wrong.
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
    // Sent to the front-end rather than inferred: a theme guessing from the
    // option ids would be a third place that has to know what a scope string
    // looks like.
    assert_eq!(ChoiceKind::Layout.as_str(), "layout");
    assert_eq!(ChoiceKind::Scope.as_str(), "scope");

    let chooser = chooser(BTreeMap::new(), BTreeSet::new());
    assert_eq!(chooser.kind, ChoiceKind::Layout);
    assert_eq!(chooser.title, "Which controller is this?");
    assert_eq!(chooser.player, 1);
}

// ---------------------------------------------------------------------------
// layout_options
// ---------------------------------------------------------------------------

#[test]
fn layout_options_offers_every_shipped_layout_in_catalogue_order() {
    // The strip is built here rather than in the theme: a hardcoded list there
    // would be a second copy of this table with nothing to notice when it fell
    // behind, and a console added on this side would simply never appear.
    let options = layout_options(&BTreeSet::new());
    let catalogue = layout::all();

    assert_eq!(options.len(), catalogue.len());
    for (option, shipped) in options.iter().zip(catalogue) {
        assert_eq!(option.id, shipped.id);
        assert_eq!(option.label, shipped.label);
        // The layout picker draws the thing it names; the scope picker is the
        // one where id and layout differ.
        assert_eq!(option.layout, option.id);
        assert!(!option.mapped);
    }
}

#[test]
fn layout_options_marks_the_layouts_already_captured() {
    // Re-mapping a layout replaces what is there, and without a mark there is
    // no way to tell which ones that would destroy.
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

// ---------------------------------------------------------------------------
// game_scope_options
// ---------------------------------------------------------------------------

#[test]
fn a_game_scope_strip_offers_the_console_first() {
    // Console first: a pad that needs remapping for one N64 game usually needs
    // it for all of them, and the first entry is the one a hurried user
    // confirms.
    let options = game_scope_options("n64", "n64/goldeneye", "GoldenEye 007", &BTreeSet::new());

    assert_eq!(options.len(), 2);
    assert_eq!(options[0].id, "console:n64");
    assert_eq!(options[0].label, "Nintendo 64 games");
    assert_eq!(options[1].id, "game:n64/goldeneye");
    assert_eq!(options[1].label, "GoldenEye 007");
}

#[test]
fn both_game_scope_entries_draw_the_consoles_pad() {
    // A mapping for one N64 game is still a mapping of the N64 control set, and
    // the pad shown has to be the pad the wizard then asks about.
    let options = game_scope_options("n64", "n64/goldeneye", "GoldenEye 007", &BTreeSet::new());

    assert_eq!(options[0].layout, "n64");
    assert_eq!(options[1].layout, "n64");
}

#[test]
fn a_game_with_no_console_is_offered_nothing_rather_than_the_generic_pad() {
    // The exporter writes a game key for every game but omits the console when
    // the collection's core is not one padmap recognises, so a front-end really
    // can send a key with no console. Offering it would draw the generic pad
    // beside the entry and then walk whatever the pad's icon guesses -- the
    // strip promising one controller while the wizard asks about another.
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
    // Better a key than a blank strip entry nobody can aim at.
    let options = game_scope_options("snes", "snes/smw", "", &BTreeSet::new());

    assert_eq!(options[1].label, "snes/smw");
}

#[test]
fn a_console_label_is_used_in_place_of_the_controllers_name() {
    // "Arcade stick games" describes the controller; "Arcade games" is what the
    // scope actually covers.
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

// ---------------------------------------------------------------------------
// scope_options
// ---------------------------------------------------------------------------

#[test]
fn a_scope_strip_offers_any_game_first_then_every_console() {
    let options = scope_options(&BTreeSet::new(), "gamecube", &[]);
    let consoles = layout::consoles();

    assert_eq!(options[0].id, scope::UNIVERSAL);
    assert_eq!(options[0].label, "Any game");
    // Drawn beside "any game" only so the strip has a picture there; it is not
    // a promise about which layout the wizard will walk, because that entry
    // leads to the layout picker.
    assert_eq!(options[0].layout, "gamecube");
    assert_eq!(options.len(), 1 + consoles.len());
    for (option, console) in options[1..].iter().zip(&consoles) {
        assert_eq!(option.id, scope::console(console));
        assert_eq!(option.layout, *console);
    }
}

#[test]
fn a_scope_strip_does_not_offer_the_generic_layout_as_a_console() {
    // "My pad, when playing generic games" is not a thing anyone can mean, and
    // the scope would never resolve because no core ever reports it.
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
    // Newest first, and several rather than only the newest: someone who has
    // since started something else would otherwise find the game they actually
    // wanted to fix no longer on offer.
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
    // padmap records every launch, including one whose core cannot be named --
    // deliberately, since a launch with an unknown core is exactly the one
    // whose controls are most likely to have felt wrong. Offering it is what
    // ends with cancel bound to an axis.
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
    // There is nothing to file a mapping under, so the entry could only ever be
    // decoration.
    let recent = vec![("n64".to_owned(), String::new(), "Nameless".to_owned())];

    let options = scope_options(&BTreeSet::new(), "generic", &recent);

    assert_eq!(options.len(), 1 + layout::consoles().len());
}

#[test]
fn a_scope_strip_does_not_offer_the_same_game_twice() {
    // The recent list is a launch history, so the game someone played three
    // times in a row is in it three times.
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
    // Found by this test in its first form, when the Rust marked the key as
    // seen *before* testing the console and so dropped the entry entirely.
    // Reachable: the launcher records every launch, including one whose core
    // it cannot name, so the same ROM really does appear twice -- once with a
    // console and once without.
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
    // Re-mapping a scope replaces it, and without a mark there is no way to
    // tell which ones that would destroy.
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
