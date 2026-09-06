#!/usr/bin/env python3
"""Determine whether reservation-by-name actually rebinds players to our pads.

The obvious signal is misleading: the "[Autoconf] X configured in port N" log
line reports autoconfig_handle->port + 1, which is the *pad index*, not the
player slot. Reservation rewrites settings->uints.input_joypad_index[], and
nothing in a release build's log shows that (the relevant lines are RARCH_DBG).

So instead: run RetroArch against a *copy* of the user's config with
config_save_on_exit enabled, quit it cleanly over the network command
interface, and read input_playerN_joypad_index back out of the copy.

Nothing here touches the real retroarch.cfg.
"""

from __future__ import annotations

import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import evdev
from evdev import ecodes

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap import retroarch  # noqa: E402
from padmap.virtual import (  # noqa: E402
    PADMAP_PID, PADMAP_VID, virtual_name, virtual_phys,
)

PLAYERS = 2
NETWORK_PORT = 55355
SETTLE_SECONDS = 7

CAPS = {
    ecodes.EV_KEY: [
        ecodes.BTN_SOUTH, ecodes.BTN_EAST, ecodes.BTN_NORTH, ecodes.BTN_WEST,
        ecodes.BTN_TL, ecodes.BTN_TR, ecodes.BTN_SELECT, ecodes.BTN_START,
    ],
    ecodes.EV_ABS: [
        (ecodes.ABS_X, evdev.AbsInfo(0, -32768, 32767, 0, 0, 0)),
        (ecodes.ABS_Y, evdev.AbsInfo(0, -32768, 32767, 0, 0, 0)),
    ],
}


def main() -> int:
    real_cfg = retroarch.CONFIG_DIR / "retroarch.cfg"
    if not real_cfg.is_file():
        print(f"no config at {real_cfg}")
        return 1

    workdir = Path(tempfile.mkdtemp(prefix="padmap-spike-"))
    cfg = workdir / "retroarch.cfg"
    shutil.copy(real_cfg, cfg)

    autoconf = workdir / "autoconfig"
    (autoconf / "udev").mkdir(parents=True)
    fake = [
        Assignment(player=i + 1, pad=Pad("", "", "", "", 0, 0, ""), button=0)
        for i in range(PLAYERS)
    ]
    retroarch.install_profiles(fake, dest=autoconf / "udev")

    with cfg.open("a") as handle:
        handle.write("\n# --- padmap spike ---\n")
        handle.write(retroarch.reservation_config(fake))
        handle.write(f'joypad_autoconfig_dir = "{autoconf / "udev"}"\n')
        handle.write('config_save_on_exit = "true"\n')
        handle.write('network_cmd_enable = "true"\n')
        handle.write(f'network_cmd_port = "{NETWORK_PORT}"\n')
        handle.write('input_max_users = "16"\n')

    # Create Player 2 first so enumeration order disagrees with player order.
    # If reservation works, player 1 still ends up on the "padmap Player 1" pad.
    pads = []
    for player in (2, 1):
        ui = evdev.UInput(
            events=CAPS, name=virtual_name(player), phys=virtual_phys(player),
            vendor=PADMAP_VID, product=PADMAP_PID, version=1,
            bustype=ecodes.BUS_VIRTUAL,
        )
        pads.append((player, ui))
        print(f"created {virtual_name(player)} -> {ui.device.path}")
        time.sleep(0.3)

    print(f"\nlaunching retroarch against {cfg}")
    proc = subprocess.Popen(
        ["retroarch", "--verbose", "--config", str(cfg)],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    time.sleep(SETTLE_SECONDS)

    print("sending QUIT over the network command interface ...")
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.sendto(b"QUIT", ("127.0.0.1", NETWORK_PORT))
        sock.close()
    except OSError as exc:
        print(f"  failed: {exc}")

    try:
        proc.wait(timeout=15)
        clean = True
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        clean = False
    print(f"exited cleanly: {clean} (rc={proc.returncode})")

    saved = cfg.read_text()
    print("\n=== resulting mapping ===")
    indices = {}
    for match in re.finditer(
        r'input_player(\d+)_joypad_index\s*=\s*"?(\d+)"?', saved
    ):
        indices[int(match.group(1))] = int(match.group(2))
    for player in sorted(indices)[:PLAYERS + 2]:
        print(f"  input_player{player}_joypad_index = {indices[player]}")

    # Our pads enumerate last (uinput lives under /sys/devices/virtual, which
    # sorts after /sys/devices/pci...), so with 8 physical pads present they
    # occupy indices 8 and 9. Player 2 was created first, so it is index 8.
    print("\n=== verdict ===")
    p1, p2 = indices.get(1), indices.get(2)
    print(f"  player 1 -> pad index {p1}")
    print(f"  player 2 -> pad index {p2}")
    if p1 == 9 and p2 == 8:
        print("PASS: reservation rebound players to the named pads, overriding")
        print("      enumeration order (player 1 got the later-created pad).")
    elif p1 == 8 and p2 == 9:
        print("FAIL: players follow enumeration order; the reservation by name")
        print("      did not take effect.")
    else:
        print("UNCLEAR: neither expected pattern. Inspect the saved config:")
        print(f"  {cfg}")

    for _, ui in pads:
        ui.close()
    print(f"\nartefacts left in {workdir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
