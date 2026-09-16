"""Turn a captured mapping into the files RetroArch and SDL actually read.

Two consumers, two formats, one capture. Written together and regenerated on
every accept, which is what makes a mapping follow the controller rather than
the player slot: SDL's database is keyed on the device name, and padmap's
virtual pads are named after the slot, so a stored line goes stale the moment
a controller is assigned somewhere else. Re-emitting for the current slot side-
steps that entirely -- nothing has to be migrated, because nothing is expected
to survive.
"""

from __future__ import annotations

import logging
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

from . import devices, emulators, layouts, mapping, profiles, virtual
from .devices import Pad
from .mapping import Binding
from .virtual import (PADMAP_PID, PADMAP_VERSION, PADMAP_VID, VIRTUAL_PREFIX,
                      identity_for, virtual_name)

log = logging.getLogger("padmap.controllercfg")

# uinput devices report this bus unless told otherwise, and SDL folds it into
# the GUID. Only right for a pad advertising padmap's own identity: in mirror
# mode the virtual pad carries the source's bus, which is what lets SDL's
# database match it at all.
BUS_VIRTUAL = 0x06

# The version uinput gives a device unless told otherwise. Part of the GUID,
# so it has to match what SDL will see rather than what seems reasonable.
VIRTUAL_VERSION = PADMAP_VERSION

MARKER = "# padmap"


ENV_SDL_DB = "PADMAP_SDL_DB"


def sdl_config_path() -> Path:
    """Where padmap writes the SDL game-controller database it generates.

    padmap's own directory, and overridable, because any SDL application can
    be pointed at it:

        SDL_GAMECONTROLLERCONFIG_FILE=~/.config/padmap/sdl_controllers.txt

    It used to be written straight into a particular front-end's config
    directory, which made the mappings that front-end's property rather than
    the controller's -- every other SDL program on the machine had a pad
    padmap had mapped and no way to hear about it.
    """
    override = os.environ.get(ENV_SDL_DB)
    if override:
        return Path(override)
    return Path(
        os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")
    ) / "padmap" / "sdl_controllers.txt"


def virtual_guid(player: int, pad: Pad | None = None) -> str:
    """The GUID SDL will compute for a player's virtual pad.

    Every input is known in advance -- padmap creates the device -- so a
    mapping can be written before SDL has ever seen it. Confirmed against the
    live pad: with padmap's own identity SDL reports
    0600c9a7091200000100000001000000 for "padmap Player 1".

    `pad` is the physical controller behind the slot, and is required for a
    correct answer: by default the virtual pad *mirrors* it (see
    virtual.identity_for), so the GUID depends on the hardware. Without one
    this falls back to padmap's own identity, which is right only in that
    mode -- callers that have a pad must pass it.
    """
    identity = (
        identity_for(pad, player=player) if pad is not None
        else virtual.Identity(PADMAP_VID, PADMAP_PID, BUS_VIRTUAL,
                              virtual.version_for(player))
    )
    return mapping.sdl_guid(
        bus=identity.bustype, vendor=identity.vendor,
        product=identity.product, version=identity.version,
        name=virtual_name(player),
    )


def sdl_line_for(
    player: int, bindings: dict[str, Binding],
    axis_codes: list[int] | None = None, pad: Pad | None = None,
    axes: dict[int, mapping.AxisSpan] | None = None,
) -> str:
    """The SDL database line for a player's virtual pad.

    `axis_codes` are the pad's ABS codes, used to declare its sticks. They are
    not part of the capture -- see mapping.stick_fields -- but a line without
    them leaves SDL believing the pad has no sticks, and a front-end with no
    way to navigate but the d-pad.

    `axes` adds where each of those axes rests, which is what separates a
    stick from a trigger. Optional, because a caller without it is no worse
    off than before; supplying it is what stops an analogue trigger being
    declared a stick that is permanently pushed over.
    """
    name = virtual_name(player)
    return mapping.sdl_mapping(
        virtual_guid(player, pad), name, bindings,
        sticks=mapping.stick_fields(axis_codes or [], bindings, axes),
    )


def write_sdl_mappings(
    lines: dict[int, str], path: Path | None = None,
    notes: dict[int, str] | None = None,
) -> Path:
    """Replace padmap's lines in the SDL controller database, keeping others.

    Rewritten rather than appended: appending a second line for the same GUID
    leaves SDL to pick one, and which one is not something to rely on. Lines
    for devices padmap does not manage are left exactly as they are -- users
    map their own controllers in there too.

    Raises OSError if the existing database cannot be read. Preserving those
    foreign lines is the entire reason this rewrites instead of appending, and
    a read that failed cannot tell "the file is empty" from "the file is there
    and I could not see it" -- so the rewrite is abandoned rather than allowed
    to publish the difference. Callers already treat a write failure as an
    OSError (the daemon's command guard among them), so a refusal is reported
    the same way a full disk would be.
    """
    target = path or sdl_config_path()
    target.parent.mkdir(parents=True, exist_ok=True)

    # Every slot padmap could ever name, not just the ones being written: a
    # controller that moved slots leaves a line behind under the old name,
    # and SDL would go on matching it.
    ours = {virtual_guid(player) for player in range(1, 17)}
    kept: list[str] = []
    try:
        existing = target.read_text(errors="replace")
    except FileNotFoundError:
        # No database yet -- a first run, or a symlink whose target has still
        # to be created. There is nothing to preserve, so there is nothing to
        # lose: write ours and be done.
        existing = ""
    except OSError:
        # Anything else means the file is there and we could not read it:
        # mode 0222 after a hand-edit, an unreadable mount, an I/O error.
        # This used to be swallowed, and "could not read" became "was empty" --
        # so a user with a hand-written mapping in sdl_controllers.txt lost it
        # on the next wizard run, silently. Refuse instead: their lines are
        # what this whole read-and-rewrite exists to keep.
        log.warning("cannot read %s -- refusing to rewrite it, because the "
                    "mappings already in it would be lost", target)
        raise

    for line in existing.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith(MARKER):
            continue
        fields = stripped.split(",")
        # A line whose GUID is one of ours is a previous generation.
        if fields[0] in ours:
            continue
        # ...and so is one bearing our *name* under a GUID we cannot
        # recompute. Mirrored identities hash to a GUID that depends on
        # which controller was plugged in at the time, so a line for a pad
        # that has since been unplugged is not findable by GUID at all --
        # and it would still match if that controller came back. The name
        # is ours by construction, which makes it the reliable half of
        # this test rather than the fallback it began as.
        if len(fields) > 1 and fields[1].startswith(VIRTUAL_PREFIX):
            continue
        kept.append(line)

    body = kept + [MARKER + " -- regenerated on every controller assignment"]
    for player in sorted(lines):
        note = (notes or {}).get(player)
        if note:
            # Every comment padmap writes starts with MARKER, because the read
            # above keeps any line it does not recognise -- a comment spelled
            # differently would survive every rewrite and accumulate.
            body.append(f"{MARKER}: player {player} -- {note}")
        body.append(lines[player])
    target.write_text("\n".join(body) + "\n")
    return target


# -- a controller that has never been through the wizard ---------------------
#
# The mapping wizard lives inside the front-end, so a front-end that cannot be
# navigated is a wizard that cannot be reached. Mirroring the source pad's
# identity (virtual.identity_for, the default) is most of the answer: SDL's own
# database then matches the virtual pad exactly as it would the real
# controller. It is not all of it.
#
# A pad SDL has never heard of -- the N64 adapter measured here is one -- gets
# a blind default instead: face buttons on b0-b3 and, fatally, the d-pad on
# b12-b15. A pad whose d-pad is a hat, which is most of them, then has no
# d-pad at all, leaving only the analogue stick, which on an uncalibrated
# controller can rest far enough over to scroll the menus by itself.
#
# So padmap writes a line for every republished pad, captured or not. Under
# padmap's own identity that line is the *only* thing between the user and a
# controller with no buttons, since SDL knows nothing about 1209:0001. Two
# sources, in order of how much they are worth trusting.

# SDL button number -> field, for a pad we know nothing about. Deliberately
# the order SDL's own default assumes, so a guess is never worse than the one
# the front-end would have made unaided.
GUESS_BUTTON_ORDER = (
    "a", "b", "x", "y",
    "leftshoulder", "rightshoulder", "lefttrigger", "righttrigger",
    "back", "start", "leftstick", "rightstick",
)

# BTN_DPAD_UP..BTN_DPAD_RIGHT, for pads that report directions as keys.
DPAD_KEYS = {0x220: "dpup", 0x221: "dpdown", 0x222: "dpleft", 0x223: "dpright"}

# Spelled out rather than imported from evdev, matching capture.py.
EV_KEY = 0x01
EV_ABS = 0x03
ABS_HAT0X = 0x10
ABS_HAT0Y = 0x11

# Fields that describe the *identity* of a line rather than a binding, and so
# must not be carried from one device to another.
_IDENTITY_FIELDS = frozenset({"platform", "crc", "hint", "sdk", "type"})


def _stick_and_dpad_fields(
    axis_codes: list[int], keys: list[int],
    axes: dict[int, mapping.AxisSpan] | None = None,
) -> dict[str, str]:
    """Directions, from what the pad actually reports rather than a guess.

    These are the ones that decide whether a front-end can be navigated at
    all, and they are also the ones that need no guessing: a hat is a hat and
    ABS_X is the left stick's X on every pad ever made.
    """
    fields: dict[str, str] = {}
    if ABS_HAT0X in axis_codes and ABS_HAT0Y in axis_codes:
        # Hats are numbered separately from axes; hat 0 is the only one any
        # measured pad has.
        fields.update(dpup="h0.1", dpright="h0.2", dpdown="h0.4", dpleft="h0.8")
    else:
        # No hat: some pads report the d-pad as four ordinary keys instead.
        for code, field in DPAD_KEYS.items():
            index = mapping.sdl_button_index(keys, code)
            if index is not None:
                fields[field] = f"b{index}"
    fields.update(mapping.stick_fields(axis_codes, axes=axes))
    return fields


def guessed_fields(
    keys: list[int], axis_codes: list[int],
    axes: dict[int, mapping.AxisSpan] | None = None,
) -> dict[str, str]:
    """A mapping for a pad nothing knows anything about.

    The face buttons really are a guess -- which physical button is A is
    exactly what the wizard exists to find out, and pads disagree (SDL's own
    database has the measured Fightstick as `a:b1,x:b0`). The order used here
    is the one SDL assumes when it has nothing better, so
    this can only be as wrong as what happens today, and no more.

    Everything below the face buttons is not a guess, and is where the value
    is: the d-pad and sticks come from the pad's own capabilities.
    """
    fields: dict[str, str] = {}
    ordered = ([code for code in sorted(keys) if code >= mapping.BTN_JOYSTICK]
               + [code for code in sorted(keys) if code < mapping.BTN_JOYSTICK])
    for index, field in enumerate(GUESS_BUTTON_ORDER):
        if index < len(ordered):
            fields[field] = f"b{index}"
    fields.update(_stick_and_dpad_fields(axis_codes, keys, axes))
    return fields


def physical_guid(pad: Pad) -> str:
    """The GUID SDL computes for the physical controller behind a virtual pad.

    Deliberately the device's *raw* name, not the cleaned one: SDL checksums
    what the kernel reports, and the N64 adapter measured here prefixes its
    name with a 0x18 byte. Stripping it changes the checksum and the lookup
    silently matches nothing.
    """
    try:
        device = devices.open_device(pad)
    except OSError:
        return ""
    try:
        info = device.info
        return mapping.sdl_guid(
            bus=info.bustype, vendor=info.vendor, product=info.product,
            version=info.version, name=device.name,
        )
    except OSError:
        return ""
    finally:
        device.close()


def _database_paths() -> list[Path]:
    """Files SDL itself would read a mapping out of.

    The user's own file first: a line in there was either written by a
    Gamepad Editor or typed by hand, and either way it is a statement about
    this machine rather than a database's guess about a product line.
    """
    paths = [sdl_config_path()]
    # Where the database used to live, read but never written.
    #
    # It sat in a particular front-end's config directory until padmap stopped
    # shipping that front-end. A user who has been here since then has their
    # hand-written lines in the old file, and dropping it from the search would
    # orphan exactly the mappings this function exists to carry over -- quietly,
    # because a carried mapping that is not found is indistinguishable from a
    # controller SDL never knew about.
    legacy = Path(
        os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")
    ) / "pegasus-frontend" / "sdl_controllers.txt"
    if legacy not in paths:
        paths.append(legacy)
    from_env = os.environ.get("SDL_GAMECONTROLLERCONFIG_FILE")
    if from_env:
        paths += [Path(part) for part in from_env.split(os.pathsep) if part]
    return paths


def carried_fields(guid: str) -> tuple[dict[str, str], str] | None:
    """An existing mapping for this GUID, from wherever one can be found.

    Returns the fields and where they came from, or None. The fields transfer
    to the virtual pad *verbatim*: virtual.create clones the source's EV_KEY
    and EV_ABS sets exactly, so SDL numbers the virtual pad's buttons, axes and
    hats identically to the physical one. Only the GUID and the name differ,
    and those are replaced.
    """
    if not guid:
        return None

    for path in _database_paths():
        try:
            text = path.read_text(errors="replace")
        except FileNotFoundError:
            # The ordinary case: SDL_GAMECONTROLLERCONFIG_FILE may name files
            # that do not exist, and a machine that has never run the wizard
            # has no sdl_controllers.txt.
            continue
        except OSError as error:
            # A database that is there and unreadable is not the same thing,
            # and skipping it quietly means a user's own mapping is passed
            # over in favour of a guess with nothing said. Nothing is
            # rewritten here, so this degrades rather than destroys -- but it
            # is worth a line in the log to explain the guess.
            log.warning("cannot read %s, so a mapping in it will not be "
                        "carried over: %s", path, error)
            continue
        for line in text.splitlines():
            parsed = mapping.parse_sdl_line(line)
            if parsed is None or parsed[0] != guid:
                continue
            # Skip padmap's own output: a line we wrote for a virtual pad
            # cannot also be the physical pad's, and matching one would let a
            # stale generation feed itself back in.
            if parsed[1].startswith(VIRTUAL_PREFIX):
                continue
            return _binding_fields(parsed[2]), str(path)

    builtin = sdl_builtin_fields(guid)
    if builtin:
        return builtin, "SDL's built-in database"
    return None


def _binding_fields(fields: dict[str, str]) -> dict[str, str]:
    return {
        field: target for field, target in fields.items()
        if field not in _IDENTITY_FIELDS
    }


# Asks SDL for a mapping instead of hunting for a copy of its database.
#
# SDL's built-in database is compiled into the library as a C array, so there
# is no file to read; the library itself is the only place it exists. The
# lookup is `padmap-rs sdl-mapping`, a process of its own, so the daemon never
# loads SDL, never holds its threads, and cannot be taken down by it. That
# binary sets the hints that keep SDL from opening any controller -- verified:
# `joysticks 0`, and the Fightstick's mapping still returned.
_builtin_cache: dict[str, dict[str, str]] = {}


def sdl_builtin_fields(guid: str) -> dict[str, str]:
    """What SDL's own database says about a GUID, or {}.

    Never raises: SDL may be absent, and a controller with no entry is the
    ordinary case rather than a failure.
    """
    if guid in _builtin_cache:
        return _builtin_cache[guid]

    binary = emulators.binary()
    if binary is None:
        log.debug("no padmap-rs; SDL's built-in database is not consulted")
        _builtin_cache[guid] = {}
        return {}
    try:
        result = subprocess.run(
            [binary, "sdl-mapping", guid],
            capture_output=True, text=True, timeout=15, check=False,
        )
    except (OSError, subprocess.SubprocessError) as error:
        log.debug("SDL mapping probe failed: %s", error)
        _builtin_cache[guid] = {}
        return {}
    if result.returncode != 0:
        log.debug("SDL mapping probe failed: %s", result.stderr.strip())
        _builtin_cache[guid] = {}
        return {}

    parsed = mapping.parse_sdl_line(result.stdout.strip())
    if parsed is not None and parsed[1].startswith(VIRTUAL_PREFIX):
        # Belt and braces: whatever the source, a line naming one of our own
        # pads describes a previous generation rather than a controller.
        parsed = None
    fields = _binding_fields(parsed[2]) if parsed else {}
    _builtin_cache[guid] = fields
    return fields


def fallback_line_for(
    player: int, pad: Pad, keys: list[int], axis_codes: list[int],
    axes: dict[int, mapping.AxisSpan] | None = None,
) -> tuple[str, str] | None:
    """A usable SDL line for a pad that has never been mapped, plus its source.

    None when the pad reports no buttons at all, which is not a controller
    anything could navigate with.
    """
    if not keys:
        return None

    carried = carried_fields(physical_guid(pad))
    if carried is not None:
        fields, source = carried
        source = f"carried over from {source}"
    else:
        fields = guessed_fields(keys, axis_codes, axes)
        source = "guessed from the controller's own capabilities"
    return (
        mapping.sdl_line(virtual_guid(player, pad), virtual_name(player),
                         fields),
        source + "; run the mapping wizard to replace it",
    )


def pad_axis_spans(pad: Pad) -> dict[int, mapping.AxisSpan]:
    """(minimum, maximum, rest) per ABS code, straight from the driver.

    Read when the SDL line is written rather than remembered, for the same
    reason pad_capabilities is: the answer belongs to whatever hardware is
    plugged in right now. Empty on any failure, which puts stick_fields back
    on its old guess rather than dropping sticks a pad really has.
    """
    try:
        device = devices.open_device(pad)
    except OSError:
        return {}
    try:
        caps: dict[int, Any] = device.capabilities()
        spans: dict[int, mapping.AxisSpan] = {}
        for entry in (caps.get(EV_ABS) or []):
            if not isinstance(entry, tuple) or len(entry) != 2:
                continue
            code, info = entry
            minimum = int(getattr(info, "min", 0))
            maximum = int(getattr(info, "max", 0))
            rest = int(getattr(info, "value", 0))
            if not minimum <= rest <= maximum:
                rest = (minimum + maximum) // 2
            spans[int(code)] = (minimum, maximum, rest)
        return spans
    except OSError:
        return {}
    finally:
        device.close()


def pad_capabilities(pad: Pad) -> tuple[list[int], list[int]]:
    """(key codes, ABS codes) for a pad, as SDL and RetroArch will see them.

    The virtual pad is a clone, so these describe both.
    """
    try:
        device = devices.open_device(pad)
    except OSError:
        return [], []
    try:
        caps: dict[int, Any] = device.capabilities()
        keys = sorted(int(code) for code in (caps.get(EV_KEY) or []))
        # evdev reports EV_ABS as (code, AbsInfo) pairs and EV_KEY as bare
        # codes. Guard rather than trust: a stub device can report either.
        axes = sorted(
            int(entry[0])
            for entry in (caps.get(EV_ABS) or [])
            if isinstance(entry, tuple) and len(entry) == 2
        )
        return keys, axes
    except OSError:
        return [], []
    finally:
        device.close()


def retroarch_profile(
    player: int, pad: Pad, bindings: dict[str, Binding], source: str = "",
    layout: str = "", scope: str = "", context: str = "",
) -> str:
    """An autoconfig profile built from what the user pressed.

    Deliberately *not* a copy of the physical pad's upstream libretro profile.
    That copy is right whenever the pad is in libretro's database and silently
    empty when it is not, and it can disagree with what the user mapped in the
    front-end -- two sets of bindings for one controller, differing in ways
    nobody is told about. A capture the user performed wins over a database
    entry they never saw.

    `layout` is the layout *id* the capture was taken under, resolved here
    rather than baked into the stored bindings, so that a later correction to
    a console's key table reaches profiles captured before it.

    `scope` and `context` are recorded for the reader only, and are worth the
    two lines: this file is regenerated at launch from whichever of the
    controller's mappings won, and "why is player 1 bound like this" is
    otherwise unanswerable from the artefact. A profile that names the scope
    it came from and the game it was resolved for can be diffed against what
    the user expected.
    """
    resolved = layouts.get(layout)
    identity = identity_for(pad, player=player)
    header = [
        f"# Generated by padmap for {virtual_name(player)}.",
        "# Bindings captured from the controller itself"
        + (f", replacing {source}." if source else "."),
        # Named in the file because a console-specific key is invisible once
        # emitted -- input_y_btn on an N64 pad looks like a mistake until you
        # know which layout asked for it.
        f"# Layout: {resolved.label} ({resolved.id}).",
        f"# Mapping scope: {scope or 'default (any game)'}"
        + (f"; resolved for {context}." if context else "."),
    ]
    lines = [
        'input_driver = "udev"',
        f'input_device = "{virtual_name(player)}"',
        f'input_device_display_name = "{virtual_name(player)}"',
        # The ids the virtual pad actually advertises, not padmap's own:
        # by default it mirrors the source controller, and a profile claiming
        # different ids scores against itself in RetroArch's autoconfig match.
        f'input_vendor_id = "{identity.vendor}"',
        f'input_product_id = "{identity.product}"',
    ]
    lines += mapping.retroarch_lines(bindings, resolved.retroarch_keys())

    # Sticks come from calibration, not from the button capture, and are the
    # same on every pad padmap republishes.
    lines += [
        'input_l_x_plus_axis = "+0"',
        'input_l_x_minus_axis = "-0"',
        'input_l_y_plus_axis = "+1"',
        'input_l_y_minus_axis = "-1"',
    ]
    return "\n".join(header + lines) + "\n"


def resolved_mapping(
    pad: Pad, console: str = "", game: str = ""
) -> tuple[str, profiles.Mapping]:
    """The capture that applies to this controller, and the scope it came from.

    Most specific wins: this game, then this console, then the controller's
    default. `console` is a layout id and `game` a `profiles.game_key`; both
    empty is the no-context case, which resolves to the default and is what
    everything writing files ahead of a launch has to use.
    """
    profile = profiles.load(pad)
    if profile is None:
        return profiles.SCOPE_UNIVERSAL, profiles.Mapping()
    return profile.resolve(console, game)


def stored_bindings(
    pad: Pad, console: str = "", game: str = ""
) -> dict[str, Binding]:
    """What was captured for this controller, if anything."""
    return dict(resolved_mapping(pad, console, game)[1].buttons)


def stored_layout(pad: Pad, console: str = "", game: str = "") -> str:
    """Which layout those bindings were captured under, as an id.

    Empty for a pad that has never been mapped, and for profiles written
    before layouts were recorded -- both of which resolve to the generic
    layout and so emit the canonical keys, exactly as they used to.

    Deliberately the layout the resolved *capture* was taken under, not the
    console asked about. They usually coincide, and where they do not the
    capture is right: a mapping recorded under the N64 layout knows about
    C-buttons and a Z trigger, and re-interpreting it against some other
    console's key table would emit keys for controls nobody pressed.
    """
    return resolved_mapping(pad, console, game)[1].layout


def has_mapping(pad: Pad) -> bool:
    """Whether this controller has been through the mapping wizard.

    A profile can exist with no buttons -- calibration writes one, and so does
    finishing a session -- so the presence of a profile is not the question.

    Any scope counts. A pad mapped only for N64 games has demonstrably been
    through the wizard, and reporting it unconfigured would offer the wizard
    again every session.
    """
    profile = profiles.load(pad)
    return profile is not None and profile.has_bindings()


def store_mapping(pad: "Pad", layout_id: str, bindings: dict,
                  scope: str = "") -> list[str]:
    """Keep a capture against the *controller*, under one scope.

    Shared by the daemon and by `padmap map`, because two implementations of
    "what a finished capture means" is exactly the shape of thing that drifts
    -- one of them would keep the icon rule and the other would not, and
    nothing would say which.

    Everything else on the profile is carried over rather than rebuilt:
    recording an N64 mapping is not a reason to forget the calibration, the
    icon, or the mapping for every other console.

    Returns the controls the layout asked for and did not get. A skipped
    control is stored as an absence, and an absence emits no RetroArch key at
    all -- so the pad is reported as mapped, nothing is said, and the control
    is simply dead in game. That is indistinguishable from a control the
    wizard never offered; naming them is what tells the two apart.
    """
    from . import icons

    existing = profiles.load(pad)
    # Only layout ids that are also icon names, which is all of them bar
    # "generic": the icon is a filename in the theme, and generic.svg does not
    # exist, so storing it would leave the pad with no picture at all rather
    # than the fallback one.
    #
    # And only from a capture with no scope. A GameCube controller mapped *for
    # N64 games* is captured against the N64 layout, and taking the icon from
    # it would relabel the pad as an N64 controller -- which it is not, and
    # which is the picture the user then sees forever after.
    icon = existing.icon if existing else ""
    if not icon and not scope and layout_id in icons.ICON_NAMES:
        icon = layout_id
    name = "".join(ch for ch in pad.name if ch.isprintable()).strip()
    profile = profiles.Profile(
        signature=profiles.signature(pad),
        name=name,
        icon=icon,
        axes=existing.axes if existing else {},
        mappings=dict(existing.mappings) if existing else {},
    )
    # The console this capture is for travels with it. Emission needs it to
    # pick the RetroArch keys the core actually reads; without it every pad
    # gets the gamepad table and the console-specific buttons are bound to
    # controls their core never looks at.
    profile.record(scope, profiles.Mapping(
        buttons=dict(bindings), layout=layout_id))
    profiles.save(profile)
    return [
        control.canonical
        for control in layouts.get(layout_id).controls
        if control.canonical not in bindings
    ]
