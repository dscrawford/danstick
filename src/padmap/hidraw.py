"""Read controllers that do not speak evdev, and look like evdev doing it.

padmap republishes evdev nodes. Some controllers never populate one usefully:
SDL and Steam drive them over /dev/hidraw* in a vendor report mode, the kernel
driver is starved, and the evdev node can be opened, grabbed and watched
without ever emitting an event. Measured at length on a Switch Pro over
Bluetooth -- see docs/HIDRAW.md.

The fix is for padmap to be the process that speaks HID. Everything downstream
stays exactly as it was: `Republisher` still reads a source and writes a uinput
clone, and a clone fed from hidraw is indistinguishable from one fed from
evdev. So the whole of it fits behind the small surface `virtual.create` uses
of an `evdev.InputDevice`:

    fd  path  name  read()  grab()  ungrab()  close()  capabilities()
    active_keys()

`Source` below implements exactly that and nothing else.

The evdev codes emitted are deliberately the ones `hid-nintendo` uses for the
same pad. A profile captured over USB, where the kernel driver does work, stays
valid when the same controller comes back over Bluetooth through this path --
which is the whole point of bothering.
"""

from __future__ import annotations

import glob
import logging
import os
from pathlib import Path
from typing import Any

import evdev
from evdev import AbsInfo, ecodes

from .devices import Pad

log = logging.getLogger("padmap.hidraw")

# Controllers this module knows how to speak to. Anything absent falls through
# to the evdev path, which is right for everything padmap grew up on.
SWITCH_PRO = (0x057E, 0x2009)
SUPPORTED = {SWITCH_PRO}

REPORT_FULL = 0x30          # INPUT: standard full mode, sticks and buttons
SUBCMD_REPORT_MODE = 0x03   # OUTPUT subcommand: set input report mode

# Every subcommand carries a rumble frame whether or not it rumbles; some
# firmware ignores a request whose rumble bytes are all zero.
RUMBLE_NEUTRAL = bytes([0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40])

# Switch Pro button bits -> the evdev code hid-nintendo publishes for it.
#
# Verified against the hardware, not just the notes: pressing A set byte 3 to
# 0x08, which is what this table says. tools/switchprobe.py is the thing that
# checked, and prints raw bytes beside the decode so a wrong row here shows up
# as a disagreement rather than a silently wrong binding.
BUTTONS_RIGHT = {
    0x01: ecodes.BTN_WEST,     # Y  (Nintendo Y is the left face button)
    0x02: ecodes.BTN_NORTH,    # X  (top)
    0x04: ecodes.BTN_SOUTH,    # B  (bottom)
    0x08: ecodes.BTN_EAST,     # A  (right)
    0x40: ecodes.BTN_TR,       # R
    0x80: ecodes.BTN_TR2,      # ZR
}
BUTTONS_SHARED = {
    0x01: ecodes.BTN_SELECT,   # Minus
    0x02: ecodes.BTN_START,    # Plus
    0x04: ecodes.BTN_THUMBR,   # right stick click
    0x08: ecodes.BTN_THUMBL,   # left stick click
    0x10: ecodes.BTN_MODE,     # Home
    0x20: ecodes.BTN_Z,        # Capture
}
BUTTONS_LEFT = {
    0x40: ecodes.BTN_TL,       # L
    0x80: ecodes.BTN_TL2,      # ZL
}
# The d-pad is four bits, and is published as a hat like every other pad's.
DPAD_LEFT = {0x01: "down", 0x02: "up", 0x04: "right", 0x08: "left"}

# Sticks are 12-bit, centred near 2048. Published with the raw range so the
# calibration padmap already has stays meaningful.
STICK_MIN, STICK_MAX = 0, 4095
STICK_FUZZ, STICK_FLAT = 16, 128

AXES = (
    (ecodes.ABS_X, "lx"), (ecodes.ABS_Y, "ly"),
    (ecodes.ABS_RX, "rx"), (ecodes.ABS_RY, "ry"),
)


def supported(pad: Pad) -> bool:
    return (pad.vid, pad.pid) in SUPPORTED


def node_for(pad: Pad) -> str | None:
    """The /dev/hidraw* belonging to this pad, or None.

    Found through sysfs rather than guessed: HID_ID in the device's uevent
    carries bus, vendor and product, so a machine with two of the same pad
    still resolves each to its own node.
    """
    for path in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
        try:
            text = (Path(path) / "device" / "uevent").read_text()
        except OSError:
            continue
        for line in text.splitlines():
            if not line.startswith("HID_ID="):
                continue
            parts = line.split("=", 1)[1].split(":")
            if len(parts) != 3:
                continue
            try:
                vendor, product = int(parts[1], 16), int(parts[2], 16)
            except ValueError:
                continue
            if (vendor, product) == (pad.vid, pad.pid):
                return "/dev/" + os.path.basename(path)
    return None


class _Event:
    """An evdev event, as much of one as anything downstream reads."""

    __slots__ = ("type", "code", "value", "timestamp")

    def __init__(self, type_: int, code: int, value: int) -> None:
        self.type = type_
        self.code = code
        self.value = value
        self.timestamp = 0.0


class Source:
    """A hidraw controller, wearing the shape of an evdev.InputDevice.

    Only the surface `virtual.create` and `Republisher` actually use. Reports
    are diffed against the previous one so `read()` yields *changes*, which is
    what an evdev node would have delivered.
    """

    def __init__(self, pad: Pad, node: str) -> None:
        self.pad = pad
        self.path = node
        self.name = pad.name
        self._fd = os.open(node, os.O_RDWR | os.O_NONBLOCK)
        # Which device this node was when we opened it. See alive().
        try:
            self._rdev: int | None = os.stat(node).st_rdev
        except OSError:
            self._rdev = None
        self._buttons: dict[int, int] = {}
        self._axes: dict[int, int] = {}
        self._hat = (0, 0)
        self._counter = 0
        self.info = evdev.device.DeviceInfo(
            bustype=0x05, vendor=pad.vid, product=pad.pid, version=0x8001)
        self._request_full_mode()

    # -- the evdev surface ------------------------------------------------
    @property
    def fd(self) -> int:
        return self._fd

    def fileno(self) -> int:
        return self._fd

    def alive(self) -> bool:
        """Is this still the device we opened, or has the pad reconnected?

        Has to be asked from outside, on a timer. A Bluetooth pad that drops
        and comes back gets a new uhid instance and usually a new hidraw
        number -- and the descriptor we are holding does not error, does not
        close, and is never readable again. select() cannot report it, so the
        republisher's read path never runs and never gets the chance to
        notice. Nothing anywhere fails; input simply stops.

        Observed exactly that: the daemon held /dev/hidraw9 (deleted) while
        the controller had come back as hidraw8, and went on reporting that it
        was forwarding, because that line is logged once.

        Comparing st_rdev rather than just existence, so a node number reused
        by some *other* device is caught too.
        """
        if self._rdev is None:
            return True         # nothing to compare; assume the best
        try:
            return os.stat(self.path).st_rdev == self._rdev
        except OSError:
            return False

    def grab(self) -> None:
        """No-op, and not an oversight.

        EVIOCGRAB is an evdev ioctl; hidraw has no equivalent. Exclusivity
        here is by convention -- whoever opens the node and drives the report
        mode owns the device -- which is exactly why Steam and the kernel
        driver fought over this pad in the first place.
        """
        return None

    def ungrab(self) -> None:
        return None

    def close(self) -> None:
        try:
            os.close(self._fd)
        except OSError:
            pass

    def capabilities(self, absinfo: bool = True, **_kw) -> dict[int, Any]:
        """Capabilities, defaulting to *with* absinfo -- as evdev does.

        evdev.InputDevice.capabilities() has absinfo=True by default, and
        `_capabilities_for` calls it with no arguments. Defaulting to False
        here meant the clone was created from bare axis codes with no ranges,
        so every axis came out min=max=0. Nothing complained: the pad appeared,
        was configured, and forwarded input.

        RetroArch then divided by that range in udev_compute_axis:

            int range = info->maximum - info->minimum;
            int axis  = (value - info->minimum) * 0xffff / range - 0x7fff;

        which is a divide by zero the moment anything polls the pad. The game
        died with SIGFPE at its first input poll -- on the start screen -- and
        the backtrace pointed at udev_joypad_poll, three frames under
        GCPad::GetInput. Nothing in it named padmap.
        """
        keys = sorted(set(BUTTONS_RIGHT.values())
                      | set(BUTTONS_SHARED.values())
                      | set(BUTTONS_LEFT.values()))
        if absinfo:
            axes: list[Any] = [
                (code, AbsInfo(value=(STICK_MIN + STICK_MAX) // 2,
                               min=STICK_MIN, max=STICK_MAX,
                               fuzz=STICK_FUZZ, flat=STICK_FLAT, resolution=0))
                for code, _ in AXES
            ]
            axes += [
                (ecodes.ABS_HAT0X, AbsInfo(0, -1, 1, 0, 0, 0)),
                (ecodes.ABS_HAT0Y, AbsInfo(0, -1, 1, 0, 0, 0)),
            ]
        else:
            axes = [code for code, _ in AXES] + [ecodes.ABS_HAT0X,
                                                 ecodes.ABS_HAT0Y]
        return {ecodes.EV_KEY: keys, ecodes.EV_ABS: axes}

    def active_keys(self) -> list[int]:
        return [code for code, value in self._buttons.items() if value]

    def read(self):
        """Every change since the last call, as evdev events.

        Raises BlockingIOError when there is nothing, and OSError(ENODEV) when
        the pad has gone -- the two things `Republisher._forward` already knows
        how to handle.
        """
        out: list[_Event] = []
        got = False
        while True:
            try:
                data = os.read(self._fd, 362)
            except BlockingIOError:
                break
            except OSError:
                raise
            if not data:
                break
            got = True
            if data[0] == REPORT_FULL and len(data) >= 12:
                out.extend(self._decode(data))
        if not got and not out:
            raise BlockingIOError(11, "Resource temporarily unavailable")
        if out:
            out.append(_Event(ecodes.EV_SYN, ecodes.SYN_REPORT, 0))
        return out

    # -- the controller ---------------------------------------------------
    def _request_full_mode(self) -> None:
        """Ask for report 0x30, which is the one carrying sticks and buttons.

        Without this the pad sends 0x3f, a cut-down report with no analogue
        data at all -- and nothing says so except the missing sticks.
        """
        packet = bytearray(64)
        packet[0] = 0x01
        packet[1] = self._counter & 0x0F
        packet[2:10] = RUMBLE_NEUTRAL
        packet[10] = SUBCMD_REPORT_MODE
        packet[11] = REPORT_FULL
        self._counter += 1
        try:
            os.write(self._fd, bytes(packet))
        except OSError as error:
            log.warning("%s: could not request full report mode: %s",
                        self.pad.name, error)

    def _decode(self, data: bytes) -> list[_Event]:
        events: list[_Event] = []

        for byte, table in ((data[3], BUTTONS_RIGHT),
                            (data[4], BUTTONS_SHARED),
                            (data[5], BUTTONS_LEFT)):
            for mask, code in table.items():
                value = 1 if byte & mask else 0
                if self._buttons.get(code, 0) != value:
                    self._buttons[code] = value
                    events.append(_Event(ecodes.EV_KEY, code, value))

        # The d-pad arrives as four independent bits; a hat is two signed
        # axes. Opposite bits held at once cancel, which is what the hardware
        # cannot physically do but a stuck bit can.
        left = data[5]
        x = (1 if left & 0x04 else 0) - (1 if left & 0x08 else 0)
        y = (1 if left & 0x01 else 0) - (1 if left & 0x02 else 0)
        if (x, y) != self._hat:
            if x != self._hat[0]:
                events.append(_Event(ecodes.EV_ABS, ecodes.ABS_HAT0X, x))
            if y != self._hat[1]:
                events.append(_Event(ecodes.EV_ABS, ecodes.ABS_HAT0Y, y))
            self._hat = (x, y)

        # Y is inverted on the way out, X is not.
        #
        # The controller reports Y increasing *upwards*: push the stick up and
        # the number grows. evdev is the other way round -- ABS_Y is 0 at the
        # top, like screen coordinates -- and every consumer downstream assumes
        # the evdev convention. Published raw, the stick works but is upside
        # down in every game, which is exactly how it was reported.
        #
        # hid-nintendo does the same flip, which is the other reason to do it
        # here: a profile captured over USB through the kernel driver has to
        # mean the same thing when the pad comes back over Bluetooth.
        values = {
            "lx": data[6] | ((data[7] & 0x0F) << 8),
            "ly": STICK_MAX - ((data[7] >> 4) | (data[8] << 4)),
            "rx": data[9] | ((data[10] & 0x0F) << 8),
            "ry": STICK_MAX - ((data[10] >> 4) | (data[11] << 4)),
        }
        for code, key in AXES:
            value = values[key]
            # Only past the fuzz: these jitter by a few counts every report,
            # and forwarding that is a wake-up per axis per report for a pad
            # sitting still on a table.
            if abs(self._axes.get(code, value) - value) >= STICK_FUZZ \
                    or code not in self._axes:
                self._axes[code] = value
                events.append(_Event(ecodes.EV_ABS, code, value))
        return events


def open_source(pad: Pad) -> Source | None:
    """A hidraw source for this pad, or None to use its evdev node."""
    if not supported(pad):
        return None
    node = node_for(pad)
    if node is None:
        log.warning("%s looks like a hidraw pad but has no hidraw node",
                    pad.name)
        return None
    try:
        source = Source(pad, node)
    except OSError as error:
        # Permissions, or something else already holding it. Falling back is
        # right: the evdev path may be dead, but failing to republish at all
        # is worse than republishing something silent.
        log.warning("%s: cannot open %s (%s); falling back to evdev",
                    pad.name, node, error)
        return None
    log.info("%s: reading over %s (hidraw), not its evdev node",
             pad.name, node)
    return source
