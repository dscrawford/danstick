"""Does a button bound to half an analog axis actually reach the core?

Reported: mapping Y to C-up for Smash Bros did nothing on the GameCube pad.
Everything padmap wrote was correct -- the capture was stored, the autoconfig
carried `input_r_y_minus_btn = "3"`, the button index matched RetroArch's own
numbering, and RetroArch's shipped database uses that same form in 40 profiles.
The fault was one layer down, in how RetroArch reads an analog axis:

    res  = abs(input_joypad_axis(..., axis_plus,  normal_mag));
    res -= abs(input_joypad_axis(..., axis_minus, normal_mag));

    if (res == 0)
    {
       ... consult bind_minus->joykey / bind_plus->joykey ...
    }

                            -- input/input_driver.c, input_joypad_analog_axis

The button is only consulted when the axis reads *exactly* zero. padmap was
emitting a button on one half and leaving an axis on the other, so the axis
decided the answer and the button was never read. Silently: `res` was 900 out
of 32767, under 3%, which sits inside the core's own deadzone -- so the stick
behaved perfectly while the button was dead, and nothing anywhere said why.

This models that arithmetic rather than asserting on the text of the profile,
because the text was never what was wrong. The three functions below are
transcribed from the RetroArch source, and the axis rest values are the ones
measured on the live pad. A run drives padmap's real `retroarch_lines` and asks
the model what the core would have been handed.

    python3 tests/check_half_axis_binds.py
"""

from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap.mapping import Binding, retroarch_lines  # noqa: E402

# input/drivers_joypad/udev_joypad.c
AXIS_NONE = None


def udev_compute_axis(minimum: int, maximum: int, value: int) -> int:
    """`(value - min) * 0xffff / range - 0x7fff`, clamped.

    Note the -0x7fff, not -0x8000: it is why a 0..255 axis has no value that
    normalises to zero. 127 gives -128 and 128 gives +129.
    """
    span = maximum - minimum
    axis = (value - minimum) * 0xFFFF // span - 0x7FFF
    return max(-0x7FFF, min(0x7FFF, axis))


def udev_joypad_axis_state(axes: dict[int, int], spec) -> int:
    """One half of an axis. The other half's deflection reads as zero."""
    if spec is AXIS_NONE:
        return 0
    index, half = spec
    value = axes[index]
    if half == "-":
        return value if value < 0 else 0
    return value if value > 0 else 0


def analog_axis(axes, buttons, axis_minus, axis_plus, key_minus, key_plus):
    """input_joypad_analog_axis, with deadzone 0 -- what padmap ships under.

    The user's retroarch.cfg carries `input_analog_deadzone = "0.000000"`, and
    RetroArch's own default is 0.0f, so the deadzone branch that might have
    rescued this never runs.
    """
    res = abs(udev_joypad_axis_state(axes, axis_plus))
    res -= abs(udev_joypad_axis_state(axes, axis_minus))

    if res == 0:
        if key_plus is not None and buttons.get(key_plus):
            res = 0x7FFF
        if key_minus is not None and buttons.get(key_minus):
            res += -0x7FFF
    return res


def parse(lines: list[str]) -> dict[str, str]:
    out = {}
    for line in lines:
        if " = " in line:
            key, value = line.split(" = ", 1)
            out[key.strip()] = value.strip().strip('"')
    return out


def binds_for(profile: dict[str, str], stem: str):
    """The (axis_minus, axis_plus, key_minus, key_plus) RetroArch would hold."""

    def axis_of(key):
        raw = profile.get(key)
        if raw is None:
            return AXIS_NONE
        return (int(raw[1:]), raw[0])

    def key_of(key):
        raw = profile.get(key)
        return None if raw is None else int(raw)

    return (
        axis_of(f"{stem}_minus_axis"), axis_of(f"{stem}_plus_axis"),
        key_of(f"{stem}_minus_btn"), key_of(f"{stem}_plus_btn"),
    )


# The stored game:n64/super-smash-bros-u mapping for the GameCube adapter,
# exactly as it sits on disk: Y captured as C-up, the C-stick left on its axes.
SMASH_MAPPING = {
    "a": Binding("button", 1, 0, 1),
    "b": Binding("button", 2, 0, 2),
    "start": Binding("button", 9, 0, 9),
    "dpup": Binding("hat", 0, 1),
    "dpdown": Binding("hat", 0, 4),
    "dpleft": Binding("hat", 0, 8),
    "dpright": Binding("hat", 0, 2),
    "lefttrigger": Binding("button", 7, 0, 7),
    "rightstick_up": Binding("button", 3, 0, 3),      # Y -> C-up
    "rightstick_down": Binding("axis", 2, 1),
    "rightstick_left": Binding("axis", 5, -1),
    "rightstick_right": Binding("axis", 5, 1),
}

# Measured on the live `padmap Player 1` node, uncalibrated, at rest. Axis 2 is
# the C-stick Y and it does not centre: 131 on a 0..255 axis.
REST = {0: 127, 1: 130, 2: 131, 3: 24, 4: 23, 5: 132}
CENTRED = {**REST, 2: 127}         # what a calibrated axis would publish


def rest_axes(raw: dict[int, int]) -> dict[int, int]:
    return {i: udev_compute_axis(0, 255, v) for i, v in raw.items()}


def check_the_measured_rest_is_not_zero() -> None:
    print("\nthe pad's own rest position, through RetroArch's normalisation:")
    axes = rest_axes(REST)
    if axes[2] == 0:
        raise SystemExit(
            "FAIL: axis 2 normalises to zero at rest, so this check is no "
            "longer reproducing the reported condition and would pass whether "
            "or not the bug is present")
    print(f"  ok  axis 2 rests at {REST[2]} -> {axes[2]} (not zero)")
    fraction = abs(axes[2]) / 0x7FFF
    if fraction > 0.05:
        raise SystemExit(
            f"FAIL: {fraction:.1%} of full scale would be visible as stick "
            f"drift. The whole difficulty of this bug was that it was not")
    print(f"  ok  and that is {fraction:.1%} of full scale -- invisible")


def check_y_reaches_the_core() -> None:
    print("\nY, captured as C-up, reaches the core as C-up:")
    profile = parse(retroarch_lines(SMASH_MAPPING))
    axes = rest_axes(REST)

    axis_minus, axis_plus, key_minus, key_plus = binds_for(profile, "input_r_y")
    if key_minus != 3:
        raise SystemExit(
            f"FAIL: input_r_y_minus_btn is {key_minus!r}, not button 3 -- the "
            f"capture is not even in the profile, so nothing below is "
            f"meaningful")

    idle = analog_axis(axes, {}, axis_minus, axis_plus, key_minus, key_plus)
    held = analog_axis(axes, {3: True}, axis_minus, axis_plus, key_minus, key_plus)

    if held == idle:
        raise SystemExit(
            f"FAIL: holding Y changed nothing -- the core reads {held} either "
            f"way. RetroArch only consults the button bind when the axis "
            f"reads exactly zero, and axis {axis_plus} is at "
            f"{udev_joypad_axis_state(axes, axis_plus)} at rest, so `res` is "
            f"never zero and input_r_y_minus_btn is dead. This is the reported "
            f"bug: Y did nothing in Smash Bros and the C-stick looked fine")
    if held >= 0:
        raise SystemExit(
            f"FAIL: Y produced {held}, which is not C-up. The minus half of "
            f"the right stick Y is what mupen64plus-next reads as C-up "
            f"(mupen64plus-u-cbutton = C4), so it has to be negative")
    print(f"  ok  idle {idle}, Y held {held} -- full deflection on C-up")


def check_the_stick_still_works_where_it_can() -> None:
    print("\nthe directions that were left on the stick still deflect:")
    profile = parse(retroarch_lines(SMASH_MAPPING))
    axes = rest_axes(REST)

    # X was never in conflict -- neither half is a button -- so it must be
    # untouched. A fix that dropped axis binds indiscriminately would show up
    # here rather than in a passing run.
    axis_minus, axis_plus, key_minus, key_plus = binds_for(profile, "input_r_x")
    if axis_minus is AXIS_NONE or axis_plus is AXIS_NONE:
        raise SystemExit(
            "FAIL: the right stick X binds were dropped. Nothing on that axis "
            "was bound to a button, so there was no conflict to resolve and "
            "the C-stick's left and right have been lost for no reason")
    pushed = {**axes, 5: udev_compute_axis(0, 255, 255)}
    if analog_axis(pushed, {}, axis_minus, axis_plus, key_minus, key_plus) <= 0:
        raise SystemExit("FAIL: pushing the C-stick right produced nothing")
    print("  ok  right stick X unaffected, still deflects")


def check_a_centred_axis_is_the_real_cure() -> None:
    print("\na calibrated axis is what gets the other direction back:")
    axes = rest_axes(CENTRED)
    if udev_joypad_axis_state(axes, (2, "+")) != 0:
        raise SystemExit(
            f"FAIL: even centred at {CENTRED[2]}, the positive half of axis 2 "
            f"reads {udev_joypad_axis_state(axes, (2, '+'))}. Then no mapping "
            f"can keep both the button and the opposing axis, and the advice "
            f"to calibrate is wrong")
    print(f"  ok  centred at {CENTRED[2]} -> {axes[2]}, positive half reads 0")
    print("      so C-down could be kept once the pad is calibrated")


def check_no_conflict_leaves_a_mapping_alone() -> None:
    print("\na mapping with no button on a stick half is emitted unchanged:")
    plain = {k: v for k, v in SMASH_MAPPING.items() if k != "rightstick_up"}
    plain["rightstick_up"] = Binding("axis", 2, -1)
    profile = parse(retroarch_lines(plain))
    for key in ("input_r_y_minus_axis", "input_r_y_plus_axis"):
        if key not in profile:
            raise SystemExit(
                f"FAIL: {key} was dropped from a mapping that has no button "
                f"anywhere on that axis -- the console mapping every N64 game "
                f"falls back to would lose its C-stick")
    print("  ok  both halves of the C-stick Y survive")


def main() -> int:
    check_the_measured_rest_is_not_zero()
    check_y_reaches_the_core()
    check_the_stick_still_works_where_it_can()
    check_a_centred_axis_is_the_real_cure()
    check_no_conflict_leaves_a_mapping_alone()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
