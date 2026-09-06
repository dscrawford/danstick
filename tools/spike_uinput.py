#!/usr/bin/env python3
"""Verify a uinput pad we create is classified as a joystick by udev.

RetroArch's udev joypad driver enumerates on ID_INPUT_JOYSTICK=1
(udev_joypad.c:1053). If our virtual pads do not carry that property they are
invisible to RetroArch and the whole approach fails. This checks that, plus
that the phys/name we set actually survive to sysfs.

Clones capabilities from a real pad but does NOT grab it, so it is safe to run
while you are using the controller.
"""

import subprocess
import sys
import time

import evdev

SOURCE = sys.argv[1] if len(sys.argv) > 1 else None


def pick_source():
    for path in evdev.list_devices():
        dev = evdev.InputDevice(path)
        caps = dev.capabilities()
        keys = caps.get(evdev.ecodes.EV_KEY, [])
        if any(k >= 0x100 for k in keys) and evdev.ecodes.EV_ABS in caps:
            return dev
        dev.close()
    return None


def main():
    src = evdev.InputDevice(SOURCE) if SOURCE else pick_source()
    if not src:
        print("No suitable source pad found.")
        return 1

    print(f"source: {src.path}  {src.name!r}")
    print(f"        phys={src.phys!r} uniq={src.uniq!r}")

    # from_device clones EV_KEY/EV_ABS/absinfo so mappings survive the hop.
    ui = evdev.UInput.from_device(
        src,
        name="padmap Player 1",
        phys="padmap/p1",
        vendor=0x1209,   # pid.codes, the free/open USB VID space
        product=0x0001,
        version=1,
    )
    print(f"\ncreated: {ui.device.path}  {ui.device.name!r}")
    print(f"         phys={ui.device.phys!r}")

    # udev needs a moment to process the add event and run input_id.
    time.sleep(0.5)

    out = subprocess.run(
        ["udevadm", "info", "-q", "property", "-n", ui.device.path],
        capture_output=True, text=True,
    ).stdout
    props = dict(
        line.partition("=")[::2] for line in out.splitlines() if "=" in line
    )

    joystick = props.get("ID_INPUT_JOYSTICK")
    print(f"\nID_INPUT_JOYSTICK = {joystick!r}")
    print(f"ID_INPUT_KEY      = {props.get('ID_INPUT_KEY')!r}")

    syspath = f"/sys/class/input/{ui.device.path.split('/')[-1]}/device"
    for attr in ("name", "phys", "uniq"):
        try:
            with open(f"{syspath}/{attr}") as f:
                print(f"sysfs {attr:<5}     = {f.read().strip()!r}")
        except OSError:
            print(f"sysfs {attr:<5}     = <unreadable>")

    src_caps = src.capabilities()
    ui_caps = ui.device.capabilities()
    src_keys = set(src_caps.get(evdev.ecodes.EV_KEY, []))
    ui_keys = set(ui_caps.get(evdev.ecodes.EV_KEY, []))
    src_abs = {c for c, _ in src_caps.get(evdev.ecodes.EV_ABS, [])}
    ui_abs = {c for c, _ in ui_caps.get(evdev.ecodes.EV_ABS, [])}
    print(f"\nbuttons: source {len(src_keys)} -> virtual {len(ui_keys)}"
          f"  {'OK' if src_keys == ui_keys else 'MISMATCH'}")
    print(f"axes:    source {len(src_abs)} -> virtual {len(ui_abs)}"
          f"  {'OK' if src_abs == ui_abs else 'MISMATCH'}")

    ff = evdev.ecodes.EV_FF in src_caps
    print(f"\nsource advertises EV_FF (rumble): {ff}")
    if ff:
        print("  -> passthrough will need explicit effect upload handling")

    print()
    if joystick == "1":
        print("PASS: RetroArch's enumeration would pick this up.")
        rc = 0
    else:
        print("FAIL: no ID_INPUT_JOYSTICK=1, RetroArch would not see this pad.")
        print("      Capabilities may need padding (e.g. a full BTN_GAMEPAD set)")
        print("      to satisfy udev's input_id builtin.")
        rc = 1

    ui.close()
    src.close()
    return rc


if __name__ == "__main__":
    sys.exit(main())
