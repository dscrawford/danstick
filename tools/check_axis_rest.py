"""How a raw evdev axis becomes a binding -- measured from *rest*.

Everything here is about one number: where the axis was sitting when the
wizard opened. An analogue trigger rests at the *end* of its declared range,
not the middle, so a wizard that normalises about the midpoint reads an
untouched trigger as fully deflected. That was reported as "when I registered
a gamecube controller, pressing R causes it to stay stuck in the interface",
and it is three separate faults wearing one coat: a resting report answered a
prompt nobody had touched the pad for, the first event of a real press
recorded the direction the trigger was travelling *away* from, and the axis
could never come back near enough to the midpoint to be re-armed -- so after
one press the trigger was dead and the wizard looked frozen.

Around that sit the other axis guards, each from its own report:

  * a face-button prompt refuses an axis outright, because nudging the stick
    while being asked for "X" once bound cancel to `-a3`, after which every
    stick movement pressed cancel
  * an axis must come back near rest before it may answer again, because a
    d-pad wired to an analogue axis springs back *through* centre and
    overshoots far enough to look like a deliberate push the other way -- so
    pressing left filled in both left and right
  * re-arming has to run *inside* the capture gap, because a release takes
    roughly a tenth as long as the gap lasts, so essentially every release
    lands in one; dropping it leaves the axis disarmed with nothing left to
    re-arm it, since a settled axis stops reporting entirely
  * an axis is recorded by its position among the pad's axes, not by its
    evdev code -- ABS_RZ is code 5 and may be axis 3

Nothing here touches a real device, a real profile store or the running
daemon: every event is a synthetic object and the clock is a dict.

    python3 tools/check_axis_rest.py
"""

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import layouts  # noqa: E402
from padmap.capture import (AXIS_RELEASE, AXIS_THRESHOLD,  # noqa: E402
                            CAPTURE_GAP_SECONDS, EV_ABS, EV_KEY, Chooser,
                            MappingRun, deflection, layout_options)


class Event:
    """The two fields of an evdev event this module reads."""

    def __init__(self, type_, code, value):
        self.type = type_
        self.code = code
        self.value = value


def axis(code, value):
    return Event(EV_ABS, code, value)


def release(code):
    return Event(EV_KEY, code, 0)


KEYS = list(range(0x120, 0x130))

# {code: (minimum, maximum, rest)}.
#
# A well-behaved pad: four analogue axes on codes 0, 1, 2 and 5, all resting
# in the middle, plus the two hat codes. Note the axis codes are not
# 0,1,2,3 -- ABS_RZ is code 5 and is the *fourth* axis, which is the whole
# reason bindings are stored by index.
CENTRED = {0: (0, 255, 128), 1: (0, 255, 128), 2: (0, 255, 128),
           5: (0, 255, 128), 0x10: (-1, 1, 0), 0x11: (-1, 1, 0)}

# The same pad with analogue triggers on codes 2 and 5, resting at their
# *minimum*. Measured on the GameCube adapter here, where the untouched
# triggers read 81% deflected if you measure from the midpoint.
TRIGGERS_LOW = {**CENTRED, 2: (0, 255, 0), 5: (0, 255, 0)}

# And an adapter whose triggers rest at their *maximum* and fall as they are
# pressed. Nothing in padmap may assume which end "rest" is -- only that it is
# where the axis sat when the wizard opened.
TRIGGERS_HIGH = {**CENTRED, 2: (0, 255, 255), 5: (0, 255, 255)}

# A driver reporting a resting value that is simply wrong: the N64 adapter
# here holds a stale power-on default of 174 on an axis that really centres at
# 128, 36% out, until the stick is physically moved.
STALE_REST = {**CENTRED, 0: (0, 255, 174)}

# Round numbers, so a value can sit *exactly* on a threshold rather than a
# rounding either side of it. Half the declared range is 1000, so
# 1000 + 1000*AXIS_THRESHOLD is the first value that counts as a push whatever
# the constant is set to.
BOUNDARY = {0: (0, 2000, 1000), 1: (0, 2000, 1000),
            0x10: (-1, 1, 0), 0x11: (-1, 1, 0)}
HALF = 1000
AT_THRESHOLD = HALF + int(round(AXIS_THRESHOLD * HALF))
AT_RELEASE = HALF + int(round(AXIS_RELEASE * HALF))

CLOCK = {"t": 0.0}


def tick(seconds=0.05):
    CLOCK["t"] += seconds


def gap():
    """Wait out the settling gap, as a person moving between prompts would."""
    tick(CAPTURE_GAP_SECONDS)


def run(layout_id="snes", axes=None, held=()):
    CLOCK["t"] = 0.0
    return MappingRun(pad=None, player=1, layout=layouts.get(layout_id),
                      keys=KEYS, axes=dict(CENTRED if axes is None else axes),
                      held=set(held), now=lambda: CLOCK["t"])


def to_kind(r, kind):
    """Skip forward to the first prompt of a given kind.

    Every layout opens with face buttons, which refuse axes on purpose, so an
    axis test has to get past them first.
    """
    while r.current is not None and r.current.kind != kind:
        r.skip()
        gap()
    return r


def sdl_at(r, index):
    """What was recorded for the control at `index` in the layout."""
    return r.bindings[r.layout.controls[index].canonical].sdl()


def close(value, wanted, tolerance=1e-9):
    return abs(value - wanted) <= tolerance


# --------------------------------------------------------------------------
# deflection(): the arithmetic, before any of the wizard's state
# --------------------------------------------------------------------------

def check_deflection_centred() -> None:
    print("\nan axis resting in the middle of its range:")
    span = (0, 200, 100)
    for value, wanted in ((100, 0.0), (200, 1.0), (0, -1.0),
                          (150, 0.5), (50, -0.5)):
        got = deflection(span, value)
        if not close(got, wanted):
            raise SystemExit(
                f"FAIL: a stick at {value} of 0-200 reads {got:.3f}, not "
                f"{wanted} -- every threshold in the wizard is a fraction of "
                f"this, so the pad answers prompts at the wrong moment")
    print("  ok  0 at rest, +-1 at the ends, signed by direction")


def check_deflection_trigger_at_minimum() -> None:
    """The GameCube trigger, and the whole reason this function exists."""
    print("\na trigger resting at its minimum reads as untouched:")
    span = (0, 200, 0)
    if deflection(span, 0) != 0.0:
        raise SystemExit(
            f"FAIL: an untouched trigger reads {deflection(span, 0):.3f}. "
            f"Measured from the midpoint it reads fully deflected, and a "
            f"resting report -- drivers emit them -- then answers a prompt "
            f"nobody has touched the pad for.")
    print("  ok  0.0 at rest, not -1.0")

    print("\n...and reads 2.0 when fully pressed, scaled by half the range:")
    if not close(deflection(span, 200), 2.0):
        raise SystemExit(
            f"FAIL: a fully pressed trigger reads "
            f"{deflection(span, 200):.3f}, not 2.0")
    if not close(deflection(span, 100), 1.0):
        raise SystemExit("FAIL: half pressed does not read 1.0")
    if deflection(span, 100) <= 0:
        raise SystemExit(
            "FAIL: pressing a trigger reads negative -- the binding would "
            "record the direction it was travelling away from")
    print("  ok  +1.0 half way, +2.0 at the end, positive throughout")


def check_deflection_trigger_at_maximum() -> None:
    """The same trigger wired the other way up. Rest is not 'the minimum'."""
    print("\na trigger resting at its maximum is untouched there instead:")
    span = (0, 200, 200)
    if deflection(span, 200) != 0.0:
        raise SystemExit(
            f"FAIL: reads {deflection(span, 200):.3f} at rest -- an inverted "
            f"trigger would answer every prompt the moment it is left alone")
    if not close(deflection(span, 0), -2.0):
        raise SystemExit(
            f"FAIL: fully pressed reads {deflection(span, 0):.3f}, not -2.0")
    if deflection(span, 100) >= 0:
        raise SystemExit(
            "FAIL: pressing an inverted trigger reads positive, so the "
            "binding records the direction it came from")
    print("  ok  0.0 at rest, -2.0 pressed; the sign follows the travel")


def check_deflection_scaling_is_by_half_range() -> None:
    """Scaled by half the *declared* range, not the travel actually left.

    Normalising by the travel available in the direction of movement would
    make an off-centre stick need a much bigger push on its long side than its
    short one, which is the opposite of what a user expects from a stick that
    merely sits a little off.
    """
    print("\nequal travel reads equal, whichever side of rest it is on:")
    span = (0, 200, 60)             # rests 40 units below centre
    up = deflection(span, 60 + 55)
    down = deflection(span, 60 - 55)
    if not close(up, -down):
        raise SystemExit(
            f"FAIL: 55 units up reads {up:.3f} but 55 units down reads "
            f"{down:.3f} -- an off-centre stick would need a different push "
            f"in each direction to answer the same prompt")
    if not close(abs(up), 0.55):
        raise SystemExit(
            f"FAIL: 55 of a 100 half-range reads {up:.3f}, not 0.55 -- "
            f"scaling by the travel that is left instead would make the "
            f"short side hair-trigger")
    print("  ok  +-0.55 either way from a rest 20% off centre")


def check_deflection_degenerate_spans() -> None:
    """Absinfo a driver got wrong. Nothing may divide by zero or go wild."""
    print("\nan axis whose declared range is empty or inverted:")
    for span in ((0, 0, 0), (7, 7, 7), (200, 0, 100), (255, -255, 0)):
        got = deflection(span, 99999)
        if got != 0.0:
            raise SystemExit(
                f"FAIL: span {span} with a wild value reads {got!r}. A pad "
                f"whose driver reports nonsense absinfo must go quiet, not "
                f"fill the whole layout in by itself.")
    print("  ok  0.0 for max <= min, at any value")


def check_deflection_threshold_is_a_floor() -> None:
    """Exactly on the threshold counts; a hair under does not."""
    print("\nthe push threshold, to the last unit:")
    span = BOUNDARY[0]
    if deflection(span, AT_THRESHOLD) < AXIS_THRESHOLD:
        raise SystemExit(
            "FAIL: a push of exactly the advertised distance does not reach "
            "the threshold, so the pad is deader than the numbers say")
    if abs(deflection(span, AT_THRESHOLD - 1)) >= AXIS_THRESHOLD:
        raise SystemExit(
            "FAIL: a lean one unit short of the threshold counts as a push, "
            "so a stick that merely rests off centre binds itself")
    print(f"  ok  {AT_THRESHOLD} pushes, {AT_THRESHOLD - 1} does not")


# --------------------------------------------------------------------------
# MappingRun: which axis events become bindings
# --------------------------------------------------------------------------

def check_button_prompt_refuses_an_axis() -> None:
    """Reported as cancel bound to `-a3`, then firing on every stick nudge."""
    print("\na face-button prompt refuses a nudged axis, and every hat:")
    # 217 and 39 are 0.70 of half-range either side of rest 128: past
    # AXIS_THRESHOLD, so they would answer a stick, d-pad or shoulder prompt,
    # and short of AXIS_AS_BUTTON_THRESHOLD. The hat is refused at any value,
    # deliberate or not -- a d-pad direction answering a face button has never
    # been anything but a mistake, and a device reporting a hat it does not
    # have would otherwise fill face buttons in from noise.
    r = run("snes", axes=CENTRED)
    if r.current.kind != "button":
        raise SystemExit("FAIL: the layout no longer opens on a face button")
    for event in (axis(0, 217), axis(0, 39), axis(1, 217), axis(2, 39),
                  axis(5, 217), axis(0x10, 1), axis(0x11, -1)):
        if r.feed(event):
            raise SystemExit(
                f"FAIL: a nudge answered a face-button prompt (code "
                f"{event.code}, value {event.value}). The axis is the stick, "
                f"so every later stick movement then presses that button.")
    if r.index != 0 or r.bindings:
        raise SystemExit(
            f"FAIL: a face-button prompt was filled in by a stick "
            f"({r.bindings})")
    print("  ok  nudged sticks and triggers ignored, hats ignored outright")

    print("\n...and accepts one held against the stop:")
    # The N64-on-GameCube case: that pad reports its C cluster as ABS_Z/ABS_RZ
    # and has no X or Y at all, so an axis is the only answer available. A
    # flat refusal left those prompts unanswerable and silent.
    # A run of its own: `r` must keep pointing at the one whose nudge was
    # refused, because the next property is about what that refusal cost it.
    deliberate = run("snes", axes=CENTRED)
    if not deliberate.feed(axis(0, 255)):
        raise SystemExit(
            "FAIL: an axis at the stop did not answer a face-button prompt; a "
            "pad whose only spare inputs are axes cannot finish the wizard")
    print("  ok  a deliberate push is taken")

    print("\n...and refusing costs the axis nothing later on:")
    # The refusal must not claim the code or disarm it, or the stick the user
    # nudged by accident is dead by the time it is actually asked for.
    to_kind(r, "dpad")
    at = r.index
    if not r.feed(axis(0, 255)):
        raise SystemExit(
            "FAIL: the stick nudged during a face-button prompt no longer "
            "works when the d-pad is asked for")
    if sdl_at(r, at) != "+a0":
        raise SystemExit(f"FAIL: recorded {sdl_at(r, at)!r}, wanted '+a0'")
    print("  ok  the same axis records +a0 at the next d-pad prompt")


def check_kinds_that_accept_an_axis() -> None:
    """Only "button" refuses. A d-pad, a shoulder and a stick all accept."""
    print("\nthe prompts an axis may answer:")
    for layout_id, kind, wanted in (("snes", "dpad", "+a0"),
                                    ("gamecube", "shoulder", "+a0"),
                                    ("n64", "stick", "+a0")):
        r = to_kind(run(layout_id, axes=CENTRED), kind)
        at = r.index
        if not r.feed(axis(0, 255)):
            raise SystemExit(
                f"FAIL: a {kind} control on the {layout_id} pad refused an "
                f"axis, so a stick-only pad can never finish the wizard")
        if sdl_at(r, at) != wanted:
            raise SystemExit(f"FAIL: {kind} recorded {sdl_at(r, at)!r}")
        print(f"  ok  {layout_id} {kind!r} prompt records {wanted}")


def check_axis_index_is_by_position() -> None:
    """ABS_RZ is code 5 and may be axis 3."""
    print("\nan axis is numbered by position among the pad's axes:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(5, 255))
    if sdl_at(r, at) != "+a3":
        raise SystemExit(
            f"FAIL: ABS_RZ recorded as {sdl_at(r, at)!r}. It is evdev code 5 "
            f"but the fourth axis, and SDL will look for a3 -- a mapping "
            f"naming a5 simply never fires.")
    print("  ok  code 5 of {0,1,2,5} is a3")

    print("\nhat codes take no axis number:")
    # SDL and RetroArch both number axes among the *real* axes and treat the
    # hat codes as a hat. Counting them shifts every axis after the hat.
    hatty = {0: (0, 255, 128), 0x10: (-1, 1, 0), 0x11: (-1, 1, 0),
             5: (0, 255, 128)}
    r = to_kind(run("snes", axes=hatty), "dpad")
    at = r.index
    r.feed(axis(5, 255))
    if sdl_at(r, at) != "+a1":
        raise SystemExit(
            f"FAIL: with hats in the absinfo, code 5 recorded as "
            f"{sdl_at(r, at)!r} rather than '+a1'")
    print("  ok  {0,0x10,0x11,5} makes code 5 a1, not a3")

    print("\nthe order the driver happened to report axes in does not count:")
    shuffled = {5: (0, 255, 128), 1: (0, 255, 128), 3: (0, 255, 128)}
    r = to_kind(run("snes", axes=shuffled), "dpad")
    at = r.index
    r.feed(axis(5, 255))
    if sdl_at(r, at) != "+a2":
        raise SystemExit(
            f"FAIL: code 5 of a pad reporting {{5,1,3}} recorded as "
            f"{sdl_at(r, at)!r}; by ascending code it is a2")
    print("  ok  {5,1,3} sorts to 1,3,5 so code 5 is a2")

    print("\nan axis the pad never declared is not a binding:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    for value in (255, 0, 128):
        if r.feed(axis(9, value)):
            raise SystemExit(
                "FAIL: an axis with no absinfo was bound anyway -- there is "
                "no range to judge it against, so it is noise")
    if r.index != at:
        raise SystemExit("FAIL: an undeclared axis advanced the wizard")
    print("  ok  code 9, undeclared, ignored at every value")


def check_degenerate_span_in_a_run() -> None:
    print("\na pad whose driver reports an empty range for an axis:")
    broken = {0: (0, 0, 0), 1: (0, 255, 128), 0x10: (-1, 1, 0)}
    r = to_kind(run("snes", axes=broken), "dpad")
    at = r.index
    for value in (0, 255, -255, 99999):
        if r.feed(axis(0, value)):
            raise SystemExit(
                "FAIL: an axis with min == max answered a prompt. Its "
                "readings mean nothing, and one of them would then be "
                "claimed as a real control.")
    if r.index != at or r.bindings:
        raise SystemExit(f"FAIL: {r.bindings} bound from a degenerate axis")
    if not r.feed(axis(1, 255)):
        raise SystemExit(
            "FAIL: one broken axis stopped the working ones being read")
    print("  ok  the broken axis is silent, the sound one still binds")


def check_threshold_both_signs() -> None:
    """Exactly on the threshold counts, and it counts the same either way."""
    print("\nhow far an axis must travel before it is a push:")
    r = to_kind(run("snes", axes=BOUNDARY), "dpad")
    at = r.index
    if r.feed(axis(0, AT_THRESHOLD - 1)):
        raise SystemExit(
            "FAIL: a lean short of the threshold was recorded -- a stick that "
            "merely rests off centre would bind itself to the first control")
    if not r.feed(axis(0, AT_THRESHOLD)):
        raise SystemExit(
            "FAIL: a push exactly on the threshold was refused, so the "
            "deadband is wider than it says it is")
    if sdl_at(r, at) != "+a0":
        raise SystemExit(f"FAIL: recorded {sdl_at(r, at)!r}, wanted '+a0'")
    print(f"  ok  {AT_THRESHOLD - 1} ignored, {AT_THRESHOLD} recorded as +a0")

    # Settle *after* the gap, so this scenario turns on the threshold alone
    # and not on whether releases are noticed while the gap is open -- that
    # is check_rearming_runs_inside_the_gap's job.
    gap()
    r.feed(axis(0, HALF))           # back to rest: armed again
    if r.feed(axis(0, HALF - int(round(AXIS_THRESHOLD * HALF)) + 1)):
        raise SystemExit("FAIL: a lean the other way was recorded too")
    if not r.feed(axis(0, HALF - int(round(AXIS_THRESHOLD * HALF)))):
        raise SystemExit("FAIL: a push exactly on the threshold, negative "
                         "side, was refused")
    if sdl_at(r, at + 1) != "-a0":
        raise SystemExit(
            f"FAIL: pushed the other way recorded {sdl_at(r, at + 1)!r}; the "
            f"two halves of one axis must be distinguishable or left and "
            f"right end up on the same binding")
    print("  ok  the same distance the other way records -a0")


def check_noise_at_rest_is_never_a_push() -> None:
    print("\nthe jitter a pad streams while nobody is touching it:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    for value in (128, 130, 126, 131, 127, 129, 128):
        r.feed(axis(0, value))
    if r.index != at or r.bindings:
        raise SystemExit(f"FAIL: idle jitter filled in {r.bindings}")
    print("  ok  seven idle samples, nothing recorded")


def check_resting_trigger_is_never_a_push() -> None:
    """Fault one of three: a resting report answering a prompt by itself."""
    print("\nthe resting reports an analogue trigger emits:")
    r = to_kind(run("gamecube", axes=TRIGGERS_LOW), "shoulder")
    at = r.index
    for _ in range(3):
        r.feed(axis(2, 0))
        r.feed(axis(5, 0))
        r.feed(axis(2, 1))          # a single unit of noise at the stop
    if r.index != at or r.bindings:
        raise SystemExit(
            f"FAIL: a trigger sitting at its resting value answered "
            f"{len(r.bindings)} prompt(s) with nobody touching the pad "
            f"({r.bindings})")
    print("  ok  nine resting samples from two triggers, nothing recorded")

    print("\nand from a trigger that rests at the *other* end:")
    r = to_kind(run("gamecube", axes=TRIGGERS_HIGH), "shoulder")
    at = r.index
    for _ in range(3):
        r.feed(axis(2, 255))
        r.feed(axis(5, 254))
    if r.index != at or r.bindings:
        raise SystemExit(
            f"FAIL: an inverted trigger at rest answered a prompt "
            f"({r.bindings})")
    print("  ok  six resting samples at the maximum, nothing recorded")


def check_trigger_press_records_the_way_it_travelled() -> None:
    """Fault two: recording the direction the trigger came *from*.

    A press arrives as a stream, and its *first* events are the ones nearest
    rest. Measured from the midpoint those read as a large deflection of the
    wrong sign, so the binding recorded the direction the trigger had just
    left -- and the control then fired when the trigger was released.
    """
    print("\npressing a trigger that rests at its minimum:")
    r = to_kind(run("gamecube", axes=TRIGGERS_LOW), "shoulder")
    at = r.index
    for value in (0, 8, 20, 60, 140, 255):
        r.feed(axis(2, value))
    if r.index != at + 1:
        raise SystemExit("FAIL: pressing the trigger recorded nothing")
    if sdl_at(r, at) != "+a2":
        raise SystemExit(
            f"FAIL: recorded {sdl_at(r, at)!r}, wanted '+a2' -- a trigger "
            f"pushed towards its maximum is a positive deflection")
    print("  ok  +a2, from a stream starting at the resting value")

    print("\npressing one that rests at its maximum:")
    r = to_kind(run("gamecube", axes=TRIGGERS_HIGH), "shoulder")
    at = r.index
    for value in (255, 247, 235, 195, 115, 0):
        r.feed(axis(2, value))
    if r.index != at + 1:
        raise SystemExit("FAIL: pressing the inverted trigger recorded "
                         "nothing")
    if sdl_at(r, at) != "-a2":
        raise SystemExit(
            f"FAIL: recorded {sdl_at(r, at)!r}, wanted '-a2'. This trigger "
            f"falls as it is pressed; a binding with the other sign fires "
            f"when the user lets go.")
    print("  ok  -a2, the sign following the travel and not the code")


def check_both_triggers_keep_working() -> None:
    """Fault three: after one press the trigger was dead, and so was the pad.

    The re-arming rule wanted the axis back near *centre*, and a trigger at
    rest is a full range away from centre. Every later press was dropped in
    silence, which is what "stuck in the interface" looked like.
    """
    print("\npressing both triggers in turn, with the release in between:")
    r = to_kind(run("gamecube", axes=TRIGGERS_LOW), "shoulder")
    at = r.index
    for value in (10, 80, 255):
        r.feed(axis(2, value))
    for value in (200, 90, 0):      # let go; lands inside the capture gap
        r.feed(axis(2, value))
    gap()
    for value in (10, 80, 255):
        r.feed(axis(5, value))
    for value in (200, 90, 0):
        r.feed(axis(5, value))
    gap()
    if r.index != at + 2:
        raise SystemExit(
            f"FAIL: two trigger presses filled {r.index - at} control(s). "
            f"This is the pad going dead partway through the wizard.")
    if sdl_at(r, at) != "+a2" or sdl_at(r, at + 1) != "+a3":
        raise SystemExit(
            f"FAIL: recorded {sdl_at(r, at)!r} and {sdl_at(r, at + 1)!r}, "
            f"wanted '+a2' and '+a3'")
    print("  ok  +a2 then +a3, distinct, both after settling at rest")


def check_axis_must_return_to_rest() -> None:
    """The N64 d-pad: pressing left filled in both left and right."""
    print("\none push must not answer two prompts:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0, 0))              # pushed left, recorded
    if r.index != at + 1:
        raise SystemExit("FAIL: a full deflection was not recorded")
    gap()                           # the gap is not what is under test here
    if r.feed(axis(0, 255)):
        raise SystemExit(
            "FAIL: the spring-back overshoot answered the next prompt -- one "
            "press filled in both left and right, exactly as reported")
    if r.feed(axis(0, 255)):
        raise SystemExit("FAIL: the overshoot was accepted on a second report")
    print("  ok  a full deflection the other way is refused while unsettled")

    print("\n...and the axis works again once it has passed through rest:")
    r.feed(axis(0, 128))
    gap()
    if not r.feed(axis(0, 255)):
        raise SystemExit(
            "FAIL: the axis never re-armed, so the stick answered one prompt "
            "and then went dead for the rest of the wizard")
    if sdl_at(r, at) == sdl_at(r, at + 1):
        raise SystemExit(
            f"FAIL: both directions recorded as {sdl_at(r, at)!r}")
    print(f"  ok  {sdl_at(r, at)} then {sdl_at(r, at + 1)}, one per push")


def check_rearm_threshold_both_sides() -> None:
    """How near rest is near enough, to the last unit."""
    print("\nhow far back an axis must come before it may answer again:")
    r = to_kind(run("snes", axes=BOUNDARY), "dpad")
    at = r.index
    r.feed(axis(0, 2000))           # recorded, and now disarmed
    if r.index != at + 1:
        raise SystemExit("FAIL: a full deflection was not recorded")
    r.feed(axis(0, AT_RELEASE))     # exactly on the release threshold
    gap()
    if r.feed(axis(0, 0)):
        raise SystemExit(
            f"FAIL: an axis that only came back to {AT_RELEASE} of a "
            f"{HALF} half-range was treated as released. A d-pad on an "
            f"analogue axis leans this far while still held.")
    r.feed(axis(0, AT_RELEASE - 1))     # one unit nearer: released
    if not r.feed(axis(0, 0)):
        raise SystemExit(
            "FAIL: an axis back inside the release threshold still refused a "
            "deliberate push, so the stick is dead from here on")
    print(f"  ok  {AT_RELEASE} is still held, {AT_RELEASE - 1} is released")


def check_rearming_runs_inside_the_gap() -> None:
    """A release lasts a tenth of the gap, so nearly every one lands in it.

    Refusing the event *without* noting the release leaves the axis disarmed
    with nothing left to re-arm it: the stick settles and stops reporting, and
    the wizard ignores it for good.
    """
    print("\nthe release lands inside the capture gap, and still counts:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0, 255))            # recorded; the gap opens
    if r.index != at + 1:
        raise SystemExit("FAIL: the first push was not recorded")
    r.feed(axis(0, 128))            # let go, well inside the gap
    gap()
    # One event, far from rest. Nothing else can re-arm the axis: a settled
    # stick reports nothing at all until it is moved again.
    if not r.feed(axis(0, 0)):
        raise SystemExit(
            "FAIL: the release was swallowed by the capture gap, so the axis "
            "stayed disarmed and the next push was dropped in silence -- the "
            "wizard looks frozen with the pad plainly working")
    print("  ok  a single settle event during the gap re-arms the axis")

    print("\nwithout that release the axis stays disarmed, as it should:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0, 255))
    gap()                           # no settle report at all
    if r.feed(axis(0, 0)):
        raise SystemExit(
            "FAIL: an axis that never returned to rest answered a second "
            "prompt -- the arming rule is doing nothing")
    if r.index != at + 1:
        raise SystemExit("FAIL: a second control was filled in")
    print("  ok  refused, which is what makes the release above meaningful")


def check_gap_and_arming_are_separate_rules() -> None:
    """The spring-back passes *through* rest, so arming alone cannot stop it.

    Releasing an analogue d-pad reports every value on the way back, including
    centre, which re-arms the axis a moment before the overshoot arrives. Only
    the gap stops the overshoot being read as a deliberate push.
    """
    print("\na spring-back reports centre on its way past:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0, 0))              # pushed left, recorded
    for value in (40, 100, 128, 170, 220, 255):     # released, overshooting
        if r.feed(axis(0, value)):
            raise SystemExit(
                f"FAIL: value {value} of a release answered the next prompt")
    if r.index != at + 1:
        raise SystemExit(
            "FAIL: letting go of the d-pad filled in a second control. The "
            "release re-arms the axis on its way through centre, so the "
            "capture gap is the only thing standing between the overshoot "
            "and a wrong binding.")
    print("  ok  a whole release, overshoot included, filled nothing in")

    print("\nnothing at all is accepted while the gap is open:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0, 255))            # recorded, gap opens
    tick(0.05)
    if r.feed(axis(1, 255)):        # a different, fully armed axis
        raise SystemExit(
            "FAIL: a second axis answered the very next prompt while the "
            "user was still letting go of the first")
    tick(0.05)
    if r.feed(axis(0x11, -1)):      # and a hat, also armed
        raise SystemExit("FAIL: a hat was accepted inside the capture gap")
    print("  ok  another axis and a hat both refused during the gap")

    print("\n...and the axis refused during the gap is not poisoned by it:")
    gap()
    if not r.feed(axis(1, 255)):
        raise SystemExit(
            "FAIL: an axis pushed during the gap is ignored ever afterwards, "
            "so the user has to guess which inputs the wizard has decided to "
            "stop listening to")
    if sdl_at(r, at + 1) != "+a1":
        raise SystemExit(f"FAIL: recorded {sdl_at(r, at + 1)!r}")
    print("  ok  the same push lands as +a1 once the gap closes")


def check_arming_is_per_axis() -> None:
    print("\ncapturing on one axis leaves the others alone:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0, 255))            # axis 0 recorded and disarmed
    gap()
    if not r.feed(axis(1, 255)):
        raise SystemExit(
            "FAIL: pushing one stick axis disarmed another, so half the pad "
            "goes quiet after the first binding")
    gap()
    if r.feed(axis(0, 0)):
        raise SystemExit(
            "FAIL: axis 0 answered again without ever returning to rest -- "
            "arming is being tracked for the run rather than per axis")
    if sdl_at(r, at) != "+a0" or sdl_at(r, at + 1) != "+a1":
        raise SystemExit(
            f"FAIL: recorded {sdl_at(r, at)!r} and {sdl_at(r, at + 1)!r}")
    print("  ok  axis 1 armed, axis 0 still held, +a0 then +a1")


def check_repeated_cycles_across_axes() -> None:
    """Four prompts filled by two sticks, pressed and released in turn."""
    print("\nfour d-pad prompts, four separate press-and-release cycles:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    for code, value in ((0, 255), (1, 255), (0, 0), (1, 0)):
        for step in (140, 180, value):
            r.feed(axis(code, step))
        for step in (150, 130, 128):    # released, inside the gap
            r.feed(axis(code, step))
        gap()
    recorded = [sdl_at(r, at + n) for n in range(4)]
    if recorded != ["+a0", "+a1", "-a0", "-a1"]:
        raise SystemExit(
            f"FAIL: four pushes recorded {recorded}. Anything but one binding "
            f"per push leaves a mapping that looks complete and is wrong.")
    if len(set(recorded)) != 4:
        raise SystemExit(f"FAIL: {recorded} are not four distinct bindings")
    print(f"  ok  {recorded}")


def check_one_direction_cannot_answer_twice() -> None:
    print("\nthe same push cannot fill in two controls:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0, 255))
    r.feed(axis(0, 128))            # settled, so arming is not what refuses
    gap()
    if r.feed(axis(0, 255)):
        raise SystemExit(
            "FAIL: +a0 was accepted for a second control. A stick left leaning "
            "would fill in the rest of the layout with one direction.")
    if r.index != at + 1:
        raise SystemExit("FAIL: a claimed direction advanced the wizard")
    if not r.feed(axis(0, 0)):
        raise SystemExit(
            "FAIL: the opposite direction of the same axis was refused too, "
            "so a stick can only ever bind two of the four d-pad controls")
    print("  ok  +a0 refused a second time, -a0 still available")


# --------------------------------------------------------------------------
# Hats: the other kind of axis event, with its own rules
# --------------------------------------------------------------------------

def check_hat_bits() -> None:
    print("\na hat records as a hat, with SDL's own bits:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    for code, value in ((0x11, -1), (0x11, 1), (0x10, -1), (0x10, 1)):
        if not r.feed(axis(code, value)):
            raise SystemExit(
                f"FAIL: hat code {code:#x} value {value} was not recorded")
        r.feed(axis(code, 0))       # released, inside the gap
        gap()
    recorded = [sdl_at(r, at + n) for n in range(4)]
    if recorded != ["h0.1", "h0.4", "h0.8", "h0.2"]:
        raise SystemExit(
            f"FAIL: up/down/left/right recorded {recorded}, wanted "
            f"['h0.1', 'h0.4', 'h0.8', 'h0.2'] -- SDL reads these as bits, "
            f"and a wrong one points the d-pad somewhere else entirely")
    ra = [r.bindings[r.layout.controls[at + n].canonical].retroarch()
          for n in range(4)]
    if ra != ["h0up", "h0down", "h0left", "h0right"]:
        raise SystemExit(f"FAIL: RetroArch would be given {ra}")
    print("  ok  h0.1/h0.4/h0.8/h0.2 for SDL, h0up/down/left/right for "
          "RetroArch")


def check_hat_is_never_an_axis() -> None:
    print("\na hat is not recorded as an axis, even with absinfo for it:")
    # CENTRED declares ranges for 0x10 and 0x11, as real drivers do. Reading
    # them as ordinary axes would number them as axes -- which SDL never
    # does -- and the d-pad would be published as a stick nothing reads.
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0x10, 1))
    binding = r.bindings[r.layout.controls[at].canonical]
    if binding.kind != "hat" or binding.sdl() != "h0.2":
        raise SystemExit(
            f"FAIL: the d-pad was recorded as {binding.kind} "
            f"{binding.sdl()!r}, not the hat it is")
    print("  ok  kind 'hat', h0.2, despite absinfo declaring a range")


def check_hat_rearms_only_at_zero() -> None:
    print("\na hat must report 0 before it may answer again:")
    # No absinfo for the hat codes here, which is realistic and also the
    # point: a hat's release is the value 0, not a fraction of a range, and
    # there is nothing to measure a deflection against.
    no_hat_info = {0: (0, 255, 128), 1: (0, 255, 128)}
    r = to_kind(run("snes", axes=no_hat_info), "dpad")
    at = r.index
    r.feed(axis(0x10, -1))
    if r.index != at + 1:
        raise SystemExit("FAIL: a hat push was not recorded")
    gap()
    if r.feed(axis(0x10, 1)):
        raise SystemExit(
            "FAIL: a hat bouncing the other way on release answered the next "
            "prompt -- a cheap adapter reports exactly this, and one press "
            "then filled in both left and right")
    if r.feed(axis(0x10, 0)):
        raise SystemExit("FAIL: releasing a hat was recorded as a binding")
    if not r.feed(axis(0x10, 1)):
        raise SystemExit(
            "FAIL: the hat never re-armed, so the d-pad answers one prompt "
            "and then goes dead for the rest of the wizard")
    if sdl_at(r, at) != "h0.8" or sdl_at(r, at + 1) != "h0.2":
        raise SystemExit(
            f"FAIL: recorded {sdl_at(r, at)!r} and {sdl_at(r, at + 1)!r}")
    print("  ok  bounce refused, 0 releases, the next push records h0.2")


def check_hat_arming_is_per_code() -> None:
    print("\nthe two hat axes arm independently:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0x10, -1))          # left, recorded; 0x10 now disarmed
    gap()
    if not r.feed(axis(0x11, -1)):
        raise SystemExit(
            "FAIL: pushing the hat left stopped it being pushed up, so half "
            "the d-pad cannot be captured")
    gap()
    if r.feed(axis(0x10, 1)):
        raise SystemExit(
            "FAIL: 0x10 answered again without ever reporting 0")
    if sdl_at(r, at) != "h0.8" or sdl_at(r, at + 1) != "h0.1":
        raise SystemExit(
            f"FAIL: recorded {sdl_at(r, at)!r} and {sdl_at(r, at + 1)!r}")
    print("  ok  h0.8 then h0.1, with 0x10 still held")


def check_hat_claims_each_direction_once() -> None:
    print("\none hat direction cannot answer two prompts:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0x10, 1))
    r.feed(axis(0x10, 0))
    gap()
    if r.feed(axis(0x10, 1)):
        raise SystemExit(
            "FAIL: hat right was accepted for a second control, so a hat "
            "stuck one way fills in the rest of the layout")
    if not r.feed(axis(0x10, -1)):
        raise SystemExit("FAIL: the opposite direction was refused as well")
    if sdl_at(r, at) != "h0.2" or sdl_at(r, at + 1) != "h0.8":
        raise SystemExit(
            f"FAIL: recorded {sdl_at(r, at)!r} and {sdl_at(r, at + 1)!r}")
    print("  ok  h0.2 refused a second time, h0.8 still available")


def check_hat_magnitude_does_not_matter() -> None:
    print("\na hat reporting more than one unit is still a direction:")
    r = to_kind(run("snes", axes=CENTRED), "dpad")
    at = r.index
    r.feed(axis(0x10, 3))
    if sdl_at(r, at) != "h0.2":
        raise SystemExit(
            f"FAIL: a hat reporting 3 recorded {sdl_at(r, at)!r}. Only the "
            f"sign says which way it went; a driver free to report a bigger "
            f"number must not produce a hat bit nothing recognises.")
    print("  ok  value 3 is still h0.2 (right)")


# --------------------------------------------------------------------------
# Rest as the driver reported it, which may be wrong
# --------------------------------------------------------------------------

def check_stale_rest_is_survivable() -> None:
    """The N64 adapter: 174 claimed on an axis that really centres at 128."""
    print("\na stick whose driver reports a resting value 36% out:")
    r = to_kind(run("snes", axes=STALE_REST), "dpad")
    at = r.index
    for value in (124, 128, 132, 128):
        r.feed(axis(0, value))
    if r.index != at:
        raise SystemExit(
            "FAIL: a stick sitting at its true centre answered a prompt "
            "because the driver claimed rest was somewhere else. The push "
            "threshold has to be generous enough to absorb that error.")
    print("  ok  the true centre still reads as untouched")

    print("\n...and both directions are still usable from that rest:")
    for value in (200, 240, 255):
        r.feed(axis(0, value))
    if r.index != at + 1:
        raise SystemExit("FAIL: a push away from the reported rest was "
                         "ignored")
    for value in (230, 190, 174, 150, 128):
        # A release reports every value on the way back, so it passes through
        # whatever the driver called rest whether or not the stick settles
        # there. That is why the release threshold needs no extra headroom
        # for a stale rest, and it all lands inside the capture gap.
        r.feed(axis(0, value))
    gap()
    for value in (100, 40, 0):
        r.feed(axis(0, value))
    if r.index != at + 2:
        raise SystemExit(
            "FAIL: the axis never re-armed, so the stick answered one prompt "
            "and then went dead")
    if sdl_at(r, at) == sdl_at(r, at + 1):
        raise SystemExit(
            f"FAIL: both directions recorded as {sdl_at(r, at)!r}")
    print(f"  ok  {sdl_at(r, at)} then {sdl_at(r, at + 1)}, from a rest 36% "
          f"out")


def check_settling_blocks_axes_too() -> None:
    print("\nan axis nudged while the opening press is still held:")
    r = to_kind(run("snes", axes=CENTRED, held={0x121}), "dpad")
    at = r.index
    if r.feed(axis(0, 255)):
        raise SystemExit(
            "FAIL: a stick answered the first prompt while the button that "
            "opened the wizard was still down")
    r.feed(release(0x121))
    if not r.feed(axis(0, 255)):
        raise SystemExit(
            "FAIL: the stick is still ignored after the opening press was "
            "let go, so the wizard cannot be started at all")
    if sdl_at(r, at) != "+a0":
        raise SystemExit(f"FAIL: recorded {sdl_at(r, at)!r}")
    print("  ok  refused while settling, +a0 immediately afterwards")


def check_nothing_recorded_after_the_run_ends() -> None:
    print("\naxis events after the last control are dropped:")
    r = run("snes", axes=CENTRED)
    while not r.finished:
        r.skip()
        gap()
    before = dict(r.bindings)
    for event in (axis(0, 255), axis(0, 0), axis(0x10, 1), axis(2, 255)):
        if r.feed(event):
            raise SystemExit("FAIL: an axis was recorded past the end of the "
                             "layout")
    if r.bindings != before:
        raise SystemExit(f"FAIL: bindings changed after the run finished "
                         f"({r.bindings})")
    print("  ok  four axis events after the end, nothing changed")


# --------------------------------------------------------------------------
# The picker, which reads the same axes under a deliberately different rule
# --------------------------------------------------------------------------

def choice(axes):
    CLOCK["t"] = 0.0
    return Chooser(pad=None, player=1, options=layout_options(),
                   axes=dict(axes), now=lambda: CLOCK["t"])


def check_picker_reads_from_rest_too() -> None:
    print("\nthe console picker also measures from rest:")
    # An adapter holding a stale power-on default far off centre. Measured
    # from the midpoint this reads as a push held permanently to one side,
    # and the picker walks the list on its own before anyone touches it.
    stale = {**CENTRED, 0: (0, 255, 210)}
    c = choice(stale)
    for value in (210, 209, 211, 210):
        c.feed(axis(0, value))
    if c.index != 0:
        raise SystemExit(
            f"FAIL: the picker moved to {c.index} on resting reports alone -- "
            f"the user cannot choose a console from a strip that scrolls by "
            f"itself")
    print("  ok  four resting samples, the selection did not move")

    print("\n...and a real push from that rest still moves it:")
    if not c.feed(axis(0, 0)):
        raise SystemExit(
            "FAIL: a full push left did not move the picker, so a pad with a "
            "stale resting value cannot be set up at all")
    print(f"  ok  pushed left, now on {c.chosen!r}")


def check_picker_ignores_other_axes() -> None:
    print("\nonly the horizontal axis moves the picker:")
    c = choice(CENTRED)
    for code in (1, 2, 5, 0x11):
        if c.feed(axis(code, 255)) or c.feed(axis(code, 0)):
            raise SystemExit(
                f"FAIL: axis {code:#x} moved the selection. Only ABS_X and "
                f"ABS_HAT0X are horizontal on every pad; anything else is a "
                f"guess, and a trigger held down would scroll the strip.")
    if c.index != 0:
        raise SystemExit(f"FAIL: the picker moved to {c.index}")
    print("  ok  vertical and trigger axes leave it alone")


def main() -> int:
    check_deflection_centred()
    check_deflection_trigger_at_minimum()
    check_deflection_trigger_at_maximum()
    check_deflection_scaling_is_by_half_range()
    check_deflection_degenerate_spans()
    check_deflection_threshold_is_a_floor()

    check_button_prompt_refuses_an_axis()
    check_kinds_that_accept_an_axis()
    check_axis_index_is_by_position()
    check_degenerate_span_in_a_run()
    check_threshold_both_signs()
    check_noise_at_rest_is_never_a_push()

    check_resting_trigger_is_never_a_push()
    check_trigger_press_records_the_way_it_travelled()
    check_both_triggers_keep_working()

    check_axis_must_return_to_rest()
    check_rearm_threshold_both_sides()
    check_rearming_runs_inside_the_gap()
    check_gap_and_arming_are_separate_rules()
    check_arming_is_per_axis()
    check_repeated_cycles_across_axes()
    check_one_direction_cannot_answer_twice()

    check_hat_bits()
    check_hat_is_never_an_axis()
    check_hat_rearms_only_at_zero()
    check_hat_arming_is_per_code()
    check_hat_claims_each_direction_once()
    check_hat_magnitude_does_not_matter()

    check_stale_rest_is_survivable()
    check_settling_blocks_axes_too()
    check_nothing_recorded_after_the_run_ends()

    check_picker_reads_from_rest_too()
    check_picker_ignores_other_axes()

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
