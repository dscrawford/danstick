"""Discovery and identity for physical joypads.

The central fact this module exists to work around: on multi-port adapters,
several ports can be completely indistinguishable. The four ports of a
Mayflash GameCube adapter share one USB interface and one HID device, and
report identical name, phys, uniq, vid, pid, version and properties. Only
their `inputN` ordinal differs, and that rides on a global counter which
libudev sorts as a string.

So `Pad.stable_key()` is deliberately *not* offered. There is no stable key.
Identity comes from the user pressing a button (see assign.py).
"""

from __future__ import annotations

import glob
import os
import subprocess
from dataclasses import dataclass

import evdev
from evdev import ecodes

# Virtual pads we publish are tagged with this phys prefix so that discovery
# never picks up our own output. Without it, restarting the daemon would grab
# its own pads and republish them, one layer deeper each time.
VIRTUAL_PHYS_PREFIX = "padmap/"

# BTN_JOYSTICK (0x120) through BTN_THUMBR (0x13f): the range udev's input_id
# builtin uses, together with absolute axes, to decide ID_INPUT_JOYSTICK.
_BTN_JOYSTICK_RANGE = range(0x120, 0x140)


@dataclass(frozen=True)
class Pad:
    """A physical joypad node, as RetroArch's udev driver would see it."""

    path: str  # /dev/input/eventN
    name: str
    phys: str
    uniq: str
    vid: int
    pid: int
    syspath: str
    # False once the `padmap hide` udev rules have cleared ID_INPUT_JOYSTICK:
    # padmap can still open and republish the pad, RetroArch cannot see it.
    retroarch_visible: bool = True

    @property
    def event(self) -> str:
        return os.path.basename(self.path)

    @property
    def retroarch_id(self) -> str:
        """The string RetroArch stores as this pad's phys.

        udev_joypad.c:498-504 reads EVIOCGPHYS then appends EVIOCGUNIQ at
        pad->phys+physlen with no separator. Reproduced here so diagnostics
        can show exactly what RetroArch would compare against.
        """
        return self.phys + self.uniq

    def describe(self) -> str:
        return f"{self.event:<9} {self.name}"


def _udev_properties(devnode: str) -> dict[str, str]:
    try:
        out = subprocess.run(
            ["udevadm", "info", "-q", "property", "-n", devnode],
            capture_output=True, text=True, timeout=5,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return {}
    props: dict[str, str] = {}
    for line in out.splitlines():
        key, sep, value = line.partition("=")
        if sep:
            props[key] = value
    return props


def _retroarch_sees(devnode: str, input_dir: str) -> bool:
    """RetroArch's own filter: ID_INPUT_JOYSTICK=1 (udev_joypad.c:1053)."""
    props = _udev_properties(devnode)
    if props:
        return props.get("ID_INPUT_JOYSTICK") == "1"
    # udevadm unavailable (stripped container): fall back to joydev's verdict.
    return bool(glob.glob(os.path.join(input_dir, "js*")))


def _looks_like_joypad(devnode: str) -> bool:
    """Is this a joypad by capability, regardless of how udev tagged it?

    padmap must not use ID_INPUT_JOYSTICK for its own discovery, because
    `padmap hide` deliberately clears that property. Sharing the filter would
    mean installing the hide rules made every controller invisible to padmap
    too, so `padmap setup` could never be run again -- unrecoverable without
    hand-removing the rules.
    """
    try:
        device = evdev.InputDevice(devnode)
    except (OSError, PermissionError):
        return False
    try:
        caps = device.capabilities()
        if not caps.get(ecodes.EV_ABS):
            return False
        return any(code in _BTN_JOYSTICK_RANGE
                   for code in caps.get(ecodes.EV_KEY, ()))
    except OSError:
        return False
    finally:
        device.close()


ENV_ONLY = "PADMAP_ONLY_DEVICE"


def discover(
    include_virtual: bool = False, retroarch_only: bool = False
) -> list[Pad]:
    """Joypads, in the order RetroArch's udev driver would enumerate them.

    libudev returns enumerate results sorted by syspath (verified against
    `udevadm trigger --dry-run`), and RetroArch assigns each the first vacant
    slot, so this ordering is what its pad indices would be.

    By default this reports every device that *is* a joypad. Pass
    `retroarch_only` to restrict it to the ones RetroArch can currently see,
    which is what pad-index prediction needs -- the two differ exactly when
    the `padmap hide` udev rules are installed.
    """
    pads: list[Pad] = []
    for input_dir in glob.glob("/sys/class/input/input*"):
        events = [
            os.path.basename(p) for p in glob.glob(os.path.join(input_dir, "event*"))
        ]
        if not events:
            continue
        devnode = f"/dev/input/{events[0]}"
        if not os.path.exists(devnode):
            continue

        visible = _retroarch_sees(devnode, input_dir)
        if not visible and not _looks_like_joypad(devnode):
            continue
        if retroarch_only and not visible:
            continue

        phys = _read(os.path.join(input_dir, "phys"))
        if not include_virtual and phys.startswith(VIRTUAL_PHYS_PREFIX):
            continue

        pads.append(Pad(
            path=devnode,
            name=_read(os.path.join(input_dir, "name")),
            phys=phys,
            uniq=_read(os.path.join(input_dir, "uniq")),
            vid=_read_hex(os.path.join(input_dir, "id/vendor")),
            pid=_read_hex(os.path.join(input_dir, "id/product")),
            syspath=os.path.realpath(os.path.join(input_dir, events[0])),
            retroarch_visible=visible,
        ))

    pads.sort(key=lambda p: p.syspath)

    # Test escape hatch: restrict discovery to one device by name. Without it
    # an isolated test daemon still finds the machine's real controllers and
    # fights the live daemon for an exclusive grab on them.
    only = os.environ.get(ENV_ONLY)
    if only:
        pads = [p for p in pads if only in p.name]

    return pads


def open_device(pad: Pad) -> evdev.InputDevice:
    return evdev.InputDevice(pad.path)


def ambiguous_groups(pads: list[Pad]) -> list[list[Pad]]:
    """Pads that no static identifier can tell apart.

    Used to explain to the user why press-to-activate is required rather than
    letting them think it is a stylistic choice.
    """
    groups: dict[tuple, list[Pad]] = {}
    for pad in pads:
        key = (pad.name, pad.phys, pad.uniq, pad.vid, pad.pid)
        groups.setdefault(key, []).append(pad)
    return [g for g in groups.values() if len(g) > 1]


def _read(path: str) -> str:
    try:
        with open(path) as handle:
            return handle.read().strip()
    except OSError:
        return ""


def _read_hex(path: str) -> int:
    raw = _read(path)
    try:
        return int(raw, 16)
    except ValueError:
        return 0
