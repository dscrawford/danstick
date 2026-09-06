"""Calibration must recentre an off-centre stick, end to end.

Builds a synthetic pad whose axes rest well off centre -- mimicking the worn
N64 stick measured here (174/185 on a 0-255 axis centred at 128) -- then
checks that:

  * sampling finds the real resting position
  * the republished virtual pad reports centre at rest, not deflection
  * full deflection still reaches the extremes, so travel is not lost

    python3 tools/spike_calibrate.py
"""

import sys
import tempfile
import time
from pathlib import Path

import evdev
from evdev import ecodes

from padmap import calibrate, devices, profiles, virtual

FAKE_NAME = "padmap calib pad"
REST_X, REST_Y = 174, 185
AXIS_MIN, AXIS_MAX = 0, 255
MID = (AXIS_MIN + AXIS_MAX) // 2


def wait_for_pad(name, timeout=5.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        pad = next((p for p in devices.discover() if p.name == name), None)
        if pad:
            return pad
        time.sleep(0.1)
    return None


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="padmap-cal-"))
    failures: list[str] = []

    def absinfo(value):
        return evdev.AbsInfo(value, AXIS_MIN, AXIS_MAX, 0, 0, 0)

    fake = evdev.UInput(
        events={
            ecodes.EV_KEY: [ecodes.BTN_SOUTH, ecodes.BTN_EAST],
            ecodes.EV_ABS: [
                (ecodes.ABS_X, absinfo(REST_X)),
                (ecodes.ABS_Y, absinfo(REST_Y)),
            ],
        },
        name=FAKE_NAME, vendor=0x1209, product=0x0003,
    )

    try:
        pad = wait_for_pad(FAKE_NAME)
        if pad is None:
            print("FAIL: synthetic pad not discovered")
            return 1
        print(f"synthetic pad at {pad.path} resting at X={REST_X} Y={REST_Y}")

        # 1. sampling finds the real rest position
        profile = calibrate.build_profile(pad, icon="n64", seconds=0.3)
        cal_x = profile.axes.get(ecodes.ABS_X)
        cal_y = profile.axes.get(ecodes.ABS_Y)
        if cal_x is None or cal_y is None:
            print(f"FAIL: expected X and Y calibrated, got {list(profile.axes)}")
            return 1
        print(f"measured centres: X={cal_x.center} Y={cal_y.center} "
              f"(deadband +/-{cal_x.flat})")
        if abs(cal_x.center - REST_X) > 2 or abs(cal_y.center - REST_Y) > 2:
            failures.append("sampled centre does not match the resting value")

        # 2. the mapping recentres rest and preserves the extremes
        if cal_x.apply(REST_X) != MID:
            failures.append(f"rest maps to {cal_x.apply(REST_X)}, expected {MID}")
        if cal_x.apply(AXIS_MIN) != AXIS_MIN:
            failures.append(f"min maps to {cal_x.apply(AXIS_MIN)}, lost travel")
        if cal_x.apply(AXIS_MAX) != AXIS_MAX:
            failures.append(f"max maps to {cal_x.apply(AXIS_MAX)}, lost travel")

        profiles.save(profile, directory=tmp)

        # 3. the republished pad starts centred rather than deflected
        import os
        os.environ[profiles.ENV_DIR] = str(tmp)
        vpad = virtual.create(pad, 1)
        try:
            time.sleep(0.4)
            node = evdev.InputDevice(vpad.ui.device.path)
            try:
                values = {
                    code: info.value
                    for code, info in node.capabilities(absinfo=True)[ecodes.EV_ABS]
                }
            finally:
                node.close()
            print(f"virtual pad at rest: X={values.get(ecodes.ABS_X)} "
                  f"Y={values.get(ecodes.ABS_Y)}  (want {MID})")
            if values.get(ecodes.ABS_X) != MID or values.get(ecodes.ABS_Y) != MID:
                failures.append("virtual pad does not rest at centre")
        finally:
            vpad.close()
            os.environ.pop(profiles.ENV_DIR, None)
    finally:
        fake.close()

    # The reported bug: a stick that rests off centre AND cannot reach the
    # declared extremes. Scaling against the declared range leaves almost no
    # travel on the short side -- "it isn't able to go left or up".
    print("\nlimited-reach case (rest 174, physically reaches 150..255):")
    limited = profiles.AxisCalibration(
        center=174, minimum=0, maximum=255, flat=10,
        reach_min=150, reach_max=255,
    )
    left_full = limited.apply(150)
    right_full = limited.apply(255)
    rest = limited.apply(174)
    print(f"  rest 174 -> {rest}   left 150 -> {left_full}   "
          f"right 255 -> {right_full}")
    if rest != MID:
        failures.append(f"limited: rest maps to {rest}, expected {MID}")
    if left_full != AXIS_MIN:
        failures.append(
            f"limited: full left maps to {left_full}, expected {AXIS_MIN} "
            f"-- this is the 'cannot go left' bug")
    if right_full != AXIS_MAX:
        failures.append(
            f"limited: full right maps to {right_full}, expected {AXIS_MAX}")

    # Without reach measurement the same stick loses its left travel, which
    # is what shipped before and what this guards against regressing to.
    naive = profiles.AxisCalibration(center=174, minimum=0, maximum=255, flat=10)
    naive_left = naive.apply(150)
    print(f"  without reach measurement, left 150 -> {naive_left} "
          f"(barely moves from {MID})")
    if naive_left < MID - 40:
        failures.append("naive case unexpectedly had usable left travel")

    if failures:
        print("\nFAIL:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print("\nPASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
