#!/usr/bin/env python3
"""Watch all pads while nobody touches them, and report which ones emit events.

Cheap multi-port adapters often float their unpopulated ports, spraying button
and axis events with no controller attached. That breaks naive press-to-detect
assignment: the phantom port wins the race before the human presses anything.

Run this with your hands off the controllers.
"""

import glob
import os
import select
import struct
import subprocess
import sys
import time
from collections import Counter

EVENT_FORMAT = "llHHi"
EVENT_SIZE = struct.calcsize(EVENT_FORMAT)

EV_SYN, EV_KEY, EV_REL, EV_ABS, EV_MSC = 0x00, 0x01, 0x02, 0x03, 0x04
TYPE_NAMES = {EV_SYN: "SYN", EV_KEY: "KEY", EV_REL: "REL",
              EV_ABS: "ABS", EV_MSC: "MSC"}

DURATION = 5.0


def read_attr(path):
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return ""


def find_pads():
    pads = []
    for input_dir in sorted(glob.glob("/sys/class/input/input*")):
        events = [os.path.basename(p) for p in glob.glob(os.path.join(input_dir, "event*"))]
        if not events:
            continue
        devnode = f"/dev/input/{events[0]}"
        if not os.path.exists(devnode):
            continue
        try:
            out = subprocess.run(
                ["udevadm", "info", "-q", "property", "-n", devnode],
                capture_output=True, text=True, timeout=5,
            ).stdout
            if "ID_INPUT_JOYSTICK=1" not in out:
                continue
        except (OSError, subprocess.SubprocessError):
            if not glob.glob(os.path.join(input_dir, "js*")):
                continue
        pads.append({
            "devnode": devnode,
            "event": events[0],
            "name": read_attr(os.path.join(input_dir, "name")),
            "syspath": os.path.realpath(os.path.join(input_dir, events[0])),
        })
    pads.sort(key=lambda p: p["syspath"])
    return pads


def main():
    pads = find_pads()
    if not pads:
        print("No pads found.")
        return 1

    handles = {}
    for pad in pads:
        try:
            handles[os.open(pad["devnode"], os.O_RDONLY | os.O_NONBLOCK)] = pad
        except PermissionError:
            print(f"Cannot read {pad['devnode']}")
            return 1

    # Drain anything already queued so we measure only what arrives from now on.
    for fd in handles:
        try:
            while os.read(fd, EVENT_SIZE * 256):
                pass
        except (BlockingIOError, OSError):
            pass

    print(f"Watching {len(pads)} pads for {DURATION:.0f}s. Do not touch anything.\n")

    stats = {fd: Counter() for fd in handles}
    keys = {fd: Counter() for fd in handles}
    deadline = time.monotonic() + DURATION
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        ready, _, _ = select.select(list(handles), [], [], remaining)
        for fd in ready:
            try:
                data = os.read(fd, EVENT_SIZE * 256)
            except (BlockingIOError, OSError):
                continue
            for off in range(0, len(data) - EVENT_SIZE + 1, EVENT_SIZE):
                _, _, etype, code, value = struct.unpack_from(EVENT_FORMAT, data, off)
                stats[fd][etype] += 1
                if etype == EV_KEY and value == 1:
                    keys[fd][code] += 1

    for fd in handles:
        os.close(fd)

    print(f"{'node':<9} {'total':>7} {'KEY':>6} {'ABS':>7} {'presses':>8}  name")
    print("-" * 78)
    noisy = []
    for fd, pad in handles.items():
        c = stats[fd]
        total = sum(c.values())
        npress = sum(keys[fd].values())
        flag = ""
        if total:
            noisy.append((pad, c, keys[fd]))
            flag = "  <-- EMITTING WHILE IDLE"
        print(f"{pad['event']:<9} {total:>7} {c[EV_KEY]:>6} {c[EV_ABS]:>7} "
              f"{npress:>8}  {pad['name'][:28]}{flag}")

    print()
    if not noisy:
        print("All quiet. Every node was silent, so the four presses you saw were")
        print("real input -- something else explains the extra two.")
    else:
        print(f"{len(noisy)} node(s) emitted events with nobody touching them.")
        for pad, c, k in noisy:
            kinds = ", ".join(f"{TYPE_NAMES.get(t, t)}={n}" for t, n in sorted(c.items()))
            print(f"  {pad['event']}: {kinds}")
            if k:
                codes = ", ".join(f"0x{code:x} x{n}" for code, n in k.most_common(6))
                print(f"    phantom presses: {codes}")
        print()
        print("Press-to-activate must therefore ignore floating ports. Options:")
        print("  - require a sustained/repeated press rather than a single edge")
        print("  - baseline each node's idle chatter first, then look for a change")
        print("  - require a press on a button the phantom stream never produces")
    return 0


if __name__ == "__main__":
    sys.exit(main())
