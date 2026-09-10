"""Read a Switch Pro Controller over hidraw, and prove the decode.

padmap clones evdev nodes. For this pad that does not work: SDL drives it over
/dev/hidraw* in a vendor report mode, hid-nintendo is starved, and the evdev
node -- openable, grabbable, watchable -- never emits an event. See
docs/HIDRAW.md for the measurements.

So padmap has to speak HID itself. This is the first half of that: talk to the
controller directly, put it in standard full mode, and decode its reports.

Deliberately prints the raw bytes next to the decode. The byte layout below
comes from dekuNukem's reverse-engineering notes, and a table copied out of a
document is exactly the sort of thing this project has been bitten by -- so the
probe is built to falsify itself. Press A and watch which raw bit changes; if
the decode disagrees with the bytes, the decode is wrong.

    python3 tools/switchprobe.py [--seconds N]

Nothing else may hold the controller while this runs: two readers split the
report stream between them. Stop Pegasus first.
"""

from __future__ import annotations

import glob
import os
import select
import sys
import time
from pathlib import Path

VENDOR, PRODUCT = 0x057E, 0x2009

# INPUT 0x30, "standard full mode": id, timer, battery, 3 button bytes, two
# 3-byte sticks, vibrator, then IMU.
REPORT_FULL = 0x30

# Byte 3 (right), byte 4 (shared), byte 5 (left). Names are the pad's own.
BUTTONS_RIGHT = [(0x01, "Y"), (0x02, "X"), (0x04, "B"), (0x08, "A"),
                 (0x10, "SR"), (0x20, "SL"), (0x40, "R"), (0x80, "ZR")]
BUTTONS_SHARED = [(0x01, "Minus"), (0x02, "Plus"), (0x04, "RStick"),
                  (0x08, "LStick"), (0x10, "Home"), (0x20, "Capture")]
BUTTONS_LEFT = [(0x01, "Down"), (0x02, "Up"), (0x04, "Right"), (0x08, "Left"),
                (0x10, "SR"), (0x20, "SL"), (0x40, "L"), (0x80, "ZL")]

# A neutral rumble frame. Every subcommand carries one, whether or not it is
# meant to rumble; sending zeroes here makes some firmware unhappy.
RUMBLE_NEUTRAL = bytes([0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40])


def find_hidraw() -> str | None:
    """The hidraw node belonging to our vendor/product, via sysfs."""
    for path in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        uevent = Path(path) / "device" / "uevent"
        try:
            text = uevent.read_text()
        except OSError:
            continue
        # HID_ID=0005:0000057E:00002009
        for line in text.splitlines():
            if not line.startswith("HID_ID="):
                continue
            parts = line.split("=", 1)[1].split(":")
            if len(parts) == 3 and int(parts[1], 16) == VENDOR \
                    and int(parts[2], 16) == PRODUCT:
                return "/dev/" + os.path.basename(path)
    return None


def send_subcommand(fd: int, counter: int, sub: int, args: bytes) -> None:
    """OUTPUT 0x01: id, packet counter, 8 rumble bytes, subcommand, args."""
    packet = bytearray(64)
    packet[0] = 0x01
    packet[1] = counter & 0x0F
    packet[2:10] = RUMBLE_NEUTRAL
    packet[10] = sub
    packet[11:11 + len(args)] = args
    os.write(fd, bytes(packet))


def decode_sticks(data: bytes) -> tuple[int, int, int, int]:
    """Two 12-bit pairs, packed three bytes each."""
    lx = data[6] | ((data[7] & 0x0F) << 8)
    ly = (data[7] >> 4) | (data[8] << 4)
    rx = data[9] | ((data[10] & 0x0F) << 8)
    ry = (data[10] >> 4) | (data[11] << 4)
    return lx, ly, rx, ry


def decode_buttons(data: bytes) -> list[str]:
    pressed = []
    for mask, name in BUTTONS_RIGHT:
        if data[3] & mask:
            pressed.append(name)
    for mask, name in BUTTONS_SHARED:
        if data[4] & mask:
            pressed.append(name)
    for mask, name in BUTTONS_LEFT:
        if data[5] & mask:
            pressed.append(name)
    return pressed


def main() -> int:
    seconds = 30.0
    if "--seconds" in sys.argv:
        seconds = float(sys.argv[sys.argv.index("--seconds") + 1])

    node = find_hidraw()
    if node is None:
        print(f"No hidraw node for {VENDOR:04x}:{PRODUCT:04x} -- is the "
              f"controller connected?")
        return 1
    print(f"hidraw node: {node}")

    try:
        fd = os.open(node, os.O_RDWR | os.O_NONBLOCK)
    except OSError as error:
        print(f"cannot open {node}: {error}")
        return 1

    print("requesting standard full mode (subcommand 0x03, arg 0x30)...")
    try:
        send_subcommand(fd, 0, 0x03, bytes([REPORT_FULL]))
    except OSError as error:
        print(f"  write failed: {error}")

    print()
    print(f"reading for {seconds:.0f}s -- PRESS BUTTONS.")
    print("raw bytes are shown next to the decode, so a wrong table is "
          "visible rather than believed.")
    print("-" * 78)

    seen = 0
    ids: dict[int, int] = {}
    last_line = ""
    previous: bytes | None = None
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        ready, _, _ = select.select([fd], [], [], 0.5)
        if not ready:
            continue
        try:
            data = os.read(fd, 362)
        except OSError:
            continue
        if not data:
            continue
        seen += 1
        ids[data[0]] = ids.get(data[0], 0) + 1
        if data[0] != REPORT_FULL or len(data) < 12:
            continue
        pressed = decode_buttons(data)
        lx, ly, rx, ry = decode_sticks(data)
        line = (f"btn={data[3]:02x} {data[4]:02x} {data[5]:02x}  "
                f"L=({lx:4d},{ly:4d}) R=({rx:4d},{ry:4d})  "
                f"{'+'.join(pressed) if pressed else '-'}")
        # Only when something changed, or the stream is unreadable noise.
        if line != last_line:
            last_line = line
            print(line)

        # Which bytes actually move, independent of the table above.
        #
        # If a press changes some byte the decode is not looking at, the table
        # is wrong and this is the only thing that will say so. Byte 1 is the
        # timer and changes every report; the sticks jitter constantly; so
        # compare only the first 13 bytes and mask those out.
        current = bytes(data[:13])
        if previous is not None:
            moved = [i for i in range(13)
                     if i not in (1, 6, 7, 8, 9, 10, 11)
                     and current[i] != previous[i]]
            if moved:
                print(f"    RAW CHANGED at byte(s) {moved}: "
                      + " ".join(f"{i}:{previous[i]:02x}->{current[i]:02x}"
                                 for i in moved))
        previous = current

    os.close(fd)
    print("-" * 78)
    print(f"{seen} report(s) read. by report id: "
          + ", ".join(f"0x{k:02x}:{v}" for k, v in sorted(ids.items())))
    if seen == 0:
        print("NOTHING arrived. The controller is not sending over hidraw "
              "either, or another process is consuming the stream.")
    elif REPORT_FULL not in ids:
        print(f"reports arrived but none were 0x{REPORT_FULL:02x}; the mode "
              f"request did not take.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
