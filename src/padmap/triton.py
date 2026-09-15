"""The 2026 Steam Controller, over hidraw.

The puck (`28de:1304`) is a four-slot wireless receiver. Every slot boots in
"lizard mode" -- pretending to be a mouse and a keyboard -- and puts real
gamepad state on a vendor-defined HID collection that no generic driver reads.
The kernel's `hid-steam` learned these ids in Linux 7.3; on anything older the
machine has eight keyboards and mice and no joypad, and nothing reports an
error.

None of which matters, because the protocol is published. This is a port of
SDL's `src/joystick/hidapi/SDL_hidapi_steam_triton.c` (zlib, upstream since
2025-11-12) and its two headers, `steam/controller_constants.h` and
`steam/controller_structs.h`. Every constant below was read off that source
rather than guessed, and `docs/STEAM-CONTROLLER-SDL3.md` records how it was
found.

Three things about it that are not obvious:

* **Leaving lizard mode is a *feature* report**, not a write -- see
  `_request_full_mode`.
* **It has to be re-sent every three seconds**, forever. The controller
  reverts on its own. Sent once, a pad works and then stops, which looks like
  failing hardware.
* **The dock is not a slot.** The receiver's fifth interface is the pogo-pin
  dock, usage `FF000002`, and it sends nothing. Pointing a capture tool at it
  reports "nothing arrived", correctly, and reads exactly like a pad that is
  asleep. That mistake is already recorded in `FINDINGS.md`.
"""

from __future__ import annotations

import errno
import fcntl
import glob
import logging
import os
import struct
import time
from pathlib import Path
from typing import Any

import evdev
from evdev import AbsInfo, ecodes

from . import devices, hidraw, safeio
from .devices import Pad
from .hidraw import _Event

log = logging.getLogger("padmap.triton")

VALVE = 0x28DE

# What padmap calls one of these. SDL's name for it, so a mapping captured
# through padmap and one captured against SDL agree about what the controller
# is.
NAME = "Steam Controller"
# The models whose gamepad state lives behind this protocol. 1304 is the puck;
# the others are the controller itself, wired and over Bluetooth, and a Steam
# Machine's built-in receiver. Named in mainline's hid-ids.h as IBEX, IBEX_BLE,
# PROTEUS and NEREID.
PRODUCTS = (0x1302, 0x1303, 0x1304, 0x1305)

# controller_structs.h:26
HID_FEATURE_REPORT_BYTES = 64
# controller_constants.h:64
ID_SET_SETTINGS_VALUES = 0x87
# SETTING_LIZARD_MODE is the tenth entry of an enum starting at
# SETTING_MOUSE_SENSITIVITY = 0 (controller_constants.h:423-432).
SETTING_LIZARD_MODE = 9
# LizardModeState_t's first entry (controller_constants.h:416).
LIZARD_MODE_OFF = 0
# sizeof(ControllerSetting) under #pragma pack(1): u8 + u16.
CONTROLLER_SETTING_BYTES = 3

# ETritonReportIDTypes, controller_structs.h:552.
REPORT_STATE = 0x42
REPORT_BATTERY = 0x43
REPORT_STATE_BLE = 0x45
REPORT_WIRELESS_X = 0x46
REPORT_STATE_TIMESTAMP = 0x47
REPORT_WIRELESS = 0x79
# All three state reports open with the same 29 bytes and differ only in the
# IMU block that follows, which is why SDL parses 0x42 through the same
# TritonMTUNoQuat_t prefix it uses for 0x45.
STATE_REPORTS = (REPORT_STATE, REPORT_STATE_BLE, REPORT_STATE_TIMESTAMP)

# ETritonWirelessState, controller_structs.h:563.
WIRELESS_DISCONNECT = 1
WIRELESS_CONNECT = 2

# How often the controller has to be told again. SDL_hidapi_steam_triton.c:496.
LIZARD_RESEND_SECONDS = 3.0

# TritonButtons, SDL_hidapi_steam_triton.c:58.
BTN_A = 0x00000001
BTN_B = 0x00000002
BTN_X = 0x00000004
BTN_Y = 0x00000008
BTN_QAM = 0x00000010
BTN_R3 = 0x00000020
BTN_VIEW = 0x00000040
BTN_R4 = 0x00000080
BTN_R5 = 0x00000100
BTN_R = 0x00000200
BTN_DPAD_DOWN = 0x00000400
BTN_DPAD_RIGHT = 0x00000800
BTN_DPAD_LEFT = 0x00001000
BTN_DPAD_UP = 0x00002000
BTN_MENU = 0x00004000
BTN_L3 = 0x00008000
BTN_STEAM = 0x00010000
BTN_L4 = 0x00020000
BTN_L5 = 0x00040000
BTN_L = 0x00080000
RIGHT_STICK_TOUCH = 0x00100000
RIGHT_PAD_TOUCH = 0x00200000
RIGHT_PAD_CLICK = 0x00400000
RIGHT_TRIGGER_CLICK = 0x00800000
LEFT_STICK_TOUCH = 0x01000000
LEFT_PAD_TOUCH = 0x02000000
LEFT_PAD_CLICK = 0x04000000
LEFT_TRIGGER_CLICK = 0x08000000
RIGHT_GRIP_TOUCH = 0x10000000
LEFT_GRIP_TOUCH = 0x20000000

# Triton bit -> evdev code.
#
# The face buttons follow SDL's own mapping (A->SOUTH, B->EAST, X->WEST,
# Y->NORTH) rather than the labels, because the labels are laid out
# Nintendo-style and a pad that reports "A" where every other pad on the
# machine reports SOUTH is a pad every stored mapping is wrong for.
#
# MENU is BTN_SELECT and VIEW is BTN_START, which reads backwards and is what
# SDL does (BUTTON_BACK and BUTTON_START respectively). Following SDL matters
# more than the names here: a mapping captured against SDL's view of this pad
# has to agree with padmap's.
BUTTONS = {
    BTN_A: ecodes.BTN_SOUTH,
    BTN_B: ecodes.BTN_EAST,
    BTN_X: ecodes.BTN_WEST,
    BTN_Y: ecodes.BTN_NORTH,
    BTN_L: ecodes.BTN_TL,
    BTN_R: ecodes.BTN_TR,
    BTN_MENU: ecodes.BTN_SELECT,
    BTN_VIEW: ecodes.BTN_START,
    BTN_STEAM: ecodes.BTN_MODE,
    BTN_L3: ecodes.BTN_THUMBL,
    BTN_R3: ecodes.BTN_THUMBR,
    LEFT_TRIGGER_CLICK: ecodes.BTN_TL2,
    RIGHT_TRIGGER_CLICK: ecodes.BTN_TR2,
    # The four paddles and the two trackpad clicks have no standard code, so
    # they take the BTN_TRIGGER_HAPPY block the kernel reserves for exactly
    # this. A stored mapping keys on the code, so what matters is that they
    # never move.
    BTN_L4: ecodes.BTN_TRIGGER_HAPPY1,
    BTN_R4: ecodes.BTN_TRIGGER_HAPPY2,
    BTN_L5: ecodes.BTN_TRIGGER_HAPPY3,
    BTN_R5: ecodes.BTN_TRIGGER_HAPPY4,
    BTN_QAM: ecodes.BTN_TRIGGER_HAPPY5,
    LEFT_PAD_CLICK: ecodes.BTN_TRIGGER_HAPPY6,
    RIGHT_PAD_CLICK: ecodes.BTN_TRIGGER_HAPPY7,
}

# Deliberately not published. They are capacitive-touch bits, not presses, and
# a finger resting on a stick is not a button: bound by the mapping wizard
# they would fire constantly. SDL sends them as capsense, which evdev has no
# equivalent for.
TOUCH_ONLY = (RIGHT_STICK_TOUCH | LEFT_STICK_TOUCH | RIGHT_PAD_TOUCH
              | LEFT_PAD_TOUCH | RIGHT_GRIP_TOUCH | LEFT_GRIP_TOUCH)

STICK_MIN, STICK_MAX = -32768, 32767
TRIGGER_MIN, TRIGGER_MAX = 0, 32767
STICK_FUZZ, STICK_FLAT = 16, 1024

# Offsets into a state report's payload -- everything after the report id.
# TritonMTUNoQuat_t, controller_structs.h:631, under #pragma pack(1).
OFF_SEQ = 0        # u8
OFF_BUTTONS = 1    # u32
OFF_TRIGGER_L = 5  # i16
OFF_TRIGGER_R = 7  # i16
OFF_STICK_LX = 9   # i16
OFF_STICK_LY = 11  # i16
OFF_STICK_RX = 13  # i16
OFF_STICK_RY = 15  # i16
# The trackpads and pressures run to offset 29, where the IMU begins. Not read:
# padmap republishes to a virtual gamepad, and there is no gamepad axis a
# trackpad belongs on.
STATE_PREFIX_BYTES = 17


def HIDIOCSFEATURE(length: int) -> int:
    """`_IOC(_IOC_READ|_IOC_WRITE, 'H', 0x06, length)`.

    Spelled out rather than imported because Python has no ioctl macros: dir
    `_IOC_READ|_IOC_WRITE` is 3 at bit 30, type 'H' is 0x48 at bit 8, number
    is 6, and the length sits at bit 16.
    """
    # Masked rather than or-ed in. The size field is 14 bits, so a length of
    # 16384 or more would run into the direction bits and quietly request a
    # different ioctl. Nothing derives a length today -- both callers pass the
    # constant 64 -- and this is what keeps that true.
    if not 0 < length <= 0x3FFF:
        raise ValueError(f"feature report length {length} is out of range")
    return 0xC0004806 | ((length & 0x3FFF) << 16)


def lizard_off_packet() -> bytes:
    """The 64 bytes that turn lizard mode off.

        01 87 03 09 00 00  then 58 zeros

    Byte for byte: report id 1, then a `FeatureReportMsg` whose header is
    `{type = ID_SET_SETTINGS_VALUES, length = sizeof(ControllerSetting)}` and
    whose single setting is `{SETTING_LIZARD_MODE, LIZARD_MODE_OFF}`.
    """
    packet = bytearray(HID_FEATURE_REPORT_BYTES)
    packet[0] = 0x01
    packet[1] = ID_SET_SETTINGS_VALUES
    packet[2] = CONTROLLER_SETTING_BYTES
    packet[3] = SETTING_LIZARD_MODE
    struct.pack_into("<H", packet, 4, LIZARD_MODE_OFF)
    return bytes(packet)


def decode_state(payload: bytes) -> dict[str, Any]:
    """One state report's gamepad fields, or {} if it is too short.

    Shared by 0x42, 0x45 and 0x47: all three open with the same 29 bytes and
    differ only in the IMU block after them, which is why SDL reads 0x42
    through the same prefix it uses for 0x45.
    """
    if len(payload) < STATE_PREFIX_BYTES:
        return {}
    (buttons, trigger_l, trigger_r,
     stick_lx, stick_ly, stick_rx, stick_ry) = struct.unpack_from(
        "<Ihhhhhh", payload, OFF_BUTTONS)
    return {
        "seq": payload[OFF_SEQ],
        "buttons": buttons,
        "trigger_left": trigger_l,
        "trigger_right": trigger_r,
        "left_x": stick_lx,
        "left_y": stick_ly,
        "right_x": stick_rx,
        "right_y": stick_ry,
    }


def hat_for(buttons: int) -> tuple[int, int]:
    """The d-pad as (ABS_HAT0X, ABS_HAT0Y) values.

    Opposite directions held together cancel, which is what a real hat does --
    it cannot report both, and a pad that reported +1 for left-and-right would
    have a game walking in one direction while the user holds neither.
    """
    x = (1 if buttons & BTN_DPAD_RIGHT else 0) - (
        1 if buttons & BTN_DPAD_LEFT else 0)
    y = (1 if buttons & BTN_DPAD_DOWN else 0) - (
        1 if buttons & BTN_DPAD_UP else 0)
    return x, y


# The dock, which mainline ignores by exactly this value:
#
#     /* The puck's pogo pin interface should be ignored as it's stripped
#      * down. It has one collection with usage page FF00 with usage ID 2. */
#     return hdev->collection[0].usage != 0xFF000002;
DOCK_USAGE = 0xFF000002


def _vendor_collections(descriptor: bytes) -> list[int]:
    """Top-level application collections, as `(page << 16) | usage`.

    Walked as HID items rather than matched on the first few bytes. On a slot
    interface the vendor collection is the *third* -- the descriptor opens with
    an emulated mouse and then an emulated keyboard -- so looking only at the
    start finds the dock and misses every slot. That exact mistake is recorded
    in FINDINGS.md; the Rust side learned it too, in `lizard.rs`.
    """
    found: list[int] = []
    page = usage = 0
    have_usage = False
    depth = at = 0
    while at < len(descriptor):
        prefix = descriptor[at]
        at += 1
        if prefix == 0xFE:                       # long item, carries its size
            if at >= len(descriptor):
                break
            at += 2 + descriptor[at]
            continue
        size = 4 if (prefix & 0x03) == 3 else prefix & 0x03
        if at + size > len(descriptor):
            break
        data = int.from_bytes(descriptor[at:at + size], "little")
        at += size
        tag = prefix & 0xFC
        if tag == 0x04:                          # Global, Usage Page
            page = data & 0xFFFF
        elif tag == 0x08:                        # Local, Usage
            usage = data if size == 4 else (page << 16) | (data & 0xFFFF)
            have_usage = True
        elif tag == 0xA0:                        # Main, Collection
            if depth == 0 and data == 0x01 and have_usage:
                found.append(usage)
            depth += 1
            have_usage = False
        elif tag == 0xC0:                        # Main, End Collection
            depth = max(0, depth - 1)
            have_usage = False
        elif tag in (0x80, 0x90, 0xB0):          # any other main item
            have_usage = False
    return found


def slot_is_live(node: str) -> bool:
    """Is a controller actually paired into this slot?

    Asked by sending the lizard-mode request and seeing whether the device
    takes it. An empty slot stalls the control transfer -- EPIPE -- because
    there is nothing on the other end of the radio link to configure. A slot
    with a controller in it accepts the same bytes.

    This matters more than it sounds. The receiver publishes four slots
    whether or not anything is paired, so without a probe padmap discovers
    four controllers, opens four clones, and offers the user four players for
    one physical pad. Three of them would never send an event, which is
    indistinguishable from three broken controllers.

    Cheap enough to do during discovery: an open, an ioctl and a close per
    slot, against a device that is already enumerated. The scan it sits
    beside spawns a subprocess.

    Any error *other* than a stall leaves the slot reported. A permission
    problem or a node held exclusively is not evidence about whether a
    controller is paired, and guessing "empty" would hide a working pad.
    """
    try:
        fd = os.open(node, os.O_RDWR | os.O_NONBLOCK)
    except OSError:
        return False
    try:
        fcntl.ioctl(fd, HIDIOCSFEATURE(HID_FEATURE_REPORT_BYTES),
                    lizard_off_packet())
        return True
    except OSError as error:
        return error.errno != errno.EPIPE
    finally:
        os.close(fd)


def slots(probe: bool = True) -> list[Pad]:
    """Every Triton slot on the machine, as pads padmap can open.

    Synthesised rather than discovered through `/sys/class/input`, because
    there is nothing there to discover: with no kernel driver the receiver
    publishes a mouse and a keyboard per slot and no joypad at all. The whole
    of `devices.discover` starts from a joypad evdev node, so without this the
    controller is not merely unmapped -- it is not a pad in the first place.

    `path` is the hidraw node. Unusual for a Pad, and honest: it is the device
    padmap opens, and the thing that makes two slots of one receiver different
    from each other.

    `probe=False` reports every slot the receiver has, paired or not, which is
    what a diagnostic wants and what a player list must not have.
    """
    found: list[Pad] = []
    for hid_dir in sorted(glob.glob("/sys/bus/hid/devices/*")):
        uevent = _uevent(Path(hid_dir))
        ids = uevent.get("HID_ID", "")
        parts = ids.split(":")
        if len(parts) != 3:
            continue
        try:
            vid, pid = int(parts[1], 16), int(parts[2], 16)
        except ValueError:
            continue
        if vid != VALVE or pid not in PRODUCTS:
            continue
        # A kernel driver that knows this device is a better answer than this
        # module, and on Linux 7.3 there will be one. Leaving the device alone
        # when hid-steam has it is what stops padmap fighting the kernel for
        # the same reports.
        if uevent.get("DRIVER", "") not in ("", "hid-generic"):
            continue
        try:
            descriptor = (Path(hid_dir) / "report_descriptor").read_bytes()
        except OSError:
            continue
        collections = _vendor_collections(descriptor)
        if not collections or collections[0] == DOCK_USAGE:
            continue
        nodes = sorted(
            os.path.basename(p)
            for p in glob.glob(os.path.join(hid_dir, "hidraw", "hidraw*")))
        if not nodes:
            continue
        node = f"/dev/{nodes[0]}"
        # PADMAP_ONLY_DEVICE means "do not touch the machine's real
        # controllers": it exists so an isolated test daemon cannot fight the
        # live one for a pad. Filtering the returned list is not enough,
        # because the probe below is a *write* to real hardware and happens
        # first -- and `server.py` calls this directly, so it does not even
        # reach `devices.discover`'s filter.
        only = os.environ.get(devices.ENV_ONLY)
        if only and only not in NAME:
            continue
        if probe and not slot_is_live(node):
            continue
        found.append(Pad(
            path=node,
            # SDL's name for it, so a mapping captured here and one captured
            # against SDL agree about what the controller is called.
            name=NAME,
            phys=uevent.get("HID_PHYS", ""),
            # The receiver's serial is the same on every slot, so on its own
            # it cannot tell slot 1 from slot 2 -- and `ambiguous_groups`
            # exists because padmap has been bitten by exactly that. The
            # interface number is what separates them.
            uniq=f"{uevent.get('HID_UNIQ', '')}/{os.path.basename(hid_dir)}",
            vid=vid, pid=pid, syspath=hid_dir,
            # Nothing else on the machine can see this as a joypad, because
            # there is no joypad node for it to see. Saying so keeps it out of
            # RetroArch's pad-index arithmetic, where it would shift every
            # other player by one.
            retroarch_visible=False,
        ))
    return found


def _uevent(path: Path) -> dict[str, str]:
    """A sysfs uevent file as a dict, empty if it cannot be read.

    safeio, not `Path.read_text`: HID_NAME is the device's own name string,
    copied into uevent verbatim by the kernel, and for a Bluetooth pad that is
    whatever the peer sent. One byte of it that is not UTF-8 raises
    UnicodeDecodeError -- a ValueError, which `except OSError` does not catch
    -- and this is reached from the daemon's tick for every HID device on the
    machine. Reproduced with a name of `Pad\xff\xfe`.
    """
    out: dict[str, str] = {}
    for line in (safeio.read_text(path / "uevent") or "").splitlines():
        key, _, value = line.partition("=")
        if value:
            out[key] = value
    return out


def owns(pad: Pad) -> bool:
    """Is this one of ours? Cheap, and asked before anything opens anything."""
    return (pad.vid == VALVE and pad.pid in PRODUCTS
            and pad.path.startswith("/dev/hidraw"))


class Source(hidraw.Source):
    """A Triton slot, wearing the shape of an `evdev.InputDevice`.

    Subclasses the Switch Pro source for the parts that are genuinely about
    hidraw rather than about any controller -- the descriptor, `alive`, grab,
    close, `read_one` -- and replaces everything protocol-shaped. Sharing the
    generic half matters because the ways a hidraw pad goes wrong (the node
    replaced under us on reconnect, a vanished device that reports readable
    forever) were each found the hard way, and a second copy would have to
    find them again.
    """

    def __init__(self, pad: Pad, node: str) -> None:
        self.pad = pad
        self.path = node
        self.name = pad.name
        self._fd = os.open(node, os.O_RDWR | os.O_NONBLOCK)
        try:
            self._rdev: int | None = os.stat(node).st_rdev
        except OSError:
            self._rdev = None
        self._buttons: dict[int, int] = {}
        self._axes: dict[int, int] = {}
        self._hat = (0, 0)
        self._pending: list[_Event] = []
        # Unused here, but `hidraw.Source`'s inherited methods read them.
        self._counter = 0
        self._simple_seen = 0
        self._last_lizard = 0.0
        # Whether a controller is actually paired into this slot. A receiver
        # slot with nothing in it opens cleanly and reads nothing forever, so
        # this is the difference between "no controller" and "broken".
        self.connected = False
        self.info = evdev.device.DeviceInfo(
            bustype=0x03, vendor=pad.vid, product=pad.pid, version=0x0111)
        self._request_full_mode()

    def _request_full_mode(self) -> None:
        """Turn lizard mode off. A feature report, not a write.

        `os.write` on this node would be an *output* report -- a different
        channel that this message does not travel on. It does not fail
        loudly; the controller simply stays a keyboard.
        """
        try:
            fcntl.ioctl(self._fd, HIDIOCSFEATURE(HID_FEATURE_REPORT_BYTES),
                        lizard_off_packet())
        except OSError as error:
            # Not fatal, and worth a line rather than an exception: a slot
            # with no controller paired into it stalls the transfer (EPIPE),
            # which is not a fault -- there is nothing there to configure.
            log.debug("%s: lizard-mode request refused: %s", self.path, error)
            return
        self._last_lizard = time.monotonic()

    def _keepalive(self) -> None:
        """Ask again, every three seconds, forever.

        The controller reverts to lizard mode on its own. SDL re-sends on the
        same schedule and for the same reason. Sent once and never again, a
        pad works for a few seconds and then turns back into a keyboard
        mid-game, which reads as failing hardware rather than as a missing
        message.
        """
        if time.monotonic() - self._last_lizard >= LIZARD_RESEND_SECONDS:
            self._request_full_mode()

    def read(self):
        """Every change since the last call, as evdev events."""
        self._keepalive()
        out: list[_Event] = []
        if self._pending:
            out.extend(self._pending)
            self._pending.clear()
        got = False
        while True:
            try:
                data = os.read(self._fd, HID_FEATURE_REPORT_BYTES)
            except BlockingIOError:
                break
            if not data:
                break
            got = True
            report, payload = data[0], data[1:]
            if report in STATE_REPORTS:
                if not self.connected:
                    self._note_connected(True)
                out.extend(self._decode_state(payload))
            elif report in (REPORT_WIRELESS, REPORT_WIRELESS_X) and payload:
                self._note_connected(payload[0] == WIRELESS_CONNECT)
        if not got and not out:
            raise BlockingIOError(11, "Resource temporarily unavailable")
        if out:
            out.append(_Event(ecodes.EV_SYN, ecodes.SYN_REPORT, 0))
        return out

    def _note_connected(self, connected: bool) -> None:
        if connected == self.connected:
            return
        self.connected = connected
        log.info("%s: controller %s", self.path,
                 "paired" if connected else "disconnected")
        if connected:
            # A pad that has just woken is in lizard mode again regardless of
            # what was sent to the empty slot before it.
            self._request_full_mode()
            return
        # Everything held goes up. A controller that vanishes mid-press
        # otherwise leaves a button down on the clone with nothing to lift
        # it, which is the stuck-input shape that made exiting a game launch
        # another one.
        for code, value in list(self._buttons.items()):
            if value:
                self._pending.append(_Event(ecodes.EV_KEY, code, 0))
        self._buttons.clear()

    def _decode_state(self, payload: bytes) -> list[_Event]:
        state = decode_state(payload)
        if not state:
            return []
        events: list[_Event] = []
        buttons = state["buttons"]

        for bit, code in BUTTONS.items():
            value = 1 if buttons & bit else 0
            # Default 0, as hidraw.Source does. Without it the first report of
            # a session finds an empty dict, `None != 0` for every button that
            # is *not* pressed, and the frame carries a key-up for all twenty
            # of them. Harmless downstream -- the input core drops a key event
            # repeating the value it already holds -- but it is twenty events
            # per connection that describe nothing, and it made the first
            # frame indistinguishable from a controller releasing everything.
            if self._buttons.get(code, 0) != value:
                self._buttons[code] = value
                events.append(_Event(ecodes.EV_KEY, code, value))

        hat = hat_for(buttons)
        if hat != self._hat:
            if hat[0] != self._hat[0]:
                events.append(_Event(ecodes.EV_ABS, ecodes.ABS_HAT0X, hat[0]))
            if hat[1] != self._hat[1]:
                events.append(_Event(ecodes.EV_ABS, ecodes.ABS_HAT0Y, hat[1]))
            self._hat = hat

        # Y is negated for the same reason SDL negates it: the controller
        # reports up as positive and evdev's convention is down-positive. A
        # pad published the other way up is not obviously wrong on a menu and
        # is completely wrong in a game.
        for code, value in (
            (ecodes.ABS_X, state["left_x"]),
            (ecodes.ABS_Y, -state["left_y"]),
            (ecodes.ABS_RX, state["right_x"]),
            (ecodes.ABS_RY, -state["right_y"]),
            (ecodes.ABS_Z, state["trigger_left"]),
            (ecodes.ABS_RZ, state["trigger_right"]),
        ):
            # -32768 has no positive counterpart in an i16, so negating it
            # returns itself. Clamped rather than left to wrap.
            value = max(STICK_MIN, min(STICK_MAX, value))
            if self._axes.get(code) != value:
                self._axes[code] = value
                events.append(_Event(ecodes.EV_ABS, code, value))
        return events

    def capabilities(self, absinfo: bool = True, **_kw) -> dict[int, Any]:
        """What the clone is built from.

        With ranges, always. A clone built from bare axis codes gets
        min=max=0, and RetroArch divides by that range on its first input
        poll -- SIGFPE on the start screen, with nothing in the backtrace
        naming padmap. That is written up at length in `hidraw.Source`.
        """
        keys = sorted(set(BUTTONS.values()))
        if absinfo:
            axes: list[Any] = [
                (code, AbsInfo(value=0, min=STICK_MIN, max=STICK_MAX,
                               fuzz=STICK_FUZZ, flat=STICK_FLAT, resolution=0))
                for code in (ecodes.ABS_X, ecodes.ABS_Y,
                             ecodes.ABS_RX, ecodes.ABS_RY)
            ]
            # Triggers rest at zero on a 0..32767 range, which is what every
            # analogue trigger on Linux reports and what RetroArch's trigger
            # handling expects. Publishing them on the stick range instead
            # would have them read as half-pulled at rest.
            axes += [
                (code, AbsInfo(value=0, min=TRIGGER_MIN, max=TRIGGER_MAX,
                               fuzz=0, flat=0, resolution=0))
                for code in (ecodes.ABS_Z, ecodes.ABS_RZ)
            ]
            axes += [
                (ecodes.ABS_HAT0X, AbsInfo(0, -1, 1, 0, 0, 0)),
                (ecodes.ABS_HAT0Y, AbsInfo(0, -1, 1, 0, 0, 0)),
            ]
        else:
            axes = [ecodes.ABS_X, ecodes.ABS_Y, ecodes.ABS_RX, ecodes.ABS_RY,
                    ecodes.ABS_Z, ecodes.ABS_RZ,
                    ecodes.ABS_HAT0X, ecodes.ABS_HAT0Y]
        return {ecodes.EV_KEY: keys, ecodes.EV_ABS: axes}


def open_source(pad: Pad) -> Source | None:
    """A Source for this pad, or None if it is not one of ours."""
    if not owns(pad):
        return None
    try:
        return Source(pad, pad.path)
    except OSError as error:
        log.warning("could not open %s: %s", pad.path, error)
        return None


def live_signature() -> frozenset[str]:
    """Which slots have a controller in them, as something comparable.

    The daemon notices controllers arriving by watching `/dev/input` for a
    node appearing. That works for anything the kernel drives and cannot work
    here: the receiver's hidraw nodes exist from the moment it is plugged in,
    whether or not a controller is paired, and never change afterwards. A pad
    waking up alters nothing a directory listing can see.

    So the daemon asks this instead. It is not cheap -- one open, one ioctl
    and one close per slot, 12.0ms measured on a four-slot receiver, because
    an empty slot stalls the transfer by design -- so the caller is
    responsible for rationing it. `Server._triton_live_slots` does, at
    `TRITON_SCAN_SECONDS`; calling this on a 50Hz loop costs 60% of the tick
    budget and was exactly the bug that made this docstring say otherwise.
    """
    try:
        return frozenset(pad.path for pad in slots())
    except OSError:
        return frozenset()
