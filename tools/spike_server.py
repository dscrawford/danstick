"""Drive the daemon over its socket against a synthetic pad.

Runs the whole protocol without touching the machine's real controllers: a
uinput gamepad is created, the Server is pointed at just that pad, and a
client socket walks begin -> claim -> confirm -> accept.

    QT_QPA_PLATFORM= python3 tools/spike_server.py
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


def wait_for_pad(name: str, timeout: float = 5.0):
    """udev classification is racy, so poll rather than sleeping a guess."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        pad = next((p for p in devices.discover() if p.name == name), None)
        if pad is not None:
            return pad
        time.sleep(0.1)
    return None


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="padmap-spike-"))
    sock_path = tmp / "padmap.sock"

    fake = evdev.UInput(
        events={
            ecodes.EV_KEY: [ecodes.BTN_SOUTH, ecodes.BTN_EAST],
            ecodes.EV_ABS: [
                (ecodes.ABS_X, evdev.AbsInfo(0, -32768, 32767, 0, 0, 0)),
                (ecodes.ABS_Y, evdev.AbsInfo(0, -32768, 32767, 0, 0, 0)),
            ],
        },
        name=FAKE_NAME,
        vendor=0x1209,
        product=0x0002,
    )

    pad = wait_for_pad(FAKE_NAME)
    if pad is None:
        print(f"FAIL: synthetic pad {FAKE_NAME!r} not discovered")
        fake.close()
        return 1
    print(f"synthetic pad at {pad.path}")

    server = Server(
        socket_path=sock_path,
        state_path=tmp / "assignments.json",
        launch_config_path=tmp / "launch.cfg",
    )
    # Restrict discovery to the synthetic pad so the real controllers are
    # never grabbed by this test.
    original_discover = devices.discover
    devices.discover = lambda *a, **kw: (  # type: ignore[assignment]
        [pad] if not kw.get("include_virtual") else original_discover(*a, **kw)
    )

    server.start()
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    events: list[dict] = []
    failures: list[str] = []

    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.connect(str(sock_path))
        client.settimeout(0.2)
        reader = protocol.LineReader()

        def drain(seconds: float) -> None:
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                try:
                    data = client.recv(65536)
                except socket.timeout:
                    continue
                except OSError:
                    break
                if not data:
                    break
                events.extend(reader.feed(data))

        drain(0.3)
        if not any(e.get("event") == "state" for e in events):
            failures.append("no state event on connect")

        client.sendall(protocol.encode({"cmd": "begin", "players": 2}))
        drain(0.4)
        if not any(e.get("event") == "pads" for e in events):
            failures.append("no pads event after begin")

        # Claim: hold past HOLD_SECONDS (0.25).
        fake.write(ecodes.EV_KEY, ecodes.BTN_SOUTH, 1)
        fake.syn()
        drain(0.15)
        progressed = [e for e in events if e.get("event") == "progress"
                      and 0.0 < e.get("frac", 0) < 1.0]
        if not progressed:
            failures.append("no partial progress event during hold")

        drain(0.3)
        fake.write(ecodes.EV_KEY, ecodes.BTN_SOUTH, 0)
        fake.syn()
        drain(0.2)

        claims = [e for e in events if e.get("event") == "claim"]
        if len(claims) != 1:
            failures.append(f"expected 1 claim, got {len(claims)}")
        elif claims[0]["name"] != FAKE_NAME:
            failures.append(f"unexpected claim {claims[0]}")

        # Confirm: hold again on the now-assigned pad.
        fake.write(ecodes.EV_KEY, ecodes.BTN_SOUTH, 1)
        fake.syn()
        drain(1.2)
        fake.write(ecodes.EV_KEY, ecodes.BTN_SOUTH, 0)
        fake.syn()
        drain(0.3)

        accepted = [e for e in events if e.get("event") == "accepted"]
        if not accepted:
            failures.append("confirm hold did not produce an accepted event")
        else:
            cfg = Path(accepted[0]["launch_config"])
            if not cfg.is_file():
                failures.append(f"launch config not written to {cfg}")
            else:
                text = cfg.read_text()
                if "input_player1_joypad_index" not in text:
                    failures.append("launch config has no joypad index line")

        ready = [e for e in events if e.get("event") == "state"
                 and e.get("state") == protocol.STATE_READY]
        if not ready:
            failures.append("never reached the ready state")

        client.close()
    finally:
        devices.discover = original_discover  # type: ignore[assignment]
        server.stop()
        thread.join(timeout=2)
        server.close()
        fake.close()

    for event in events:
        if event.get("event") in ("claim", "accepted", "state", "error"):
            print(f"  {event}")

    if failures:
        print("\nFAIL:")
        for failure in failures:
            print(f"  - {failure}")
        return 1

    print("\nPASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
