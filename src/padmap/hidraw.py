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

# Where the device tree is read from. A module-level constant so the
# resolution below can be pointed at a constructed tree in a check: it decides
# which physical controller a player gets, and it was wrong without anything
# being able to exercise it short of two identical pads physically present.
SYS_CLASS = "/sys/class"

# Controllers this module knows how to speak to. Anything absent falls through
# to the evdev path, which is right for everything padmap grew up on.
SWITCH_PRO = (0x057E, 0x2009)
SUPPORTED = {SWITCH_PRO}

REPORT_FULL = 0x30          # INPUT: standard full mode, sticks and buttons
REPORT_SIMPLE = 0x3F        # INPUT: cut-down mode the pad powers up in

# How many 0x3f reports to tolerate before asking for full mode again.
#
# The request at open is not reliable. It is written the instant the node
# opens, and a pad that has just finished associating over Bluetooth can drop
# it: the write succeeds, the controller never acts on it, and the pad goes on
# sending 0x3f for ever. Nothing downstream notices, because reports *are*
# arriving -- the descriptor is live, `alive()` is true, `read()` never errors
# -- they are simply all discarded by the report-id filter. The pad looks
# connected, padmap looks healthy, and not one button works.
#
# Retrying costs a 64-byte write. At the pad's ~67 reports/second this waits
# roughly a second and a half between attempts, which is long enough not to
# spam a controller that is mid-handshake and short enough that nobody gets
# as far as unpairing it.
SIMPLE_REPORTS_BEFORE_RETRY = 100
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


def _uevent(path: Path) -> dict[str, str]:
    """A sysfs uevent file as a dict, empty if it cannot be read."""
    try:
        text = (path / "uevent").read_text()
    except OSError:
        return {}
    out = {}
    for line in text.splitlines():
        key, _, value = line.partition("=")
        if value:
            out[key] = value
    return out


def _hid_device_dir(pad: Pad) -> Path | None:
    """The HID device that owns this pad's evdev node.

    Walks up from /sys/class/input/eventN/device until it finds the directory
    carrying a `hidraw` subdirectory. That parent-child link is the only thing
    that ties *this* pad to *its* hidraw node; every other property a pad has
    is shared with an identical one plugged in beside it.
    """
    start = (Path(SYS_CLASS) / "input"
             / os.path.basename(pad.path) / "device")
    if not start.exists():
        # No link to follow. Walking up from a path that does not resolve
        # climbs through /sys/class, which *contains* a directory called
        # `hidraw` -- the class directory holding every node on the machine.
        # That satisfies the test below and hands back whichever node sorts
        # first, which is the bug this function exists to fix, reintroduced
        # one level up.
        return None
    try:
        current = Path(os.path.realpath(start))
    except OSError:
        return None
    for _ in range(8):
        # A real HID device, not merely something with a `hidraw` child: the
        # uevent is what makes it a device rather than a class directory.
        if (current / "hidraw").is_dir() and (current / "uevent").is_file():
            return current
        if current.parent == current:
            break
        current = current.parent
    return None


def node_for(pad: Pad) -> str | None:
    """The /dev/hidraw* belonging to this pad, or None.

    Resolved through the pad's own device, because vendor and product do not
    identify a controller -- they identify a *model*. Two Pro Controllers
    report the same 057e:2009, and the previous version of this function
    returned the first hidraw node whose uevent carried those ids, to every
    pad that asked.

    Two of them then read the same physical controller. One player's pad does
    nothing at all, while the log says both are forwarding and both clones
    exist. That is what it looked like on this machine: a second Pro
    Controller paired, and `republisher watching ... [(1, 6, '/dev/hidraw10'),
    (2, 10, '/dev/hidraw10')]` -- the same node twice.

    Sorting made it worse rather than merely arbitrary. `sorted()` on
    "hidraw10" and "hidraw8" is a *string* sort, so hidraw10 wins, and the
    node that gets handed to everybody is whichever one happens to sort first
    -- not even the one that was there before.

    So: the sysfs walk first, which is exact. Then HID_UNIQ, which on
    Bluetooth is the controller's own address and distinguishes two of a
    model. Only then vendor/product, and only when it is unambiguous -- with
    several candidates and no way to tell them apart, returning the wrong
    controller is worse than falling back to evdev, because a pad reading
    somebody else's input cannot be diagnosed from anything it reports.
    """
    hid = _hid_device_dir(pad)
    if hid is not None:
        nodes = sorted(
            entry.name for entry in (hid / "hidraw").iterdir()
            if entry.name.startswith("hidraw")
        )
        if nodes:
            return "/dev/" + nodes[0]

    candidates: list[str] = []
    for path in glob.glob(os.path.join(SYS_CLASS, "hidraw", "hidraw*")):
        props = _uevent(Path(path) / "device")
        parts = props.get("HID_ID", "").split(":")
        if len(parts) != 3:
            continue
        try:
            vendor, product = int(parts[1], 16), int(parts[2], 16)
        except ValueError:
            continue
        if (vendor, product) != (pad.vid, pad.pid):
            continue
        if pad.uniq and props.get("HID_UNIQ", "") == pad.uniq:
            return "/dev/" + os.path.basename(path)
        candidates.append("/dev/" + os.path.basename(path))

    if len(candidates) == 1:
        return candidates[0]
    if candidates:
        log.warning(
            "%s: %d hidraw nodes report %04x:%04x (%s) and none could be tied "
            "to this pad; refusing to guess, falling back to evdev",
            pad.name, len(candidates), pad.vid, pad.pid,
            ", ".join(sorted(candidates)))
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
        # Consecutive 0x3f reports since the last usable one. Reset by a 0x30
        # rather than only counted up, so a pad that drops back into simple
        # mode later in the session is caught the same way as one that never
        # left it.
        self._simple_seen = 0
        # Events read but not yet handed out, for read_one().
        self._pending: list[_Event] = []
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
        if self._pending:
            out.extend(self._pending)
            self._pending.clear()
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
                self._simple_seen = 0
                out.extend(self._decode(data))
            elif data[0] == REPORT_SIMPLE:
                self._note_simple_report()
        if not got and not out:
            raise BlockingIOError(11, "Resource temporarily unavailable")
        if out:
            out.append(_Event(ecodes.EV_SYN, ecodes.SYN_REPORT, 0))
        return out

    def read_one(self):
        """One event, or None when there is nothing.

        Part of the evdev surface because `Assigner._drain` uses it to throw
        away whatever was queued before a session started. Without it a
        hidraw pad cannot be handed to the assigner at all -- which is why
        the setup screen read these pads from their evdev node instead, and
        why a Switch pad could be republished perfectly while being unmappable
        on the screen that maps it.
        """
        if not self._pending:
            try:
                self._pending.extend(self.read())
            except BlockingIOError:
                return None
        if not self._pending:
            return None
        return self._pending.pop(0)

    # -- the controller ---------------------------------------------------
    def _note_simple_report(self) -> None:
        """A 0x3f arrived, which means the pad is not in the mode we asked for.

        Every one of these is discarded by the filter in `read`, so a pad stuck
        here delivers no input at all while looking perfectly healthy from
        every other angle: the node exists, the descriptor is live, reports are
        flowing, nothing raises and nothing is logged. The only visible symptom
        is that no button does anything -- which is indistinguishable from a
        mapping problem, and got diagnosed as one.

        So count them and ask again. The first line is the one that matters:
        it names the actual fault, in the log, at the moment it happens.
        """
        self._simple_seen += 1
        if self._simple_seen == 1:
            log.warning(
                "%s: sending report 0x%02x, not the 0x%02x full mode it was "
                "asked for -- no input can be decoded until it switches; "
                "re-requesting",
                self.pad.name, REPORT_SIMPLE, REPORT_FULL)
        if self._simple_seen % SIMPLE_REPORTS_BEFORE_RETRY == 0:
            log.warning("%s: still in report 0x%02x after %d reports; "
                        "re-requesting full mode",
                        self.pad.name, REPORT_SIMPLE, self._simple_seen)
            self._request_full_mode()

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
