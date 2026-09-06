#!/usr/bin/env python3
"""Live event monitor across all pads, with simultaneity detection.

Press ONE button on ONE controller at a time. If a single physical press
lights up more than one event node, the adapter is mirroring its report
across multiple input devices -- which means press-to-activate cannot
assume one press == one node.

Ctrl-C to stop.
"""

import glob
import os
import select
import struct
import subprocess
import sys
import time
from collections import defaultdict

EVENT_FORMAT = "llHHi"
EVENT_SIZE = struct.calcsize(EVENT_FORMAT)

EV_KEY, EV_ABS = 0x01, 0x03
BTN_FIRST = 0x100

# Presses landing inside this window are treated as one physical action.
SIMULTANEITY_WINDOW = 0.030


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

    for fd in handles:
        try:
            while os.read(fd, EVENT_SIZE * 256):
                pass
        except (BlockingIOError, OSError):
            pass

    print(f"Watching {len(pads)} pads: "
          f"{', '.join(p['event'] for p in pads)}")
    print("Press ONE button on ONE controller. Ctrl-C to stop.\n")

    # node -> set of nodes it has ever fired alongside
    co_fire = defaultdict(set)
    burst = []
    burst_start = None

    def flush_burst():
        if not burst:
            return
        nodes = sorted({e[0] for e in burst})
        stamp = time.strftime("%H:%M:%S")
        if len(nodes) == 1:
            detail = ", ".join(f"0x{c:x}" for _, c, _ in burst)
            print(f"[{stamp}] {nodes[0]:<9} {detail}")
        else:
            print(f"[{stamp}] MIRRORED across {len(nodes)} nodes: {', '.join(nodes)}")
            for node, code, name in burst:
                print(f"            {node:<9} 0x{code:x}  {name[:34]}")
            for a in nodes:
                for b in nodes:
                    if a != b:
                        co_fire[a].add(b)
        burst.clear()

    try:
        while True:
            timeout = None if burst_start is None else SIMULTANEITY_WINDOW
            ready, _, _ = select.select(list(handles), [], [], timeout)
            now = time.monotonic()

            if not ready:
                flush_burst()
                burst_start = None
                continue

            for fd in ready:
                try:
                    data = os.read(fd, EVENT_SIZE * 256)
                except (BlockingIOError, OSError):
                    continue
                pad = handles[fd]
                for off in range(0, len(data) - EVENT_SIZE + 1, EVENT_SIZE):
                    _, _, etype, code, value = struct.unpack_from(EVENT_FORMAT, data, off)
                    if etype == EV_KEY and value == 1 and code >= BTN_FIRST:
                        if burst_start is None:
                            burst_start = now
                        burst.append((pad["event"], code, pad["name"]))

            if burst_start is not None and now - burst_start >= SIMULTANEITY_WINDOW:
                flush_burst()
                burst_start = None

    except KeyboardInterrupt:
        flush_burst()
        print("\n")

    for fd in handles:
        os.close(fd)

    if co_fire:
        print("=== nodes that fired together ===")
        seen = set()
        for node, partners in sorted(co_fire.items()):
            group = tuple(sorted({node} | partners))
            if group in seen:
                continue
            seen.add(group)
            print(f"  {' + '.join(group)}")
        print()
        print("These are one physical controller presenting as several input")
        print("devices. Press-to-activate must collapse each group to a single")
        print("pad, and the daemon should republish one virtual pad per group.")
    else:
        print("No mirroring observed: every press hit exactly one node.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
