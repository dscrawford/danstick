"""Qt bridge between the Assigner and the QML setup screen.

The Assigner's own `run()` owns a select loop, which cannot coexist with Qt's.
Instead this drives the same state machine from Qt's event loop:

  * one QSocketNotifier per device fd -> `Assigner.handle_readable`
  * one QTimer -> `Assigner.tick`, which is what notices a hold completing
    (a held button emits no further events, so nothing else would)

Confirming is a second hold on a pad that already owns a slot. That keeps the
whole screen operable from a controller, which is the point -- and it works on
any pad, unlike keying it to a specific button such as START, which not every
adapter reports.
"""

from __future__ import annotations

import time
from functools import partial
from typing import Any

from PySide6.QtCore import (
    Property, QObject, QSocketNotifier, QTimer, Signal, Slot,
)

from .. import devices
from ..assign import Assigner, Assignment
from ..devices import Pad

# Longer than HOLD_SECONDS: confirming ends the screen, so it should take a
# deliberate press rather than the same flick that claims a slot.
CONFIRM_HOLD_SECONDS = 0.7

TICK_MS = 16


class SetupModel(QObject):
    slotsChanged = Signal()
    holdChanged = Signal()
    confirmHoldChanged = Signal()
    statusChanged = Signal()
    accepted = Signal()

    def __init__(
        self,
        players: int = 4,
        parent: QObject | None = None,
        pads: list[Pad] | None = None,
        grab: bool = True,
    ) -> None:
        """`pads` and `grab` exist so this can be driven against a synthetic
        uinput pad in a test without grabbing the machine's real controllers.
        """
        super().__init__(parent)
        self._players = players
        self._hold = 0.0
        self._confirm_hold = 0.0
        self._status = ""
        self._notifiers: list[QSocketNotifier] = []
        # Keyed by device path rather than fd: the callback hands us a Pad.
        self._confirm_started: dict[str, float] = {}

        if pads is None:
            pads = devices.discover()
        self._assigner = Assigner(pads, grab=grab)
        self._assigner.__enter__()
        self._assigner.on_claimed_event = self._on_claimed_event

        for fd in self._assigner.fds:
            notifier = QSocketNotifier(fd, QSocketNotifier.Type.Read, self)
            # Bind the fd ahead of the signal's own arguments.
            #
            # Two traps here, both of which bite silently. `activated` hands
            # back a QSocketDescriptor rather than an int, and it has no
            # __int__ -- and it emits *two* values (descriptor, Type), so a
            # `lambda _d, fd=fd:` default gets overwritten by the second one.
            # Either mistake makes handle_readable miss in _devices, which
            # means the device is never drained, so the fd stays readable and
            # the notifier fires in a tight loop forever.
            notifier.activated.connect(partial(self._on_readable, fd))
            self._notifiers.append(notifier)

        self._timer = QTimer(self)
        self._timer.setInterval(TICK_MS)
        self._timer.timeout.connect(self._on_tick)
        self._timer.start()

        self._update_status(f"{len(pads)} controller(s) detected")

    # -- properties read by QML -------------------------------------------

    @Property(int, constant=True)
    def players(self) -> int:
        return self._players

    @Property("QVariantList", notify=slotsChanged)
    def slots(self) -> list[dict[str, Any]]:
        """One entry per player slot, claimed or not."""
        out: list[dict[str, Any]] = []
        for index in range(self._players):
            if index < len(self._assigner.assignments):
                assignment = self._assigner.assignments[index]
                out.append({
                    "player": assignment.player,
                    "name": _clean(assignment.pad.name),
                    "event": assignment.pad.event,
                    "claimed": True,
                })
            else:
                out.append({
                    "player": index + 1,
                    "name": "",
                    "event": "",
                    "claimed": False,
                })
        return out

    @Property(float, notify=holdChanged)
    def hold(self) -> float:
        """0..1 progress of a hold in flight, for the next unclaimed slot."""
        return self._hold

    @Property(float, notify=confirmHoldChanged)
    def confirmHold(self) -> float:
        return self._confirm_hold

    @Property(str, notify=statusChanged)
    def status(self) -> str:
        return self._status

    @Property(int, notify=slotsChanged)
    def claimedCount(self) -> int:
        return len(self._assigner.assignments)

    # -- actions invoked from QML -----------------------------------------

    @Slot()
    def reset(self) -> None:
        self._assigner.reset()
        self._hold = 0.0
        self._confirm_hold = 0.0
        self._confirm_started.clear()
        self.slotsChanged.emit()
        self.holdChanged.emit()
        self.confirmHoldChanged.emit()
        self._update_status("Cleared. Hold a button to claim Player 1.")

    @Slot()
    def accept(self) -> None:
        if not self._assigner.assignments:
            return
        self._timer.stop()
        for notifier in self._notifiers:
            notifier.setEnabled(False)
        self.accepted.emit()

    def assignments(self) -> list[Assignment]:
        return list(self._assigner.assignments)

    def close(self) -> None:
        self._timer.stop()
        for notifier in self._notifiers:
            notifier.setEnabled(False)
        self._assigner.close()

    # -- event loop plumbing ----------------------------------------------

    def _on_readable(self, fd: int, *_signal_args: object) -> None:
        self._assigner.handle_readable(fd)

    def _on_tick(self) -> None:
        before = len(self._assigner.assignments)
        # Reset each tick: _check_holds only reports pads currently held, so a
        # released button would otherwise leave the ring frozen at its last
        # fraction rather than snapping back to empty.
        progress = 0.0

        def on_progress(_pad: Pad, fraction: float) -> None:
            nonlocal progress
            progress = max(progress, min(fraction, 1.0))

        self._assigner.tick(on_progress=on_progress, on_claim=None)

        if progress != self._hold:
            self._hold = progress
            self.holdChanged.emit()

        if len(self._assigner.assignments) != before:
            self.slotsChanged.emit()
            claimed = len(self._assigner.assignments)
            if claimed >= self._players:
                self._update_status("All slots filled. Hold again to continue.")
            else:
                self._update_status(
                    f"Player {claimed} set. "
                    f"Hold a button for Player {claimed + 1}, "
                    f"or hold again to continue."
                )

        self._tick_confirm()

    def _on_claimed_event(self, pad: Pad, _code: int, value: int) -> None:
        """Track press/release on pads that already own a slot."""
        fd_key = pad.path
        if value == 1:
            self._confirm_started.setdefault(fd_key, time.monotonic())
        elif value == 0:
            self._confirm_started.pop(fd_key, None)

    def _tick_confirm(self) -> None:
        if not self._confirm_started:
            if self._confirm_hold:
                self._confirm_hold = 0.0
                self.confirmHoldChanged.emit()
            return

        now = time.monotonic()
        elapsed = max(now - started for started in self._confirm_started.values())
        fraction = min(elapsed / CONFIRM_HOLD_SECONDS, 1.0)
        if fraction != self._confirm_hold:
            self._confirm_hold = fraction
            self.confirmHoldChanged.emit()
        if fraction >= 1.0:
            self.accept()

    def _update_status(self, text: str) -> None:
        if text != self._status:
            self._status = text
            self.statusChanged.emit()


def _clean(name: str) -> str:
    """Strip control characters some adapters prefix to their device name.

    The N64 adapter here reports "\\x18USB GamePad USB GamePad"; rendering that
    verbatim puts a replacement glyph in the middle of the UI.
    """
    return "".join(ch for ch in name if ch.isprintable()).strip()
