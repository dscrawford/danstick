"""A client vanishing mid-session must not leave the pads grabbed.

An assignment session holds EVIOCGRAB on every controller. If the front-end
that opened it dies without sending `cancel`, nothing releases those grabs and
every controller on the machine goes dead until the daemon restarts. This
reproduces that: begin a session, drop the socket without cancelling, and
check the daemon released everything.

    python3 tools/spike_disconnect.py
"""

import socket
import sys
import tempfile
import threading
import time
from pathlib import Path

import evdev
from evdev import ecodes

from padmap import devices, protocol
from padmap.server import Server

FAKE_NAME = "padmap test pad"


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="padmap-disc-"))
    sock_path = tmp / "padmap.sock"

    fake = evdev.UInput(
        # Two buttons and two axes: udev's input_id wants a gamepad-shaped
        # device before it sets ID_INPUT_JOYSTICK, and a single axis is not
        # enough to be classified as one.
        events={
            ecodes.EV_KEY: [ecodes.BTN_SOUTH, ecodes.BTN_EAST],
            ecodes.EV_ABS: [
                (ecodes.ABS_X, evdev.AbsInfo(0, -32768, 32767, 0, 0, 0)),
                (ecodes.ABS_Y, evdev.AbsInfo(0, -32768, 32767, 0, 0, 0)),
            ],
        },
        name=FAKE_NAME, vendor=0x1209, product=0x0002,
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

    server = Server(socket_path=sock_path, state_path=tmp / "a.json",
                    launch_config_path=tmp / "l.cfg")
    original = devices.discover
    devices.discover = lambda *a, **kw: (  # type: ignore[assignment]
        [pad] if not kw.get("include_virtual") else original(*a, **kw))

    server.start()
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    failures: list[str] = []
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.connect(str(sock_path))
        client.sendall(protocol.encode({"cmd": "begin", "players": 2}))
        time.sleep(0.4)

        if server._state != protocol.STATE_ASSIGNING:
            failures.append(f"expected assigning, got {server._state}")

        # The pad is grabbed now: a second opener cannot grab it too.
        probe = evdev.InputDevice(pad.path)
        try:
            probe.grab()
            failures.append("pad was NOT grabbed during the session")
            probe.ungrab()
        except OSError:
            pass  # expected: already grabbed
        finally:
            probe.close()

        # Vanish without cancelling.
        client.close()
        time.sleep(0.5)

        if server._state != protocol.STATE_IDLE:
            failures.append(
                f"session survived client disconnect (state={server._state})")

        # And the grab must be gone.
        probe = evdev.InputDevice(pad.path)
        try:
            probe.grab()
            probe.ungrab()
        except OSError:
            failures.append("pad is STILL grabbed after client disconnect")
        finally:
            probe.close()
    finally:
        devices.discover = original  # type: ignore[assignment]
        server.stop()
        thread.join(timeout=2)
        server.close()
        fake.close()

    if failures:
        print("FAIL:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print("PASS: session ended and pads released on client disconnect")
    return 0


if __name__ == "__main__":
    sys.exit(main())
