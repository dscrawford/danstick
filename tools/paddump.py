#!/usr/bin/env python3
"""Dump identity fields for every connected joypad and flag ambiguity.

Answers three questions about telling the pads on this machine apart:

  1. Is phys+uniq actually unique across them?
  2. In what order does udev enumerate them?
  3. Does SDL's enumeration agree with that order?

Stdlib only, so it runs without entering the nix shell. SDL comparison is
skipped unless pysdl2 or pygame happens to be importable.
"""

import glob
import os
import subprocess
import sys
from collections import defaultdict

HEX = lambda s: int(s, 16) if s else 0


def read(path):
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return ""


def udev_props(devnode):
    """Properties as udev sees them - what joystick enumeration filters on."""
    try:
        out = subprocess.run(
            ["udevadm", "info", "-q", "property", "-n", devnode],
            capture_output=True, text=True, timeout=5,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return {}
    props = {}
    for line in out.splitlines():
        if "=" in line:
            k, _, v = line.partition("=")
            props[k] = v
    return props


def collect():
    """Every input device carrying ID_INPUT_JOYSTICK=1, as udev enumerates them."""
    pads = []
    for input_dir in glob.glob("/sys/class/input/input*"):
        event_names = [
            os.path.basename(p) for p in glob.glob(os.path.join(input_dir, "event*"))
        ]
        if not event_names:
            continue
        event = event_names[0]
        devnode = f"/dev/input/{event}"
        if not os.path.exists(devnode):
            continue

        props = udev_props(devnode)
        js_nodes = [os.path.basename(p) for p in glob.glob(os.path.join(input_dir, "js*"))]
        # Prefer udev's own verdict; fall back to joydev's presence if udevadm
        # is unavailable (e.g. running inside a stripped container).
        is_pad = props.get("ID_INPUT_JOYSTICK") == "1" if props else bool(js_nodes)
        if not is_pad:
            continue

        phys = read(os.path.join(input_dir, "phys"))
        uniq = read(os.path.join(input_dir, "uniq"))
        pads.append({
            "syspath": os.path.realpath(os.path.join(input_dir, event)),
            "event": event,
            "js": js_nodes[0] if js_nodes else "-",
            "devnode": devnode,
            "name": read(os.path.join(input_dir, "name")),
            "phys": phys,
            "uniq": uniq,
            # phys and uniq with no separator: one string naming the unit.
            "id": phys + uniq,
            "vid": read(os.path.join(input_dir, "id/vendor")),
            "pid": read(os.path.join(input_dir, "id/product")),
        })

    # libudev returns enumerate results sorted by syspath. Note this is a
    # string sort: event10 sorts before event9.
    pads.sort(key=lambda p: p["syspath"])
    for i, p in enumerate(pads):
        p["predicted_index"] = i
    return pads


def sdl_order():
    """SDL's enumeration, for comparison against the udev order."""
    try:
        import sdl2
        sdl2.SDL_Init(sdl2.SDL_INIT_JOYSTICK)
        out = []
        for i in range(sdl2.SDL_NumJoysticks()):
            nm = sdl2.SDL_JoystickNameForIndex(i)
            out.append(nm.decode() if nm else "?")
        sdl2.SDL_Quit()
        return out
    except Exception:
        pass
    try:
        os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
        import pygame
        pygame.init()
        pygame.joystick.init()
        out = [pygame.joystick.Joystick(i).get_name()
               for i in range(pygame.joystick.get_count())]
        pygame.quit()
        return out
    except Exception:
        return None


def collisions(pads, field, label):
    groups = defaultdict(list)
    for p in pads:
        groups[p[field]].append(p)
    bad = {k: v for k, v in groups.items() if len(v) > 1 or not k}
    if not bad:
        print(f"  OK    {label}: unique across all {len(pads)} pads")
        return True
    for key, members in bad.items():
        nodes = ", ".join(m["event"] for m in members)
        if not key:
            print(f"  EMPTY {label}: not reported by {nodes}")
        else:
            print(f"  CLASH {label}: {nodes} all report {key!r}")
    return False


def main():
    pads = collect()
    if not pads:
        print("No joypads found (nothing with ID_INPUT_JOYSTICK=1).")
        print("Plug a controller in and re-run.")
        return 1

    print(f"=== {len(pads)} pad(s) ===\n")
    for p in pads:
        print(f"[{p['predicted_index']}] {p['name']}")
        print(f"      node    {p['devnode']}  ({p['js']})")
        print(f"      vid:pid {p['vid']}:{p['pid']}")
        print(f"      phys    {p['phys'] or '(empty)'}")
        print(f"      uniq    {p['uniq'] or '(empty)'}")
        print(f"      id      {p['id'] or '(EMPTY - unusable as an identifier)'}")
        print(f"      syspath {p['syspath']}")
        print()

    print("=== uniqueness ===")
    ok_phys = collisions(pads, "phys", "phys alone      ")
    ok_id = collisions(pads, "id", "phys+uniq (id)   ")
    collisions(pads, "name", "name            ")
    print()

    print("=== udev pad order ===")
    print("(libudev sorts by syspath)")
    for p in pads:
        print(f"  index {p['predicted_index']} -> {p['name']}  [{p['event']}]")
    print()

    sdl = sdl_order()
    if sdl is None:
        print("=== SDL order: skipped (no pysdl2/pygame) ===")
    else:
        print("=== SDL enumeration order ===")
        for i, nm in enumerate(sdl):
            print(f"  index {i} -> {nm}")
        udev_names = [p["name"] for p in pads]
        print()
        print("  MATCH" if sdl == udev_names else "  DIFFER",
              "- SDL order vs predicted udev order")
    print()

    if not ok_id:
        print("phys+uniq is NOT a usable key on this hardware; tell these pads")
        print("apart by vid:pid+name, or by the phys danstick's clones carry.")
    elif not ok_phys:
        print("phys alone collides but phys+uniq does not: compare the full")
        print("concatenated string, not a prefix.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
