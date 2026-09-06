"""Drive SetupModel end to end against a synthetic pad.

This exists because of a bug the QML smoke test could not have caught: the
QSocketNotifier.activated payload is a QSocketDescriptor, not an int, so the
readable-callback raised TypeError on the first button press. Nothing
exercised that path -- smoke_qml.py stubs the model out entirely, and mypy
cannot help because PySide6 types `activated` as a bare ClassVar[Signal] with
the payload types only in a comment.

So: make a real uinput gamepad, point a real SetupModel at it (grab=False, so
the machine's actual controllers are untouched), press a button, and assert a
slot gets claimed.

    QT_QPA_PLATFORM=offscreen python3 tools/spike_ui_assign.py
"""

import sys
import time

import evdev
from evdev import ecodes
from PySide6.QtCore import QEventLoop, QTimer
from PySide6.QtGui import QGuiApplication

from padmap import devices
from padmap.ui.setup_model import SetupModel

FAKE_NAME = "padmap test pad"


def spin(ms: int) -> None:
    """Run the Qt event loop for a while, so notifiers and timers fire."""
    loop = QEventLoop()
    QTimer.singleShot(ms, loop.quit)
    loop.exec()


def main() -> int:
    app = QGuiApplication(sys.argv)

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
    try:
        # udev classification is racy -- poll rather than sleeping a guessed
        # interval, which was flaky between runs.
        pad = None
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            pad = next((p for p in devices.discover() if p.name == FAKE_NAME), None)
            if pad is not None:
                break
            time.sleep(0.1)
        if pad is None:
            print(f"FAIL: synthetic pad {FAKE_NAME!r} not discovered")
            return 1
        print(f"synthetic pad at {pad.path}")

        # grab=False: this must not steal input from the real controllers.
        model = SetupModel(players=2, pads=[pad], grab=False)
        try:
            spin(100)
            if model.claimedCount != 0:
                print("FAIL: a slot was claimed before any button press")
                return 1

            # Hold past HOLD_SECONDS (0.25).
            fake.write(ecodes.EV_KEY, ecodes.BTN_SOUTH, 1)
            fake.syn()
            spin(150)

            partial = model.hold
            if not 0.0 < partial < 1.0:
                print(f"FAIL: expected a partial hold, got {partial}")
                return 1
            print(f"hold in flight: {partial:.2f}")

            spin(250)
            fake.write(ecodes.EV_KEY, ecodes.BTN_SOUTH, 0)
            fake.syn()
            spin(100)

            if model.claimedCount != 1:
                print(f"FAIL: expected 1 claim, got {model.claimedCount}")
                return 1

            slot = model.slots[0]
            if not slot["claimed"] or slot["name"] != FAKE_NAME:
                print(f"FAIL: unexpected slot contents {slot}")
                return 1
            print(f"claimed: Player {slot['player']} = {slot['name']}")

            # A second hold on the now-claimed pad is the confirm gesture.
            accepted: list[bool] = []
            model.accepted.connect(lambda: accepted.append(True))
            fake.write(ecodes.EV_KEY, ecodes.BTN_SOUTH, 1)
            fake.syn()
            spin(900)
            if not accepted:
                print("FAIL: holding a claimed pad did not confirm")
                return 1
            print("confirm gesture fired")
        finally:
            model.close()
    finally:
        fake.close()
        del app

    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
