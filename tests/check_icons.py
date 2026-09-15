"""Do the icon name, the artwork and the picker all agree?

    python3 tests/check_icons.py

`icons.ICON_NAMES` and the SVG files under assets/icons are two lists of the
same thing, and nothing makes them agree: an icon with no artwork draws as a
blank square in whatever renders it, and artwork no name refers to is dead
weight nobody notices.

There used to be a third copy, `iconChoices` in the front-end theme, and it had
already fallen two behind -- "switch" and "genesis" shipped artwork and could
not be chosen. The theme is gone; the lesson is why this file exists.

Also pins the two identification rules that are easy to get subtly wrong, both
of which were found by a user asking why their Steam Controller said Xbox.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import icons  # noqa: E402

ICON_DIR = REPO / "assets" / "icons"


class _Pad:
    """Only the three fields `icons.for_pad` reads off a device."""

    def __init__(self, name: str, vid: int = 0x1234, pid: int = 0x5678) -> None:
        self.name, self.vid, self.pid = name, vid, pid


def main() -> int:
    failures = 0
    names = set(icons.ICON_NAMES)

    print("every icon name has artwork:")
    shipped = {path.stem for path in ICON_DIR.glob("*.svg")}
    missing = sorted(names - shipped)
    if missing:
        failures += 1
        print(f"  FAIL: no SVG for {missing} -- they draw as a blank square")
    else:
        print(f"  ok  {len(names)} names, {len(shipped)} files")
    extra = sorted(shipped - names)
    if extra:
        failures += 1
        print(f"  FAIL: {extra} ship artwork no name refers to")

    print("\nthe kernel's own name for an Xbox pad is recognised:")
    # xpad calls every one of them "Microsoft X-Box 360 pad" -- with the
    # hyphen. A pattern of "xbox" alone matches none of them, which is the most
    # common controller on Linux falling through to the generic icon.
    for name, want in (
        ("Microsoft X-Box 360 pad 0", icons.XBOX),
        ("Microsoft X-Box One pad", icons.XBOX),
        ("Xbox Wireless Controller", icons.XBOX),
        ("xinput device", icons.XBOX),
    ):
        got = icons.for_pad(_Pad(name), overrides={})
        if got != want:
            failures += 1
            print(f"  FAIL: {name!r} -> {got!r}, wanted {want!r}")
        else:
            print(f"  ok  {name!r} -> {got!r}")

    print("\nSteam's virtual pad is not believed when it says it is an Xbox:")
    # The one name that is deliberately somebody else's. Steam publishes a
    # uinput pad on Valve's vendor id called "Microsoft X-Box 360 pad" so that
    # games treat it as XInput, so the usual rule -- a name beats a vendor id
    # -- is exactly backwards for this one device.
    vid, pid = icons.STEAM_VIRTUAL_ID
    got = icons.for_pad(_Pad("Microsoft X-Box 360 pad 0", vid, pid), overrides={})
    if got != icons.STEAM:
        failures += 1
        print(f"  FAIL: Steam's virtual pad -> {got!r}, wanted {icons.STEAM!r}")
    else:
        print(f"  ok  {vid:04x}:{pid:04x} named as an Xbox -> {got!r}")

    # ...and a real Xbox pad is still an Xbox pad.
    got = icons.for_pad(_Pad("Microsoft X-Box 360 pad 0", 0x045E, 0x028E), overrides={})
    if got != icons.XBOX:
        failures += 1
        print(f"  FAIL: a genuine 360 pad -> {got!r}, wanted {icons.XBOX!r}")
    else:
        print(f"  ok  045e:028e named as an Xbox -> {got!r}")

    print("\nValve's own controllers are recognised by name:")
    for name in ("Steam Controller", "Steam Deck",
                 "Valve Software Steam Controller Puck"):
        got = icons.for_pad(_Pad(name), overrides={})
        if got != icons.STEAM:
            failures += 1
            print(f"  FAIL: {name!r} -> {got!r}")
        else:
            print(f"  ok  {name!r} -> {got!r}")

    print("\na more specific rule still wins over a vaguer one:")
    # The name table is ordered and first match wins; an arcade stick that
    # happens to mention Steam must still be an arcade stick.
    for name, want in (
        ("Steampunk Arcade Fightstick", icons.ARCADE),
        ("Nintendo Co., Ltd. N64 Controller", icons.N64),
    ):
        got = icons.for_pad(_Pad(name), overrides={})
        if got != want:
            failures += 1
            print(f"  FAIL: {name!r} -> {got!r}, wanted {want!r}")
        else:
            print(f"  ok  {name!r} -> {got!r}")

    if failures:
        print(f"\n{failures} check(s) failed")
        return 1
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
