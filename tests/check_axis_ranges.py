"""Every published axis must have a range, or consumers divide by zero.

RetroArch normalises an axis with

    int range = info->maximum - info->minimum;
    int axis  = (value - info->minimum) * 0xffff / range - 0x7fff;

                    -- input/drivers_joypad/udev_joypad.c, udev_compute_axis

so an axis whose min equals its max is not a degraded axis, it is a division
by zero in the consumer. The failure is remote from the cause in every way that
matters: padmap creates the pad, the pad enumerates, autoconfig matches it, the
mapping applies, input forwards -- and then the *game* dies with SIGFPE at its
first input poll, with a backtrace through udev_joypad_poll that names nothing
of padmap's.

That is exactly how it shipped. `hidraw.Source.capabilities` defaulted to
absinfo=False where evdev's defaults to True, `_capabilities_for` called it with
no arguments, and the clone was built from bare axis codes with no ranges. Every
axis came out 0..0 and Kirby Air Ride crashed on its start screen.

So this checks the property directly, on every source padmap can publish from.

    python3 tests/check_axis_ranges.py
"""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

_SANDBOX = tempfile.mkdtemp(prefix="padmap-axes-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from evdev import ecodes  # noqa: E402

from padmap import hidraw  # noqa: E402


def axis_name(code: int) -> str:
    name = ecodes.ABS.get(code, str(code))
    return name if isinstance(name, str) else name[0]


def check_hidraw_defaults_to_absinfo() -> None:
    print("\nhidraw capabilities() defaults to including absinfo:")
    # The signature is the bug: _capabilities_for calls capabilities() with no
    # arguments, so the default is what the clone is built from.
    import inspect
    signature = inspect.signature(hidraw.Source.capabilities)
    default = signature.parameters["absinfo"].default
    if default is not True:
        raise SystemExit(
            f"FAIL: capabilities(absinfo={default!r}) by default. evdev's "
            f"defaults to True and virtual._capabilities_for calls it with no "
            f"arguments, so the clone would be built from bare axis codes -- "
            f"every axis min=max=0, and every consumer that normalises an axis "
            f"divides by zero")
    print("  ok  absinfo=True")


def check_every_axis_has_a_range() -> None:
    print("\nevery axis a hidraw source publishes has a non-zero range:")

    class Fake(hidraw.Source):
        """The capability table without opening a device."""

        def __init__(self) -> None:            # noqa: D107
            pass

    caps = Fake.capabilities(Fake())           # type: ignore[arg-type]
    axes = caps.get(ecodes.EV_ABS, [])
    if not axes:
        raise SystemExit("FAIL: no axes published at all")

    bad = []
    for entry in axes:
        if not isinstance(entry, tuple):
            raise SystemExit(
                f"FAIL: axis {entry!r} published without an AbsInfo. That is "
                f"the bare-code form, which becomes min=max=0 on the clone")
        code, info = entry
        if info.max - info.min == 0:
            bad.append(axis_name(code))
    if bad:
        raise SystemExit(
            f"FAIL: {', '.join(bad)} have a zero range. RetroArch divides by "
            f"(max - min) on every poll, so a game crashes with SIGFPE the "
            f"moment it reads the pad -- which is what happened on Kirby Air "
            f"Ride's start screen")
    print(f"  ok  {len(axes)} axes, all with a range: "
          + ", ".join(f"{axis_name(c)}={i.min}..{i.max}" for c, i in axes))


def check_sticks_are_centred_within_range() -> None:
    print("\n...and each axis rests inside its own range:")

    class Fake(hidraw.Source):
        def __init__(self) -> None:            # noqa: D107
            pass

    caps = Fake.capabilities(Fake())           # type: ignore[arg-type]
    for code, info in caps.get(ecodes.EV_ABS, []):
        if not (info.min <= info.value <= info.max):
            raise SystemExit(
                f"FAIL: {axis_name(code)} rests at {info.value}, outside "
                f"{info.min}..{info.max}. A pad that reports itself out of "
                f"range reads as permanently deflected")
    print("  ok  every resting value is within range")


def _report(lx: int, ly: int, rx: int = 2048, ry: int = 2048) -> bytes:
    """A synthetic 0x30 report with the sticks at the given raw positions.

    Raw meaning as the controller reports them, before any flip: Y grows
    upwards here, which is the thing the decode has to correct.
    """
    data = bytearray(49)
    data[0] = 0x30
    data[6] = lx & 0xFF
    data[7] = ((lx >> 8) & 0x0F) | ((ly & 0x0F) << 4)
    data[8] = (ly >> 4) & 0xFF
    data[9] = rx & 0xFF
    data[10] = ((rx >> 8) & 0x0F) | ((ry & 0x0F) << 4)
    data[11] = (ry >> 4) & 0xFF
    return bytes(data)


def check_y_is_flipped_and_x_is_not() -> None:
    print("\npushing the stick up produces a LOW ABS_Y, as evdev expects:")

    class Fake(hidraw.Source):
        def __init__(self) -> None:            # noqa: D107
            self._buttons = {}
            self._axes = {}
            self._hat = (0, 0)

    source = Fake()
    # Raw Y at maximum is the stick held fully UP on this controller.
    events = {(e.code): e.value for e in source._decode(_report(2048, 4095))
              if e.type == ecodes.EV_ABS}
    y = events.get(ecodes.ABS_Y)
    if y is None:
        raise SystemExit("FAIL: no ABS_Y emitted at all")
    if y > hidraw.STICK_MAX // 2:
        raise SystemExit(
            f"FAIL: stick up produced ABS_Y={y}, in the upper half of "
            f"0..{hidraw.STICK_MAX}. The controller reports Y increasing "
            f"upwards and evdev has 0 at the top, so publishing it raw makes "
            f"the stick inverted vertically in every game -- which is exactly "
            f"how it was reported")
    print(f"  ok  raw Y 4095 (up) -> ABS_Y {y}")

    # And X must NOT be flipped, or the fix trades one inversion for another.
    source = Fake()
    events = {(e.code): e.value for e in source._decode(_report(4095, 2048))
              if e.type == ecodes.EV_ABS}
    x = events.get(ecodes.ABS_X)
    if x is None or x < hidraw.STICK_MAX // 2:
        raise SystemExit(
            f"FAIL: stick right produced ABS_X={x}; X shares evdev's "
            f"direction already and must be passed through untouched")
    print(f"  ok  raw X 4095 (right) -> ABS_X {x}")


def check_the_dpad_matches_evdev_too() -> None:
    print("\n...and the d-pad hat points the same way:")

    class Fake(hidraw.Source):
        def __init__(self) -> None:            # noqa: D107
            self._buttons = {}
            self._axes = {}
            self._hat = (0, 0)

    up = bytearray(_report(2048, 2048))
    up[5] = 0x02          # d-pad Up
    events = {e.code: e.value for e in Fake()._decode(bytes(up))
              if e.type == ecodes.EV_ABS and e.code == ecodes.ABS_HAT0Y}
    if events.get(ecodes.ABS_HAT0Y) != -1:
        raise SystemExit(
            f"FAIL: d-pad up gave ABS_HAT0Y={events.get(ecodes.ABS_HAT0Y)}, "
            f"expected -1. evdev hats put up at -1, and a hat inverted "
            f"against the stick is worse than both being wrong together")
    print("  ok  d-pad up -> ABS_HAT0Y -1")


def main() -> int:
    check_hidraw_defaults_to_absinfo()
    check_every_axis_has_a_range()
    check_sticks_are_centred_within_range()
    check_y_is_flipped_and_x_is_not()
    check_the_dpad_matches_evdev_too()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
