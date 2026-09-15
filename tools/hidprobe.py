"""Watch a controller's raw HID reports, so its protocol can be read off it.

    nix develop --command python3 tools/hidprobe.py /dev/hidraw11

For a pad whose gamepad state is on a vendor-defined interface, which no
generic driver can decode. The only way to learn what the bytes mean is to
press one thing at a time and watch which byte changes -- this prints exactly
that: every report, with the bytes that differ from the last one highlighted,
and a running record of which bit each press moved.

This is how the Switch Pro decode in `hidraw.py` was established; see
docs/HIDRAW.md. It prints raw bytes beside any decode so a wrong guess shows up
as a disagreement rather than as a silently wrong binding.

    --settle SECONDS   ignore everything for this long first, so the report the
                       pad was already sending is not mistaken for a press
    --raw              print every report, not just the ones that changed

Reading is enough for most of it. Some pads send nothing until they are asked
to leave their keyboard-and-mouse mode, which needs a write this tool does not
make -- if nothing arrives while you are pressing buttons, that is the reason,
and the device needs its own driver rather than a decode.
"""

from __future__ import annotations

import argparse
import os
import select
import sys
import time


def describe_changes(previous: bytes, current: bytes) -> list[str]:
    """Which bytes moved, and which bits within them.

    Bits rather than bytes because a button is a bit: a report where byte 3
    went from 0x00 to 0x08 says "bit 3 of byte 3", which is the thing that goes
    into a table.
    """
    out = []
    for index in range(max(len(previous), len(current))):
        before = previous[index] if index < len(previous) else 0
        after = current[index] if index < len(current) else 0
        if before == after:
            continue
        moved = before ^ after
        bits = [bit for bit in range(8) if moved & (1 << bit)]
        if bits and moved in (1, 2, 4, 8, 16, 32, 64, 128):
            out.append(f"byte {index:2d}: {before:02x} -> {after:02x}"
                       f"  bit {bits[0]} {'set' if after & moved else 'cleared'}")
        else:
            out.append(f"byte {index:2d}: {before:02x} -> {after:02x}"
                       f"  (delta {after - before:+d})")
    return out


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("node", help="the hidraw node, e.g. /dev/hidraw11")
    parser.add_argument("--settle", type=float, default=1.0)
    parser.add_argument("--raw", action="store_true")
    parser.add_argument("--seconds", type=float, default=0.0,
                        help="stop after this long (default: until Ctrl-C)")
    args = parser.parse_args(argv)

    if not os.access(args.node, os.R_OK):
        print(f"{args.node} is not readable by this user.")
        print("On this machine hidraw access is a logind seat ACL, so check you")
        print("are on an active session -- or run this as root.")
        return 1

    fd = os.open(args.node, os.O_RDONLY | os.O_NONBLOCK)
    # Per report id, because a pad interleaves several and diffing across them
    # would report every byte as changed on every report.
    previous: dict[int, bytes] = {}
    counts: dict[int, int] = {}
    started = time.monotonic()
    quiet_until = started + args.settle

    print(f"reading {args.node}; press one control at a time. Ctrl-C to stop.\n")
    try:
        while True:
            if args.seconds and time.monotonic() - started > args.seconds:
                break
            ready, _, _ = select.select([fd], [], [], 0.5)
            if not ready:
                continue
            try:
                data = os.read(fd, 512)
            except BlockingIOError:
                continue
            except OSError as error:
                print(f"\nread failed: {error} -- the device has gone")
                break
            if not data:
                continue

            report = data[0]
            counts[report] = counts.get(report, 0) + 1
            if time.monotonic() < quiet_until:
                previous[report] = data
                continue

            changes = describe_changes(previous.get(report, b""), data)
            previous[report] = data
            if not changes and not args.raw:
                continue
            print(f"[{counts[report]:6d}] id=0x{report:02x} len={len(data)}")
            print(f"         {data.hex(' ')}")
            for line in changes:
                print(f"         {line}")
    except KeyboardInterrupt:
        print()
    finally:
        os.close(fd)

    if not counts:
        print("Nothing arrived at all.")
        print("If you were pressing buttons, the pad is not sending on this")
        print("interface yet -- some send only keyboard and mouse until a")
        print("command tells them otherwise, and this tool does not send one.")
        return 1

    print("\nreports seen:")
    for report, count in sorted(counts.items()):
        print(f"  id=0x{report:02x}  {count}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
