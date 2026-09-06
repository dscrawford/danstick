"""Calibration driven over the socket, as a front-end will drive it.

The daemon cannot call the blocking samplers: they own a loop for up to five
seconds, which would stall every other pad and client. This checks the
incremental version actually works -- both phases advance from the tick, the
axis samples arrive through the assigner's raw event stream, and a profile
lands on disk.

    python3 tools/spike_calibrate_protocol.py
"""

import socket
import sys
import tempfile
import threading
import time
from pathlib import Path

import evdev
from evdev import ecodes

from padmap import devices, profiles, protocol
from padmap.server import (
    PHASE_AWAIT_REACH, PHASE_AWAIT_REST, PHASE_ICON, PHASE_REACH, PHASE_REST,
    Server,
)

FAKE_NAME = "padmap calib proto pad"
REST_X = 128
AXIS_MIN, AXIS_MAX = 0, 255
# Deliberately narrower than the declared range: the reach phase must notice.
REACH_LOW, REACH_HIGH = 40, 210


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="padmap-calproto-"))
    profile_dir = tmp / "devices"
    import os
    os.environ[profiles.ENV_DIR] = str(profile_dir)

    def absinfo(value):
        return evdev.AbsInfo(value, AXIS_MIN, AXIS_MAX, 0, 0, 0)

    fake = evdev.UInput(
        events={
            ecodes.EV_KEY: [ecodes.BTN_SOUTH, ecodes.BTN_EAST],
            ecodes.EV_ABS: [
                (ecodes.ABS_X, absinfo(REST_X)),
                (ecodes.ABS_Y, absinfo(REST_X)),
            ],
        },
        name=FAKE_NAME, vendor=0x1209, product=0x0004,
    )

    pad = None
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        pad = next((p for p in devices.discover() if p.name == FAKE_NAME), None)
        if pad:
            break
        time.sleep(0.1)
    if pad is None:
        print("FAIL: synthetic pad not discovered")
        fake.close()
        return 1
    print(f"synthetic pad at {pad.path}")

    server = Server(socket_path=tmp / "s.sock", state_path=tmp / "a.json",
                    launch_config_path=tmp / "l.cfg")
    original = devices.discover
    devices.discover = lambda *a, **kw: (  # type: ignore[assignment]
        [pad] if not kw.get("include_virtual") else original(*a, **kw))
    server.start()
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    events: list[dict] = []
    failures: list[str] = []

    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.connect(str(tmp / "s.sock"))
        client.settimeout(0.1)
        reader = protocol.LineReader()

        def drain(seconds):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                try:
                    data = client.recv(65536)
                except socket.timeout:
                    continue
                except OSError:
                    break
                if not data:
                    break
                events.extend(reader.feed(data))

        client.sendall(protocol.encode({"cmd": "begin", "players": 1}))
        drain(0.3)

        # Claim the pad so calibration has a player to target.
        fake.write(ecodes.EV_KEY, ecodes.BTN_SOUTH, 1); fake.syn()
        drain(0.45)
        fake.write(ecodes.EV_KEY, ecodes.BTN_SOUTH, 0); fake.syn()
        drain(0.2)

        claims = [e for e in events if e.get("event") == "claim"]
        if not claims:
            print("FAIL: pad never claimed a slot")
            return 1
        if claims[0].get("configured") is not False:
            failures.append("a never-seen pad should report configured=false")

        def press():
            """A button tap, which is how the wizard advances between phases."""
            fake.write(ecodes.EV_KEY, ecodes.BTN_EAST, 1); fake.syn()
            drain(0.15)
            fake.write(ecodes.EV_KEY, ecodes.BTN_EAST, 0); fake.syn()
            drain(0.15)

        def phases_seen():
            return {e.get("phase") for e in events
                    if e.get("event") == "calibration"}

        client.sendall(protocol.encode({"cmd": "calibrate", "player": 1}))
        drain(0.4)

        # It must WAIT rather than start measuring on its own: being measured
        # before the user is ready is the "too quick" problem.
        if PHASE_AWAIT_REST not in phases_seen():
            failures.append("did not wait for the user before sampling rest")
        if PHASE_REST in phases_seen():
            failures.append("rest sampling started without a button press")

        press()          # -> rest
        drain(1.2)       # rest is timed (0.8s) then moves to await_reach
        if PHASE_REST not in phases_seen():
            failures.append("rest phase never ran")
        if PHASE_AWAIT_REACH not in phases_seen():
            failures.append("did not wait for the user before sampling reach")

        press()          # -> reach
        drain(0.3)
        if PHASE_REACH not in phases_seen():
            failures.append("reach phase never started")

        # Sweep the stick across its real travel.
        end = time.monotonic() + 2.0
        toggle = True
        while time.monotonic() < end:
            value = REACH_LOW if toggle else REACH_HIGH
            toggle = not toggle
            fake.write(ecodes.EV_ABS, ecodes.ABS_X, value)
            fake.write(ecodes.EV_ABS, ecodes.ABS_Y, value)
            fake.syn()
            drain(0.2)

        press()          # -> icon
        drain(0.5)
        if PHASE_ICON not in phases_seen():
            failures.append("did not reach the icon step")
        if "done" in phases_seen():
            failures.append(
                "flow ended before the icon was chosen -- the daemon must stay "
                "modal, or presses leak through to the confirm gesture")

        # Claim detection must be suspended throughout: a button pressed while
        # configuring must not claim a second slot.
        claims_now = [e for e in events if e.get("event") == "claim"]
        if len(claims_now) != 1:
            failures.append(
                f"expected 1 claim, got {len(claims_now)} -- presses during "
                f"configuration leaked into claim detection")

        # And the profile must be on disk with the measured reach.
        saved = profiles.load(pad, directory=profile_dir)
        if saved is None:
            failures.append("no profile written")
        else:
            cal = saved.axes.get(ecodes.ABS_X)
            if cal is None:
                failures.append("ABS_X missing from the saved profile")
            else:
                print(f"saved: centre={cal.center} reach={cal.low}..{cal.high} "
                      f"flat=+/-{cal.flat}")
                if not (REACH_LOW - 2 <= cal.low <= REACH_LOW + 2):
                    failures.append(
                        f"reach_min {cal.low} does not match swept {REACH_LOW}")
                if not (REACH_HIGH - 2 <= cal.high <= REACH_HIGH + 2):
                    failures.append(
                        f"reach_max {cal.high} does not match swept {REACH_HIGH}")

        # Icon choice must persist too -- this is what retires the vid/pid table.
        client.sendall(protocol.encode(
            {"cmd": "set_icon", "player": 1, "icon": "n64"}))
        drain(0.4)
        saved = profiles.load(pad, directory=profile_dir)
        if saved is None or saved.icon != "n64":
            failures.append(f"icon not stored (got {saved.icon if saved else None})")
        elif not saved.axes:
            failures.append("set_icon discarded the calibration")

        client.close()
    finally:
        devices.discover = original  # type: ignore[assignment]
        server.stop()
        thread.join(timeout=2)
        server.close()
        fake.close()
        os.environ.pop(profiles.ENV_DIR, None)

    if failures:
        print("\nFAIL:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print("\nPASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
