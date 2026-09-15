"""Does `padmap map` actually record a mapping, end to end?

    nix develop --command python3 tools/check_map_cli.py

It is the only way to map a controller now that the front-end wizard is gone,
so "it starts and prints prompts" is not enough -- what matters is that the
profile on disk afterwards says what the user pressed.

A uinput device stands in for the controller and the presses are injected, so
this needs no hardware and no hands. Everything it touches is redirected into
a temp tree: XDG_CONFIG_HOME, XDG_DATA_HOME, XDG_RUNTIME_DIR and
PADMAP_PROFILE_DIR, plus PADMAP_ONLY_DEVICE so the real controllers on the
machine are never opened or grabbed.

Needs /dev/uinput to be writable.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

import evdev  # noqa: E402
from evdev import AbsInfo, ecodes  # noqa: E402

NAME = "padmap map probe"
VID, PID = 0xF057, 0x0003
# One key per control the SNES layout asks for, in the order it asks. Distinct
# codes, so a mapping that records the wrong one is visible rather than
# plausible.
BUTTONS = [
    ecodes.BTN_SOUTH, ecodes.BTN_EAST, ecodes.BTN_NORTH, ecodes.BTN_WEST,
    ecodes.BTN_TRIGGER_HAPPY1, ecodes.BTN_TRIGGER_HAPPY2,
    ecodes.BTN_TRIGGER_HAPPY3, ecodes.BTN_TRIGGER_HAPPY4,
    ecodes.BTN_SELECT, ecodes.BTN_START, ecodes.BTN_TL, ecodes.BTN_TR,
]
SNES_ORDER = ["a", "b", "x", "y", "dpup", "dpdown", "dpleft", "dpright",
              "back", "start", "leftshoulder", "rightshoulder"]


def make_pad() -> evdev.UInput:
    caps = {
        ecodes.EV_KEY: list(BUTTONS),
        ecodes.EV_ABS: [
            (ecodes.ABS_X, AbsInfo(value=128, min=0, max=255, fuzz=0, flat=0,
                                   resolution=0)),
            (ecodes.ABS_Y, AbsInfo(value=128, min=0, max=255, fuzz=0, flat=0,
                                   resolution=0)),
        ],
    }
    return evdev.UInput(events=caps, name=NAME, vendor=VID, product=PID,
                        version=1)


def tap(pad: evdev.UInput, code: int) -> None:
    """A press and a release, far enough apart to be a tap and not a hold.

    The wizard binds on the *release*, because how long a button was held is
    what separates "this is the button" from "skip this control", and it
    refuses anything for CAPTURE_GAP_SECONDS afterwards.
    """
    pad.write(ecodes.EV_KEY, code, 1)
    pad.syn()
    time.sleep(0.05)
    pad.write(ecodes.EV_KEY, code, 0)
    pad.syn()


def main() -> int:
    if not os.access("/dev/uinput", os.W_OK):
        print("SKIP: /dev/uinput is not writable by this user.")
        return 0

    from padmap import capture

    sandbox = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "padmap-mapcli"
    shutil.rmtree(sandbox, ignore_errors=True)
    store = sandbox / "profiles"
    store.mkdir(parents=True)

    pad = make_pad()
    time.sleep(0.5)

    env = dict(os.environ)
    env["PYTHONPATH"] = str(REPO / "src") + os.pathsep + env.get("PYTHONPATH", "")
    env["PADMAP_PROFILE_DIR"] = str(store)
    env["PADMAP_ONLY_DEVICE"] = NAME
    env["XDG_CONFIG_HOME"] = str(sandbox / "config")
    env["XDG_DATA_HOME"] = str(sandbox / "data")
    env["XDG_RUNTIME_DIR"] = str(sandbox / "run")
    (sandbox / "run").mkdir(parents=True, exist_ok=True)

    child = subprocess.Popen(
        [sys.executable, "-m", "padmap.cli", "map", "--layout", "snes"],
        env=env, cwd=str(REPO), stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, text=True,
    )

    failures = 0
    try:
        # Let it open and grab the pad before anything is injected: a press
        # that lands before the grab is a press the wizard never sees.
        time.sleep(2.0)
        for code in BUTTONS:
            tap(pad, code)
            # Longer than CAPTURE_GAP_SECONDS, or the next tap is refused as
            # the same continuous input.
            time.sleep(capture.CAPTURE_GAP_SECONDS + 0.25)
        output, _ = child.communicate(timeout=30)
    except subprocess.TimeoutExpired:
        child.kill()
        output, _ = child.communicate()
        print("FAIL: `padmap map` did not finish after every control was "
              "pressed.")
        print(output[-1500:])
        return 1
    finally:
        pad.close()

    print("`padmap map` walked the layout:")
    if child.returncode != 0:
        print(f"  FAIL: exited {child.returncode}")
        print(output[-1500:])
        return 1
    print(f"  ok  exited 0")

    from padmap import profiles

    class _Pad:
        vid, pid, name = VID, PID, NAME

    stored = profiles.load(_Pad(), store)   # type: ignore[arg-type]
    if stored is None:
        print("  FAIL: no profile was written, so nothing was saved at all")
        return 1
    print(f"  ok  a profile was written for {profiles.signature(_Pad())!r}")  # type: ignore[arg-type]

    mapping = stored.mappings.get("")
    if mapping is None or not mapping.buttons:
        print("  FAIL: the profile has no universal mapping")
        return 1
    if mapping.layout != "snes":
        failures += 1
        print(f"  FAIL: layout stored as {mapping.layout!r}, wanted 'snes'")
    else:
        print("  ok  the capture carries the layout it was made against")

    print("\nevery control the layout asked for was recorded, in order:")
    for control, code in zip(SNES_ORDER, BUTTONS):
        binding = mapping.buttons.get(control)
        if binding is None:
            failures += 1
            print(f"  FAIL: {control} was not recorded")
            continue
        # The wizard stores SDL's button *number*, not the evdev code, and the
        # two differ on any pad carrying a code below 0x120. Recomputing the
        # expected number the way mapping.sdl_button_index does is the point:
        # a test that just checked "something was stored" would pass on a
        # mapping that named a different button.
        from padmap import mapping as mapping_mod
        want = mapping_mod.sdl_button_index(sorted(BUTTONS), code)
        if binding.index != want:
            failures += 1
            print(f"  FAIL: {control} -> b{binding.index}, wanted b{want} "
                  f"(evdev {code:#x})")
        else:
            print(f"  ok  {control:<14} -> {binding.sdl()}  (evdev {code:#x})")

    shutil.rmtree(sandbox, ignore_errors=True)
    if failures:
        print(f"\n{failures} check(s) failed")
        return 1
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
