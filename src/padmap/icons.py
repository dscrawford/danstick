"""Pick an icon for a controller.

Capability sniffing does not work for this. Measured on the hardware here, a
MAYFLASH Arcade Fightstick F300 and an N64 adapter expose an identical axis
set -- ABS_X, ABS_Y, ABS_Z, ABS_RZ, ABS_HAT0X, ABS_HAT0Y -- because both are
cheap HID adapters emitting the same generic descriptor. Button counts differ
only 13 vs 16. Nothing in the capability bits says "this is a joystick" or
"this is an N64 pad".

So matching is on vid:pid and name, with two consequences worth knowing:

  * Vendor 0x0079 is DragonRise, resold in a great many unrelated adapters.
    `0079:1879` is an N64 adapter *here*, but the same ID ships on generic
    pads elsewhere. Built-in rules for such IDs are a guess.
  * Therefore the user gets the last word, via a config file. An icon being
    wrong is a cosmetic problem with an obvious fix, not something to hide
    behind cleverer heuristics.
"""

from __future__ import annotations

import json
import os
import re
from pathlib import Path

from .devices import Pad

# Icon names, each backed by <name>.svg in the theme.
ARCADE = "arcade"
N64 = "n64"
GAMECUBE = "gamecube"
SNES = "snes"
SWITCH = "switch"
GENESIS = "genesis"
PLAYSTATION = "playstation"
XBOX = "xbox"
WHEEL = "wheel"
GAMEPAD = "gamepad"  # the fallback

ICON_NAMES = (
    ARCADE, N64, GAMECUBE, SNES, SWITCH, GENESIS, PLAYSTATION, XBOX, WHEEL,
    GAMEPAD,
)

# Fallback only, for a pad that has not been configured yet -- a learned
# profile always wins (see for_pad). Kept small and treated as a guess: these
# IDs are not reliable identity. 0x0079 in particular is DragonRise, resold in
# many unrelated adapters, so any entry under it is wrong for somebody.
_BY_ID: dict[tuple[int, int], str] = {
    (0x0079, 0x1830): ARCADE,     # MAYFLASH Arcade Fightstick F300
    (0x0079, 0x1843): GAMECUBE,   # Mayflash GameCube adapter
    (0x057E, 0x0337): GAMECUBE,   # Nintendo official GC adapter
    (0x054C, 0x0268): PLAYSTATION,  # DualShock 3
    (0x054C, 0x05C4): PLAYSTATION,  # DualShock 4
    (0x054C, 0x09CC): PLAYSTATION,  # DualShock 4 v2
    (0x054C, 0x0CE6): PLAYSTATION,  # DualSense
    (0x045E, 0x028E): XBOX,       # Xbox 360 pad
    (0x045E, 0x02FD): XBOX,       # Xbox One S pad
    (0x057E, 0x2009): SWITCH,     # Switch Pro
    # Nintendo's reissued pads for Switch Online really are those controllers,
    # button for button, so they get the layout of the console they came from
    # rather than the Switch one. A user handed the Switch wizard for an N64
    # pad would be asked to press an X and a Y it does not have.
    (0x057E, 0x2017): SNES,       # SNES pad for Switch Online
    (0x057E, 0x2019): N64,        # N64 pad for Switch Online
}

# Ordered: first match wins. Case-insensitive.
_BY_NAME: tuple[tuple[str, str], ...] = (
    (r"fight ?stick|arcade|joystick|street ?fighter|qanba|hori.*stick", ARCADE),
    (r"\bn64\b|nintendo 64|retrolink.*64", N64),
    (r"gamecube|\bgc\b|wii ?u? ?gc", GAMECUBE),
    (r"\bsnes\b|super nintendo|\bsfc\b", SNES),
    # After the SNES and N64 entries above on purpose: Nintendo's Switch Online
    # reissues carry both words ("Nintendo Co., Ltd. N64 Controller"), and the
    # console they copy is the more useful answer than the console they plug
    # into. Only pads that are *only* Switch pads should land here.
    (r"pro controller|switch pro|joy-?con|\bnso\b", SWITCH),
    (r"genesis|mega ?drive|\bm30\b|retro-?bit|saturn", GENESIS),
    (r"dualshock|dualsense|playstation|\bps[3-5]\b", PLAYSTATION),
    (r"xbox|xinput", XBOX),
    (r"wheel|racing|g29|g27|driving", WHEEL),
)


def _config_path() -> Path:
    base = os.environ.get("XDG_CONFIG_HOME") or str(Path.home() / ".config")
    return Path(base) / "padmap" / "icons.json"


def load_overrides(path: Path | None = None) -> dict[str, str]:
    """User icon overrides: {"0079:1879": "n64"}.

    Generic adapter IDs are genuinely ambiguous, so this is the documented
    fix rather than a workaround.
    """
    target = path or _config_path()
    try:
        raw = json.loads(target.read_text())
    except (OSError, ValueError):
        return {}
    if not isinstance(raw, dict):
        return {}
    return {
        str(key).lower(): str(value)
        for key, value in raw.items()
        if str(value) in ICON_NAMES
    }


def for_pad(pad: Pad, overrides: dict[str, str] | None = None) -> str:
    """Icon name for a pad. Always returns something renderable.

    Order of authority, most trusted first:

      1. the learned per-device profile, set when the pad was first configured
      2. the icons.json override file
      3. the device name
      4. the built-in vid/pid table
      5. a generic pad

    The profile wins because it is the only source that reflects the actual
    controller in the user's hands rather than a guess about a product line.
    Everything below it exists to give a sensible default *before* the pad has
    been through setup once.
    """
    from .profiles import load as load_profile

    profile = load_profile(pad)
    if profile is not None and profile.icon:
        return profile.icon

    overrides = overrides if overrides is not None else load_overrides()

    key = f"{pad.vid:04x}:{pad.pid:04x}"
    if key in overrides:
        return overrides[key]

    # Name patterns come before the built-in ID table: a device that says
    # "Fightstick" in its name is better evidence than a resold vendor ID.
    name = pad.name.lower()
    for pattern, icon in _BY_NAME:
        if re.search(pattern, name):
            return icon

    by_id = _BY_ID.get((pad.vid, pad.pid))
    if by_id is not None:
        return by_id

    return GAMEPAD
