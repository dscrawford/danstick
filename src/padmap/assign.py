"""Press-and-hold controller assignment.

For pads that are indistinguishable by every static attribute, a human
pressing a button is the only available source of identity. This module turns
that press into a player ordering.

A *held* button is required rather than a single rising edge. During earlier
testing an empty adapter port registered a stray press that a first-edge
scheme would have accepted, silently burning a player slot. A transient cannot
hold; a human cannot tell the difference.
"""

from __future__ import annotations

import logging
import select
import time
from dataclasses import dataclass
from typing import Any, Callable, Iterable

import evdev
from evdev import ecodes

from . import hidraw
from .devices import Pad, open_device

log = logging.getLogger("padmap.assign")

HOLD_SECONDS = 0.25
BTN_FIRST = 0x100  # ignore the keyboard range on combo devices


@dataclass
class Assignment:
    player: int  # 1-based
    pad: Pad
    button: int  # the BTN_* code that claimed the slot


# (pad, held_fraction) -> None. Lets a UI draw a fill/progress ring.
ProgressFn = Callable[[Pad, float], None]
# (assignment) -> None, fired the moment a slot is claimed.
ClaimFn = Callable[[Assignment], None]
# (pad, button code, value) -> None, for button activity on an already-claimed
# pad. A UI can build "hold again to continue" out of this.
ClaimedEventFn = Callable[[Pad, int, int], None]
# (pad, event) -> None, for every event read from any pad. Calibration needs
# the EV_ABS traffic that claim detection ignores, and the pads are grabbed
# during a session so nothing else can read them.
RawEventFn = Callable[[Pad, "evdev.InputEvent"], None]


class Assigner:
    """Watches a set of pads and yields player order from held presses."""

    def __init__(
        self,
        pads: Iterable[Pad],
        grab: bool = True,
        hold_seconds: float = HOLD_SECONDS,
    ) -> None:
        self.pads = list(pads)
        self.hold_seconds = hold_seconds
        self._grab = grab
        # `Any`, not `evdev.InputDevice`, because a hidraw pad is not one --
        # `hidraw.Source` wears the shape of an InputDevice and implements only
        # the twelve members this class and the republisher actually use. Saying
        # InputDevice here was a lie mypy caught and nothing else would have:
        # the two types diverge exactly where force feedback is, and calling a
        # member Source does not have raises AttributeError, which is not an
        # OSError and so passes through every guard between here and the
        # selector loop. Same annotation virtual.create already uses, for the
        # same reason.
        self._devices: dict[int, Any] = {}
        self._pad_by_fd: dict[int, Pad] = {}
        # fd -> (button code, monotonic time it went down)
        self._holding: dict[int, tuple[int, float]] = {}
        self._claimed: set[int] = set()
        self.assignments: list[Assignment] = []
        # Set by a caller that wants button activity from already-claimed pads.
        self.on_claimed_event: ClaimedEventFn | None = None
        # Set by a caller that needs the raw stream, e.g. axis calibration.
        self.on_raw_event: RawEventFn | None = None
        # Pads that could not be grabbed exclusively; their presses reach
        # other applications as well as us.
        self.grab_failures: list[Pad] = []

    def __enter__(self) -> "Assigner":
        for pad in self.pads:
            # The same source the republisher picks, not always the evdev
            # node. A pad that has to be read over hidraw has to be read that
            # way *here too*: the assigner is what the setup screen and the
            # mapping wizard listen through, so reading a different device
            # from the one being republished means the screen that maps a
            # controller cannot see the controller it is mapping.
            device = hidraw.open_source(pad) or open_device(pad)
            if self._grab:
                # Keep setup presses out of whatever else has focus.
                try:
                    device.grab()
                except OSError as exc:
                    # Worth saying out loud rather than swallowing: a failed
                    # grab means every press during setup ALSO reaches the
                    # front-end, so the button that claims a slot doubles as a
                    # UI keypress. Usually means something else already holds
                    # an exclusive grab on the device.
                    log.warning("could not grab %s (%s): presses will leak "
                                "through to other applications",
                                pad.event, exc)
                    self.grab_failures.append(pad)
            self._devices[device.fd] = device
            self._pad_by_fd[device.fd] = pad
        self._drain()
        return self

    def __exit__(self, *_exc) -> None:
        self.close()

    def close(self) -> None:
        for device in self._devices.values():
            if self._grab:
                try:
                    device.ungrab()
                except OSError:
                    pass
            device.close()
        self._devices.clear()

    def _drain(self) -> None:
        """Discard anything queued before we started listening."""
        for device in self._devices.values():
            try:
                while device.read_one() is not None:
                    pass
            except OSError:
                pass

    # -- event-loop integration -------------------------------------------
    #
    # `run` below owns a select loop, which cannot coexist with a GUI toolkit's
    # own loop. These three let an external loop drive the same state machine:
    # watch `fds` for readability, call `handle_readable` when one fires, and
    # call `tick` on a short timer. The timer is not optional -- a held button
    # emits no further events, so a hold completing is only ever noticed by
    # looking at the clock.

    @property
    def fds(self) -> list[int]:
        """Device file descriptors to watch for readability."""
        return list(self._devices)

    def handle_readable(self, fd: int) -> None:
        """Consume pending events on a descriptor reported readable."""
        self._consume(fd)

    def device_for(self, pad: Pad) -> evdev.InputDevice | None:
        """The open handle for a pad, or None if it is not in this session.

        Exposed so calibration can read absinfo without opening a second
        handle: the pads are grabbed here, and a second reader would compete
        for the same events.
        """
        for fd, candidate in self._pad_by_fd.items():
            if candidate.path == pad.path:
                return self._devices.get(fd)
        return None

    def tick(
        self,
        on_progress: ProgressFn | None = None,
        on_claim: ClaimFn | None = None,
    ) -> None:
        """Advance hold timers. Call roughly every 20ms."""
        self._check_holds(on_progress, on_claim)

    def run(
        self,
        wanted: int | None = None,
        timeout: float | None = None,
        on_progress: ProgressFn | None = None,
        on_claim: ClaimFn | None = None,
    ) -> list[Assignment]:
        """Collect assignments until `wanted` slots are filled or time runs out.

        `wanted=None` means "until every discovered pad is claimed", which is
        rarely what you want with multi-port adapters -- most of their ports
        have nothing plugged in and will never press anything.
        """
        target = wanted if wanted is not None else len(self.pads)
        deadline = None if timeout is None else time.monotonic() + timeout

        while len(self.assignments) < target:
            now = time.monotonic()
            if deadline is not None and now >= deadline:
                break

            # Wake often enough to notice a hold completing, since a held
            # button generates no further events on its own.
            wait = 0.02
            if deadline is not None:
                wait = min(wait, max(0.0, deadline - now))

            readable, _, _ = select.select(self.fds, [], [], wait)
            for fd in readable:
                self.handle_readable(fd)

            self.tick(on_progress, on_claim)

        return self.assignments

    def reset(self) -> None:
        """Drop every claim and start over.

        Also drains queued events, so a button still held from the previous
        round cannot immediately re-claim a slot.
        """
        self.assignments.clear()
        self._claimed.clear()
        self._holding.clear()
        self._drain()

    def _consume(self, fd: int) -> None:
        device = self._devices.get(fd)
        if device is None:
            return
        try:
            events = list(device.read())
        except (OSError, BlockingIOError):
            return

        if self.on_raw_event is not None:
            pad = self._pad_by_fd[fd]
            for event in events:
                self.on_raw_event(pad, event)

        if fd in self._claimed:
            # Claimed pads still have to be drained. They are grabbed, so
            # nothing else will empty their buffer, and an unread descriptor
            # stays permanently readable -- which would make any select-based
            # loop spin at 100% CPU. Forwarding the events also gives callers
            # a way to build a confirm gesture on an assigned pad.
            if self.on_claimed_event is not None:
                pad = self._pad_by_fd[fd]
                for event in events:
                    if event.type == ecodes.EV_KEY and event.code >= BTN_FIRST:
                        self.on_claimed_event(pad, event.code, event.value)
            return

        for event in events:
            if event.type != ecodes.EV_KEY or event.code < BTN_FIRST:
                continue
            if event.value == 1:
                # Track only the first button down; a second button pressed
                # during a hold should not restart the timer.
                if fd not in self._holding:
                    self._holding[fd] = (event.code, time.monotonic())
            elif event.value == 0:
                held = self._holding.get(fd)
                if held and held[0] == event.code:
                    # Released early: a tap, or a transient. Not a claim.
                    del self._holding[fd]

    def _check_holds(
        self, on_progress: ProgressFn | None, on_claim: ClaimFn | None
    ) -> None:
        now = time.monotonic()
        for fd, (code, started) in list(self._holding.items()):
            if fd in self._claimed:
                continue
            elapsed = now - started
            if elapsed < self.hold_seconds:
                if on_progress:
                    on_progress(self._pad_by_fd[fd], elapsed / self.hold_seconds)
                continue

            pad = self._pad_by_fd[fd]
            assignment = Assignment(
                player=len(self.assignments) + 1, pad=pad, button=code
            )
            self.assignments.append(assignment)
            self._claimed.add(fd)
            del self._holding[fd]
            if on_progress:
                on_progress(pad, 1.0)
            if on_claim:
                on_claim(assignment)
