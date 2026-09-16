"""The `controller` event: everything needed to bind a pad, in one message.

padmap's premise is that a program attaches to it and gets stable virtual
gamepads instead of configuring controllers itself. That works at launch, when
the launcher reads the files padmap wrote. It did not work *during* a game: a
controller plugged in mid-session produced no event a running program could act
on, so the only way to pick it up was to quit and start again.

This builds the message that closes that gap. A consumer receiving one has
enough to bind the new pad live -- the virtual node, the SDL GUID and mapping
line, the RetroArch port index and its bind lines -- without reading a file,
asking padmap anything further, or knowing how padmap works.

**Self-sufficient on purpose.** Every event carries the whole roster, not just
the controller that changed. A consumer that connected a moment ago, or missed
an event while it was busy, can apply the latest message it holds and be
correct; there is no log to replay and no way to be subtly out of step. The
cost is a bigger message on a socket that carries a few per hour.

Pure: no I/O, no daemon state. The server decides *when* to announce, this
decides *what* the announcement says, and a test can check the second without
standing up the first.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from . import (controllercfg, mapping, profiles, protocol, retroarch,
               virtual)
from .devices import Pad

# A controller arrived and is live: padmap is republishing it and everything
# in `virtual` and `retroarch` describes a pad that exists now.
ACTION_ADDED = "added"
# A controller that was live has gone. Its player slot is deliberately *not*
# reused -- see `next_player` -- so a consumer may keep the port bound and
# expect the same controller back.
ACTION_REMOVED = "removed"
# A controller arrived that padmap cannot bind, because no mapping has ever
# been recorded for this model. There is no virtual pad and nothing to apply.
# Announced anyway: "a controller appeared and does nothing" is the exact
# situation a user needs told, and silence is what makes it baffling.
ACTION_UNCONFIGURED = "unconfigured"

EVENT = "controller"

# Why an `unconfigured` controller cannot be bound. Two different situations
# with the same consequence, and a consumer that wants to tell the user
# something useful needs to know which.
#
# Nobody has ever mapped this model.
REASON_UNMAPPED = "unmapped"
# It has a mapping, but its device node could not be opened -- usually a
# permission that never arrived. See ATTACH_ATTEMPTS in server.py.
REASON_UNREADABLE = "unreadable"


@dataclass(frozen=True)
class Attached:
    """One controller padmap is republishing, and the clone it publishes."""

    player: int
    pad: Pad
    # /dev/input/eventN of the virtual pad. Empty for an unconfigured
    # controller, which has no clone.
    virtual_node: str = ""
    # What the clone advertises, taken from the live VirtualPad. Left None
    # only by callers that have no clone to ask, in which case it is derived
    # -- at the cost of opening the source, and of answering for whatever
    # device is at that path now rather than for the one the clone was made
    # from.
    identity: virtual.Identity | None = None


def next_player(taken: list[int]) -> int:
    """The lowest free 1-based slot.

    Lowest free rather than highest-plus-one, so a controller arriving after
    another was unplugged takes the empty slot instead of opening a fifth one
    beyond three live pads. RetroArch ports are positional; leaving a hole
    means a four-player game with a gap at player 2.
    """
    used = set(taken)
    player = 1
    while player in used:
        player += 1
    return player


def _controller_fields(pad: Pad, *, configured: bool) -> dict[str, Any]:
    return {
        "name": pad.name,
        # Hex strings rather than ints: this is how every other tool on the
        # machine writes a USB id, and a consumer matching against lsusb or a
        # udev rule should not have to reformat it.
        "vid": f"{pad.vid:04x}",
        "pid": f"{pad.pid:04x}",
        "path": pad.path,
        "phys": pad.phys,
        "uniq": pad.uniq,
        # The key padmap stores a profile under. A consumer wanting its own
        # per-controller state should use this and not the name, which is not
        # unique -- two identical pads share it.
        "signature": profiles.signature(pad),
        "configured": configured,
        # False once `padmap hide` has cleared ID_INPUT_JOYSTICK. A consumer
        # enumerating joypads itself will not see this pad at all, which is
        # the intended arrangement and worth being able to confirm.
        "retroarch_visible": pad.retroarch_visible,
    }


def _virtual_fields(entry: Attached) -> dict[str, Any]:
    # In mirror mode this is the pad's own identity; in padmap mode it is
    # 1209:0001. Either way it is what the clone advertises, and a consumer
    # matching on vid/pid has to be told the advertised one rather than the
    # hardware's -- they differ exactly when it matters.
    identity = entry.identity or virtual.identity_for(
        entry.pad, player=entry.player)
    name = virtual.virtual_name(entry.player)
    return {
        "name": name,
        "node": entry.virtual_node,
        "phys": virtual.virtual_phys(entry.player),
        "vid": f"{identity.vendor:04x}",
        "pid": f"{identity.product:04x}",
        "bustype": identity.bustype,
        # Computed from the identity above rather than by asking
        # controllercfg.virtual_guid, which re-derives it from the pad.
        #
        # Those two can disagree, and did: virtual_guid opens the source to
        # read its ids, and when that open fails it falls back to padmap's own
        # identity. An event would then carry vid/pid/bus from the live clone
        # and a GUID from the fallback -- 057e:2009 bus 3 beside a GUID saying
        # 1209:0001 bus 6. A consumer keying on the GUID would register its
        # mapping under one that SDL never looks up, and nothing anywhere
        # would report an error. One source, one answer.
        "guid": mapping.sdl_guid(
            bus=identity.bustype, vendor=identity.vendor,
            product=identity.product, version=identity.version, name=name),
        # Whether the clone is wearing the controller's own ids or padmap's.
        # A consumer that hides physical pads needs to know which, because
        # the two modes need opposite ignore-lists.
        "identity_mode": virtual.identity_mode(),
    }


def _retroarch_fields(
    entry: Attached,
    indices: dict[int, int],
    console: str,
    game: str,
) -> dict[str, Any]:
    """Port, index and the actual bind lines.

    The binds are included rather than only the profile path because a
    consumer may not be RetroArch, and a path is only useful to something
    willing to parse RetroArch's config format. Both are given: the file is
    authoritative, this is the same content already parsed.
    """
    profile = retroarch.parse_profile_text(retroarch.profile_text(
        entry.pad, entry.player, console=console, game=game))
    return {
        # 1-based, as RetroArch's own `input_playerN_*` settings count.
        "port": entry.player,
        # 0-based joypad index, which is what `input_playerN_joypad_index`
        # takes and is *not* the port: hidden pads get no index at all.
        "index": indices.get(entry.player, -1),
        "profile": str(
            retroarch.runtime_autoconfig_dir()
            / f"{virtual.virtual_name(entry.player)}.cfg"),
        "binds": profile,
    }


def entry_fields(
    entry: Attached,
    indices: dict[int, int],
    *,
    console: str = "",
    game: str = "",
    configured: bool = True,
) -> dict[str, Any]:
    """One controller, complete."""
    fields: dict[str, Any] = {
        "player": entry.player,
        "controller": _controller_fields(entry.pad, configured=configured),
    }
    if not configured:
        return fields
    fields["virtual"] = _virtual_fields(entry)
    fields["retroarch"] = _retroarch_fields(entry, indices, console, game)
    fields["sdl_mapping"] = _sdl_line(entry, console, game)
    return fields


def _sdl_line(entry: Attached, console: str, game: str) -> str:
    """The SDL database line for this player's clone, or "".

    Empty when the controller has no capture, which is the same condition
    under which `write_sdl_mappings` writes nothing for it: a line built from
    no bindings claims a pad with no buttons, and SDL believing that is worse
    than SDL falling back to its own database.
    """
    bindings = controllercfg.stored_bindings(entry.pad, console, game)
    if not bindings:
        return ""
    return controllercfg.sdl_line_for(
        entry.player, bindings,
        axis_codes=controllercfg.pad_capabilities(entry.pad)[1],
        pad=entry.pad,
        axes=controllercfg.pad_axis_spans(entry.pad),
    )


def controller_event(
    action: str,
    subject: Attached,
    roster: list[Attached],
    *,
    console: str = "",
    game: str = "",
    indices: dict[int, int] | None = None,
    reason: str = "",
) -> dict[str, Any]:
    """The whole message.

    `subject` is what changed; `roster` is everything live afterwards. For a
    removal the subject is not in the roster, which is the only way a consumer
    can tell which port to release.
    """
    if indices is None:
        indices = {}
    configured = action != ACTION_UNCONFIGURED
    event: dict[str, Any] = {
        "event": EVENT,
        "action": action,
        # Repeated at the top level so the common case -- "which port do I
        # rebind?" -- is one lookup rather than a nested one.
        "player": subject.player,
        "changed": entry_fields(
            subject, indices, console=console, game=game,
            configured=configured),
        "roster": [
            entry_fields(entry, indices, console=console, game=game)
            for entry in sorted(roster, key=lambda item: item.player)
        ],
        # Which game the mappings in this message were resolved for. A scoped
        # mapping differs per console and per game, so a consumer caching
        # binds needs to know what they were scoped to -- otherwise it applies
        # N64 binds to a SNES game and nothing says why the buttons moved.
        "scope": {"console": console, "game": game},
        # Same reason `ensure-daemon` compares it: a consumer that reconnects
        # to a daemon it did not start can tell whether the code changed under
        # it. Cheap here, and impossible to ask for later without a round trip.
        "build": protocol.build_id(),
    }
    if reason:
        event["reason"] = reason
    return event
