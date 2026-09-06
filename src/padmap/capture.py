"""Watching a pad to find out where each control lives.

Deliberately separate from the daemon's socket handling so the interesting
part -- deciding what an incoming event means -- can be exercised without a
session, a client or a controller.

The hard part is not reading events, it is refusing most of them. A pad
streams axis noise continuously, an analogue trigger reports a resting value
that is not zero on some hardware, and the button someone pressed to reach
this screen is often still travelling when the first prompt appears. Every
guard below exists because one of those otherwise fills several controls in
with the same accidental input.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Any, Callable

from . import layouts
from .layouts import Layout
from .mapping import (Binding, axis_index, retroarch_button_index,
                      sdl_button_index)

# evdev constants, spelled out rather than imported: this module is pure logic
# and importing evdev drags in a device library for the sake of five numbers.
EV_KEY = 0x01
EV_ABS = 0x03
ABS_X = 0x00
ABS_HAT0X = 0x10
ABS_HAT0Y = 0x11

# How far an axis must travel from rest before it counts as deliberate.
# Generous, because the alternative -- catching drift -- silently binds a
# control to a stick that merely leans.
AXIS_THRESHOLD = 0.55

# How close to centre an axis must come back before it may answer another
# prompt. Without this, one push answers two controls: release a d-pad that is
# wired to an analogue axis and it springs back *through* centre, overshooting
# far enough to read as a deliberate push the other way. Pressing left then
# filled in both left and right, which is exactly what it looked like.
AXIS_RELEASE = 0.30

# SDL hat bits, which is also how a hat binding is written.
HAT_UP, HAT_RIGHT, HAT_DOWN, HAT_LEFT = 1, 2, 4, 8

# Hold any button this long to skip a control the pad does not have.
#
# It cannot be a *particular* button. The daemon holds EVIOCGRAB on the pads
# for the whole session and republishing is stopped, so the front-end receives
# no controller input at all while this runs -- a "press Select to skip"
# prompt could never have worked from a controller, and Select is not mapped
# until halfway through anyway. A hold needs nothing mapped and works from the
# first prompt, and it is the same gesture that claims a player slot.
SKIP_HOLD_SECONDS = 0.8

# Nothing is accepted for this long after a control is recorded.
#
# Advancing instantly means a single continuous input can answer two prompts:
# hold a d-pad a moment too long and whatever it does next -- a analogue
# oscillation crossing centre, a hat bouncing, a second event from the same
# push -- lands on the control that just became current. The per-axis arming
# rule catches the specific case of one axis springing back; this catches the
# general one, including inputs arriving on a different code entirely.
#
# Long enough to outlast a release and its bounce, short enough that someone
# working quickly does not notice it.
CAPTURE_GAP_SECONDS = 0.35


@dataclass
class LayoutChoice:
    """Choosing which console a controller is, before mapping its buttons.

    Necessary because the layout decides *which prompts the wizard shows*, and
    a wrong one cannot be answered: an N64 pad walked through the generic
    layout is asked for an X, a Y and two analogue triggers it does not have,
    and the natural response -- pressing the stick, since nothing else is left
    -- binds face buttons to axes. That is how a real mapping ended up with
    cancel on `-a3`.

    It has to be driven from the pad itself, and that is the whole difficulty.
    The daemon holds EVIOCGRAB for the duration of a session and republishing
    is stopped, so the front-end receives no controller input at all; a picker
    the theme navigates could only ever be worked from a keyboard. And nothing
    is mapped yet, so no gesture may name a button.

    Both constraints are answered the same way the wizard's skip is:

      * push the stick or d-pad left/right to move, read from the raw axis --
        ABS_X and ABS_HAT0X are horizontal on every pad, with no mapping
        needed to know it
      * hold any button to accept, which needs no mapping either and is the
        same gesture that claims a player slot

    The front-end draws it; the daemon decides it.
    """

    pad: Any
    player: int
    # Layout ids, in catalogue order. Kept as ids rather than Layout objects
    # so the front-end and the daemon are agreeing about the same list.
    choices: list[str]
    index: int = 0
    axes: dict[int, tuple[int, int]] = field(default_factory=dict)
    held: set[int] = field(default_factory=set)
    now: Callable[[], float] = time.monotonic

    confirmed: bool = False
    _down_at: dict[int, float] = field(default_factory=dict)
    # Per axis: which way it is currently pushed, -1, 0 or 1. Moving happens
    # on the transition into a direction, so a stick held over does not spin
    # the selection and a released one does not move it back.
    _pushed: dict[int, int] = field(default_factory=dict)

    def __post_init__(self) -> None:
        self._held = set(self.held)

    @property
    def settling(self) -> bool:
        return bool(self._held & self.held)

    @property
    def chosen(self) -> str:
        return self.choices[self.index] if self.choices else ""

    def move(self, delta: int) -> bool:
        if not self.choices:
            return False
        self.index = (self.index + delta) % len(self.choices)
        return True

    def feed(self, event: Any) -> bool:
        """Offer one evdev event. True if the selection or state changed."""
        if self.confirmed:
            return False
        if event.type == EV_KEY:
            return self._feed_key(event)
        if event.type == EV_ABS:
            return self._feed_abs(event)
        return False

    def _feed_key(self, event: Any) -> bool:
        if event.value == 1:
            self._held.add(event.code)
            self._down_at[event.code] = self.now()
            return False
        if event.value != 0:
            return False

        self._held.discard(event.code)
        started = self._down_at.pop(event.code, None)
        if event.code in self.held:
            # The press that opened the picker, finally released.
            self.held.discard(event.code)
            return False
        if started is None or self.settling:
            return False
        # A tap does nothing on purpose. The button that claimed the slot is
        # often still travelling when this appears, and a picker that accepts
        # the first press anyone makes is a picker nobody gets to use.
        if self.now() - started >= SKIP_HOLD_SECONDS:
            self.confirmed = True
            return True
        return False

    def _feed_abs(self, event: Any) -> bool:
        if self.settling:
            return False

        if event.code == ABS_HAT0X:
            direction = 0 if event.value == 0 else (1 if event.value > 0 else -1)
        else:
            span = self.axes.get(event.code)
            if event.code != ABS_X or span is None:
                return False
            minimum, maximum = span
            if maximum <= minimum:
                return False
            centre = (minimum + maximum) / 2
            position = (event.value - centre) / ((maximum - minimum) / 2)
            direction = 0 if abs(position) < AXIS_THRESHOLD else (
                1 if position > 0 else -1)

        previous = self._pushed.get(event.code, 0)
        self._pushed[event.code] = direction
        if direction == 0 or direction == previous:
            # Deliberately *not* the wizard's re-arming rule, which needs the
            # axis back inside AXIS_RELEASE of centre. An uncalibrated stick
            # can rest at 36% deflection -- measured on the N64 adapter here --
            # which never re-arms, and a picker that stops responding after one
            # move is worse than one that occasionally moves twice. Anything
            # short of the threshold counts as released; a wrong step here
            # costs a nudge back, not a mis-recorded binding.
            return False
        return self.move(direction)

    def to_event(self) -> dict[str, Any]:
        return {
            "event": "layout_choice",
            "active": not self.confirmed,
            "player": self.player,
            "index": self.index,
            "chosen": self.chosen,
            # Built from the same ids the daemon will act on, so the picture
            # the user chose from and the layout the wizard walks cannot
            # disagree about which entry index 2 is.
            "choices": [layouts.get(name).to_json() for name in self.choices],
        }


@dataclass
class MappingRun:
    """One pass through a layout, recording what the user presses."""

    pad: Any
    player: int
    layout: Layout
    keys: list[int]
    # Absolute axis ranges, as {code: (minimum, maximum)}, for deciding when
    # an axis has been pushed rather than nudged.
    axes: dict[int, tuple[int, int]] = field(default_factory=dict)

    index: int = 0
    bindings: dict[str, Binding] = field(default_factory=dict)
    # Raw evdev codes already used, so one button cannot answer two prompts.
    claimed: set[tuple[str, int, int]] = field(default_factory=set)
    # True until everything the user was holding when this started is released.
    # Buttons already down when the run started, read from the device rather
    # than guessed: the press that opened the wizard is usually still held,
    # and its release must not answer the first prompt.
    held: set[int] = field(default_factory=set)
    now: Callable[[], float] = time.monotonic

    _down_at: dict[int, float] = field(default_factory=dict)
    _blocked_until: float = 0.0
    # Axes that have returned near centre since they last answered a prompt.
    # Absent means armed: an axis that has never been touched is ready.
    _axis_armed: dict[int, bool] = field(default_factory=dict)

    def __post_init__(self) -> None:
        self._held = set(self.held)

    @property
    def settling(self) -> bool:
        """True while something held from before the run is still down."""
        return bool(self._held & self.held)

    @property
    def finished(self) -> bool:
        return self.index >= len(self.layout.controls)

    @property
    def current(self):
        if self.finished:
            return None
        return self.layout.controls[self.index]

    def skip(self) -> None:
        """Move past a control this pad does not have."""
        if not self.finished:
            self.index += 1
            # Same gap as a capture: the button being released after a
            # skip-hold must not answer the control it moved on to.
            self._blocked_until = self.now() + CAPTURE_GAP_SECONDS

    def _record(self, binding: Binding, key: tuple[str, int, int]) -> bool:
        control = self.current
        if control is None:
            return False
        self.bindings[control.canonical] = binding
        self.claimed.add(key)
        self.index += 1
        self._blocked_until = self.now() + CAPTURE_GAP_SECONDS
        return True

    def feed(self, event: Any) -> bool:
        """Offer one evdev event. True if it answered the current prompt.

        Returns False for everything else, which is the overwhelming majority
        of what arrives.
        """
        if self.finished:
            return False

        if self.now() < self._blocked_until:
            # Just recorded something. Track releases so a button held across
            # the gap is not still considered down afterwards, but accept
            # nothing.
            if event.type == EV_KEY and event.value == 0:
                self._held.discard(event.code)
                self._down_at.pop(event.code, None)
                self.held.discard(event.code)
            return False

        if event.type == EV_KEY:
            return self._feed_key(event)
        if event.type == EV_ABS:
            return self._feed_abs(event)
        return False

    def _feed_key(self, event: Any) -> bool:
        if event.value == 1:
            self._held.add(event.code)
            self._down_at[event.code] = self.now()
            return False
        if event.value != 0:
            # Autorepeat. Holding a button must not walk the whole wizard.
            return False

        # Release. Binding happens here rather than on the press, because how
        # long it was held is what separates "this is the button" from "skip
        # this control", and that is only known once it comes back up.
        self._held.discard(event.code)
        started = self._down_at.pop(event.code, None)
        if event.code in self.held:
            # Whatever opened the wizard has now been let go.
            self.held.discard(event.code)
            return False
        if started is None or self.settling:
            return False

        if self.now() - started >= SKIP_HOLD_SECONDS:
            self.skip()
            return True

        key = ("button", event.code, 0)
        if key in self.claimed:
            return False

        index = sdl_button_index(self.keys, event.code)
        if index is None:
            return False
        # Both numberings are stored. Recomputing RetroArch's at emission time
        # would need the pad's key list to still be around, and would silently
        # shift every binding on a pad carrying sub-0x120 codes.
        return self._record(
            Binding("button", index,
                    ra_index=retroarch_button_index(self.keys, event.code)),
            key,
        )

    def _feed_abs(self, event: Any) -> bool:
        if self.settling:
            return False

        control = self.current
        if control is not None and control.kind == "button":
            # A face button cannot be a stick. Without this, nudging the stick
            # while being asked for "X" binds X to an axis -- and since that
            # axis is also the stick, every later stick movement presses X.
            # Exactly how a mapping ended up with cancel on `-a3`.
            return False

        if event.code in (ABS_HAT0X, ABS_HAT0Y):
            if event.value == 0:
                # Released. A hat only reads 0 at rest, so this is the
                # unambiguous re-arm point.
                self._axis_armed[event.code] = True
                return False
            if event.code == ABS_HAT0X:
                bit = HAT_RIGHT if event.value > 0 else HAT_LEFT
            else:
                bit = HAT_DOWN if event.value > 0 else HAT_UP
            if not self._axis_armed.get(event.code, True):
                return False
            key = ("hat", 0, bit)
            if key in self.claimed:
                return False
            self._axis_armed[event.code] = False
            return self._record(Binding("hat", 0, bit), key)

        span = self.axes.get(event.code)
        if span is None:
            return False
        minimum, maximum = span
        if maximum <= minimum:
            return False

        # Normalised to -1..1 about the centre of the declared range. A
        # trigger that rests at its minimum reads as fully negative, so only
        # a push *towards* an end counts, and only past the threshold.
        centre = (minimum + maximum) / 2
        half = (maximum - minimum) / 2
        position = (event.value - centre) / half
        if abs(position) < AXIS_RELEASE:
            # Back at rest: this axis may answer a prompt again.
            self._axis_armed[event.code] = True
        if abs(position) < AXIS_THRESHOLD:
            return False

        if not self._axis_armed.get(event.code, True):
            return False

        sign = 1 if position > 0 else -1
        key = ("axis", event.code, sign)
        if key in self.claimed:
            return False
        # By axis *index*, not evdev code: ABS_RZ is code 5 but may be axis 3.
        index = axis_index(list(self.axes), event.code)
        if index is None:
            return False
        # Disarmed until it settles again, so the spring-back does not answer
        # the next prompt too.
        self._axis_armed[event.code] = False
        return self._record(Binding("axis", index, sign), key)

    def to_event(self) -> dict[str, Any]:
        """What a front-end needs to draw the current step."""
        control = self.current
        return {
            "event": "mapping",
            "player": self.player,
            "layout": self.layout.to_json(),
            "index": self.index,
            "total": len(self.layout.controls),
            "control": control.canonical if control else "",
            "label": control.label if control else "",
            "done": self.finished,
            "captured": {
                name: binding.sdl()
                for name, binding in self.bindings.items()
            },
        }
