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
import logging
import os
import subprocess
from dataclasses import dataclass

import evdev
from evdev import ecodes

log = logging.getLogger("padmap.devices")

# Virtual pads we publish are tagged with this phys prefix so that discovery
# never picks up our own output. Without it, restarting the daemon would grab
# its own pads and republish them, one layer deeper each time.
VIRTUAL_PHYS_PREFIX = "padmap/"

# ...and by name, for a clone whose phys did not take. Duplicated from
# virtual.VIRTUAL_PREFIX rather than imported: importing virtual here would
# pull in profiles and the whole mapping stack for the sake of one string, and
# `padmap list` is meant to start quickly.
VIRTUAL_NAME_PREFIX = "padmap Player "

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


def _parse_properties(text: str) -> dict[str, str]:
    props: dict[str, str] = {}
    for line in text.splitlines():
        key, sep, value = line.partition("=")
        if sep:
            props[key] = value
    return props


# Properties for the devices `discover` is about to ask about, or None when
# nothing has primed it. See `_ask_udev_about`.
_PROPERTY_CACHE: dict[str, dict[str, str]] | None = None


def _ask_udev_about(devnodes: list[str]) -> dict[str, dict[str, str]] | None:
    """Every device's properties, in one subprocess instead of one each.

    `udevadm info -q property` takes any number of devices, and asking it once
    is the difference between 9ms and 596ms on this machine -- 33 input devices,
    33 process spawns, each paying fork, exec and a dynamic link to answer one
    question. Measured, because the cost is entirely in the spawning and not in
    the query.

    Deliberately still udevadm, and still the same query. The obvious deeper
    fix is to read /run/udev/data directly, and it is not taken here: the thing
    being asked is "does RetroArch's udev driver consider this a joypad", the
    only authority on that is udev's own database as udev presents it, and
    reimplementing the presentation is how the two answers start to differ. One
    process asking the same question is a speed change; parsing the database by
    hand would be a semantic one.

    Blocks are not blank-line separated -- each simply begins with DEVPATH and
    carries a DEVNAME, so DEVNAME is what ties a block back to the device that
    was asked about, rather than argument order.

    Returns None if the call could not be made at all, which puts each device
    back on its own lookup and, failing that, on the joydev fallback below.
    """
    if not devnodes:
        return {}
    try:
        result = subprocess.run(
            ["udevadm", "info", "-q", "property", *devnodes],
            capture_output=True, text=True, timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return None

    out: dict[str, dict[str, str]] = {}
    current: list[str] = []

    def flush() -> None:
        if not current:
            return
        props = _parse_properties("\n".join(current))
        name = props.get("DEVNAME")
        if name:
            out[name] = props

    for line in result.stdout.splitlines():
        if line.startswith("DEVPATH="):
            flush()
            current = []
        current.append(line)
    flush()
    return out or None


def _udev_properties(devnode: str) -> dict[str, str]:
    if _PROPERTY_CACHE is not None:
        # Primed by discover(). A device missing from it was one udevadm had
        # nothing to say about, which is the same empty answer a single lookup
        # would have given.
        return _PROPERTY_CACHE.get(devnode, {})
    try:
        out = subprocess.run(
            ["udevadm", "info", "-q", "property", "-n", devnode],
            capture_output=True, text=True, timeout=5,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return {}
    return _parse_properties(out)


def _retroarch_sees(devnode: str, input_dir: str) -> bool:
    """RetroArch's own filter: ID_INPUT_JOYSTICK=1 (udev_joypad.c:1053)."""
    props = _udev_properties(devnode)
    if props:
        return props.get("ID_INPUT_JOYSTICK") == "1"
    # udevadm unavailable (stripped container): fall back to joydev's verdict.
    return bool(glob.glob(os.path.join(input_dir, "js*")))


def _capability_mask(path: str) -> int | None:
    """A sysfs capability bitmap as one integer, or None if it is not there.

    The file is space-separated 64-bit hex words, most significant first, so
    shifting each word in as it is read reconstructs the mask in the order the
    kernel wrote it.

    None and 0 are different answers and the distinction is load-bearing: a
    device with no absolute axes at all has an `abs` file containing "0", and
    reading that as "the file could not be read" sends it down the fallback
    that opens the device -- which is the expensive path this exists to avoid.
    Twelve of this machine's 33 input devices are exactly that shape.
    """
    raw = _read(path)
    if not raw:
        return None
    value = 0
    for word in raw.split():
        try:
            value = (value << 64) | int(word, 16)
        except ValueError:
            return None
    return value


def _looks_like_joypad(devnode: str) -> bool:
    """Is this a joypad by capability, regardless of how udev tagged it?

    padmap must not use ID_INPUT_JOYSTICK for its own discovery, because
    `padmap hide` deliberately clears that property. Sharing the filter would
    mean installing the hide rules made every controller invisible to padmap
    too, so `padmap setup` could never be run again -- unrecoverable without
    hand-removing the rules.

    Read from sysfs rather than by opening the device, and that is the whole
    cost of a scan. Opening every input node to ask two questions about it
    means closing every input node afterwards, and releasing a USB HID
    descriptor takes about 11ms because the driver tears down its URB:
    measured at 390ms of a 400ms `discover()`, in 36 calls to `posix.close`.
    The bitmaps here are the same ones udev's own `input_id` builtin reads to
    decide ID_INPUT_JOYSTICK, and reading two small files costs microseconds.
    """
    caps = os.path.join(
        "/sys/class/input", os.path.basename(devnode), "device", "capabilities")
    absolute = _capability_mask(os.path.join(caps, "abs"))
    keys = _capability_mask(os.path.join(caps, "key"))
    if absolute is None or keys is None:
        # Not there to read. Fall back to asking the device itself, which is
        # what this used to do always.
        return _looks_like_joypad_by_opening(devnode)
    if not absolute:
        return False
    return any((keys >> code) & 1 for code in _BTN_JOYSTICK_RANGE)


def _looks_like_joypad_by_opening(devnode: str) -> bool:
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
    # Everything with an event node, before asking udev anything. Collecting
    # first is what lets the whole set be asked about in one subprocess rather
    # than one each -- see `_ask_udev_about`.
    candidates: list[tuple[str, list[str], str]] = []
    for input_dir in glob.glob("/sys/class/input/input*"):
        events = [
            os.path.basename(p) for p in glob.glob(os.path.join(input_dir, "event*"))
        ]
        if not events:
            continue
        devnode = f"/dev/input/{events[0]}"
        if not os.path.exists(devnode):
            continue
        candidates.append((input_dir, events, devnode))

    global _PROPERTY_CACHE
    _PROPERTY_CACHE = _ask_udev_about([node for _, _, node in candidates])
    try:
        pads = _collect(candidates, include_virtual, retroarch_only)
    finally:
        # Only ever primed for the length of one scan. Held across calls it
        # would be a cache of which controllers are plugged in, which is the
        # one thing about a controller that changes without warning.
        _PROPERTY_CACHE = None

    # Controllers with no joypad node of their own.
    #
    # Everything above starts from /sys/class/input, which is the right place
    # to look for a pad the kernel is driving. A 2026 Steam Controller is not
    # one: with no hid-steam that knows it, the receiver publishes a mouse and
    # a keyboard per slot and no joypad at all, so it is not merely unmapped
    # -- there is nothing for any of this to find. padmap speaks its protocol
    # directly, so it is a pad here even though the kernel says otherwise.
    #
    # After, not before: these are appended to a list whose order is
    # RetroArch's enumeration order, and they are not in that enumeration at
    # all. Putting one in the middle would shift every pad below it.
    #
    # `retroarch_only` excludes them for the same reason -- RetroArch cannot
    # see a device with no evdev node, and the whole point of that flag is to
    # predict its pad indices.
    if not retroarch_only:
        from . import triton
        try:
            pads.extend(triton.slots())
        except OSError as error:                     # noqa: BLE001
            # A scan that cannot read sysfs must not take the pads that were
            # found with it.
            log.warning("could not scan for Steam Controllers: %s", error)
    # Again, because these arrived after `_collect` applied it. See `_restrict`.
    return _restrict(pads)


def _collect(
    candidates: list[tuple[str, list[str], str]],
    include_virtual: bool,
    retroarch_only: bool,
) -> list[Pad]:
    pads: list[Pad] = []
    for input_dir, events, devnode in candidates:
        visible = _retroarch_sees(devnode, input_dir)
        if not visible and not _looks_like_joypad(devnode):
            continue
        if retroarch_only and not visible:
            continue

        phys = _read(os.path.join(input_dir, "phys"))
        name = _read(os.path.join(input_dir, "name"))
        # Either tag is enough, because either can be absent. The phys prefix
        # is the intended one; the name is the fallback for a clone whose phys
        # could not be set, which is every clone the Rust republisher makes --
        # evdev 0.13.2 encodes UI_SET_PHYS with the wrong payload size and the
        # kernel refuses it. Missing one of the two means discovery grabs
        # padmap's own output and republishes it, one layer deeper on every
        # restart.
        if not include_virtual and (phys.startswith(VIRTUAL_PHYS_PREFIX)
                                    or name.startswith(VIRTUAL_NAME_PREFIX)):
            continue

        pads.append(Pad(
            path=devnode,
            name=name,
            phys=phys,
            uniq=_read(os.path.join(input_dir, "uniq")),
            vid=_read_hex(os.path.join(input_dir, "id/vendor")),
            pid=_read_hex(os.path.join(input_dir, "id/product")),
            syspath=os.path.realpath(os.path.join(input_dir, events[0])),
            retroarch_visible=visible,
        ))

    pads.sort(key=lambda p: p.syspath)

    return _restrict(pads)


def _restrict(pads: list[Pad]) -> list[Pad]:
    """Test escape hatch: restrict discovery to one device by name.

    Without it an isolated test daemon still finds the machine's real
    controllers and fights the live daemon for an exclusive grab on them.

    A function rather than a few lines at the end of `_collect`, because there
    is more than one way into the pad list now: controllers padmap drives
    itself are appended by `discover` after `_collect` has returned, and the
    first version of that appended them *past* this filter. A harness asking
    to see one device got a real Steam Controller anyway, and
    check_hostile_cli refused to run -- correctly, and loudly, which is the
    only reason it was not found by a test grabbing a pad out from under the
    live daemon.
    """
    only = os.environ.get(ENV_ONLY)
    if not only:
        return pads
    return [pad for pad in pads if only in pad.name]


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
