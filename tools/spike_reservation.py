#!/usr/bin/env python3
"""Check that RetroArch actually honours reservation-by-name for our pads.

The design assumes `input_playerN_reserved_device = "padmap Player N"` pins a
player to a uniquely named virtual pad, making enumeration order irrelevant
and a patched RetroArch unnecessary. That assumption is load-bearing, and
reading reallocate_port_if_needed() suggests at least one way it can silently
fail (see retroarch.reservation_config).

Creates throwaway uinput pads -- no physical controllers involved -- launches
RetroArch briefly with --verbose, and reports what its autoconfig actually did.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import evdev
from evdev import ecodes

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap.virtual import (  # noqa: E402
    PADMAP_PID, PADMAP_VID, virtual_name, virtual_phys,
)

PLAYERS = int(os.environ.get("PADMAP_TEST_PLAYERS", "2"))
RUN_SECONDS = 6

# A plausible gamepad: enough buttons and axes for udev's input_id builtin to
# classify it as a joystick.
CAPS = {
    ecodes.EV_KEY: [
        ecodes.BTN_SOUTH, ecodes.BTN_EAST, ecodes.BTN_NORTH, ecodes.BTN_WEST,
        ecodes.BTN_TL, ecodes.BTN_TR, ecodes.BTN_SELECT, ecodes.BTN_START,
        ecodes.BTN_MODE, ecodes.BTN_THUMBL, ecodes.BTN_THUMBR,
    ],
    ecodes.EV_ABS: [
        (ecodes.ABS_X, evdev.AbsInfo(0, -32768, 32767, 0, 0, 0)),
        (ecodes.ABS_Y, evdev.AbsInfo(0, -32768, 32767, 0, 0, 0)),
        (ecodes.ABS_RX, evdev.AbsInfo(0, -32768, 32767, 0, 0, 0)),
        (ecodes.ABS_RY, evdev.AbsInfo(0, -32768, 32767, 0, 0, 0)),
        (ecodes.ABS_HAT0X, evdev.AbsInfo(0, -1, 1, 0, 0, 0)),
        (ecodes.ABS_HAT0Y, evdev.AbsInfo(0, -1, 1, 0, 0, 0)),
    ],
}


def main() -> int:
    pads = []
    for player in range(1, PLAYERS + 1):
        pads.append(evdev.UInput(
            events=CAPS,
            name=virtual_name(player),
            phys=virtual_phys(player),
            vendor=PADMAP_VID,
            product=PADMAP_PID,
            version=1,
            bustype=ecodes.BUS_VIRTUAL,
        ))
    print(f"created {len(pads)} virtual pads:")
    for ui in pads:
        print(f"  {ui.device.path}  {ui.device.name!r}")
    time.sleep(0.5)

    from padmap.assign import Assignment  # noqa: E402
    from padmap.devices import Pad  # noqa: E402
    from padmap import retroarch  # noqa: E402

    fake = [
        Assignment(player=i + 1,
                   pad=Pad("", "", "", "", 0, 0, ""),
                   button=0)
        for i in range(PLAYERS)
    ]
    with tempfile.NamedTemporaryFile("w", suffix=".cfg", delete=False) as handle:
        handle.write(retroarch.reservation_config(fake))
        # A null video driver makes RetroArch exit before joypad init, so it
        # has to come up for real. Expect a window to flash briefly.
        max_users = os.environ.get("PADMAP_TEST_MAX_USERS")
        if max_users:
            handle.write(f'input_max_users = "{max_users}"\n')
        override = handle.name

    print(f"\nappendconfig: {override}")
    print(Path(override).read_text())

    print(f"launching retroarch --verbose for {RUN_SECONDS}s ...\n")
    try:
        proc = subprocess.run(
            ["retroarch", "--verbose", "--appendconfig", override],
            capture_output=True, text=True, timeout=RUN_SECONDS,
        )
        output = proc.stdout + proc.stderr
    except subprocess.TimeoutExpired as exc:
        output = (exc.stdout or "") + (exc.stderr or "")
        if isinstance(output, bytes):
            output = output.decode(errors="replace")
    except FileNotFoundError:
        print("retroarch not on PATH -- run inside `nix develop`.")
        return 1

    interesting = [
        line for line in output.splitlines()
        if re.search(r"Autoconf|\[Input\]|reserv|joypad|Pad #", line, re.I)
    ]
    print("=== relevant log lines ===")
    for line in interesting[:70]:
        print(" ", line.strip())
    if not interesting:
        print("  (none -- RetroArch may have exited before input init)")

    print("\n=== verdict ===")
    matched = [l for l in interesting if "Reserved device matched" in l]
    padmap_pads = [l for l in interesting if "padmap Player" in l]
    if matched:
        print(f"PASS: {len(matched)} reservation match(es) logged.")
    elif padmap_pads:
        print("PARTIAL: RetroArch saw the padmap pads but logged no reservation")
        print("         match. Check whether the early-return path fired.")
    else:
        print("INCONCLUSIVE: RetroArch did not report our pads at all.")

    for ui in pads:
        ui.close()
    os.unlink(override)
    return 0


if __name__ == "__main__":
    sys.exit(main())
