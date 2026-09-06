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
import os
import re
from dataclasses import dataclass, field
from pathlib import Path

from . import mapping
from .devices import Pad

ENV_DIR = "PADMAP_PROFILE_DIR"


def profile_dir() -> Path:
    override = os.environ.get(ENV_DIR)
    if override:
        return Path(override)
    base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(base) / "padmap" / "devices"


def signature(pad: Pad) -> str:
    """Stable identity for a *model* of controller, across replugs.

    Not `Pad.stable_key()` -- that deliberately does not exist, because it
    cannot distinguish two identical pads on one adapter. This is coarser on
    purpose: two identical controllers should share a profile.
    """
    name = "".join(ch for ch in pad.name if ch.isprintable()).strip()
    return f"{pad.vid:04x}:{pad.pid:04x}:{name}"


def _filename(sig: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "_", sig)[:120] + ".json"


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
class Profile:
    signature: str
    name: str = ""
    icon: str = ""
    # evdev ABS code -> calibration
    axes: dict[int, AxisCalibration] = field(default_factory=dict)
    # Canonical control name -> where it lives on this pad. Keyed by the
    # *physical* controller's signature, so a mapping follows the controller
    # rather than the player slot it happened to claim -- which is how SDL's
    # own database loses it, since that is keyed on "padmap Player N".
    buttons: dict[str, "mapping.Binding"] = field(default_factory=dict)
    # Which layout the buttons above were captured with, as an id.
    #
    # Emission needs it: cores wire the abstract RetroPad onto real console
    # buttons themselves and not identically, so the RetroArch key a control
    # belongs under depends on the console, and the bindings alone cannot say
    # which console that was. The *id* is stored rather than the resolved
    # keys so that correcting a layout fixes every profile already captured
    # under it -- a wrong key is a button that silently does nothing, which
    # is not something a user will think to re-capture for.
    layout: str = ""

    def to_json(self) -> dict:
        return {
            "signature": self.signature,
            "name": self.name,
            "icon": self.icon,
            "layout": self.layout,
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
            "buttons": {
                control: binding.to_json()
                for control, binding in self.buttons.items()
            },
        }

    @classmethod
    def from_json(cls, raw: dict) -> "Profile":
        axes: dict[int, AxisCalibration] = {}
        for code, values in (raw.get("axes") or {}).items():
            try:
                reach_min = values.get("reach_min")
                reach_max = values.get("reach_max")
                axes[int(code)] = AxisCalibration(
                    center=int(values["center"]),
                    minimum=int(values["min"]),
                    maximum=int(values["max"]),
                    flat=int(values.get("flat", 0)),
                    reach_min=None if reach_min is None else int(reach_min),
                    reach_max=None if reach_max is None else int(reach_max),
                )
            except (KeyError, TypeError, ValueError):
                continue
        buttons: dict[str, mapping.Binding] = {}
        for control, values in (raw.get("buttons") or {}).items():
            if not isinstance(values, dict):
                continue
            try:
                buttons[str(control)] = mapping.Binding.from_json(values)
            except (TypeError, ValueError):
                continue

        return cls(
            signature=str(raw.get("signature", "")),
            name=str(raw.get("name", "")),
            icon=str(raw.get("icon", "")),
            # Absent in profiles written before layouts were recorded. Empty
            # resolves to the generic layout, whose override set is empty, so
            # such a profile emits exactly what it emitted before.
            layout=str(raw.get("layout", "")),
            axes=axes,
            buttons=buttons,
        )


def load(pad: Pad, directory: Path | None = None) -> Profile | None:
    target = (directory or profile_dir()) / _filename(signature(pad))
    try:
        return Profile.from_json(json.loads(target.read_text()))
    except (OSError, ValueError):
        return None


def save(profile: Profile, directory: Path | None = None) -> Path:
    base = directory or profile_dir()
    base.mkdir(parents=True, exist_ok=True)
    path = base / _filename(profile.signature)
    path.write_text(json.dumps(profile.to_json(), indent=2))
    return path


def is_known(pad: Pad, directory: Path | None = None) -> bool:
    """Has this model of controller been set up before?

    Drives the "first time I've seen this pad" prompt, so calibration happens
    once rather than every session.
    """
    return load(pad, directory) is not None
