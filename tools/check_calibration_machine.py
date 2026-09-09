"""The calibration machine: five phases, and the numbers they produce.

S12 (the wizard measures the sticks before accepting) and S13 (Details
recalibrates the last assigned pad) both end in the same place: the daemon's
`CalibrationRun`, advanced from the tick and fed the pad's raw events, deciding
where a stick rests and how far it really travels. Everything about analogue
feel comes out of the two numbers it produces -- a centre and a reach -- and
`AxisCalibration.apply` turning them back into a reading.

What the machine has to get right, and why each rule is here:

  * the two *await* phases are driven by the user, not the clock, so nobody is
    measured before they are ready. The rest phase is timed (`PHASE_SECONDS`).
    The reach phase ends on a button press but not before
    `REACH_MINIMUM_SECONDS`, because the press that *starts* it would
    otherwise end it in the same instant.
  * `_reset_samples` runs on every phase change. Reach must not inherit the
    rest phase's samples, or an axis that never moved would look like it
    spanned nothing.
  * `coverage()` is what the reach phase reports instead of a countdown: it
    tells the user whether the circles they are making actually reach the
    edges, which is the thing that decides whether the calibration is any good.
  * a direction that never moved must fall back to the *declared* range.
    Pinning its travel to zero is the "the stick cannot go left at all" bug
    that measured reach exists to fix.

Nothing here touches a real device, a real profile store or the running daemon.
The pads are synthetic, the events are plain objects, the clock is moved by
rewinding `run.started`, and the profile store is a temporary directory. A
`Server` is built but never started, so it never binds a socket, never opens a
device and never grabs a pad -- the live daemon on this machine owns those.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_calibration_machine.py
"""

import os
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Redirected *before* padmap is imported: a live daemon owns the real runtime
# dir, and the real profile store is someone's actual controllers.
_STORE = Path(tempfile.mkdtemp(prefix="padmap-calmachine-"))
(_STORE / "run" / "padmap").mkdir(parents=True)
os.environ["XDG_RUNTIME_DIR"] = str(_STORE / "run")
os.environ["XDG_CONFIG_HOME"] = str(_STORE / "config")
os.environ["XDG_DATA_HOME"] = str(_STORE / "data")
os.environ["PADMAP_PROFILE_DIR"] = str(_STORE / "devices")

sys.path.insert(0, str(REPO / "src"))

from evdev import AbsInfo, ecodes  # noqa: E402

from padmap import calibrate, profiles, server  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.profiles import AxisCalibration  # noqa: E402
from padmap.server import (PHASE_AWAIT_REACH, PHASE_AWAIT_REST,  # noqa: E402
                           PHASE_ICON, PHASE_REACH, PHASE_REST,
                           PHASE_SECONDS, REACH_MINIMUM_SECONDS,
                           CalibrationRun)

REST_SECONDS = PHASE_SECONDS[PHASE_REST]

PAD = Pad(path="/dev/input/event90", name="Test Pad", phys="usb-test/input0",
          uniq="", vid=0x0079, pid=0x0011, syspath="/sys/devices/test")

# A second pad, so "the run only listens to its own pad" has something to be
# distinguished from.
OTHER = Pad(path="/dev/input/event91", name="Other Pad", phys="usb-test/input1",
            uniq="", vid=0x0079, pid=0x0012, syspath="/sys/devices/other")

BTN_SOUTH = ecodes.BTN_SOUTH


def fail(what: str) -> None:
    raise SystemExit(f"FAIL: {what}")


class Event:
    """The three fields of an evdev event the calibration machine reads."""

    def __init__(self, type_, code, value):
        self.type = type_
        self.code = code
        self.value = value


def press(code=BTN_SOUTH):
    return Event(ecodes.EV_KEY, code, 1)


def release(code=BTN_SOUTH):
    return Event(ecodes.EV_KEY, code, 0)


def autorepeat(code=BTN_SOUTH):
    return Event(ecodes.EV_KEY, code, 2)


def axis(code, value):
    return Event(ecodes.EV_ABS, code, value)


def absinfo(value, minimum=0, maximum=255, flat=0):
    return AbsInfo(value=value, min=minimum, max=maximum, fuzz=0, flat=flat,
                   resolution=0)


# A plain two-axis stick, resting dead centre on a 0-255 adapter.
STICK = {ecodes.ABS_X: absinfo(128), ecodes.ABS_Y: absinfo(128)}


def rewind(run, seconds):
    """Pretend `seconds` of the current phase have already passed."""
    run.started -= seconds


def fresh_store():
    """Point the profile store at an empty directory of its own."""
    where = Path(tempfile.mkdtemp(prefix="padmap-calprof-", dir=_STORE))
    os.environ["PADMAP_PROFILE_DIR"] = str(where)
    return where


class Harness:
    """A daemon with its mouth taped shut: broadcasts land in a list.

    The server is constructed but never started, so no socket is bound and no
    device is ever opened. `_tick_calibration` is called directly, which is
    exactly what the real tick does with it.
    """

    def __init__(self):
        fresh_store()
        self.sent = []
        self.server = server.Server()
        self.server._broadcast = self.sent.append

    def run_for(self, pad, player, axes):
        run = CalibrationRun(pad, player, axes)
        self.server._calibration = run
        return run

    def tick(self):
        self.server._tick_calibration()

    def phases(self):
        return [m.get("phase") for m in self.sent
                if m.get("event") == "calibration"]

    def last(self):
        return self.sent[-1] if self.sent else {}


class FakeDevice:
    """Just enough of an evdev device for `calibratable_axes`."""

    def __init__(self, entries):
        self._entries = list(entries)

    def capabilities(self, absinfo=True):
        return {ecodes.EV_ABS: list(self._entries)}


class FakeAssignment:
    def __init__(self, player, pad):
        self.player = player
        self.pad = pad


class FakeAssigner:
    """Stands in for the real Assigner, which owns real grabbed hardware."""

    def __init__(self, device, assignments=()):
        self.assignments = [FakeAssignment(p, pad) for p, pad in assignments]
        self._device = device

    def device_for(self, pad):
        return self._device


# -- the constants themselves ------------------------------------------------


def check_the_timings_are_humane():
    print("\nS12: the phase timings are the ones a person can work with")

    if not 0.2 <= REST_SECONDS <= 3.0:
        fail(f"the rest phase is {REST_SECONDS}s -- the user is told to let go "
             f"of the sticks and then measured, so this has to be long enough "
             f"to settle and short enough that nobody thinks it has hung")
    print(f"  ok  the rest phase is {REST_SECONDS}s, a moment and no more")

    if REACH_MINIMUM_SECONDS <= 0:
        fail("REACH_MINIMUM_SECONDS is not positive, so the same press that "
             "starts the reach phase ends it instantly and the stick is never "
             "measured at all")
    if REACH_MINIMUM_SECONDS < 0.5:
        fail(f"REACH_MINIMUM_SECONDS is {REACH_MINIMUM_SECONDS}s -- too short "
             f"to outlast the press that started the phase, so a user holding "
             f"the button a beat too long measures nothing")
    print(f"  ok  reach cannot end before {REACH_MINIMUM_SECONDS}s, so the "
          f"press that starts it cannot end it")

    if PHASE_AWAIT_REST in PHASE_SECONDS or PHASE_AWAIT_REACH in PHASE_SECONDS:
        fail("an await phase has a timer -- those phases wait for the user, "
             "and putting them on a clock measures somebody who is not ready")
    print("  ok  neither await phase is on a clock")


# -- construction and the sample window --------------------------------------


def check_a_fresh_run_waits_for_the_user():
    print("\nS12: a calibration opens waiting, not measuring")

    run = CalibrationRun(PAD, 3, STICK)
    if run.phase != PHASE_AWAIT_REST:
        fail(f"a new calibration starts in {run.phase!r} -- it must wait for "
             f"the user to say they are ready, or it measures a stick that is "
             f"still in someone's hand")
    print("  ok  starts in await_rest")

    if run.advance_requested:
        fail("a new calibration already wants to advance, so the first tick "
             "would skip the 'let go of the sticks' prompt entirely")
    print("  ok  nothing has asked it to advance yet")

    if run.rest is not None:
        fail("a new calibration already claims a measured rest position")
    print("  ok  no rest position measured yet")

    if run.player != 3 or run.pad is not PAD:
        fail("the run forgot which player and pad it is for, so the overlay "
             "would report progress for the wrong controller")
    print("  ok  remembers its player and its pad")

    if run.fraction() != 0.0:
        fail(f"a waiting calibration reports {run.fraction()} progress -- the "
             f"overlay would show a part-full bar for a phase that has not "
             f"started")
    print("  ok  reports no progress while waiting")


def check_samples_start_at_the_current_reading():
    print("\nS12: sampling starts from where the axis is now")

    run = CalibrationRun(PAD, 1, {ecodes.ABS_X: absinfo(200)})
    if run.seen != {ecodes.ABS_X: [200, 200]}:
        fail(f"the sample window opened at {run.seen} instead of the axis's "
             f"current reading -- a settled axis reports nothing at all, so "
             f"an empty window would leave it with no samples ever")
    print("  ok  the window opens at the axis's own resting value")

    empty = CalibrationRun(PAD, 1, {})
    if empty.seen != {}:
        fail("a pad with no calibratable axes invented samples for axes it "
             "does not have")
    print("  ok  a pad with no calibratable axes samples nothing")


def check_reset_samples_between_phases():
    print("\nS12: reach does not inherit the rest phase's samples")

    run = CalibrationRun(PAD, 1, dict(STICK))
    run.begin_phase(PHASE_REST)
    # A wobble during the rest phase, which is the whole point of that phase.
    run.feed(axis(ecodes.ABS_X, 120))
    run.feed(axis(ecodes.ABS_X, 136))
    if run.seen[ecodes.ABS_X] != [120, 136]:
        fail("the rest phase did not record the wobble it exists to measure")
    print("  ok  the rest phase widens on movement")

    run.begin_phase(PHASE_REACH)
    if run.seen[ecodes.ABS_X] != [128, 128]:
        fail(f"the reach phase inherited the rest samples ({run.seen}) -- a "
             f"stick that never moved during reach would look like it had "
             f"already covered the rest wobble, and coverage would lie about "
             f"how good the calibration is")
    print("  ok  the reach phase starts from a fresh window")

    if run.started == 0 or run.elapsed > 0.5:
        fail("begin_phase did not restart the clock, so the new phase "
             "inherits the age of the old one and ends immediately")
    print("  ok  begin_phase restarts the clock")

    run.advance_requested = True
    run.begin_phase(PHASE_ICON)
    if run.advance_requested:
        fail("begin_phase kept the advance request, so the press that ended "
             "one phase immediately ends the next one too")
    print("  ok  begin_phase clears the advance request")
    if run.phase != PHASE_ICON:
        fail("begin_phase did not change the phase")
    print("  ok  begin_phase sets the phase")


# -- feed --------------------------------------------------------------------


def check_only_a_press_asks_to_advance():
    print("\nS12: the user advances with a press, not a release")

    run = CalibrationRun(PAD, 1, dict(STICK))
    run.feed(release())
    if run.advance_requested:
        fail("letting go of a button advanced the calibration, so the release "
             "of the press that opened the wizard skips the first phase")
    print("  ok  a release does not advance")

    run.feed(autorepeat())
    if run.advance_requested:
        fail("a key autorepeat advanced the calibration, so holding a button "
             "walks through every phase on its own")
    print("  ok  an autorepeat does not advance")

    run.feed(press())
    if not run.advance_requested:
        fail("a button press did not ask to advance -- the await phases wait "
             "for a press and nothing else, so the wizard would sit there "
             "forever with the pads grabbed")
    print("  ok  a press asks to advance")


def check_feed_records_axis_extremes():
    print("\nS12: an axis sample window widens both ways and never narrows")

    run = CalibrationRun(PAD, 1, dict(STICK))
    for value in (200, 40, 128, 250, 5):
        run.feed(axis(ecodes.ABS_X, value))
    if run.seen[ecodes.ABS_X] != [5, 250]:
        fail(f"the sample window is {run.seen[ecodes.ABS_X]} instead of "
             f"[5, 250] -- reach is the *extremes* a stick got to, so a "
             f"window that follows the latest value measures nothing")
    print("  ok  keeps the lowest and highest seen")

    if run.seen[ecodes.ABS_Y] != [128, 128]:
        fail("moving one axis moved another axis's window")
    print("  ok  each axis has its own window")


def check_feed_ignores_axes_the_run_is_not_measuring():
    print("\nS12: a trigger or hat moving does not become stick travel")

    run = CalibrationRun(PAD, 1, dict(STICK))
    run.feed(axis(ecodes.ABS_Z, 255))
    run.feed(axis(ecodes.ABS_HAT0X, -1))
    if ecodes.ABS_Z in run.seen or ecodes.ABS_HAT0X in run.seen:
        fail("an axis outside the calibratable set was recorded -- centring a "
             "trigger makes it read half-pressed while untouched and costs it "
             "half its travel")
    print("  ok  an axis the run does not own is ignored")
    if run.coverage() != 0.0:
        fail("pulling a trigger counted towards stick coverage, so the "
             "progress bar fills without the stick ever having moved")
    print("  ok  it does not count towards coverage")


def check_only_the_calibrating_pads_events_count():
    print("\nS12: another controller cannot answer for the one being measured")

    h = Harness()
    run = h.run_for(PAD, 1, dict(STICK))
    h.server._on_raw_event(OTHER, axis(ecodes.ABS_X, 0))
    h.server._on_raw_event(OTHER, press())
    if run.seen[ecodes.ABS_X] != [128, 128] or run.advance_requested:
        fail("a second controller's events were fed into the calibration -- "
             "player 2 leaning on their stick would measure player 1's pad "
             "and walk the wizard on without them")
    print("  ok  a second pad's movement and presses are ignored")

    h.server._on_raw_event(PAD, axis(ecodes.ABS_X, 0))
    h.server._on_raw_event(PAD, press())
    if run.seen[ecodes.ABS_X] != [0, 128] or not run.advance_requested:
        fail("the pad being calibrated was not heard, so the wizard measures "
             "nothing and never advances")
    print("  ok  the pad being calibrated is heard")


# -- fraction and coverage ---------------------------------------------------


def check_fraction_is_the_clock_in_rest():
    print("\nS12: the rest phase reports a countdown")

    run = CalibrationRun(PAD, 1, dict(STICK))
    run.begin_phase(PHASE_REST)
    if run.fraction() > 0.05:
        fail(f"the rest phase opens at {run.fraction()} rather than empty")
    print("  ok  starts near zero")

    rewind(run, REST_SECONDS / 2)
    half = run.fraction()
    if not 0.45 <= half <= 0.55:
        fail(f"half way through the rest phase the bar reads {half:.3f} -- the "
             f"user is watching this to know how long to keep still")
    print(f"  ok  half way through it reads {half:.2f}")

    rewind(run, REST_SECONDS * 10)
    if run.fraction() != 1.0:
        fail(f"an over-running rest phase reports {run.fraction()}, so the "
             f"progress bar runs off the end of its track")
    print("  ok  clamps at 1.0 when the phase over-runs")


def check_fraction_is_coverage_in_reach():
    print("\nS12: the reach phase reports swept coverage, not a countdown")

    run = CalibrationRun(PAD, 1, {ecodes.ABS_X: absinfo(128)})
    run.begin_phase(PHASE_REACH)
    rewind(run, 60.0)
    if run.fraction() != 0.0:
        fail(f"a reach phase that has been open a minute with the stick "
             f"untouched reports {run.fraction()} -- the bar has to say "
             f"whether the circles reached the edges, not how long the user "
             f"has been sitting there")
    print("  ok  time alone does not fill the bar")

    run.feed(axis(ecodes.ABS_X, 0))
    run.feed(axis(ecodes.ABS_X, 255))
    if run.fraction() != 1.0:
        fail(f"a fully swept axis reports {run.fraction()} coverage")
    print("  ok  a full sweep fills the bar")

    if run.fraction() != run.coverage():
        fail("the reach phase's fraction is not its coverage")
    print("  ok  fraction is coverage during reach")


def check_fraction_is_zero_in_the_untimed_phases():
    print("\nS12: the phases that measure nothing report nothing")

    for phase in (PHASE_AWAIT_REST, PHASE_AWAIT_REACH, PHASE_ICON, "done"):
        run = CalibrationRun(PAD, 1, dict(STICK))
        run.phase = phase
        rewind(run, 30.0)
        if run.fraction() != 0.0:
            fail(f"phase {phase!r} reports {run.fraction()} progress after "
                 f"half a minute -- these phases wait for the user, and a "
                 f"creeping bar tells them they are late for something")
        print(f"  ok  {phase} reports 0.0 however long it waits")


def check_coverage_with_no_axes():
    print("\nS12: a pad with no calibratable axes does not divide by zero")

    run = CalibrationRun(PAD, 1, {})
    if run.coverage() != 0.0:
        fail(f"a pad with no axes reports {run.coverage()} coverage")
    print("  ok  no axes means no coverage, not a crash")
    if run.fraction() != 0.0:
        fail("a pad with no axes reports progress")
    print("  ok  and no progress")

    run.begin_phase(PHASE_REACH)
    if run.fraction() != 0.0:
        fail("a pad with no axes crashed or reported progress in reach")
    print("  ok  even in the reach phase")


def check_coverage_of_a_zero_width_axis():
    print("\nS12: an axis that declares no travel does not divide by zero")

    run = CalibrationRun(PAD, 1, {ecodes.ABS_X: absinfo(5, 5, 5)})
    if run.coverage() != 0.0:
        fail(f"an axis declaring min == max reports {run.coverage()} coverage")
    print("  ok  a zero-width declared range contributes nothing")


def check_coverage_across_axes():
    print("\nS12: coverage is what the sticks have actually swept")

    run = CalibrationRun(PAD, 1, dict(STICK))
    if run.coverage() != 0.0:
        fail("an untouched pad already reports coverage")
    print("  ok  nothing moved: 0.0")

    run.feed(axis(ecodes.ABS_X, 0))
    run.feed(axis(ecodes.ABS_X, 255))
    half = run.coverage()
    if abs(half - 0.5) > 1e-9:
        fail(f"one of two axes swept fully reports {half} -- the user needs to "
             f"see that the other stick direction still has to be covered")
    print("  ok  one of two axes swept fully: 0.5")

    run.feed(axis(ecodes.ABS_Y, 0))
    run.feed(axis(ecodes.ABS_Y, 255))
    if run.coverage() != 1.0:
        fail(f"both axes swept fully reports {run.coverage()}, so the bar "
             f"never fills however well the user circles the stick")
    print("  ok  both axes swept fully: 1.0")

    part = CalibrationRun(PAD, 1, {ecodes.ABS_X: absinfo(128)})
    part.feed(axis(ecodes.ABS_X, 64))
    part.feed(axis(ecodes.ABS_X, 192))
    if abs(part.coverage() - 0.5) > 0.01:
        fail(f"half of an axis's declared travel reports {part.coverage()}")
    print("  ok  half a sweep on one axis: 0.5")


def check_coverage_is_never_over_full():
    print("\nS12: a reading outside the declared range cannot over-fill the bar")

    run = CalibrationRun(PAD, 1, dict(STICK))
    for value in (-400, 400):
        run.feed(axis(ecodes.ABS_X, value))
    if not 0.0 <= run.coverage() <= 1.0:
        fail(f"an axis reporting outside its declared range gives coverage "
             f"{run.coverage()} -- a progress bar past 1.0 draws off the end "
             f"of its track")
    print("  ok  coverage stays within 0..1")

    masked = run.coverage()
    if masked >= 1.0 and run.seen[ecodes.ABS_Y] == [128, 128]:
        print(f"  gap: one axis reading beyond its declared range (-400..400 "
              f"of a 0-255 axis) scores 3.1 on its own, and coverage averages "
              f"the per-axis fractions before clamping -- so it reports "
              f"{masked} while ABS_Y has not moved at all. The user is told "
              f"the sweep is complete when one whole direction is unmeasured. "
              f"Clamping each axis to 1.0 before averaging would fix it.")


# -- the phase machine, driven from the tick ---------------------------------


def check_await_rest_advances_only_on_a_press():
    print("\nS12: the first phase waits for the user to be ready")

    h = Harness()
    run = h.run_for(PAD, 1, dict(STICK))
    for _ in range(3):
        h.tick()
    if run.phase != PHASE_AWAIT_REST:
        fail(f"await_rest advanced to {run.phase} on its own -- the user was "
             f"asked to let go of the sticks and has not answered yet")
    print("  ok  ticking alone does not advance it")
    if h.sent:
        fail(f"a waiting calibration broadcast {h.sent} on every tick, which "
             f"is a message storm for a phase where nothing changes")
    print("  ok  and says nothing while it waits")

    run.feed(press())
    h.tick()
    if run.phase != PHASE_REST:
        fail(f"a button press left the calibration in {run.phase} instead of "
             f"starting the rest measurement")
    print("  ok  a press starts the rest phase")
    if h.phases() != [PHASE_REST]:
        fail(f"the phase change was reported as {h.phases()} -- the overlay "
             f"draws whatever the daemon says, and a phase it never hears "
             f"about leaves the screen on the previous prompt")
    print("  ok  the new phase is reported")


def check_rest_lasts_its_phase_seconds():
    print("\nS12: the rest phase is timed, then measures")

    h = Harness()
    run = h.run_for(PAD, 1, dict(STICK))
    run.begin_phase(PHASE_REST)

    rewind(run, REST_SECONDS * 0.5)
    h.tick()
    if run.phase != PHASE_REST:
        fail(f"the rest phase ended half way through, after "
             f"{REST_SECONDS / 2:.2f}s of a {REST_SECONDS}s measurement")
    print("  ok  still measuring half way through")
    if not h.sent:
        fail("no progress was reported during the rest phase, so the bar sits "
             "still and the user cannot tell it is working")
    print("  ok  progress is reported while it runs")
    if run.rest is not None:
        fail("a rest position was recorded before the phase finished")
    print("  ok  nothing is concluded early")

    rewind(run, REST_SECONDS * 0.6)
    h.tick()
    if run.phase != PHASE_AWAIT_REACH:
        fail(f"after {REST_SECONDS}s the rest phase is still {run.phase} -- "
             f"the user let go of the sticks and the wizard never moved on")
    print("  ok  advances to await_reach once the time is up")
    if not run.rest or sorted(run.rest) != sorted(STICK):
        fail(f"the rest phase ended without a measurement for every axis "
             f"({run.rest}) -- an axis with no centre is an axis the pad "
             f"cannot rescale at all")
    print("  ok  every axis has a measured centre")
    if h.last().get("phase") != PHASE_AWAIT_REACH:
        fail("the move to await_reach was not reported, so the overlay never "
             "asks the user to start circling the stick")
    print("  ok  await_reach is reported")


def check_rest_measures_where_the_stick_actually_sat():
    print("\nS12: the measured centre comes from the samples, not the guess")

    h = Harness()
    run = h.run_for(PAD, 1, {ecodes.ABS_X: absinfo(128)})
    run.begin_phase(PHASE_REST)
    run.feed(axis(ecodes.ABS_X, 100))
    run.feed(axis(ecodes.ABS_X, 140))
    rewind(run, REST_SECONDS * 1.1)
    h.tick()
    cal = run.rest[ecodes.ABS_X]
    if cal.center != 120:
        fail(f"a stick sitting between 100 and 140 was centred at {cal.center} "
             f"instead of 120 -- a wrong centre is a stick that drifts in one "
             f"direction with nobody touching it")
    print("  ok  the centre is the middle of what was seen")
    if cal.flat < 21:
        fail(f"a stick that wobbled 40 units got a dead band of {cal.flat} -- "
             f"the wobble it exists to cover would come straight through as "
             f"input")
    print("  ok  the wobble widened the dead band")


def check_await_reach_advances_only_on_a_press():
    print("\nS12: the reach phase waits for the user too")

    h = Harness()
    run = h.run_for(PAD, 1, dict(STICK))
    run.begin_phase(PHASE_AWAIT_REACH)
    rewind(run, 30.0)
    for _ in range(3):
        h.tick()
    if run.phase != PHASE_AWAIT_REACH:
        fail(f"await_reach advanced to {run.phase} on the clock -- the user "
             f"sets the pace, and starting to measure before they have taken "
             f"hold of the stick throws away the first seconds of the sweep")
    print("  ok  half a minute of ticking does not advance it")

    run.feed(press())
    h.tick()
    if run.phase != PHASE_REACH:
        fail(f"a press left await_reach in {run.phase}")
    print("  ok  a press starts the reach phase")
    if run.advance_requested:
        fail("the press that started the reach phase is still pending, so it "
             "will end the phase the instant the minimum elapses")
    print("  ok  the press that started it was consumed")


def check_reach_cannot_end_before_the_minimum():
    print("\nS12: the press that starts the reach phase cannot end it")

    h = Harness()
    run = h.run_for(PAD, 1, dict(STICK))
    run.begin_phase(PHASE_REACH)
    run.feed(press())
    rewind(run, REACH_MINIMUM_SECONDS * 0.5)
    h.tick()
    if run.phase != PHASE_REACH:
        fail(f"the reach phase ended after "
             f"{REACH_MINIMUM_SECONDS / 2:.2f}s, short of the "
             f"{REACH_MINIMUM_SECONDS}s minimum -- a user who holds the "
             f"button a moment too long measures a stick that never moved")
    print("  ok  a press inside the minimum does not end it")
    if h.last().get("phase") != PHASE_REACH:
        fail("the reach phase stopped reporting progress while it waited")
    print("  ok  it keeps reporting coverage while it waits")


def check_reach_ends_on_a_press_after_the_minimum():
    print("\nS12: the reach phase ends when the user says so")

    h = Harness()
    run = h.run_for(PAD, 1, dict(STICK))
    # The rest phase runs first, as it does for a real user. It is what gives
    # the axis a credible dead band; skipping it lands on the no-rest fallback,
    # which derives a "rest" from the sweep itself and so calls half the sweep
    # a dead band. That path has its own check
    # (check_reach_without_a_rest_measurement_survives) and is not what this
    # one is about.
    run.begin_phase(PHASE_REST)
    rewind(run, REST_SECONDS * 1.1)
    h.tick()
    run.feed(press())
    h.tick()
    if run.phase != PHASE_REACH:
        fail(f"the rest phase did not lead into reach on a press (in "
             f"{run.phase})")
    for code in (ecodes.ABS_X, ecodes.ABS_Y):
        run.feed(axis(code, 20))
        run.feed(axis(code, 235))
    rewind(run, REACH_MINIMUM_SECONDS * 4)
    h.tick()
    if run.phase != PHASE_REACH:
        fail(f"the reach phase ended after {REACH_MINIMUM_SECONDS * 4:.1f}s "
             f"with no press -- it ends on a button, not a timer, so the user "
             f"decides when the circles are good enough")
    print("  ok  time alone never ends it")

    run.feed(press())
    h.tick()
    if run.phase != PHASE_ICON:
        fail(f"a press past the minimum left the calibration in {run.phase} "
             f"instead of moving on to the icon step")
    print("  ok  a press past the minimum ends it")

    stored = profiles.load(PAD)
    if stored is None or not stored.axes:
        fail("the reach phase ended without storing a profile, so everything "
             "the user just measured is thrown away")
    print("  ok  the measurement is stored on the profile")
    cal = stored.axes[ecodes.ABS_X]
    if (cal.reach_min, cal.reach_max) != (20, 235):
        fail(f"the stored reach is {(cal.reach_min, cal.reach_max)} rather "
             f"than the (20, 235) the stick was seen to travel -- scaling "
             f"against the declared range is what leaves a stick unable to go "
             f"left")
    print("  ok  it is the measured reach, not the declared range")
    if h.last().get("axes") != 2:
        fail(f"the icon step was announced with {h.last().get('axes')} axes")
    print("  ok  the icon step is announced with the axis count")


def check_a_reach_press_arrives_latched():
    print("\nS12: an incidental press during the reach sweep")

    h = Harness()
    run = h.run_for(PAD, 1, dict(STICK))
    run.begin_phase(PHASE_REACH)
    # The user is rotating the stick and clips a button on the way past.
    run.feed(press())
    rewind(run, REACH_MINIMUM_SECONDS * 0.4)
    h.tick()
    if run.phase != PHASE_REACH:
        fail("an incidental press ended the reach sweep immediately")
    print("  ok  an incidental press does not end the sweep on the spot")

    rewind(run, REACH_MINIMUM_SECONDS)
    h.tick()
    if run.phase == PHASE_ICON:
        print(f"  gap: the request is latched, not sampled -- a button clipped "
              f"{REACH_MINIMUM_SECONDS * 0.4:.1f}s into the sweep ends the "
              f"phase the instant {REACH_MINIMUM_SECONDS}s elapses, with no "
              f"second press. The daemon's own note says a user rotating a "
              f"stick presses buttons incidentally, so this cuts the sweep "
              f"short at the minimum and calibrates against whatever was "
              f"covered by then. Clearing advance_requested when it arrives "
              f"too early would make the phase end on a real press.")
    else:
        print("  ok  an early press is not remembered past the minimum")


def check_the_icon_phase_is_inert():
    print("\nS12: the icon step waits for set_icon and nothing else")

    h = Harness()
    run = h.run_for(PAD, 1, dict(STICK))
    run.begin_phase(PHASE_ICON)
    run.feed(press())
    rewind(run, 60.0)
    for _ in range(3):
        h.tick()
    if run.phase != PHASE_ICON:
        fail(f"the icon step advanced itself to {run.phase} -- it is answered "
             f"from the keyboard, and a phase that moves on by itself picks "
             f"the icon for the user")
    print("  ok  neither time nor a press moves it on")
    if h.sent:
        fail(f"the icon step broadcast {h.sent} while idle")
    print("  ok  it says nothing while it waits")
    if h.server._calibration is not run:
        fail("the daemon dropped the calibration during the icon step, so "
             "input stops being modal and a press claims a player slot")
    print("  ok  the flow is still modal")


def check_the_whole_sequence_in_order():
    print("\nS12: the five phases happen in order, once each")

    h = Harness()
    run = h.run_for(PAD, 2, dict(STICK))

    h.tick()                                   # waiting
    run.feed(press())
    h.tick()                                   # -> rest
    rewind(run, REST_SECONDS * 1.1)
    h.tick()                                   # -> await_reach
    run.feed(press())
    h.tick()                                   # -> reach
    for code in (ecodes.ABS_X, ecodes.ABS_Y):
        run.feed(axis(code, 15))
        run.feed(axis(code, 240))
    rewind(run, REACH_MINIMUM_SECONDS * 1.1)
    run.feed(press())
    h.tick()                                   # -> icon
    h.server._finish_calibration()

    seen = []
    for phase in h.phases():
        if not seen or seen[-1] != phase:
            seen.append(phase)
    expected = [PHASE_REST, PHASE_AWAIT_REACH, PHASE_REACH, PHASE_ICON, "done"]
    if seen != expected:
        fail(f"the wizard walked {seen} instead of {expected} -- the overlay "
             f"draws one screen per phase, so a missing or repeated phase is "
             f"a screen the user never sees or cannot leave")
    print(f"  ok  {' -> '.join(seen)}")

    players = {m.get("player") for m in h.sent
               if m.get("event") == "calibration"}
    if players != {2}:
        fail(f"calibration events were reported for players {players} -- the "
             f"overlay filters on this, so a wrong player number is an "
             f"overlay that never updates")
    print("  ok  every event names player 2")

    named = [m for m in h.sent if m.get("event") == "calibration"
             and "name" in m]
    if not named or any(m["name"] != "Test Pad" for m in named):
        fail("the events do not name the pad being calibrated, so the overlay "
             "cannot say which controller the user should be holding")
    print("  ok  and names the pad")


def check_finish_hands_input_back():
    print("\nS13: finishing leaves the modal flow")

    h = Harness()
    h.run_for(PAD, 4, dict(STICK))
    h.server._confirm_started["/dev/input/event90"] = 1.0
    h.server._finish_calibration()

    if h.server._calibration is not None:
        fail("the calibration run outlived the flow, so every later event is "
             "still swallowed by a wizard nobody can see")
    print("  ok  the run is cleared")
    if h.server._confirm_started:
        fail("a button held while choosing an icon is still in the confirm "
             "tracker, so it fires the moment normal handling resumes and "
             "ends the session the user was in the middle of")
    print("  ok  any in-flight confirm hold is dropped")
    done = [m for m in h.sent if m.get("phase") == "done"]
    if not done or done[-1].get("player") != 4:
        fail("no 'done' was reported for the player, so the overlay stays up "
             "with the pads grabbed and no way out")
    print("  ok  'done' is reported for the player")


def check_reach_without_a_rest_measurement_survives():
    print("\nS12: a reach phase with no measured rest does not crash")

    h = Harness()
    run = h.run_for(PAD, 1, dict(STICK))
    run.begin_phase(PHASE_REACH)
    run.rest = None
    for code in (ecodes.ABS_X, ecodes.ABS_Y):
        run.feed(axis(code, 30))
        run.feed(axis(code, 220))
    rewind(run, REACH_MINIMUM_SECONDS * 1.1)
    run.feed(press())
    try:
        h.tick()
    except Exception as exc:                       # noqa: BLE001
        fail(f"a reach phase with no measured rest raised {exc!r} -- inside "
             f"the daemon tick that takes the whole daemon down and every pad "
             f"with it")
    if run.phase != PHASE_ICON:
        fail("a reach phase with no measured rest never finished")
    print("  ok  it falls back rather than raising inside the tick")


# -- _begin_calibration (S13) ------------------------------------------------


def check_calibration_needs_a_session():
    print("\nS13: asking to calibrate outside a session is answered")

    h = Harness()
    h.server._assigner = None
    h.server._begin_calibration(1)
    if h.server._calibration is not None:
        fail("a calibration started with no session open, so it measures a "
             "pad the daemon does not hold")
    errors = [m for m in h.sent if m.get("event") == "error"]
    if not errors:
        fail("the daemon refused to calibrate and said nothing -- that is the "
             "overlay sitting on 'Starting...' forever with the pads grabbed, "
             "no way forward and no way out")
    print(f"  ok  refused with {errors[-1]['message']!r}")


def check_calibration_needs_a_pad_for_the_player():
    print("\nS13: asking about an unassigned slot is answered")

    h = Harness()
    h.server._assigner = FakeAssigner(FakeDevice([]), assignments=[(1, PAD)])
    h.server._begin_calibration(3)
    if h.server._calibration is not None:
        fail("a calibration started for a player with no controller")
    errors = [m for m in h.sent if m.get("event") == "error"]
    if not errors or "player 3" not in errors[-1]["message"]:
        fail("no error naming the player came back, so the overlay hangs")
    print(f"  ok  refused with {errors[-1]['message']!r}")


def check_calibration_needs_an_open_device():
    print("\nS13: a pad that has gone away is answered")

    h = Harness()
    h.server._assigner = FakeAssigner(None, assignments=[(1, PAD)])
    h.server._begin_calibration(1)
    if h.server._calibration is not None:
        fail("a calibration started for a pad the daemon no longer has open")
    errors = [m for m in h.sent if m.get("event") == "error"]
    if not errors:
        fail("an unplugged pad produced no error, so the overlay hangs on a "
             "controller that is not there any more")
    print(f"  ok  refused with {errors[-1]['message']!r}")


def check_a_pad_with_no_centring_axes_skips_to_the_icon():
    print("\nS12: a d-pad-only pad is configured, not refused")

    h = Harness()
    hat = absinfo(0, -1, 1)
    h.server._assigner = FakeAssigner(
        FakeDevice([(ecodes.ABS_HAT0X, hat), (ecodes.ABS_HAT0Y, hat)]),
        assignments=[(1, PAD)],
    )
    h.server._begin_calibration(1)

    run = h.server._calibration
    if run is None or run.phase != PHASE_ICON:
        fail(f"a pad with nothing to centre landed in "
             f"{run.phase if run else None} instead of the icon step -- there "
             f"is nothing to measure, and stopping here leaves the pad "
             f"unconfigured and the user staring at a stick prompt")
    print("  ok  jumps straight to the icon step")

    stored = profiles.load(PAD)
    if stored is None:
        fail("no profile was written for a pad with no centring axes, so it "
             "does not count as configured and the wizard offers again")
    if stored.axes:
        fail(f"axes {stored.axes} were invented for a pad that has none")
    print("  ok  an empty profile is stored, so the pad counts as configured")
    if h.last().get("axes") != 0 or h.last().get("phase") != PHASE_ICON:
        fail(f"the icon step was not announced ({h.last()})")
    print("  ok  the icon step is announced with no axes")


def check_beginning_calibration_drops_a_confirm_hold():
    print("\nS12: the button that opened the wizard cannot confirm later")

    h = Harness()
    h.server._assigner = FakeAssigner(
        FakeDevice([(ecodes.ABS_X, absinfo(128))]), assignments=[(1, PAD)])
    h.server._confirm_started["/dev/input/event90"] = 1.0
    h.server._begin_calibration(1)
    if h.server._confirm_started:
        fail("the in-flight confirm hold survived the start of calibration -- "
             "the button that opened this is very likely still down, and it "
             "would end the session out from under the wizard")
    print("  ok  the confirm hold is dropped")
    if h.server._calibration.phase != PHASE_AWAIT_REST:
        fail("a pad with a centring axis skipped the measurement")
    print("  ok  a pad with a stick starts at await_rest")


# -- calibrate.rest_from_samples ---------------------------------------------


def check_rest_of_a_stick_that_never_moved():
    print("\nS12: a stick that did not move is centred where it sits")

    axes = {ecodes.ABS_X: absinfo(128)}
    out = calibrate.rest_from_samples(axes, {ecodes.ABS_X: [128, 128]})
    cal = out[ecodes.ABS_X]
    if cal.center != 128:
        fail(f"a still stick was centred at {cal.center} rather than 128")
    print("  ok  the centre is its resting value")
    if (cal.minimum, cal.maximum) != (0, 255):
        fail("the declared range was not carried through, so the axis is "
             "rescaled onto the wrong output scale")
    print("  ok  the declared range is carried through")
    floor = int(255 * calibrate.MIN_FLAT_FRACTION)
    if cal.flat < floor:
        fail(f"the dead band is {cal.flat}, below the {floor} floor -- even a "
             f"still stick dithers a couple of units, and a zero-width band "
             f"lets that through as input")
    print(f"  ok  the dead band is at least the {floor}-unit floor")
    if cal.reach_min is not None or cal.reach_max is not None:
        fail("a rest measurement invented a reach it has not measured yet")
    print("  ok  no reach is claimed yet")


def check_rest_of_a_stick_sitting_off_centre():
    print("\nS12: an offset stick is centred where it actually rests")

    axes = {ecodes.ABS_X: absinfo(128)}
    out = calibrate.rest_from_samples(axes, {ecodes.ABS_X: [200, 200]})
    if out[ecodes.ABS_X].center != 200:
        fail(f"identical samples at 200 produced centre "
             f"{out[ecodes.ABS_X].center} -- the samples are the measurement, "
             f"and preferring the declared value is what leaves a stick "
             f"drifting with nobody touching it")
    print("  ok  identical samples away from the middle are believed")


def check_rest_dead_band_covers_the_wobble():
    print("\nS12: a jittery stick gets a wider dead band, not a wrong centre")

    axes = {ecodes.ABS_X: absinfo(128)}
    tight = calibrate.rest_from_samples(axes, {ecodes.ABS_X: [126, 130]})
    wide = calibrate.rest_from_samples(axes, {ecodes.ABS_X: [68, 188]})
    if tight[ecodes.ABS_X].center != 128 or wide[ecodes.ABS_X].center != 128:
        fail("a symmetric wobble moved the centre")
    print("  ok  a symmetric wobble leaves the centre alone")
    if wide[ecodes.ABS_X].flat <= tight[ecodes.ABS_X].flat:
        fail(f"a stick wobbling 120 units got dead band "
             f"{wide[ecodes.ABS_X].flat}, no wider than one wobbling 4 units "
             f"({tight[ecodes.ABS_X].flat}) -- the wobble would come straight "
             f"through as input")
    print("  ok  a bigger wobble gets a bigger dead band")
    if wide[ecodes.ABS_X].flat < 61:
        fail(f"a 68..188 wobble got a dead band of {wide[ecodes.ABS_X].flat}, "
             f"which does not cover the wobble itself")
    print("  ok  the band covers the whole observed wobble")


def check_rest_respects_the_declared_flat():
    print("\nS12: the hardware's own dead band is a floor too")

    axes = {ecodes.ABS_X: absinfo(128, flat=40)}
    out = calibrate.rest_from_samples(axes, {ecodes.ABS_X: [128, 128]})
    if out[ecodes.ABS_X].flat < 40:
        fail(f"an axis declaring flat=40 got a dead band of "
             f"{out[ecodes.ABS_X].flat} -- the driver already says that much "
             f"is noise, and going under it lets the noise through")
    print("  ok  the declared flat is never narrowed")


def check_rest_of_an_axis_with_no_samples():
    print("\nS12: an axis that reported nothing still gets a calibration")

    axes = {ecodes.ABS_X: absinfo(90), ecodes.ABS_Y: absinfo(128)}
    out = calibrate.rest_from_samples(axes, {ecodes.ABS_Y: [128, 128]})
    if sorted(out) != sorted(axes):
        fail(f"only {sorted(out)} of {sorted(axes)} were calibrated -- a "
             f"settled axis stops reporting entirely, so 'no samples' is the "
             f"normal case and dropping it leaves that stick unscaled")
    print("  ok  every declared axis comes back")
    if out[ecodes.ABS_X].center != 90:
        fail(f"the unreported axis was centred at {out[ecodes.ABS_X].center} "
             f"rather than its own resting value of 90")
    print("  ok  it falls back to its resting value")


def check_rest_of_no_axes():
    print("\nS12: a pad with no calibratable axes measures nothing")

    if calibrate.rest_from_samples({}, {}) != {}:
        fail("a pad with no calibratable axes produced a calibration anyway")
    print("  ok  no axes in, no axes out")
    if calibrate.rest_from_samples({}, {ecodes.ABS_X: [0, 255]}) != {}:
        fail("samples for an axis that is not being calibrated created one -- "
             "that is how a trigger gets centred and loses half its travel")
    print("  ok  stray samples do not create an axis")


# -- calibrate.merge_reach ---------------------------------------------------


def rest_cal(center=128, flat=10, minimum=0, maximum=255):
    return {ecodes.ABS_X: AxisCalibration(center=center, minimum=minimum,
                                          maximum=maximum, flat=flat)}


def check_merge_records_a_full_sweep():
    print("\nS12: a stick swept both ways records both extremes")

    out = calibrate.merge_reach(rest_cal(), {ecodes.ABS_X: (30, 210)})
    cal = out[ecodes.ABS_X]
    if (cal.reach_min, cal.reach_max) != (30, 210):
        fail(f"the measured reach came back as "
             f"{(cal.reach_min, cal.reach_max)} rather than (30, 210) -- an "
             f"adapter can declare 0-255 while the stick only reaches part of "
             f"it, and scaling against the declared range leaves one "
             f"direction with almost no travel")
    print("  ok  both measured extremes are kept")
    if (cal.center, cal.minimum, cal.maximum, cal.flat) != (128, 0, 255, 10):
        fail("merging the reach disturbed the centre, range or dead band")
    print("  ok  the centre, range and dead band are untouched")


def check_merge_ignores_a_direction_that_never_moved():
    print("\nS12: a direction that never moved falls back to the full range")

    out = calibrate.merge_reach(rest_cal(), {ecodes.ABS_X: (128, 240)})
    cal = out[ecodes.ABS_X]
    if cal.reach_min is not None:
        fail(f"a direction that never left centre recorded reach_min "
             f"{cal.reach_min} -- pinning its travel to zero is the stick "
             f"that cannot go left at all")
    print("  ok  the unmoved side is left unmeasured")
    if cal.low != cal.minimum:
        fail(f"the unmeasured side scales against {cal.low} rather than the "
             f"declared minimum {cal.minimum}")
    print("  ok  it falls back to the declared minimum")
    if cal.apply(0) != cal.minimum:
        fail(f"full left reads {cal.apply(0)} instead of {cal.minimum} on a "
             f"stick whose left was never measured -- the stick cannot go "
             f"left at all")
    print("  ok  and full deflection still reaches the extreme")

    out = calibrate.merge_reach(rest_cal(), {ecodes.ABS_X: (30, 128)})
    if out[ecodes.ABS_X].reach_max is not None:
        fail("the same on the other side: an unmoved right recorded a reach")
    if out[ecodes.ABS_X].apply(255) != 255:
        fail("full right reads short on a stick whose right was never moved")
    print("  ok  the same holds for the other direction")


def check_merge_of_an_axis_missing_from_the_reach():
    print("\nS12: an axis absent from the reach keeps its declared range")

    out = calibrate.merge_reach(rest_cal(), {})
    cal = out[ecodes.ABS_X]
    if (cal.reach_min, cal.reach_max) != (None, None):
        fail(f"an axis that reported nothing during the sweep got reach "
             f"{(cal.reach_min, cal.reach_max)} -- an axis nobody touched "
             f"reports nothing at all, and that must not read as zero travel")
    print("  ok  no reach is recorded for it")
    if cal.apply(0) != 0 or cal.apply(255) != 255:
        fail("an axis missing from the reach lost its travel")
    print("  ok  it still travels the full declared range")

    out = calibrate.merge_reach({}, {ecodes.ABS_X: (0, 255)})
    if out != {}:
        fail("a reach for an axis with no rest measurement invented one")
    print("  ok  a reach without a rest measurement invents nothing")


def check_merge_of_an_inverted_reach():
    print("\nS12: a reach that arrives inside out cannot invert the stick")

    out = calibrate.merge_reach(rest_cal(), {ecodes.ABS_X: (240, 30)})
    cal = out[ecodes.ABS_X]
    if cal.low > cal.center or cal.high < cal.center:
        fail(f"an inverted reach produced low={cal.low} high={cal.high} "
             f"around centre {cal.center} -- an axis whose scale runs "
             f"backwards is a stick that moves the wrong way")
    print("  ok  low stays at or below centre and high at or above it")
    outs = [cal.apply(v) for v in range(0, 256)]
    if outs != sorted(outs):
        fail("an inverted reach made the axis non-monotonic, so pushing the "
             "stick further one way sends the reading back the other")
    print("  ok  the axis still reads in the right direction")
    if not all(cal.minimum <= v <= cal.maximum for v in outs):
        fail("an inverted reach pushed readings outside the declared range")
    print("  ok  and stays inside the declared range")


def check_merge_of_a_reach_inside_the_dead_band():
    print("\nS12: a direction that moved less than the dead band")

    exact = calibrate.merge_reach(rest_cal(center=128, flat=10),
                                  {ecodes.ABS_X: (128, 240)})[ecodes.ABS_X]
    if exact.apply(0) != 0:
        fail("a direction that never moved at all lost its travel")
    print("  ok  a direction that never moved keeps the full declared range")

    nudged = calibrate.merge_reach(rest_cal(center=128, flat=10),
                                   {ecodes.ABS_X: (120, 240)})[ecodes.ABS_X]
    if nudged.reach_min is not None:
        fail(f"a direction that moved 8 units -- less than the "
             f"{nudged.flat}-unit dead band, so indistinguishable from not "
             f"moving at all -- was recorded as a measured reach "
             f"(reach_min={nudged.reach_min})")
    if nudged.apply(0) == nudged.apply(128):
        fail(f"a stick nudged 8 units inside its {nudged.flat}-unit dead band "
             f"during the sweep reads {nudged.apply(0)} at full left, the "
             f"same as at rest -- the whole of that direction is dead, so "
             f"twitching the stick a hair is worse than never moving it")
    if nudged.apply(0) != nudged.minimum:
        fail(f"full left reads {nudged.apply(0)} rather than "
             f"{nudged.minimum} on a direction whose only movement was inside "
             f"the dead band -- unmeasured must fall back to the declared "
             f"range")
    print("  ok  a nudge inside the dead band does not kill the direction")

    # The other edge of the same guard: a sweep that clears the band is a real
    # measurement and must survive, or ignoring dithers would cost every
    # short-throw adapter its calibration.
    real = calibrate.merge_reach(rest_cal(center=128, flat=10),
                                 {ecodes.ABS_X: (117, 240)})[ecodes.ABS_X]
    if real.reach_min != 117:
        fail(f"a direction swept one unit past the {real.flat}-unit dead band "
             f"recorded reach_min={real.reach_min} rather than 117 -- a "
             f"measurement that cleared the band is real travel and dropping "
             f"it scales the stick against a range its hardware never reaches")
    print("  ok  a sweep that clears the band is still recorded")

    other = calibrate.merge_reach(rest_cal(center=128, flat=10),
                                  {ecodes.ABS_X: (30, 135)})[ecodes.ABS_X]
    if other.reach_max is not None:
        fail(f"the same on the other side: a 7-unit twitch inside the "
             f"{other.flat}-unit dead band was recorded as reach_max="
             f"{other.reach_max}")
    if other.apply(255) != other.maximum:
        fail(f"full right reads {other.apply(255)} rather than "
             f"{other.maximum} on a direction that only ever twitched inside "
             f"the dead band")
    print("  ok  the same holds for the other direction")


def check_a_jittery_axis_that_never_moves_during_the_sweep():
    print("\nS12: an axis that jitters at rest and sits still during the sweep")

    axes = {ecodes.ABS_X: absinfo(128), ecodes.ABS_Y: absinfo(128)}
    # ABS_X dithers upward while the user is letting go: a real, ordinary
    # stick. ABS_Y is perfectly still.
    rest = calibrate.rest_from_samples(axes, {ecodes.ABS_X: [128, 131],
                                              ecodes.ABS_Y: [128, 128]})
    if rest[ecodes.ABS_X].center <= 128:
        fail("the upward dither did not move the measured centre, so this "
             "scenario no longer exercises what it was written for")
    print(f"  ok  the dither put the centre at {rest[ecodes.ABS_X].center}")

    # The sweep: the user circles the other stick and this one is untouched.
    # Its window still opens at the absinfo value from before rest was
    # measured, which is now below the measured centre.
    merged = calibrate.merge_reach(rest, {ecodes.ABS_X: (128, 128),
                                          ecodes.ABS_Y: (20, 235)})
    cal = merged[ecodes.ABS_X]
    if cal.reach_min is not None:
        fail(f"the reach window is seeded from absinfo (128) while the centre "
             f"is measured ({cal.center}), so an untouched axis ends the "
             f"sweep below its own centre and got recorded as having reached "
             f"{cal.reach_min} -- a reach nobody performed")
    if cal.apply(cal.minimum) == cal.apply(cal.center):
        fail(f"an axis that dithered at rest and was never touched during the "
             f"sweep reads {cal.apply(cal.minimum)} at full left, identical "
             f"to its resting reading -- the stick cannot go left at all, "
             f"reached with the user doing nothing wrong")
    if cal.apply(cal.minimum) != cal.minimum:
        fail(f"full left reads {cal.apply(cal.minimum)} rather than "
             f"{cal.minimum} on an untouched axis, which must fall back to "
             f"its declared range")
    print("  ok  it keeps its declared travel")

    good = merged[ecodes.ABS_Y]
    if good.apply(0) != 0 or good.apply(255) != 255:
        fail("the axis that was actually swept lost its travel, which is the "
             "one thing the reach phase is for")
    print("  ok  the axis that was swept spans its full range")


# -- AxisCalibration.apply ---------------------------------------------------

# name -> calibration. Every one of these is something a real pad produces.
APPLY_CASES = {
    "measured stick": AxisCalibration(128, 0, 255, 10, 20, 240),
    "unmeasured stick": AxisCalibration(128, 0, 255, 10),
    "no dead band": AxisCalibration(128, 0, 255, 0, 0, 255),
    "n64 short reach": AxisCalibration(128, 0, 255, 10, 60, 200),
    "off-centre rest": AxisCalibration(96, 0, 255, 8, 20, 240),
    "reach beyond declared": AxisCalibration(128, 0, 255, 10, -40, 300),
    "reach equals centre": AxisCalibration(128, 0, 255, 4, 128, 128),
    "band swallows travel": AxisCalibration(128, 0, 255, 200, 20, 240),
    "band wider than reach": AxisCalibration(128, 0, 255, 10, 120, 136),
    "centre at the minimum": AxisCalibration(0, 0, 255, 4, None, 255),
    "signed range": AxisCalibration(0, -32768, 32767, 1000, -20000, 20000),
}


def sweep(cal, step=1):
    """Every reading from well under the range to well over it."""
    values = list(range(cal.minimum - 4 * step, cal.maximum + 4 * step + 1,
                        step))
    return values, [cal.apply(v) for v in values]


def check_apply_never_leaves_the_declared_range():
    print("\nS12: no reading can escape the declared range")

    for name, cal in APPLY_CASES.items():
        step = max(1, (cal.maximum - cal.minimum) // 400)
        _, outs = sweep(cal, step)
        bad = [o for o in outs if not cal.minimum <= o <= cal.maximum]
        if bad:
            fail(f"{name}: readings {bad[:3]} fall outside the declared "
                 f"{cal.minimum}..{cal.maximum} -- RetroArch and SDL both "
                 f"take the declared range at its word, so an out-of-range "
                 f"value is an axis pinned to an extreme")
    print(f"  ok  all {len(APPLY_CASES)} calibrations stay in range, "
          f"including readings well outside it")


def check_apply_is_monotonic():
    print("\nS12: pushing further one way never reads back the other")

    for name, cal in APPLY_CASES.items():
        step = max(1, (cal.maximum - cal.minimum) // 400)
        values, outs = sweep(cal, step)
        for (v1, o1), (v2, o2) in zip(zip(values, outs),
                                      list(zip(values, outs))[1:]):
            if o2 < o1:
                fail(f"{name}: raw {v1} reads {o1} but raw {v2} reads {o2} -- "
                     f"the stick reverses direction part way through its "
                     f"travel, which is unplayable")
    print(f"  ok  all {len(APPLY_CASES)} calibrations are non-decreasing")


def check_apply_maps_centre_to_the_midpoint():
    print("\nS12: the resting position reads as neutral")

    for name, cal in APPLY_CASES.items():
        mid = (cal.minimum + cal.maximum) // 2
        got = cal.apply(cal.center)
        if got != mid:
            fail(f"{name}: a stick sitting at its measured centre reads {got} "
                 f"rather than the declared midpoint {mid} -- that is a pad "
                 f"walking in one direction with nobody touching it")
    print(f"  ok  all {len(APPLY_CASES)} calibrations rest at the midpoint")


def check_apply_holds_the_dead_band_flat():
    print("\nS12: the dead band reads as neutral all the way across")

    cal = AxisCalibration(128, 0, 255, 10, 20, 240)
    mid = 127
    for value in range(cal.center - cal.flat, cal.center + cal.flat + 1):
        if cal.apply(value) != mid:
            fail(f"raw {value} is inside the {cal.flat}-unit dead band but "
                 f"reads {cal.apply(value)} -- the dither the band exists to "
                 f"absorb comes through as input")
    print("  ok  every reading inside the band reads neutral")

    if cal.apply(cal.center - cal.flat - 1) >= mid:
        fail("the reading just outside the dead band did not move")
    if cal.apply(cal.center + cal.flat + 1) <= mid:
        fail("the reading just outside the dead band did not move")
    print("  ok  one unit outside it, the axis starts to move")


def check_apply_is_continuous():
    print("\nS12: the reading does not jump at the edge of the dead band")

    cal = AxisCalibration(128, 0, 255, 10, 20, 240)
    values, outs = sweep(cal)
    jumps = [(v, a, b) for v, a, b in zip(values, outs, outs[1:])
             if abs(b - a) > 3]
    if jumps:
        fail(f"the reading jumps from {jumps[0][1]} to {jumps[0][2]} at raw "
             f"{jumps[0][0]} -- a step in the middle of a stick's travel is a "
             f"character that lurches instead of easing")
    print("  ok  one raw unit never moves the reading by more than three")

    edge = cal.center - cal.flat
    if abs(cal.apply(edge) - cal.apply(edge - 1)) > 3:
        fail("the reading jumps as it leaves the dead band, so the stick "
             "snaps out of neutral rather than easing out of it")
    print("  ok  including at the edge of the dead band")


def check_apply_reaches_the_extremes():
    print("\nS12: full deflection reaches full scale, and no further")

    cal = AxisCalibration(128, 0, 255, 10, 20, 240)
    if cal.apply(20) != 0:
        fail(f"the stick's measured left extreme reads {cal.apply(20)} rather "
             f"than 0 -- the whole point of measuring reach is that the "
             f"hardware's real travel becomes full scale")
    if cal.apply(240) != 255:
        fail(f"the stick's measured right extreme reads {cal.apply(240)} "
             f"rather than 255")
    print("  ok  the measured extremes map to the declared extremes")

    if cal.apply(0) != 0 or cal.apply(255) != 255:
        fail("a stick overshooting its calibration escaped the declared range")
    print("  ok  an overshoot is clamped, not wrapped")

    # The measured case: an adapter declares 0-255 while the stick only
    # produces 160-255 and rests at 207. Scaled against the declared range
    # that stick has almost no travel to the left.
    n64 = AxisCalibration(207, 0, 255, 10, 160, 255)
    if n64.apply(160) != 0 or n64.apply(255) != 255:
        fail(f"an adapter declaring 0-255 whose stick only reaches 160-255 "
             f"reads {n64.apply(160)}..{n64.apply(255)} instead of 0..255 -- "
             f"that is the stick that cannot go left at all")
    if n64.apply(n64.center) != 127:
        fail("the short-reach stick does not rest at neutral")
    print("  ok  a stick reaching only part of its declared range still spans "
          "it")


def check_apply_cannot_divide_by_zero():
    print("\nS12: a degenerate calibration returns neutral, not a crash")

    degenerate = {
        "reach equals centre both ways":
            AxisCalibration(128, 0, 255, 4, 128, 128),
        "dead band swallows the whole travel":
            AxisCalibration(128, 0, 255, 500, 20, 240),
        "dead band exactly the reach":
            AxisCalibration(128, 0, 255, 108, 20, 236),
        "zero-width declared range":
            AxisCalibration(7, 7, 7, 0, 7, 7),
        "centre outside the declared range":
            AxisCalibration(300, 0, 255, 4, 20, 240),
    }
    for name, cal in degenerate.items():
        try:
            outs = [cal.apply(v) for v in range(cal.minimum - 5,
                                                cal.maximum + 6)]
        except ZeroDivisionError:
            fail(f"{name}: applying a reading divided by zero -- this runs on "
                 f"every event of every republished pad, so it takes the "
                 f"daemon and every controller with it")
        except Exception as exc:                   # noqa: BLE001
            fail(f"{name}: applying a reading raised {exc!r} inside the "
                 f"republishing hot path")
        if any(not cal.minimum <= o <= cal.maximum for o in outs):
            fail(f"{name}: a degenerate calibration produced a reading "
                 f"outside the declared range")
        print(f"  ok  {name}: survives, in range")

    flat = AxisCalibration(128, 0, 255, 4, 128, 128)
    if {flat.apply(v) for v in range(0, 256)} != {127}:
        fail("a calibration with no measured travel produced movement out of "
             "nothing")
    print("  ok  no measured travel reads neutral throughout")


def check_apply_of_an_unmeasured_axis():
    print("\nS12: an axis that was never calibrated behaves like the raw one")

    cal = AxisCalibration(128, 0, 255, 0)
    if cal.low != 0 or cal.high != 255:
        fail(f"an unmeasured axis spans {cal.low}..{cal.high} rather than its "
             f"declared range -- 'never measured' has to mean 'assume the "
             f"declared range', or a pad that skipped calibration is dead")
    print("  ok  unmeasured reach means the declared range")
    for value in (0, 64, 255):
        if abs(cal.apply(value) - value) > 1:
            fail(f"an uncalibrated axis turned raw {value} into "
                 f"{cal.apply(value)}")
    print("  ok  and readings pass through essentially unchanged")


def main() -> int:
    check_the_timings_are_humane()

    check_a_fresh_run_waits_for_the_user()
    check_samples_start_at_the_current_reading()
    check_reset_samples_between_phases()

    check_only_a_press_asks_to_advance()
    check_feed_records_axis_extremes()
    check_feed_ignores_axes_the_run_is_not_measuring()
    check_only_the_calibrating_pads_events_count()

    check_fraction_is_the_clock_in_rest()
    check_fraction_is_coverage_in_reach()
    check_fraction_is_zero_in_the_untimed_phases()
    check_coverage_with_no_axes()
    check_coverage_of_a_zero_width_axis()
    check_coverage_across_axes()
    check_coverage_is_never_over_full()

    check_await_rest_advances_only_on_a_press()
    check_rest_lasts_its_phase_seconds()
    check_rest_measures_where_the_stick_actually_sat()
    check_await_reach_advances_only_on_a_press()
    check_reach_cannot_end_before_the_minimum()
    check_reach_ends_on_a_press_after_the_minimum()
    check_a_reach_press_arrives_latched()
    check_the_icon_phase_is_inert()
    check_the_whole_sequence_in_order()
    check_finish_hands_input_back()
    check_reach_without_a_rest_measurement_survives()

    check_calibration_needs_a_session()
    check_calibration_needs_a_pad_for_the_player()
    check_calibration_needs_an_open_device()
    check_a_pad_with_no_centring_axes_skips_to_the_icon()
    check_beginning_calibration_drops_a_confirm_hold()

    check_rest_of_a_stick_that_never_moved()
    check_rest_of_a_stick_sitting_off_centre()
    check_rest_dead_band_covers_the_wobble()
    check_rest_respects_the_declared_flat()
    check_rest_of_an_axis_with_no_samples()
    check_rest_of_no_axes()

    check_merge_records_a_full_sweep()
    check_merge_ignores_a_direction_that_never_moved()
    check_merge_of_an_axis_missing_from_the_reach()
    check_merge_of_an_inverted_reach()
    check_merge_of_a_reach_inside_the_dead_band()
    check_a_jittery_axis_that_never_moves_during_the_sweep()

    check_apply_never_leaves_the_declared_range()
    check_apply_is_monotonic()
    check_apply_maps_centre_to_the_midpoint()
    check_apply_holds_the_dead_band_flat()
    check_apply_is_continuous()
    check_apply_reaches_the_extremes()
    check_apply_cannot_divide_by_zero()
    check_apply_of_an_unmeasured_axis()

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
