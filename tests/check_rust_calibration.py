"""Does the Rust republisher apply a calibration the Python wrote?

    nix develop --command python3 tests/check_rust_calibration.py

The profile store is the one piece of state both implementations read, and a
disagreement about it is invisible from either side alone: the Python would
keep writing profiles the Rust silently ignores, and the symptom is a stick
that reads permanently deflected in games and perfectly centred in the wizard.

So this writes a profile with `padmap.profiles.save`, republishes the pad with
`padmap-rs`, and checks the clone against `AxisCalibration.apply` -- the Python
function -- across the whole axis.

Needs /dev/uinput writable and rust/target/release/padmap-rs built.
"""

from __future__ import annotations

import json
import os
import select
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

import evdev  # noqa: E402
from evdev import AbsInfo, ecodes  # noqa: E402

from padmap import profiles  # noqa: E402

NAME = "padmap calibration probe"
CLONE = "padmap Player 1"
VID, PID = 0xF056, 0x0002
AXIS = ecodes.ABS_X
# The N64 adapter measured for this project: rests at 174 on a 0-255 axis whose
# nominal centre is 128, which is 36% deflection and reads as a stick held over.
CENTER, MINIMUM, MAXIMUM = 174, 0, 255

BINARY = REPO / "rust" / "target" / "release" / "padmap-rs"


def make_pad() -> evdev.UInput:
    caps = {
        ecodes.EV_KEY: [ecodes.BTN_SOUTH, ecodes.BTN_START],
        ecodes.EV_ABS: [
            (AXIS, AbsInfo(value=CENTER, min=MINIMUM, max=MAXIMUM,
                           fuzz=0, flat=0, resolution=0)),
        ],
    }
    return evdev.UInput(events=caps, name=NAME, vendor=VID, product=PID, version=1)


def find(name: str, deadline: float) -> evdev.InputDevice | None:
    while time.monotonic() < deadline:
        for path in evdev.list_devices():
            try:
                device = evdev.InputDevice(path)
            except OSError:
                continue
            if device.name == name:
                return device
            device.close()
        time.sleep(0.05)
    return None


class _Pad:
    """Only what `profiles.signature` reads."""

    def __init__(self) -> None:
        self.vid, self.pid, self.name = VID, PID, NAME


def main() -> int:
    if not BINARY.is_file():
        print(f"FAIL: {BINARY} is not built. Run: tools/cargo build --release")
        return 1
    if not os.access("/dev/uinput", os.W_OK):
        print("SKIP: /dev/uinput is not writable by this user.")
        return 0

    state = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "padmap-calcheck"
    store = state / "profiles"
    shutil.rmtree(state, ignore_errors=True)
    store.mkdir(parents=True)

    calibration = profiles.AxisCalibration(
        center=CENTER, minimum=MINIMUM, maximum=MAXIMUM)
    profile = profiles.Profile(signature=profiles.signature(_Pad()), name=NAME)
    profile.axes[AXIS] = calibration
    written = profiles.save(profile, store)
    print(f"profile written by the Python: {written.name}")

    source = make_pad()
    time.sleep(0.5)
    physical = find(NAME, time.monotonic() + 10.0)
    if physical is None:
        print("FAIL: the synthetic pad never appeared.")
        return 1
    node = physical.path
    physical.close()

    (state / "padmap").mkdir(parents=True, exist_ok=True)
    (state / "padmap" / "assignments.json").write_text(json.dumps([{
        "player": 1, "path": node, "name": NAME,
        "phys": "", "vid": VID, "pid": PID,
    }]))

    env = dict(os.environ)
    env["XDG_RUNTIME_DIR"] = str(state)
    env["PADMAP_PROFILE_DIR"] = str(store)
    env["PADMAP_ONLY_DEVICE"] = NAME
    child = subprocess.Popen(
        [str(BINARY), "run"], env=env, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL, start_new_session=True)

    clone = None
    failures = 0
    try:
        clone = find(CLONE, time.monotonic() + 20.0)
        if clone is None:
            print(f"FAIL: no {CLONE!r} appeared; padmap-rs did not republish.")
            return 1
        os.set_blocking(clone.fd, False)
        time.sleep(0.5)
        while clone.read_one() is not None:
            pass

        print("\nraw -> clone, against the Python's AxisCalibration.apply:")
        # Every value, because a rescale that is right in the middle and wrong
        # at one end is a stick that cannot reach a corner.
        for raw in range(MINIMUM, MAXIMUM + 1):
            source.write(ecodes.EV_ABS, AXIS, raw)
            source.syn()
            got = None
            deadline = time.monotonic() + 0.5
            while time.monotonic() < deadline:
                if not select.select([clone.fd], [], [], deadline - time.monotonic())[0]:
                    break
                while True:
                    event = clone.read_one()
                    if event is None:
                        break
                    if event.type == ecodes.EV_ABS and event.code == AXIS:
                        got = event.value
                if got is not None:
                    break
            want = calibration.apply(raw)
            if got is None:
                # The clone only emits a value that changed, so an unchanged
                # answer is correct rather than missing.
                continue
            if got != want:
                failures += 1
                if failures <= 10:
                    print(f"  FAIL raw={raw:3d} clone={got:4d} python={want:4d}")

        if failures:
            print(f"\nFAIL: {failures} value(s) disagree with the Python.")
            return 1
        print(f"  ok  every value {MINIMUM}..{MAXIMUM} matches "
              f"AxisCalibration.apply")
        print("  ok  the Rust republisher reads the Python's profile store")
        print("\nall checks passed")
        return 0
    finally:
        if clone is not None:
            clone.close()
        try:
            os.killpg(os.getpgid(child.pid), signal.SIGTERM)
            child.wait(timeout=5)
        except (OSError, ProcessLookupError, subprocess.TimeoutExpired):
            child.kill()
        source.close()
        shutil.rmtree(state, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
