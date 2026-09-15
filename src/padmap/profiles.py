"""Per-device profiles, learned rather than hardcoded.

Everything device-specific padmap needs -- which icon to show, where a stick
actually rests -- is measured once when a controller is first seen and stored
under the user's data dir. Nothing here ships a table of known vendor IDs.

That matters beyond tidiness. The hardware measured for this project shows
why a built-in table cannot work:

  * Vendor 0x0079 (DragonRise) is resold in a great many unrelated adapters,
    so `0079:1879` is an N64 adapter on one machine and a generic pad on the
    next. Any shipped default is a guess that is wrong for somebody.
  * A worn N64 stick rests at 174/185 on a 0-255 axis whose nominal centre is
    128 -- 36-45% deflection. That is a property of one physical controller,
    not of a product line, so no database could contain it.

Both are things the person holding the controller can settle in a second.

The signature deliberately includes the device name as well as vid:pid, since
several distinct adapters share an ID. It deliberately does *not* include
anything positional like the event node or phys, which change between plugs.
"""

from __future__ import annotations

import json
import logging
import os
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

from . import mapping

log = logging.getLogger("padmap.profiles")

if TYPE_CHECKING:  # pragma: no cover
    # Only for annotations. Importing it for real would pull evdev in, and
    # `capture` -- which documents itself as pure logic precisely so the
    # interesting decisions can be exercised without a device library -- now
    # needs the scope vocabulary below. Nothing here touches a device: the
    # profile store is files and strings.
    from .devices import Pad

ENV_DIR = "PADMAP_PROFILE_DIR"


# -- scopes ------------------------------------------------------------------
#
# One controller can need more than one mapping. The case that forced this: a
# GameCube controller used to play N64 games. The console decides which
# *controls exist* and which RetroArch key each one is emitted under, so
# "where is A on this pad" is not a single answer -- it is one answer per
# console, and occasionally one per game.
#
# A scope is the string a mapping is filed under. Three kinds, most specific
# first:
#
#   game:<console>/<stem>   this controller, this game
#   console:<layout id>     this controller, this console
#   ""                      this controller, everything else
#
# Deliberately a dict keyed by a flat string rather than three separate
# fields. Resolution is then "walk a list of candidate keys and take the first
# hit" -- one loop, with no precedence logic to get wrong -- and adding a
# fourth kind later costs a line in `scope_order` and nothing else.
SCOPE_UNIVERSAL = ""
_CONSOLE_PREFIX = "console:"
_GAME_PREFIX = "game:"


def console_scope(layout_id: str) -> str:
    """Scope covering every game on one console.

    Console identity is a `layouts.ALL` id. Not an accident of reuse: the
    console is exactly what decides the control set and the RetroArch key
    table, and that is what a layout already is. A separate console enum
    would be a second list to keep in step, with nothing to notice when it
    fell behind -- the same failure the theme's console list was given
    `layouts.catalogue` to avoid.
    """
    return _CONSOLE_PREFIX + layout_id


def game_scope(key: str) -> str:
    return _GAME_PREFIX + key


def scope_console(scope: str) -> str:
    """The layout id a console scope names, or "" for any other scope."""
    if scope.startswith(_CONSOLE_PREFIX):
        return scope[len(_CONSOLE_PREFIX):]
    return ""


def scope_game(scope: str) -> str:
    """The game key a game scope names, or "" for any other scope."""
    if scope.startswith(_GAME_PREFIX):
        return scope[len(_GAME_PREFIX):]
    return ""


_KEY_NOISE = re.compile(r"[^a-z0-9]+")


def game_key(console: str, rom: str) -> str:
    """A stable identity for one game, from the path a launcher was handed.

    Deliberately **not** the absolute path. The path is what the front-end
    happens to hold today; it changes when a library moves, a drive is
    remounted elsewhere, or a collection is regenerated -- and a per-game
    mapping that silently stops applying because a directory moved is worse
    than one that was never made, because nothing reports it.

    Deliberately not a content hash either. That means reading the file at
    launch, some of which are hundreds of megabytes, and a patched or
    re-dumped ROM would then be a different game to padmap while being the
    same game to the person holding the controller.

    So: the filename stem, normalised, under the console. The stem is already
    what a front-end shows as the title, which makes the key legible in the
    profile on disk, and the console prefix is what stops `sonic` on an
    arcade board sharing a mapping with `sonic` on a console.
    """
    stem = Path(rom).name
    # Only the *last* suffix: "Legend of Zelda, The (v1.2).z64" must not lose
    # everything after the first dot, and a name with no dot at all is normal
    # for a directory-shaped "ROM".
    if "." in stem:
        stem = stem.rsplit(".", 1)[0]
    slug = _KEY_NOISE.sub("-", stem.lower()).strip("-")
    if not slug:
        return ""
    return f"{console or 'unknown'}/{slug}"


def scope_order(console: str = "", game: str = "") -> list[str]:
    """Scopes to try, most specific first.

    The entire precedence rule lives here, so everything that needs to know
    which mapping applies agrees with everything else by construction rather
    than by two implementations happening to match.
    """
    order: list[str] = []
    if game:
        order.append(game_scope(game))
    if console:
        order.append(console_scope(console))
    order.append(SCOPE_UNIVERSAL)
    return order


def profile_dir() -> Path:
    override = os.environ.get(ENV_DIR)
    if override:
        return Path(override)
    base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(base) / "padmap" / "devices"


def signature(pad: "Pad") -> str:
    """Stable identity for a *model* of controller, across replugs.

    Not `Pad.stable_key()` -- that deliberately does not exist, because it
    cannot distinguish two identical pads on one adapter. This is coarser on
    purpose: two identical controllers should share a profile.
    """
    name = "".join(ch for ch in pad.name if ch.isprintable()).strip()
    return f"{pad.vid:04x}:{pad.pid:04x}:{name}"


def _filename(sig: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "_", sig)[:120] + ".json"


# The width of the field an axis value travels in. `input_event.value` is an
# `__s32`, and python-evdev packs it with `struct`, so a number outside this
# range does not become a wrong reading -- it raises OverflowError. That is
# neither an OSError nor a ValueError, so every guard between the profile
# store and the uinput write misses it, and the process ends. Measured: a
# stored profile declaring min/max of -+2**40 made `apply(150)` return
# 1099511627776, `write_event` raise, and the whole daemon exit mid-game with
# every player's controller going dead at once.
EVDEV_VALUE_MIN = -(2 ** 31)
EVDEV_VALUE_MAX = 2 ** 31 - 1


@dataclass
class AxisCalibration:
    """Where an axis rests, how far it actually travels, and its dead band.

    `minimum`/`maximum` are the *declared* range from absinfo -- the output
    scale. `reach_min`/`reach_max` are what the stick was observed to actually
    produce, and default to the declared range when unmeasured.

    Keeping those separate matters. An adapter can declare 0-255 while the
    physical stick only ever emits 160-255; scaling against the declared range
    then leaves almost no travel on one side, so the stick cannot go left at
    all. Scaling against the measured reach restores full movement in both
    directions.
    """

    center: int
    minimum: int
    maximum: int
    # Half-width of the dead band around centre, in raw units.
    flat: int = 0
    # Observed extremes. None means "never measured, assume declared range".
    reach_min: int | None = None
    reach_max: int | None = None

    @property
    def low(self) -> int:
        value = self.reach_min if self.reach_min is not None else self.minimum
        return min(value, self.center)

    @property
    def high(self) -> int:
        value = self.reach_max if self.reach_max is not None else self.maximum
        return max(value, self.center)

    def fits_evdev(self) -> bool:
        """Whether everything this calibration can emit is writable at all.

        Checked on the stored numbers rather than on each result, because the
        stored numbers bound every result: `apply` clamps into
        `minimum`..`maximum`, and the only other value that leaves here is the
        centre seed `virtual.create` writes, which is their midpoint. So one
        check when a profile is read stands in for a check on every event, and
        the hot path stays arithmetic.
        """
        bounds = [self.center, self.minimum, self.maximum, self.flat]
        if self.reach_min is not None:
            bounds.append(self.reach_min)
        if self.reach_max is not None:
            bounds.append(self.reach_max)
        return all(EVDEV_VALUE_MIN <= n <= EVDEV_VALUE_MAX for n in bounds)

    def apply(self, value: int) -> int:
        """Rescale a raw reading so `center` maps to the declared midpoint.

        Piecewise linear either side of centre, scaled by measured reach, and
        clamped so a stick that overshoots its calibration cannot exceed the
        declared range.
        """
        mid = (self.minimum + self.maximum) // 2

        if abs(value - self.center) <= self.flat:
            return mid

        # Both the offset and the span are measured from the edge of the dead
        # band. Taking the offset from there but the span from centre would
        # scale every reading down by the band width, so a fully deflected
        # stick would stop short of the declared extreme.
        if value < self.center:
            edge = self.center - self.flat
            span = edge - self.low
            if span <= 0:
                return mid
            scaled = (value - edge) / span  # -1 at full reach, 0 at the band
            out = mid + scaled * (mid - self.minimum)
        else:
            edge = self.center + self.flat
            span = self.high - edge
            if span <= 0:
                return mid
            scaled = (value - edge) / span  # 0 at the band, 1 at full reach
            out = mid + scaled * (self.maximum - mid)

        return int(round(max(self.minimum, min(self.maximum, out))))


@dataclass
class Mapping:
    """One capture: where every control of one layout lives on this pad.

    The unit a scope points at. Carrying the layout id *inside* the mapping
    rather than beside it is what makes a per-console mapping meaningful at
    all: the whole reason a GameCube pad needs a separate N64 mapping is that
    the N64 layout asks for different controls and emits different RetroArch
    keys, so a binding set is only interpretable together with the layout it
    was captured under.

    The *id* is stored rather than the resolved keys, so that correcting a
    console's key table fixes every profile already captured under it -- a
    wrong key is a button that silently does nothing, which is not something
    a user will think to re-capture for.
    """

    # Canonical control name -> where it lives on this pad.
    buttons: dict[str, "mapping.Binding"] = field(default_factory=dict)
    layout: str = ""
    # What to call this mapping in a list. Empty means "describe it from the
    # scope", which is what almost every one of them is.
    name: str = ""

    def to_json(self) -> dict:
        return {
            "layout": self.layout,
            "name": self.name,
            "buttons": {
                control: binding.to_json()
                for control, binding in self.buttons.items()
            },
        }

    @classmethod
    def from_json(cls, raw: dict) -> "Mapping":
        if not isinstance(raw, dict):
            return cls()
        buttons: dict[str, mapping.Binding] = {}
        stored = raw.get("buttons")
        for control, values in (stored if isinstance(stored, dict) else {}).items():
            if not isinstance(values, dict):
                continue
            try:
                buttons[str(control)] = mapping.Binding.from_json(values)
            except (TypeError, ValueError):
                continue
        return cls(
            buttons=buttons,
            layout=str(raw.get("layout", "")),
            name=str(raw.get("name", "")),
        )


@dataclass
class Profile:
    signature: str
    name: str = ""
    icon: str = ""
    # evdev ABS code -> calibration
    axes: dict[int, AxisCalibration] = field(default_factory=dict)
    # Scope -> capture. Keyed by the *physical* controller's signature (the
    # file this profile lives in), so a mapping follows the controller rather
    # than the player slot it happened to claim -- which is how SDL's own
    # database loses it, since that is keyed on "padmap Player N".
    #
    # SCOPE_UNIVERSAL is the one every other consumer falls back to, and is
    # the only one an SDL client ever sees: menu navigation is the same job
    # whatever is about to be played, so the SDL line is written from the
    # universal mapping alone. Per-console and per-game captures exist for
    # RetroArch's autoconfig, which is the only consumer that knows what is
    # running.
    mappings: dict[str, Mapping] = field(default_factory=dict)

    # -- the universal mapping, under the names the rest of padmap used to
    # -- reach it by. Read-only: writing goes through `record`, so the rule
    # -- about a first capture also becoming the default lives in one place.

    @property
    def buttons(self) -> dict[str, "mapping.Binding"]:
        return dict(self.mappings.get(SCOPE_UNIVERSAL, Mapping()).buttons)

    @property
    def layout(self) -> str:
        return self.mappings.get(SCOPE_UNIVERSAL, Mapping()).layout

    def has_bindings(self) -> bool:
        """Whether *any* scope has been captured.

        Not just the universal one: a pad mapped only for N64 has been
        through the wizard, and reporting it as unconfigured would offer the
        wizard again on every session.
        """
        return any(m.buttons for m in self.mappings.values())

    def resolve(self, console: str = "", game: str = "") -> tuple[str, Mapping]:
        """The mapping that applies, and the scope it came from.

        Most specific wins; an empty Mapping under SCOPE_UNIVERSAL is the
        answer when nothing has been captured, so callers never have to
        handle None. A scope holding an *empty* capture is skipped rather
        than matched -- it would otherwise shadow the more general mapping
        that does have bindings, which is the one case where "most specific
        wins" is not what anybody means.
        """
        for scope in scope_order(console, game):
            found = self.mappings.get(scope)
            if found is not None and found.buttons:
                return scope, found
        return SCOPE_UNIVERSAL, Mapping()

    def record(self, scope: str, captured: Mapping) -> None:
        """File a capture under a scope, seeding the default from the first.

        The seeding is the part worth explaining. Someone whose first act is
        "map this pad for N64 games" would otherwise end up with no universal
        mapping at all -- which means no SDL line for a front-end to
        navigate with, and nothing for any other console, so the pad they just
        configured would still be driven by a guess everywhere else. A
        capture the user performed is strictly better than a guess, so the
        first one becomes the default too. Later captures do not disturb it:
        once a default exists, saying "and for N64, this instead" must not
        silently change what every other console does.
        """
        self.mappings[scope] = captured
        if scope != SCOPE_UNIVERSAL and not self.mappings.get(
                SCOPE_UNIVERSAL, Mapping()).buttons:
            self.mappings[SCOPE_UNIVERSAL] = captured

    def to_json(self) -> dict:
        universal = self.mappings.get(SCOPE_UNIVERSAL, Mapping())
        return {
            "signature": self.signature,
            "name": self.name,
            "icon": self.icon,
            "axes": {
                str(code): {
                    "center": cal.center,
                    "min": cal.minimum,
                    "max": cal.maximum,
                    "flat": cal.flat,
                    "reach_min": cal.reach_min,
                    "reach_max": cal.reach_max,
                }
                for code, cal in self.axes.items()
            },
            "mappings": {
                scope: found.to_json()
                for scope, found in sorted(self.mappings.items())
            },
            # The universal mapping is *also* written where it has always
            # been. Nothing padmap ships reads these two keys any more, but a
            # profile store is user data that outlives any one version: a
            # rollback to a build predating scopes then still finds the
            # controller mapped, instead of finding it blank and offering the
            # wizard again. Costs a few lines of JSON.
            "layout": universal.layout,
            "buttons": {
                control: binding.to_json()
                for control, binding in universal.buttons.items()
            },
        }

    @classmethod
    def from_json(cls, raw: dict) -> "Profile":
        # A file can hold valid JSON that is not a profile at all -- `null`,
        # a list, a bare string -- and every read below assumes an object.
        # `load` catches OSError and ValueError to turn a damaged file into
        # "no profile stored", but an AttributeError from .get() on a list
        # went straight past it. is_known() calls load() for every pad during
        # discovery, so one corrupted file stopped that controller being
        # handled at all rather than presenting it as a pad nobody has set up.
        if not isinstance(raw, dict):
            raw = {}

        axes: dict[int, AxisCalibration] = {}
        stored_axes = raw.get("axes")
        for code, values in (
                stored_axes if isinstance(stored_axes, dict) else {}).items():
            if not isinstance(values, dict):
                # Junk in one axis slot costs that axis, not the profile.
                continue
            try:
                reach_min = values.get("reach_min")
                reach_max = values.get("reach_max")
                number = int(code)
                cal = AxisCalibration(
                    center=int(values["center"]),
                    minimum=int(values["min"]),
                    maximum=int(values["max"]),
                    flat=int(values.get("flat", 0)),
                    reach_min=None if reach_min is None else int(reach_min),
                    reach_max=None if reach_max is None else int(reach_max),
                )
            except (KeyError, TypeError, ValueError, OverflowError):
                # OverflowError belongs beside the rest: JSON's `1e400` parses
                # to `inf`, and `int(inf)` raises it. Without it here, a single
                # such number escaped from_json entirely -- past `load`, whose
                # except only wraps the read and the parse -- and since
                # `is_known` is `load() is not None` and discovery calls it for
                # every pad, one damaged file stopped the whole controller list
                # rather than costing one axis.
                continue
            if not cal.fits_evdev():
                # Rejected, not clamped, and the choice is deliberate.
                #
                # A range wider than an evdev value can hold is not a
                # measurement that overshot: no stick reports 2**40. It is a
                # hand-edit, or a save interrupted halfway, and the numbers
                # beside it are worth nothing either. Clamping them to +-2**31
                # keeps the daemon alive but keeps scaling every reading
                # against nonsense, so the user trades a dead daemon for a
                # stick that reads permanently slammed into a corner -- a
                # frontend acts on that immediately, which is the runaway menu
                # navigation `virtual.create`'s centre seed exists to avoid.
                # Dropping the axis falls back to forwarding it verbatim,
                # which is exactly what an uncalibrated pad already does and
                # is the one behaviour here known to work. One axis loses its
                # correction, the controller and the game survive, and the log
                # says which axis and why so it can be re-calibrated.
                log.warning(
                    "axis %s of %s has a calibration no evdev value can carry "
                    "(centre %d, range %d..%d); ignoring it, so the axis is "
                    "forwarded uncorrected -- re-run calibration to restore it",
                    code, raw.get("signature", "this profile"),
                    cal.center, cal.minimum, cal.maximum)
                continue
            axes[number] = cal

        mappings: dict[str, Mapping] = {}
        stored_mappings = raw.get("mappings")
        for scope, values in (
                stored_mappings if isinstance(stored_mappings, dict) else {}
        ).items():
            if isinstance(values, dict):
                mappings[str(scope)] = Mapping.from_json(values)

        # Migration, in place and without a version number. Every profile
        # written before scopes carries flat `buttons`/`layout` and no
        # `mappings`, and that pair is exactly the universal scope -- it was
        # the only scope there was. Reading it here rather than in a one-shot
        # upgrade pass means a profile is migrated the first time it is
        # looked at, including one restored from a backup years later, and
        # there is no separate code path that can be forgotten.
        #
        # `layout` is absent in profiles older still. Empty resolves to the
        # generic layout, whose override set is empty, so such a profile
        # emits exactly what it emitted before.
        if SCOPE_UNIVERSAL not in mappings:
            legacy = Mapping.from_json({
                "buttons": raw.get("buttons") or {},
                "layout": raw.get("layout", ""),
            })
            if legacy.buttons or legacy.layout:
                mappings[SCOPE_UNIVERSAL] = legacy

        return cls(
            signature=str(raw.get("signature", "")),
            name=str(raw.get("name", "")),
            icon=str(raw.get("icon", "")),
            axes=axes,
            mappings=mappings,
        )


def load(pad: "Pad", directory: Path | None = None) -> Profile | None:
    target = (directory or profile_dir()) / _filename(signature(pad))
    try:
        raw = json.loads(target.read_text())
    except (OSError, ValueError):
        return None
    if not isinstance(raw, dict):
        # Valid JSON, but not a profile: `null`, a list, a bare string. None
        # rather than an empty Profile, because is_known() is `load() is not
        # None` -- an empty one would mark the controller as already set up,
        # so a corrupted file would stop it ever being offered the wizard.
        # "Damaged" has to read the same as "nothing stored".
        return None
    return Profile.from_json(raw)


def save(profile: Profile, directory: Path | None = None) -> Path:
    base = directory or profile_dir()
    base.mkdir(parents=True, exist_ok=True)
    path = base / _filename(profile.signature)
    path.write_text(json.dumps(profile.to_json(), indent=2))
    return path


def forget(pad: "Pad", directory: Path | None = None) -> bool:
    """Delete a controller's stored profile. True if there was one.

    Every scope goes with it, since the file holds them all. That is the
    intent: "reset this controller" meaning "reset some of this controller"
    leaves someone re-running the wizard and still meeting old behaviour from
    a per-console mapping they had forgotten was there.
    """
    target = (directory or profile_dir()) / _filename(signature(pad))
    try:
        target.unlink()
        return True
    except OSError:
        return False


def is_known(pad: "Pad", directory: Path | None = None) -> bool:
    """Has this model of controller been set up before?

    Drives the "first time I've seen this pad" prompt, so calibration happens
    once rather than every session.
    """
    return load(pad, directory) is not None
