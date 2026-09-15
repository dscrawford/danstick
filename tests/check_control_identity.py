"""An axis code does not say what the control is.

One adapter proved that twice, on the same two codes. The Mayflash GameCube
adapter reports its analogue L and R triggers on ABS_RX and ABS_RY -- the
codes a right stick conventionally lives on -- and they rest near the bottom
of their range, not in the middle:

    a0 ABS_X  rest 127 of 0-255      a3 ABS_RX rest 24 of 0-255   <- L trigger
    a1 ABS_Y  rest 130 of 0-255      a4 ABS_RY rest 25 of 0-255   <- R trigger

Told by code alone, padmap did two harmful things with those axes:

* published them to SDL as the right stick, so the front-end saw a stick
  shoved 80% into its upper-left corner and held there forever. Reported by
  the user as a pad "stuck to the left".
* offered them to calibration, which takes the resting value as the *centre*
  and maps it to the middle of the declared range -- so an untouched trigger
  would read half pressed and keep half its travel. Harmless while no profile
  had any axes; the moment the wizard began calibrating automatically, every
  mapping would have quietly wrecked the triggers it had just captured.

Both fixes are the same rule, in `mapping.rests_centred`: a stick centres and
a trigger does not. This file checks that rule at both ends, on several pad
shapes, and checks the two things it must *not* break -- a genuinely centred
right stick, and a caller with no absinfo to consult.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tests/check_control_identity.py
"""

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

import evdev  # noqa: E402
from evdev import ecodes  # noqa: E402

from padmap import calibrate, mapping  # noqa: E402
from padmap.mapping import Binding  # noqa: E402


class StubDevice:
    """Just enough InputDevice for calibratable_axes: its EV_ABS capabilities.

    A stub rather than a real handle on purpose. A live padmap daemon holds
    the pads on this machine, and the adapter these numbers came from may not
    be plugged in at all -- a check whose result depends on what is in the USB
    ports is not a check.
    """

    def __init__(self, entries):
        self._entries = entries

    def capabilities(self, absinfo=True):
        return {ecodes.EV_ABS: list(self._entries)}


def absinfo(value, minimum=0, maximum=255, flat=15):
    # AbsInfo(value, min, max, fuzz, flat, resolution). `value` is the
    # driver's current reading -- what the axis says while nothing is touched,
    # which is the whole question here.
    return evdev.AbsInfo(value, minimum, maximum, 0, flat, 0)


def names(codes):
    return [ecodes.ABS.get(code, code) for code in sorted(codes)]


# ---------------------------------------------------------------------------
# The pads. Real measurements where there are any.

# Mayflash GameCube adapter, read off its absinfo: sticks centred, the two
# analogue triggers down at 24/25 on stick codes, plus a hat.
GAMECUBE_ENTRIES = [
    (ecodes.ABS_X, absinfo(127)),
    (ecodes.ABS_Y, absinfo(130)),
    (ecodes.ABS_Z, absinfo(132)),
    (ecodes.ABS_RX, absinfo(24)),
    (ecodes.ABS_RY, absinfo(25)),
    (ecodes.ABS_RZ, absinfo(131)),
    (ecodes.ABS_HAT0X, absinfo(0, -1, 1, flat=0)),
    (ecodes.ABS_HAT0Y, absinfo(0, -1, 1, flat=0)),
]
GAMECUBE_CODES = [code for code, _ in GAMECUBE_ENTRIES]
GAMECUBE_SPANS = {
    code: (info.min, info.max, info.value) for code, info in GAMECUBE_ENTRIES
}

# A conventional pad: two centred sticks, triggers where the names suggest.
CONVENTIONAL_ENTRIES = [
    (ecodes.ABS_X, absinfo(128)),
    (ecodes.ABS_Y, absinfo(128)),
    (ecodes.ABS_Z, absinfo(0)),
    (ecodes.ABS_RX, absinfo(128)),
    (ecodes.ABS_RY, absinfo(127)),
    (ecodes.ABS_RZ, absinfo(0)),
    (ecodes.ABS_HAT0X, absinfo(0, -1, 1, flat=0)),
    (ecodes.ABS_HAT0Y, absinfo(0, -1, 1, flat=0)),
]
CONVENTIONAL_CODES = [code for code, _ in CONVENTIONAL_ENTRIES]
CONVENTIONAL_SPANS = {
    code: (info.min, info.max, info.value)
    for code, info in CONVENTIONAL_ENTRIES
}


def check_rests_centred_basics() -> None:
    """The one rule both consumers share: where does the axis sit at rest?"""
    print("rests_centred, on the axes that caused the reports:")
    for code in (ecodes.ABS_X, ecodes.ABS_Y):
        if not mapping.rests_centred(GAMECUBE_SPANS[code]):
            raise SystemExit(
                f"FAIL: the adapter's real stick axis {ecodes.ABS[code]} "
                f"{GAMECUBE_SPANS[code]} is not being called centred -- the "
                f"pad would lose its stick, and with it menu navigation")
    print("  ok  ABS_X rest 127 and ABS_Y rest 130 of 0-255 are centred")

    for code in (ecodes.ABS_RX, ecodes.ABS_RY):
        if mapping.rests_centred(GAMECUBE_SPANS[code]):
            raise SystemExit(
                f"FAIL: the adapter's analogue trigger on {ecodes.ABS[code]} "
                f"{GAMECUBE_SPANS[code]} is being called centred -- it would "
                f"go back to being published as a stick jammed hard over")
    print("  ok  ABS_RX rest 24 and ABS_RY rest 25 of 0-255 are not")

    print("\na trigger at either end of its travel, and dead centre:")
    if mapping.rests_centred((0, 255, 0)):
        raise SystemExit("FAIL: an axis resting at its minimum is not a stick")
    if mapping.rests_centred((0, 255, 255)):
        raise SystemExit("FAIL: an axis resting at its maximum is not a stick")
    if not mapping.rests_centred((0, 255, 128)):
        raise SystemExit(
            "FAIL: an axis resting dead centre must be a stick, or every pad "
            "on the machine loses its sticks at once")
    print("  ok  rest 0 no, rest 255 no, rest 128 yes")

    print("\nsigned and offset ranges, not just 0-255:")
    # Plenty of drivers report -32768..32767, and a few -128..127. The rule is
    # about position within the declared range, so it has to be scale-free.
    if not mapping.rests_centred((-32768, 32767, 0)):
        raise SystemExit(
            "FAIL: a signed stick resting at 0 was refused -- pads reporting "
            "-32768..32767 would lose both sticks")
    if mapping.rests_centred((-32768, 32767, -32768)):
        raise SystemExit("FAIL: a signed axis pinned at its minimum is not a "
                         "stick")
    if not mapping.rests_centred((-128, 127, 0)):
        raise SystemExit("FAIL: an 8-bit signed stick resting at 0 was "
                         "refused")
    if not mapping.rests_centred((0, 1023, 512)):
        raise SystemExit("FAIL: a 10-bit stick resting at 512 was refused")
    print("  ok  -32768..32767, -128..127 and 0..1023 all judged by position")


def check_tolerance_edges() -> None:
    """The tolerance is 0.5 of half-range, and the boundary is inclusive.

    Deliberately wide. The measured pads rest inside 4% of centre, but one
    N64 adapter here reports a resting value 36% off true centre -- that is
    still a stick, and narrowing this would silently take its stick away.
    """
    print("\nthe 0.5-of-half-range boundary:")
    if mapping.STICK_REST_TOLERANCE != 0.5:
        raise SystemExit(
            f"FAIL: the tolerance moved to {mapping.STICK_REST_TOLERANCE}; "
            f"widening it lets a trigger back in as a stick, narrowing it "
            f"drops sticks on adapters that report rest off-centre")
    # 0..200: centre 100, half-range 100, so rest 150 is exactly 0.5 out.
    if not mapping.rests_centred((0, 200, 150)):
        raise SystemExit(
            "FAIL: an axis exactly at the tolerance was refused -- the "
            "boundary must be inclusive or a borderline stick disappears")
    if mapping.rests_centred((0, 200, 151)):
        raise SystemExit(
            "FAIL: an axis past the tolerance was accepted as a stick")
    if not mapping.rests_centred((0, 200, 50)):
        raise SystemExit("FAIL: the boundary is not symmetric below centre")
    if mapping.rests_centred((0, 200, 49)):
        raise SystemExit("FAIL: past the tolerance below centre was accepted")
    print("  ok  150/200 in, 151 out, 50 in, 49 out -- symmetric, inclusive")

    print("\nan adapter that reports rest well off centre keeps its stick:")
    # 36% off centre, as one N64 adapter here really reports. Inside 0.5.
    off = int(127.5 + 0.36 * 127.5)
    if not mapping.rests_centred((0, 255, off)):
        raise SystemExit(
            f"FAIL: rest {off} of 0-255 (36% off centre, as a real adapter "
            f"reports) was called a trigger -- that pad loses its stick")
    print(f"  ok  rest {off} of 0-255 is still a stick")

    print("\n...and a trigger is nowhere near that boundary:")
    # 24 of 0-255 is 81% deflected: the guard has three quarters of the gap
    # to spare, which is why it can afford to be this forgiving.
    offset = abs((24 - 127.5) / 127.5)
    if offset <= mapping.STICK_REST_TOLERANCE:
        raise SystemExit("FAIL: the adapter's trigger sits inside tolerance")
    print(f"  ok  the adapter's trigger rests {offset:.2f} out, "
          f"tolerance {mapping.STICK_REST_TOLERANCE}")


def check_degenerate_spans() -> None:
    """A range that says nothing must not be read as saying "centred".

    Drivers do report these -- an absent or unconfigured axis comes back with
    min == max. Dividing by that half-range is a ZeroDivisionError, and
    calling it centred would publish a phantom axis as a stick.
    """
    print("\na degenerate range is never centred:")
    for span in ((0, 0, 0), (5, 5, 5), (255, 0, 128), (1, -1, 0)):
        if mapping.rests_centred(span):
            raise SystemExit(
                f"FAIL: {span} was called centred -- an axis with no declared "
                f"travel would be published as a stick that cannot move")
    print("  ok  min == max and max < min both refused, no exception raised")


def check_gamecube_sticks() -> None:
    """The published SDL stick fields for the adapter that broke."""
    print("\nstick_fields on the GameCube adapter, with absinfo:")
    fields = mapping.stick_fields(GAMECUBE_CODES, axes=GAMECUBE_SPANS)
    if fields.get("leftx") != "a0" or fields.get("lefty") != "a1":
        raise SystemExit(
            f"FAIL: the adapter's real stick was dropped ({fields}) -- the "
            f"front-end would have nothing to navigate with but the d-pad")
    print("  ok  leftx:a0 lefty:a1 -- the real stick is still published")

    if "rightx" in fields or "righty" in fields:
        raise SystemExit(
            f"FAIL: the analogue triggers were published as the right stick "
            f"({fields}) -- SDL reads that stick as held 80% into its corner "
            f"and never released, which the user sees as a pad stuck left")
    print("  ok  no rightx/righty -- the triggers are not a stick")

    print("\na genuinely centred right stick is still declared:")
    centred = dict(GAMECUBE_SPANS)
    centred[ecodes.ABS_RX] = (0, 255, 128)
    centred[ecodes.ABS_RY] = (0, 255, 127)
    fields = mapping.stick_fields(GAMECUBE_CODES, axes=centred)
    if fields.get("rightx") != "a3" or fields.get("righty") != "a4":
        raise SystemExit(
            f"FAIL: a genuinely centred right stick was numbered {fields} -- "
            f"SDL would read the wrong axis for the stick")
    print("  ok  a centred RX/RY on this pad comes out a3/a4")

    print("\nthe correction does not over-correct:")
    if fields.get("leftx") != "a0" or fields.get("lefty") != "a1":
        raise SystemExit(f"FAIL: the left stick went missing too ({fields})")
    print("  ok  a real right stick is still declared, alongside the left")


def check_conventional_pad_untouched() -> None:
    """A pad whose triggers really are on Z/RZ must be exactly as before."""
    print("\na conventional pad keeps both sticks:")
    fields = mapping.stick_fields(CONVENTIONAL_CODES, axes=CONVENTIONAL_SPANS)
    # X, Y, Z, RX, RY, RZ number 0..5, so the right stick is a3/a4: the
    # analogue triggers on Z/RZ sit between the sticks and take numbers.
    want = {"leftx": "a0", "lefty": "a1", "rightx": "a3", "righty": "a4"}
    if fields != want:
        raise SystemExit(
            f"FAIL: an ordinary pad came out {fields}, wanted {want} -- the "
            f"trigger guard is costing working pads their sticks")
    print(f"  ok  {fields}")

    print("\n...and its triggers are not mistaken for a stick either:")
    # ABS_Z/ABS_RZ rest at 0 here, so they are refused on the same rule --
    # not that stick_fields would ever offer them, but the rule agrees.
    for code in (ecodes.ABS_Z, ecodes.ABS_RZ):
        if mapping.rests_centred(CONVENTIONAL_SPANS[code]):
            raise SystemExit(
                f"FAIL: {ecodes.ABS[code]} resting at 0 was called centred")
    print("  ok  ABS_Z and ABS_RZ at rest 0 are triggers by the same rule")


def check_no_absinfo_keeps_the_old_guess() -> None:
    """A caller that cannot read the driver is no worse off than before.

    Refusing every stick when unsure would be the cautious-looking choice and
    the wrong one: it would take navigation away from pads that work today,
    to protect against a pad we have no evidence about.
    """
    print("\nwith no absinfo at all, the old guess by code still stands:")
    fields = mapping.stick_fields(GAMECUBE_CODES)
    if fields.get("rightx") != "a3" or fields.get("righty") != "a4":
        raise SystemExit(
            f"FAIL: sticks vanished when no absinfo was supplied ({fields}) "
            f"-- every pad the driver cannot be read for loses navigation")
    if fields.get("leftx") != "a0" or fields.get("lefty") != "a1":
        raise SystemExit(f"FAIL: the left stick vanished too ({fields})")
    print(f"  ok  {fields}")

    print("\nan empty axes dict is the same as none:")
    if mapping.stick_fields(GAMECUBE_CODES, axes={}) != fields:
        raise SystemExit(
            "FAIL: {} and None disagree -- a caller whose absinfo read came "
            "back empty would silently lose its sticks")
    print("  ok  both fall back to the guess")

    print("\nabsinfo for some axes only judges those, and guesses the rest:")
    partial = {ecodes.ABS_RX: (0, 255, 24)}
    fields = mapping.stick_fields(GAMECUBE_CODES, axes=partial)
    if "rightx" in fields:
        raise SystemExit(
            f"FAIL: the one axis we did measure, and measured as a trigger, "
            f"was still published as a stick ({fields})")
    if fields.get("righty") != "a4" or fields.get("leftx") != "a0":
        raise SystemExit(
            f"FAIL: axes with no absinfo were dropped ({fields}) -- one "
            f"unreadable axis must not cost the others")
    print(f"  ok  {fields}")


def check_capture_claim_wins() -> None:
    """An axis a capture already claims is not also a stick.

    Independent of absinfo, and it catches the same adapter by a second
    route: its shoulders capture as +a3/+a4. `leftshoulder:+a3` and
    `rightx:a3` on one line means the two disagree about what is pressed and
    the front-end believes whichever it reads first.
    """
    print("\nan axis bound by the capture is not published as a stick:")
    bound = {"leftshoulder": Binding("axis", 3, 1),
             "rightshoulder": Binding("axis", 4, 1)}
    fields = mapping.stick_fields(GAMECUBE_CODES, bound)
    if "rightx" in fields or "righty" in fields:
        raise SystemExit(
            f"FAIL: a3/a4 are captured as the shoulders and were published as "
            f"the right stick as well ({fields}) -- SDL sees one axis being "
            f"two controls at once")
    if fields.get("leftx") != "a0" or fields.get("lefty") != "a1":
        raise SystemExit(f"FAIL: an unclaimed stick was dropped ({fields})")
    print(f"  ok  {fields}")

    print("\nthe claim holds even when the axis does rest centred:")
    # The two guards are independent on purpose. A centred axis that is also a
    # captured shoulder is still not a stick.
    centred = dict(GAMECUBE_SPANS)
    centred[ecodes.ABS_RX] = (0, 255, 128)
    centred[ecodes.ABS_RY] = (0, 255, 127)
    fields = mapping.stick_fields(GAMECUBE_CODES, bound, centred)
    if "rightx" in fields or "righty" in fields:
        raise SystemExit(
            f"FAIL: absinfo overruled the capture ({fields}) -- the axis is "
            f"bound to a shoulder and would fight the stick")
    print(f"  ok  {fields}")

    print("\na *button* index does not block the axis of the same number:")
    # Buttons and axes are separate numbering. Blocking a0 because button 0
    # was captured would take the left stick off nearly every pad here.
    buttons = {"a": Binding("button", 0), "b": Binding("button", 1),
               "x": Binding("button", 3), "y": Binding("button", 4),
               "dpup": Binding("hat", 0, 1)}
    fields = mapping.stick_fields(GAMECUBE_CODES, buttons, GAMECUBE_SPANS)
    if fields.get("leftx") != "a0" or fields.get("lefty") != "a1":
        raise SystemExit(
            f"FAIL: captured buttons b0/b1 suppressed axes a0/a1 ({fields}) "
            f"-- the left stick would disappear on almost every pad")
    print(f"  ok  {fields}")


def check_other_pad_shapes() -> None:
    """Shapes other than a full twin-stick pad must not surprise anything."""
    print("\nan arcade stick with only a hat has no sticks to publish:")
    # The F300 here: buttons and a hat, no analogue axes at all.
    fields = mapping.stick_fields([0x10, 0x11], axes={0x10: (-1, 1, 0),
                                                      0x11: (-1, 1, 0)})
    if fields:
        raise SystemExit(
            f"FAIL: a hat was published as a stick ({fields}) -- SDL would "
            f"read a stick that snaps between its extremes")
    print("  ok  {}")

    print("\na pad with one axis publishes just that one:")
    single = mapping.stick_fields([ecodes.ABS_X], axes={ecodes.ABS_X:
                                                        (0, 255, 128)})
    if single != {"leftx": "a0"}:
        raise SystemExit(f"FAIL: a single-axis device came out {single}")
    print(f"  ok  {single}")

    print("\na pad with no axes at all is empty, not an error:")
    if mapping.stick_fields([], axes={}) != {}:
        raise SystemExit("FAIL: a pad with no axes produced stick fields")
    print("  ok  {}")

    print("\nsparse axis codes are numbered by position:")
    # X, Y, RX, RY with nothing between them: the right stick is a2/a3, not
    # a3/a4. Publishing the code as the index would point SDL at axes this
    # pad does not have.
    sparse = [ecodes.ABS_X, ecodes.ABS_Y, ecodes.ABS_RX, ecodes.ABS_RY]
    fields = mapping.stick_fields(
        sparse, axes={code: (0, 255, 128) for code in sparse})
    if fields != {"leftx": "a0", "lefty": "a1",
                  "rightx": "a2", "righty": "a3"}:
        raise SystemExit(
            f"FAIL: sparse codes were numbered {fields} -- SDL would read "
            f"axes that do not exist on this pad")
    print(f"  ok  {fields}")


def check_stick_held_when_read() -> None:
    """A stick held over as absinfo is read looks like a trigger.

    Known and accepted. It needs the stick more than half deflected at the
    one instant the driver is read, and the cost is one missing stick until
    the pad is set up again -- against a stick permanently jammed in a corner,
    which is what the guard exists to stop.
    """
    print("\na stick held hard over while absinfo is read is refused:")
    held = dict(CONVENTIONAL_SPANS)
    held[ecodes.ABS_X] = (0, 255, 254)
    fields = mapping.stick_fields(CONVENTIONAL_CODES, axes=held)
    if "leftx" in fields:
        raise SystemExit(
            "FAIL: an axis reading 99% deflected was published as a stick -- "
            "whatever it is, SDL would read it as held over")
    if fields.get("lefty") != "a1" or fields.get("rightx") != "a3":
        raise SystemExit(
            f"FAIL: one held axis took the others with it ({fields})")
    print(f"  ok  leftx dropped, the other axes unaffected: {fields}")

    print("\n...but a light lean is still a stick:")
    # 30% over is well inside tolerance, so a resting-position report that is
    # merely imprecise costs nothing.
    leaning = dict(CONVENTIONAL_SPANS)
    leaning[ecodes.ABS_X] = (0, 255, int(127.5 + 0.3 * 127.5))
    fields = mapping.stick_fields(CONVENTIONAL_CODES, axes=leaning)
    if fields.get("leftx") != "a0":
        raise SystemExit(
            "FAIL: a stick reading 30% off centre was dropped -- pads whose "
            "resting report is imprecise would lose navigation")
    print("  ok  30% off centre still counts")


def check_calibration_excludes_triggers() -> None:
    """calibratable_axes must refuse the same two axes, for its own reason.

    Calibration maps the resting value to the middle of the declared range.
    A trigger resting at 24 of 0-255 would come out reading half pressed
    while untouched, with half its travel gone -- and the wizard now
    calibrates at the end of every mapping, so it would happen to the very
    triggers just captured.
    """
    print("\ncalibratable_axes on the GameCube adapter:")
    got = sorted(calibrate.calibratable_axes(StubDevice(GAMECUBE_ENTRIES)))
    want = [ecodes.ABS_X, ecodes.ABS_Y]
    if got != sorted(want):
        raise SystemExit(
            f"FAIL: would calibrate {names(got)}, wanted {names(want)} -- "
            f"calibration records an axis's resting value as its centre, so "
            f"a trigger in that list reads half pressed untouched with half "
            f"its travel gone, and a hat gets a dead band over a -1..1 range")
    print("  ok  the two sticks, and neither analogue trigger")

    print("\nthe returned absinfo is the driver's, not a copy of the code:")
    axes = calibrate.calibratable_axes(StubDevice(GAMECUBE_ENTRIES))
    info = axes[ecodes.ABS_X]
    if (info.value, info.min, info.max) != (127, 0, 255):
        raise SystemExit(
            f"FAIL: absinfo came back as {info} -- calibration would centre "
            f"and scale the axis against the wrong range")
    print("  ok  ABS_X comes back with value 127 of 0-255")

    print("\nhats are never calibrated:")
    # -1/0/1 and already centred; a dead band computed over that range would
    # swallow the d-pad entirely.
    hats = [(code, absinfo(0, -1, 1, flat=0)) for code in (
        ecodes.ABS_HAT0X, ecodes.ABS_HAT0Y, ecodes.ABS_HAT1X,
        ecodes.ABS_HAT1Y, ecodes.ABS_HAT2X, ecodes.ABS_HAT2Y,
        ecodes.ABS_HAT3X, ecodes.ABS_HAT3Y)]
    got = calibrate.calibratable_axes(StubDevice(hats))
    if got:
        raise SystemExit(
            f"FAIL: hats offered for calibration ({names(got)}) -- a dead "
            f"band across a -1..1 axis would swallow the d-pad")
    print("  ok  all four hat pairs skipped")

    print("\nan arcade stick with only a hat offers nothing to calibrate:")
    stick = StubDevice([(ecodes.ABS_HAT0X, absinfo(0, -1, 1, flat=0)),
                        (ecodes.ABS_HAT0Y, absinfo(0, -1, 1, flat=0))])
    if calibrate.calibratable_axes(stick):
        raise SystemExit("FAIL: a hat-only pad had axes to calibrate")
    if calibrate.sample_rest(None, device=stick) != {}:
        raise SystemExit(
            "FAIL: sampling a hat-only pad did not return empty at once -- "
            "the wizard would sit there measuring nothing")
    print("  ok  no axes, and sample_rest returns immediately")

    print("\na degenerate range is not calibrated:")
    # Nothing to scale against, and the flat band would come out zero.
    broken = StubDevice([(ecodes.ABS_X, absinfo(0, 0, 0)),
                         (ecodes.ABS_Y, absinfo(5, 10, 10)),
                         (ecodes.ABS_RX, absinfo(0, 255, 0))])
    got = calibrate.calibratable_axes(broken)
    if got:
        raise SystemExit(
            f"FAIL: axes with no declared travel were offered ({names(got)})")
    print("  ok  no travel, and a full-range axis resting at its floor, "
          "both refused")


def check_calibration_conventional_pads() -> None:
    """The pads that already worked must be unaffected by all of this."""
    print("\na pad whose triggers really are on Z/RZ is unchanged:")
    got = sorted(calibrate.calibratable_axes(StubDevice(CONVENTIONAL_ENTRIES)))
    want = sorted([ecodes.ABS_X, ecodes.ABS_Y, ecodes.ABS_RX, ecodes.ABS_RY])
    if got != want:
        raise SystemExit(
            f"FAIL: an ordinary pad offers {names(got)}, wanted {names(want)} "
            f"-- a working pad has lost calibration on its sticks")
    print(f"  ok  {names(got)}")

    print("\nthe conventional trigger codes are excluded even when centred:")
    # The code list is kept as a shortcut on top of the resting-value rule.
    # Some pads report a right stick axis on ABS_Z or ABS_RZ, so a centred
    # reading there is not proof it is a stick -- and getting it wrong on a
    # trigger is the expensive direction.
    odd = StubDevice([
        (ecodes.ABS_X, absinfo(128)),
        (ecodes.ABS_Z, absinfo(128)),
        (ecodes.ABS_RZ, absinfo(127)),
        (ecodes.ABS_GAS, absinfo(128)),
        (ecodes.ABS_BRAKE, absinfo(129)),
    ])
    got = sorted(calibrate.calibratable_axes(odd))
    if got != [ecodes.ABS_X]:
        raise SystemExit(
            f"FAIL: {names(got)} offered for calibration -- ABS_Z, ABS_RZ, "
            f"ABS_GAS and ABS_BRAKE are the conventional trigger codes and "
            f"centring one halves its travel")
    print("  ok  ABS_Z, ABS_RZ, ABS_GAS and ABS_BRAKE all skipped")

    print("\na stick held over as calibration runs is skipped, not centred:")
    # Accepted cost, and the safe direction: skipping a stick is recoverable,
    # silently ruining a trigger is what this guard is for.
    held = StubDevice([(ecodes.ABS_X, absinfo(250)),
                       (ecodes.ABS_Y, absinfo(128))])
    got = sorted(calibrate.calibratable_axes(held))
    if got != [ecodes.ABS_Y]:
        raise SystemExit(
            f"FAIL: {names(got)} -- an axis held near an end must not have "
            f"that position recorded as its centre")
    print("  ok  the deflected axis is skipped, the centred one kept")


def check_one_rule_two_consumers() -> None:
    """Both consumers must reach the same verdict about the same axis.

    They fail in different ways -- a stuck stick in the front-end, a ruined
    trigger in a game -- but the question is identical, and a pad where the
    two disagreed would be publishing an axis as a stick while calibration
    treats it as a trigger, or the reverse.
    """
    print("\nthe stick guard and the calibration guard agree per axis:")
    calibratable = set(calibrate.calibratable_axes(
        StubDevice(GAMECUBE_ENTRIES)))
    published = set(mapping.stick_fields(
        GAMECUBE_CODES, axes=GAMECUBE_SPANS).values())
    for code in (ecodes.ABS_RX, ecodes.ABS_RY):
        index = mapping.axis_index(GAMECUBE_CODES, code)
        if code in calibratable or f"a{index}" in published:
            raise SystemExit(
                f"FAIL: {ecodes.ABS[code]} is refused by one guard and "
                f"accepted by the other -- the same axis is a trigger to one "
                f"half of padmap and a stick to the other")
    for code in (ecodes.ABS_X, ecodes.ABS_Y):
        index = mapping.axis_index(GAMECUBE_CODES, code)
        if code not in calibratable or f"a{index}" not in published:
            raise SystemExit(
                f"FAIL: the real stick axis {ecodes.ABS[code]} was dropped by "
                f"one of the two guards")
    print("  ok  ABS_X/ABS_Y accepted by both, ABS_RX/ABS_RY by neither")


def main() -> int:
    check_rests_centred_basics()
    check_tolerance_edges()
    check_degenerate_spans()
    check_gamecube_sticks()
    check_conventional_pad_untouched()
    check_no_absinfo_keeps_the_old_guess()
    check_capture_claim_wins()
    check_other_pad_shapes()
    check_stick_held_when_read()
    check_calibration_excludes_triggers()
    check_calibration_conventional_pads()
    check_one_rule_two_consumers()

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
