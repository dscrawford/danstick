"""Measure where a controller's axes rest.

The user is told to let go of the sticks; we sample for a moment and take the
resting position as centre. Anything that moves during the sample widens that
axis's dead band, so a jittery stick gets a proportionally larger one rather
than a wrong centre.

Only axes that plausibly *have* a centre are calibrated. Hats are -1/0/1 and
already centred; triggers rest at an end of their range, not the middle, so
recentring one would halve its travel.
"""

from __future__ import annotations

import time
from typing import Callable, cast

import evdev
from evdev import ecodes

from .devices import Pad, open_device
from .mapping import rests_centred
from .profiles import AxisCalibration, Profile, signature

# (fraction 0..1) -> None, so a UI can draw a progress bar.
ProgressFn = Callable[[float], None]

SAMPLE_SECONDS = 0.6
REACH_SECONDS = 4.0
# Minimum dead band, as a fraction of total travel. Even a still stick dithers
# by a couple of units, and a zero-width band would let that through as input.
MIN_FLAT_FRACTION = 0.04

# Hats are already centred and only ever -1/0/1.
_SKIP_AXES = frozenset({
    ecodes.ABS_HAT0X, ecodes.ABS_HAT0Y,
    ecodes.ABS_HAT1X, ecodes.ABS_HAT1Y,
    ecodes.ABS_HAT2X, ecodes.ABS_HAT2Y,
    ecodes.ABS_HAT3X, ecodes.ABS_HAT3Y,
})

# Triggers rest at one end of travel, so their resting value is not a centre.
#
# A shortcut, not the rule -- see calibratable_axes. The codes a trigger is
# conventionally reported on, which is not the same as the codes this
# machine's triggers are actually on.
_TRIGGER_AXES = frozenset({
    ecodes.ABS_Z, ecodes.ABS_RZ, ecodes.ABS_GAS, ecodes.ABS_BRAKE,
})


def abs_entries(device: evdev.InputDevice) -> list[tuple[int, evdev.AbsInfo]]:
    """EV_ABS capabilities as (code, absinfo) pairs.

    evdev's stubs type `capabilities()` as returning list[int] whichever way
    `absinfo` is set, so the tuples it actually yields need spelling out.
    """
    caps = device.capabilities(absinfo=True)
    return cast("list[tuple[int, evdev.AbsInfo]]", caps.get(ecodes.EV_ABS, []))


def calibratable_axes(device: evdev.InputDevice) -> dict[int, evdev.AbsInfo]:
    """Axes worth centring, with their current absinfo.

    A trigger must not be here. Calibration takes the resting value as the
    centre and maps it to the middle of the declared range, so centring a
    trigger makes it read half pressed while untouched and costs it half its
    travel. The code list above catches the conventional cases; it does not
    catch this machine's GameCube adapter, whose analogue triggers are on
    ABS_RX and ABS_RY -- stick codes -- resting at 24 of 0-255.

    So the resting value decides, and the code list is only a shortcut. A
    stick centres and a trigger does not, which is the difference the code
    number cannot carry. Same test, and the same reasoning, as the one that
    stopped those two axes being published as a right stick.

    An axis being *held* over as this runs would be read as a trigger and
    skipped. That needs more than half deflection at the moment the wizard
    reaches this point, having just asked the user to leave the pad alone --
    and skipping one stick is recoverable, where silently ruining a trigger
    is what this is here to prevent.
    """
    out: dict[int, evdev.AbsInfo] = {}
    for code, info in abs_entries(device):
        if code in _SKIP_AXES or code in _TRIGGER_AXES:
            continue
        if info.max <= info.min:
            continue
        if not rests_centred((info.min, info.max, info.value)):
            continue
        out[code] = info
    return out


def sample_rest(
    pad: Pad,
    seconds: float = SAMPLE_SECONDS,
    device: evdev.InputDevice | None = None,
) -> dict[int, AxisCalibration]:
    """Watch a pad at rest and derive a centre and dead band per axis.

    Pass `device` to reuse an already-open (and possibly grabbed) handle --
    during a setup session the daemon holds the pads, and opening a second
    handle just to read absinfo would be wasteful.
    """
    own = device is None
    dev = device if device is not None else open_device(pad)
    try:
        axes = calibratable_axes(dev)
        if not axes:
            return {}

        # Start from the current reading, then widen with anything observed.
        seen: dict[int, list[int]] = {
            code: [info.value, info.value] for code, info in axes.items()
        }

        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                event = dev.read_one()
            except OSError:
                break
            if event is None:
                time.sleep(0.005)
                continue
            if event.type != ecodes.EV_ABS or event.code not in seen:
                continue
            low, high = seen[event.code]
            seen[event.code] = [min(low, event.value), max(high, event.value)]

        return rest_from_samples(axes, seen)
    finally:
        if own:
            dev.close()


def rest_from_samples(
    axes: dict[int, evdev.AbsInfo], seen: dict[int, list[int]]
) -> dict[int, AxisCalibration]:
    """Turn observed rest samples into per-axis calibration.

    Split out from the sampling loop so the daemon can collect samples from
    its own event loop -- it cannot block for a second inside a select() tick
    without stalling every other pad and client.
    """
    result: dict[int, AxisCalibration] = {}
    for code, info in axes.items():
        low, high = seen.get(code, [info.value, info.value])
        center = (low + high) // 2
        travel = info.max - info.min
        # Dead band covers the observed wobble, never less than the floor.
        flat = max(
            (high - low) // 2 + 1,
            int(travel * MIN_FLAT_FRACTION),
            info.flat or 0,
        )
        result[code] = AxisCalibration(
            center=center,
            minimum=info.min,
            maximum=info.max,
            flat=flat,
        )
    return result


def sample_reach(
    pad: Pad,
    seconds: float = REACH_SECONDS,
    device: evdev.InputDevice | None = None,
    on_progress: ProgressFn | None = None,
) -> dict[int, tuple[int, int]]:
    """Watch a moving stick and record how far each axis actually travels.

    Necessary because an adapter's declared range is not what the hardware
    produces. Measured here: an N64 adapter declares 0-255 while the stick
    only reaches part of that, so scaling against the declared range leaves
    one direction with almost no travel -- the stick cannot go left at all.
    """
    own = device is None
    dev = device if device is not None else open_device(pad)
    try:
        axes = calibratable_axes(dev)
        if not axes:
            return {}

        seen: dict[int, list[int]] = {
            code: [info.value, info.value] for code, info in axes.items()
        }

        start = time.monotonic()
        deadline = start + seconds
        while time.monotonic() < deadline:
            if on_progress is not None:
                elapsed = time.monotonic() - start
                on_progress(min(elapsed / seconds, 1.0))
            try:
                event = dev.read_one()
            except OSError:
                break
            if event is None:
                time.sleep(0.005)
                continue
            if event.type != ecodes.EV_ABS or event.code not in seen:
                continue
            low, high = seen[event.code]
            seen[event.code] = [min(low, event.value), max(high, event.value)]

        return {code: (low, high) for code, (low, high) in seen.items()}
    finally:
        if own:
            dev.close()


def merge_reach(
    axes: dict[int, AxisCalibration], reach: dict[int, tuple[int, int]]
) -> dict[int, AxisCalibration]:
    """Fold measured extremes into an existing centre calibration.

    A direction has to clear the dead band before it counts as measured. The
    old test was `low < cal.center`, one dead band short of its own intent: a
    reading inside the band is by definition indistinguishable from the stick
    sitting still, and recording it as a reach makes `apply` compute a span
    from the edge of the band that is zero or negative, so the whole of that
    direction reads dead centre.

    That is not a corner case. The reach sample window opens at the axis's
    absinfo value while the centre comes from the *measured* rest samples, so
    the two disagree by however far the stick dithered while the user was
    letting go. An axis that jitters up a couple of units at rest and is then
    never touched during the sweep -- the user circling the other stick --
    ends the sweep one unit below its own centre, which the old test recorded
    as a reach_min. Result: full left reads dead centre. The exact "the stick
    cannot go left at all" failure that measuring reach exists to prevent,
    with the user doing nothing wrong.
    """
    out: dict[int, AxisCalibration] = {}
    for code, cal in axes.items():
        low, high = reach.get(code, (cal.center, cal.center))
        out[code] = AxisCalibration(
            center=cal.center,
            minimum=cal.minimum,
            maximum=cal.maximum,
            flat=cal.flat,
            # Ignore a direction that never left the dead band: leaving it
            # None falls back to the declared range rather than pinning travel
            # to zero.
            reach_min=low if low < cal.center - cal.flat else None,
            reach_max=high if high > cal.center + cal.flat else None,
        )
    return out


def build_profile(
    pad: Pad,
    icon: str = "",
    seconds: float = SAMPLE_SECONDS,
    device: evdev.InputDevice | None = None,
) -> Profile:
    return Profile(
        signature=signature(pad),
        name="".join(ch for ch in pad.name if ch.isprintable()).strip(),
        icon=icon,
        axes=sample_rest(pad, seconds=seconds, device=device),
    )
