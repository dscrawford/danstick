#!/usr/bin/env python3
"""Press-to-activate prototype: assign player order by button press.

This validates the assumption the whole design rests on -- that pads which
are indistinguishable by every static evdev attribute (the four Mayflash
GameCube ports) still deliver events on distinct event nodes, so a human
pressing a button can supply the identity the kernel cannot.

Stdlib only. Reads raw input_event structs and selects across all pads.
"""

import glob
import os
import select
import struct
import subprocess
import sys

# struct input_event on 64-bit Linux:
#   struct timeval (2x long) + __u16 type + __u16 code + __s32 value
EVENT_FORMAT = "llHHi"
EVENT_SIZE = struct.calcsize(EVENT_FORMAT)

EV_KEY = 0x01
# Ignore the low keyboard range so a stray keypress on a combo device
# does not register as a pad press.
BTN_FIRST = 0x100


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
            "phys": read_attr(os.path.join(input_dir, "phys")),
            "syspath": os.path.realpath(os.path.join(input_dir, events[0])),
        })
    pads.sort(key=lambda p: p["syspath"])
    return pads


def main():
    pads = find_pads()
    if not pads:
        print("No pads found. Plug a controller in and re-run.")
        return 1

    handles = {}
    for pad in pads:
        try:
            handles[os.open(pad["devnode"], os.O_RDONLY | os.O_NONBLOCK)] = pad
        except PermissionError:
            print(f"Cannot read {pad['devnode']} -- need membership in the "
                  f"'input' group, or run with sudo.")
            return 1

    print(f"Watching {len(pads)} pads. Press a button on each, in the order")
    print("you want them assigned. Ctrl-C to stop.\n")

    assigned = []
    seen_fds = set()
    try:
        while len(assigned) < len(pads):
            ready, _, _ = select.select(list(handles), [], [], None)
            for fd in ready:
                try:
                    data = os.read(fd, EVENT_SIZE * 64)
                except OSError:
                    continue
                if fd in seen_fds:
                    continue
                for off in range(0, len(data) - EVENT_SIZE + 1, EVENT_SIZE):
                    _, _, etype, code, value = struct.unpack_from(
                        EVENT_FORMAT, data, off)
                    if etype == EV_KEY and value == 1 and code >= BTN_FIRST:
                        pad = handles[fd]
                        seen_fds.add(fd)
                        assigned.append(pad)
                        print(f"  Player {len(assigned)} -> {pad['event']}  "
                              f"({pad['name']})  code=0x{code:x}")
                        break
    except KeyboardInterrupt:
        print("\n\nstopped early")

    for fd in handles:
        os.close(fd)

    if not assigned:
        return 0

    print("\n=== result ===")
    distinct = len({p["event"] for p in assigned})
    print(f"{len(assigned)} presses across {distinct} distinct event nodes")
    if distinct == len(assigned):
        print("Press order yields unique nodes -- press-to-activate is viable.")
    else:
        print("Some presses landed on the same node. That breaks the design;")
        print("investigate before going further.")

    print("\n=== port order this implies ===")
    for i, pad in enumerate(assigned):
        print(f"  player {i+1}: {pad['devnode']}  {pad['name']}")
        print(f"             phys={pad['phys']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
