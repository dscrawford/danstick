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

from . import layouts, profiles
from .layouts import Layout
from .mapping import (AxisSpan, Binding, axis_index,
                      retroarch_button_index, sdl_button_index)

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

# How close to rest an axis must come back before it may answer another
# prompt. Without this, one push answers two controls: release a d-pad that is
# wired to an analogue axis and it springs back *through* centre, overshooting
# far enough to read as a deliberate push the other way. Pressing left then
# filled in both left and right, which is exactly what it looked like.
#
# Measured from rest, like everything else here, and rest may itself be wrong:
# it is read from the driver as the wizard opens, and an adapter can report a
# stale power-on default until the stick is physically moved -- the N64 one
# here reads 36% off true centre that way. That turns out not to need extra
# tolerance, because a release reports every value on the way back and so
# passes *through* whatever the driver called rest, whether or not the axis
# settles there.
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


def deflection(span: AxisSpan, value: int) -> float:
    """How far an axis has moved from rest, as a fraction of half its range.

    Signed, because the direction of travel is what a binding records.

    Measured from *rest* rather than from the middle of the declared range,
    and that distinction is the entire point of this function. An analogue
    trigger rests at its minimum, so measuring from the midpoint reports an
    untouched trigger as fully deflected. On a GameCube pad that made L and R
    unusable in the wizard: the first press recorded the direction the trigger
    was travelling *from*, and afterwards the axis could never come back near
    enough to the midpoint to be re-armed, so every later press was dropped
    and the wizard looked frozen.

    Scaled by half the declared range rather than by the travel actually
    available in the direction of movement, so a trigger reads 0 at rest and
    2.0 fully pressed. Every threshold in this module is a floor, so reading
    high is harmless; normalising by the available travel would instead make
    an off-centre stick need a bigger push on its long side than its short one.
    """
    minimum, maximum, rest = span
    if maximum <= minimum:
        return 0.0
    return (value - rest) / ((maximum - minimum) / 2)


# What a chooser is asking about. Sent to the front-end so a theme holds no
# list of its own -- one there would silently fall behind, and a console (or a
# scope) added on this side would simply never appear.
KIND_LAYOUT = "layout"
KIND_SCOPE = "scope"


@dataclass(frozen=True)
class Option:
    """One entry on a chooser's strip.

    `layout` is what gets *drawn* while this entry is selected, which is the
    entire reason the two pickers share one overlay: the pad shown is the pad
    the wizard will then ask about, from one set of coordinates. A separate
    list of names could offer an entry whose layout says something else, and
    nothing would notice.
    """

    # What the daemon acts on: a layout id for the layout picker, a
    # `profiles` scope string for the scope picker.
    id: str
    # What the user reads.
    label: str
    # Layout id to draw. May differ from `id` -- a scope option's id is
    # "console:n64" while the picture is the N64 pad.
    layout: str = ""
    # Whether something is already recorded here. Shown, because otherwise
    # there is no way to tell which scopes a controller already has a mapping
    # for, and re-mapping one is destructive.
    mapped: bool = False

    def to_json(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "label": self.label,
            "mapped": self.mapped,
            "layout": layouts.get(self.layout).to_json(),
        }


@dataclass
class Chooser:
    """A strip of options worked from the pad, before mapping its buttons.

    Two questions use it. Which console a controller is -- necessary because
    the layout decides *which prompts the wizard shows*, and a wrong one
    cannot be answered: an N64 pad walked through the generic layout is asked
    for an X, a Y and two analogue triggers it does not have, and the natural
    response (pressing the stick, since nothing else is left) binds face
    buttons to axes. That is how a real mapping ended up with cancel on
    `-a3`. And what a mapping is *for* -- every game, one console, or one
    game -- which is the same shape of question and had no reason to be a
    second mechanism.

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
    options: list[Option]
    # Which question this is, so the front-end can title it. Sent rather than
    # inferred: a theme guessing from the option ids would be a third place
    # that has to know what a scope string looks like.
    kind: str = KIND_LAYOUT
    title: str = ""
    index: int = 0
    axes: dict[int, AxisSpan] = field(default_factory=dict)
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
        return self.options[self.index].id if self.options else ""

    @property
    def chosen_layout(self) -> str:
        return self.options[self.index].layout if self.options else ""

    def move(self, delta: int) -> bool:
        if not self.options:
            return False
        self.index = (self.index + delta) % len(self.options)
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
            position = deflection(span, event.value)
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
            # Kept as `layout_choice` although it now carries both questions.
            # The event name is part of the C++ client and the theme, and
            # renaming it would buy nothing but a wider patch.
            "event": "layout_choice",
            "active": not self.confirmed,
            "player": self.player,
            "kind": self.kind,
            "title": self.title,
            "index": self.index,
            "chosen": self.chosen,
            # Built from the same options the daemon will act on, so the
            # picture the user chose from and what the daemon does next
            # cannot disagree about which entry index 2 is.
            "choices": [option.to_json() for option in self.options],
        }


def layout_options(mapped_layouts: set[str] | None = None) -> list[Option]:
    """"Which controller is this?", as a strip of every layout padmap knows.

    Whole layouts travel to the front-end (see `Option.to_json`) rather than
    names, so the theme holds no console list of its own: one there would
    silently fall behind, and a layout added to `layouts.ALL` would simply
    never appear.
    """
    already = mapped_layouts or set()
    return [
        Option(id=layout.id, label=layout.label, layout=layout.id,
               mapped=layout.id in already)
        for layout in layouts.ALL.values()
    ]


def game_scope_options(
    console: str, key: str, title: str, scopes: set[str],
) -> list[Option]:
    """"Console or just this game?", asked with both answers already known.

    The strip `scope_options` builds has to offer every console and a handful
    of recently played games, because it is reached from the controller setup
    screen, which knows nothing about what the user wants to play. Reached
    from a game in the library instead, both facts are already in hand -- this
    *is* an N64 game and it *is* GoldenEye -- so the question collapses to two
    entries and there is nothing to scroll past and nothing to get wrong.

    Both draw the console's pad, because both are captured against it: a
    mapping for one N64 game is still a mapping of the N64 control set.

    Console first. It is the answer that is right more often -- a pad that
    needs remapping for one N64 game usually needs it for all of them -- and
    the first entry is the one a hurried user confirms.
    """
    # Both entries are captured against the console's control set, so without
    # a console there is nothing coherent to offer -- not even the game. The
    # daemon relies on an empty list here to say "no console known for this
    # game" rather than record a mapping under a scope it cannot draw. This is
    # reachable: the exporter writes x-gamekey for every game but omits
    # x-console when the collection's core is not one padmap recognises, so a
    # front-end really can send a key with no console.
    if not console:
        return []

    options: list[Option] = []
    scope = profiles.console_scope(console)
    label = layouts.get(console)
    options.append(Option(
        id=scope,
        label=f"{label.console_label or label.label} games",
        layout=console, mapped=scope in scopes,
    ))
    if key:
        scope = profiles.game_scope(key)
        options.append(Option(
            id=scope, label=title or key, layout=console,
            mapped=scope in scopes,
        ))
    return options


def scope_options(
    scopes: set[str], default_layout: str = "",
    recent: list[tuple[str, str, str]] | None = None,
) -> list[Option]:
    """"What is this mapping for?", as a strip of scopes.

    `scopes` is what the controller already has a capture under, so the strip
    can show it -- re-mapping a scope replaces it, and without a mark there is
    no way to tell which ones that would destroy.

    `default_layout` is the pad's best guess, drawn beside the "any game"
    entry only so the strip has a picture there; it is not a promise about
    which layout the wizard will walk, because that entry leads to the layout
    picker.

    `recent` is [(console layout id, game key, title), ...] for games launched
    through padmap-play, newest first. That is the only way a per-game scope
    can be offered at all: the setup screen is reached from the front-end,
    never from inside a game, so nothing else on this screen knows which game
    the user means.

    Several rather than only the newest. "The controls were wrong in the game
    I just played" is the obvious case, but it is not the only one -- someone
    who has since started something else would otherwise find the game they
    actually wanted to fix no longer on offer, with no way to reach it but to
    launch it again. Kept short deliberately; see protocol.RECENT_GAMES.
    """
    options = [
        Option(id=profiles.SCOPE_UNIVERSAL, label="Any game",
               layout=default_layout,
               mapped=profiles.SCOPE_UNIVERSAL in scopes),
    ]
    for layout_id in layouts.CONSOLES:
        scope = profiles.console_scope(layout_id)
        console = layouts.get(layout_id)
        options.append(Option(
            id=scope,
            label=f"{console.console_label or console.label} games",
            layout=layout_id, mapped=scope in scopes,
        ))
    seen: set[str] = set()
    for game_console, key, title in (recent or []):
        if not key or key in seen:
            continue
        seen.add(key)
        scope = profiles.game_scope(key)
        options.append(Option(
            id=scope, label=title or key, layout=game_console,
            mapped=scope in scopes,
        ))
    return options


@dataclass
class MappingRun:
    """One pass through a layout, recording what the user presses."""

    pad: Any
    player: int
    layout: Layout
    keys: list[int]
    # Which scope the result will be filed under -- see profiles.scope_order.
    # Carried on the run rather than remembered beside it, because the answer
    # is needed at the *end*, and a wizard that can be abandoned, restarted,
    # or opened for a different player in between is exactly the shape of
    # thing that loses a value parked elsewhere.
    scope: str = ""
    # Absolute axis travel, as {code: (minimum, maximum, rest)}, for deciding
    # when an axis has been pushed rather than nudged. Rest is measured, not
    # assumed to be the centre -- an analogue trigger rests at its minimum.
    axes: dict[int, AxisSpan] = field(default_factory=dict)

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
            # Just recorded something. Accept nothing -- but go on tracking
            # what is *released*, or the gap leaves state behind that nothing
            # afterwards can correct.
            if event.type == EV_KEY and event.value == 0:
                # So a button held across the gap is not still considered down.
                self._held.discard(event.code)
                self._down_at.pop(event.code, None)
                self.held.discard(event.code)
            elif event.type == EV_ABS:
                # An axis let go inside the gap has genuinely been let go, and
                # a release takes about a tenth of the time the gap lasts, so
                # this is where nearly every one of them lands. Dropping it
                # leaves the axis disarmed with nothing left to re-arm it: a
                # trigger settles at rest and stops reporting entirely, and
                # the wizard then ignores it for good.
                self._rearm(event)
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

    def _rearm(self, event: Any) -> None:
        """Note an axis that has come back to rest, so it may answer again.

        Separate from _feed_abs because it has to run in places that accept
        nothing at all -- notably inside the capture gap, where the release of
        whatever was just recorded arrives.
        """
        if event.code in (ABS_HAT0X, ABS_HAT0Y):
            # A hat only reads 0 at rest, so this is unambiguous.
            if event.value == 0:
                self._axis_armed[event.code] = True
            return
        span = self.axes.get(event.code)
        if span is None:
            return
        if abs(deflection(span, event.value)) < AXIS_RELEASE:
            self._axis_armed[event.code] = True

    def _feed_abs(self, event: Any) -> bool:
        self._rearm(event)
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
                return False        # released; _rearm has already noted it
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
        # Signed travel away from where this axis sat when the wizard opened.
        # From *rest*, not from the middle of the declared range: see
        # deflection, and the analogue trigger that made the difference.
        # _rearm has already decided whether this reading counts as a release.
        position = deflection(span, event.value)
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
