"""Real controllers, reproduced well enough to test against without owning one.

padmap's bugs are mostly not in padmap. They are in the gap between what a
controller actually reports and what the code assumed it would: a trigger that
rests at 81% of its range, a d-pad that is four buttons on one pad and a hat on
the next, a stick that counts upwards where evdev counts down, a controller
with no evdev node at all. Every one of those was found by plugging something
in and being surprised.

This is that surprise, written down. Each class is one real controller model,
with its identity, its capabilities and its event stream taken from a citable
source -- the kernel driver that publishes it, SDL's driver, or padmap's own
`FINDINGS.md` where the measurement was made here. `source` on each class says
which, and nothing in this file is from memory.

Two transports, because padmap has two:

* [`EvdevController`] spawns a real uinput device. `devices.discover()` finds
  it, the daemon republishes it, and a test can drive the whole pipeline.
* [`HidrawController`] emits raw report bytes, because a Steam Controller has
  no evdev node to spawn -- there is nothing for the kernel to publish. Its
  reports go to the decoder in `triton.py` or `hidraw.py` directly.

Both speak one vocabulary of control names, so a test asks every controller to
press "a" and asserts what came out. That is the point of the interface: the
controllers differ in every detail *except* what the user pressed.
"""

from __future__ import annotations

import struct
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any, Iterable

import evdev
from evdev import AbsInfo, ecodes

# The shared vocabulary. A name here means the same thing to a player on every
# pad, which is exactly what the evdev codes below do not.
FACE = ("a", "b", "x", "y")
SHOULDERS = ("l", "r", "l2", "r2")
THUMBS = ("l3", "r3")
MENU = ("start", "select", "home")
DPAD = ("up", "down", "left", "right")
STICKS = ("lx", "ly", "rx", "ry")
TRIGGERS = ("lt", "rt")


@dataclass(frozen=True)
class Axis:
    """One absolute axis, as its driver declares it.

    `rest` is separate from the midpoint and is the whole reason this is a
    class rather than a tuple. An analogue trigger does not rest in the middle
    of its range, and assuming it does broke padmap three ways at once on a
    GameCube adapter: a capture recorded the direction the axis was travelling
    *away* from, a resting report answered a prompt nothing had touched, and
    re-arming waited for a value the axis would never return to. See
    `MayflashGameCube`.
    """

    code: int
    minimum: int
    maximum: int
    rest: int
    fuzz: int = 0
    flat: int = 0
    #: Does the controller count this axis the opposite way from evdev?
    #:
    #: True on the Y axes of both hidraw pads here, and for different reasons:
    #: the Steam Controller reports up as positive on a signed range, so SDL
    #: negates; the Switch Pro reports up as 4095 on 0..4095, so hid-nintendo
    #: subtracts. Declaring *that* it is inverted here, and how in the
    #: controller's own `to_evdev`, keeps the expectation independent of the
    #: decoder that has to perform it.
    inverted: bool = False

    def absinfo(self) -> AbsInfo:
        return AbsInfo(value=self.rest, min=self.minimum, max=self.maximum,
                       fuzz=self.fuzz, flat=self.flat, resolution=0)


@dataclass(frozen=True)
class Event:
    """One evdev event. Compares equal, which is what assertions want."""

    type: int
    code: int
    value: int

    def __repr__(self) -> str:                      # pragma: no cover
        kind = {ecodes.EV_KEY: "KEY", ecodes.EV_ABS: "ABS",
                ecodes.EV_MSC: "MSC", ecodes.EV_SYN: "SYN"}.get(
                    self.type, str(self.type))
        return f"{kind}:{self.code}={self.value}"


SYN = Event(ecodes.EV_SYN, ecodes.SYN_REPORT, 0)


class Controller(ABC):
    """One real controller model.

    Subclasses declare what the hardware is and what it reports; the shared
    behaviour here is only the vocabulary and the bookkeeping that makes a
    sequence of presses into a sequence of frames.
    """

    #: Exactly the string the driver publishes. padmap matches profiles on it,
    #: so "close enough" is a different controller.
    name: str = ""
    vid: int = 0
    pid: int = 0
    bustype: int = ecodes.BUS_USB
    version: int = 0
    #: Where the numbers in this class came from. Not decoration: every one of
    #: them is checkable, and a fixture nobody can check is a guess with a
    #: class around it.
    source: str = ""
    #: Vocabulary name -> evdev key code.
    buttons: dict[str, int] = {}
    #: Vocabulary name -> Axis.
    axes: dict[str, Axis] = {}
    #: Is the d-pad a hat (ABS_HAT0X/Y) rather than four keys?
    dpad_is_hat: bool = True

    def __init__(self) -> None:
        self._held: set[str] = set()
        self._values: dict[str, int] = {
            name: axis.rest for name, axis in self.axes.items()}
        self._hat = (0, 0)

    # -- what this pad can do ---------------------------------------------
    def controls(self) -> set[str]:
        """Every name this controller answers to."""
        names = set(self.buttons) | set(self.axes)
        if self.dpad_is_hat:
            names |= set(DPAD)
        return names

    def has(self, control: str) -> bool:
        return control in self.controls()

    # -- driving it -------------------------------------------------------
    @abstractmethod
    def press(self, control: str) -> list[Event]:
        """Hold `control`. The frame a real pad would send."""

    @abstractmethod
    def release(self, control: str) -> list[Event]:
        """Let go of `control`."""

    @abstractmethod
    def move(self, control: str, value: int) -> list[Event]:
        """Put an axis at `value`, in the units the hardware reports."""

    def tap(self, control: str) -> list[Event]:
        """Press and release, as two frames."""
        return self.press(control) + self.release(control)

    def held(self) -> frozenset[str]:
        return frozenset(self._held)

    def value(self, control: str) -> int:
        return self._values[control]

    def __repr__(self) -> str:                      # pragma: no cover
        return f"<{type(self).__name__} {self.vid:04x}:{self.pid:04x}>"


def _hat_from(held: Iterable[str]) -> tuple[int, int]:
    """Four held directions as a hat.

    Opposites cancel. The hardware cannot report both, and a fixture that did
    would be testing padmap against a pad that does not exist.
    """
    held = set(held)
    x = (1 if "right" in held else 0) - (1 if "left" in held else 0)
    y = (1 if "down" in held else 0) - (1 if "up" in held else 0)
    return x, y


class EvdevController(Controller):
    """A controller the kernel publishes as an evdev joypad."""

    #: Does its driver emit MSC_SCAN before each key?
    #:
    #: hid-generic does -- `hid-input.c:1787` sends `EV_MSC, MSC_SCAN,
    #: usage->hid` ahead of the key -- and xpad, which is not a hid-input
    #: driver, does not. padmap sees both, so both are reproduced: a capture
    #: that mistook the scancode for the press would work on an Xbox pad and
    #: fail on everything driven by hid-generic.
    emits_scan: bool = False
    #: Vocabulary name -> the HID usage its driver puts in MSC_SCAN.
    scancodes: dict[str, int] = {}

    def press(self, control: str) -> list[Event]:
        return self._key(control, 1)

    def release(self, control: str) -> list[Event]:
        return self._key(control, 0)

    def _key(self, control: str, value: int) -> list[Event]:
        if not self.has(control):
            raise KeyError(f"{type(self).__name__} has no {control!r}")
        if (control in self._held) == bool(value):
            # Nothing changed, so nothing is sent. The input core drops a key
            # event repeating the value it already holds, and a controller
            # whose state did not change has no new report to send. A fixture
            # that emitted one anyway would be asserting an event no real
            # pad produces -- which quietly weakens every test using it.
            return []
        if value:
            self._held.add(control)
        else:
            self._held.discard(control)

        if control in DPAD and self.dpad_is_hat:
            return self._hat_frame()

        code = self.buttons[control]
        frame: list[Event] = []
        if self.emits_scan and control in self.scancodes:
            frame.append(Event(ecodes.EV_MSC, ecodes.MSC_SCAN,
                               self.scancodes[control]))
        frame.append(Event(ecodes.EV_KEY, code, value))
        frame.append(SYN)
        return frame

    def _hat_frame(self) -> list[Event]:
        hat = _hat_from(self._held)
        frame: list[Event] = []
        if hat[0] != self._hat[0]:
            frame.append(Event(ecodes.EV_ABS, ecodes.ABS_HAT0X, hat[0]))
        if hat[1] != self._hat[1]:
            frame.append(Event(ecodes.EV_ABS, ecodes.ABS_HAT0Y, hat[1]))
        self._hat = hat
        # A frame with nothing in it is still a frame -- a driver syncs even
        # when the press changed no axis, and padmap has to tolerate that.
        frame.append(SYN)
        return frame

    def move(self, control: str, value: int) -> list[Event]:
        axis = self.axes.get(control)
        if axis is None:
            raise KeyError(f"{type(self).__name__} has no axis {control!r}")
        value = max(axis.minimum, min(axis.maximum, value))
        self._values[control] = value
        return [Event(ecodes.EV_ABS, axis.code, value), SYN]

    def rest(self) -> list[Event]:
        """The neutral state, as a driver reports it on the first poll.

        Not all zeros. This is where a resting trigger at 24 of 0-255 comes
        from, and a test that starts from zeros would never meet it.
        """
        frame = [Event(ecodes.EV_ABS, axis.code, axis.rest)
                 for axis in self.axes.values()]
        if self.dpad_is_hat:
            frame += [Event(ecodes.EV_ABS, ecodes.ABS_HAT0X, 0),
                      Event(ecodes.EV_ABS, ecodes.ABS_HAT0Y, 0)]
        return frame + [SYN]

    # -- becoming a real device -------------------------------------------
    def capabilities(self) -> dict[int, Any]:
        """What `uinput` needs, and what `devices.discover` will read back."""
        keys = sorted(set(self.buttons.values()))
        axes: list[tuple[int, AbsInfo]] = [
            (axis.code, axis.absinfo()) for axis in self.axes.values()]
        if self.dpad_is_hat:
            axes += [
                (ecodes.ABS_HAT0X, AbsInfo(0, -1, 1, 0, 0, 0)),
                (ecodes.ABS_HAT0Y, AbsInfo(0, -1, 1, 0, 0, 0)),
            ]
        caps: dict[int, Any] = {ecodes.EV_KEY: keys, ecodes.EV_ABS: axes}
        if self.emits_scan:
            caps[ecodes.EV_MSC] = [ecodes.MSC_SCAN]
        return caps

    def spawn(self) -> evdev.UInput:
        """A real device node, with this controller's identity.

        Close it when finished: the node outlives nothing else, and a test
        that leaks one leaves a joypad on the machine that padmap will happily
        pick up in the *next* test.
        """
        return evdev.UInput(self.capabilities(), name=self.name,
                            vendor=self.vid, product=self.pid,
                            version=self.version, bustype=self.bustype)


class HidrawController(Controller):
    """A controller padmap reads as raw HID reports.

    `press` and `move` record state and return the *events padmap should
    produce*, which is what a test asserts against. The bytes that should
    produce them come from `report()`. The two are declared independently and
    on purpose: generating the expectation from the decoder under test would
    assert only that the decoder agrees with itself.
    """

    #: The report id `report()` builds.
    report_id: int = 0

    def press(self, control: str) -> list[Event]:
        if not self.has(control):
            raise KeyError(f"{type(self).__name__} has no {control!r}")
        # See EvdevController._key: an unchanged state produces no report
        # change, so there is nothing for a decoder to emit and nothing for
        # this to claim. Both decoders diff against their own previous state,
        # so claiming an event here would be an expectation no controller can
        # meet.
        if control in self._held:
            return []
        self._held.add(control)
        return self._expected(control, 1)

    def release(self, control: str) -> list[Event]:
        if not self.has(control):
            raise KeyError(f"{type(self).__name__} has no {control!r}")
        if control not in self._held:
            return []
        self._held.discard(control)
        return self._expected(control, 0)

    def _expected(self, control: str, value: int) -> list[Event]:
        if control in DPAD and self.dpad_is_hat:
            hat = _hat_from(self._held)
            frame = []
            if hat[0] != self._hat[0]:
                frame.append(Event(ecodes.EV_ABS, ecodes.ABS_HAT0X, hat[0]))
            if hat[1] != self._hat[1]:
                frame.append(Event(ecodes.EV_ABS, ecodes.ABS_HAT0Y, hat[1]))
            self._hat = hat
            return frame
        return [Event(ecodes.EV_KEY, self.buttons[control], value)]

    def move(self, control: str, value: int) -> list[Event]:
        """Put an axis at `value`, **in the units the controller reports**.

        Not evdev units. A fixture that stored the evdev value would have to
        un-invert it to build a report, and the inversion is one of the things
        under test -- the report must carry what the hardware would send.
        """
        axis = self.axes.get(control)
        if axis is None:
            raise KeyError(f"{type(self).__name__} has no axis {control!r}")
        value = max(axis.minimum, min(axis.maximum, value))
        self._values[control] = value
        return [Event(ecodes.EV_ABS, axis.code, self.to_evdev(control, value))]

    def to_evdev(self, control: str, value: int) -> int:
        """What padmap should publish for a hardware reading of `value`.

        The default is "unchanged". Controllers whose axes run the other way
        override it, and what they override it with is the fixture's claim
        about the hardware -- checked against the decoder, not taken from it.
        """
        return value

    @abstractmethod
    def report(self) -> bytes:
        """The current state as one input report, report id included."""


# -- the controllers ------------------------------------------------------
#
# Numbers below are from the driver that publishes each pad. Where padmap
# measured something on this machine that the driver does not state -- a
# resting trigger value, say -- the measurement is cited instead.

def _xpad_stick(code: int) -> Axis:
    """A 360-family stick, exactly as `xpad_set_up_abs` declares one.

        input_set_abs_params(input_dev, abs, -32768, 32767, 16, 128);

    The flat of 128 is the deadzone the driver asks for and is worth carrying:
    padmap's calibration reads it, and a fixture that dropped it would test a
    stick more sensitive than any real one.
    """
    return Axis(code=code, minimum=-32768, maximum=32767, rest=0,
                fuzz=16, flat=128)


class Xbox360(EvdevController):
    """The pad every other pad is compared against.

    Worth having as a fixture precisely because it is unremarkable: sticks
    centred at zero, triggers starting at zero, a hat for the d-pad. Anything
    that only works here is assuming this shape.
    """

    name = "Microsoft X-Box 360 pad"
    vid, pid = 0x045E, 0x028E
    bustype = ecodes.BUS_USB
    source = ("drivers/input/joystick/xpad.c: device table line 123, "
              "xpad_common_btn/xpad_btn_pad line 411, "
              "xpad_set_up_abs line 1898")
    # xpad is not a hid-input driver and sends no scancodes.
    emits_scan = False
    buttons = {
        "a": ecodes.BTN_A, "b": ecodes.BTN_B,
        "x": ecodes.BTN_X, "y": ecodes.BTN_Y,
        "l": ecodes.BTN_TL, "r": ecodes.BTN_TR,
        "select": ecodes.BTN_SELECT, "start": ecodes.BTN_START,
        "home": ecodes.BTN_MODE,
        "l3": ecodes.BTN_THUMBL, "r3": ecodes.BTN_THUMBR,
    }
    axes = {
        "lx": _xpad_stick(ecodes.ABS_X),
        "ly": _xpad_stick(ecodes.ABS_Y),
        "rx": _xpad_stick(ecodes.ABS_RX),
        "ry": _xpad_stick(ecodes.ABS_RY),
        # XTYPE_XBOX360: 0..255. The Series X below is 0..1023 from the same
        # switch, which is the only difference a mapping would notice.
        "lt": Axis(code=ecodes.ABS_Z, minimum=0, maximum=255, rest=0),
        "rt": Axis(code=ecodes.ABS_RZ, minimum=0, maximum=255, rest=0),
    }
    dpad_is_hat = True


class XboxSeriesX(EvdevController):
    """Same driver, wider triggers, one extra button.

    Here to catch anything that hard-codes 0..255 for a trigger because the
    360 does. `xpad_set_up_abs` picks 0..1023 for every XTYPE_XBOXONE pad, and
    a binding captured on one and applied to the other is wrong by a factor of
    four with nothing to say so.
    """

    name = "Microsoft Xbox Series S|X Controller"
    vid, pid = 0x045E, 0x0B12
    bustype = ecodes.BUS_USB
    source = ("drivers/input/joystick/xpad.c: device table line 134 "
              "(MAP_SHARE_BUTTON|MAP_SHARE_OFFSET, XTYPE_XBOXONE), "
              "xpad_set_up_abs line 1904")
    emits_scan = False
    buttons = {
        "a": ecodes.BTN_A, "b": ecodes.BTN_B,
        "x": ecodes.BTN_X, "y": ecodes.BTN_Y,
        "l": ecodes.BTN_TL, "r": ecodes.BTN_TR,
        "select": ecodes.BTN_SELECT, "start": ecodes.BTN_START,
        "home": ecodes.BTN_MODE,
        "l3": ecodes.BTN_THUMBL, "r3": ecodes.BTN_THUMBR,
        # MAP_SHARE_BUTTON. A key code, not a BTN_ -- which is itself worth
        # testing: padmap must not assume every control on a joypad is a BTN_.
        "capture": ecodes.KEY_RECORD,
    }
    axes = {
        "lx": _xpad_stick(ecodes.ABS_X),
        "ly": _xpad_stick(ecodes.ABS_Y),
        "rx": _xpad_stick(ecodes.ABS_RX),
        "ry": _xpad_stick(ecodes.ABS_RY),
        "lt": Axis(code=ecodes.ABS_Z, minimum=0, maximum=1023, rest=0),
        "rt": Axis(code=ecodes.ABS_RZ, minimum=0, maximum=1023, rest=0),
    }
    dpad_is_hat = True


class MayflashGameCube(EvdevController):
    """The adapter that broke three things at once.

    Everything awkward about it is real and measured on this machine, and is
    why it is in here rather than a second Xbox pad:

    * **the triggers are ABS_RX and ABS_RY**, not the ABS_Z/ABS_RZ the names
      would suggest. Code that assumed which codes a trigger lives on found
      nothing at all.
    * **they rest at 24 and 25 of 0-255** -- 81% deflected, untouched. A
      capture recorded the direction the axis was moving away from; a resting
      report answered a prompt nothing had touched; and re-arming waited for a
      return to centre that a trigger never makes, so after one press the
      trigger was dead and every later press was dropped in silence.
    * **no axis rests at zero**, because 0..255 has no such value once the
      midpoint is 127.5. Half-axis button binds cannot work in both directions
      on it.
    * it is driven by hid-generic, so it **emits MSC_SCAN** before every key.
    * it presents four ports whether or not controllers are plugged into them.

    FINDINGS.md, "An analogue trigger does not rest in the middle of its
    range" and "Y mapped to C-up did nothing".
    """

    name = "mayflash MAYFLASH GameCube Controller Adapter"
    vid, pid = 0x0079, 0x1843
    bustype = ecodes.BUS_USB
    source = ("FINDINGS.md: measured absinfo on this machine -- "
              "ABS_RX rest=24, ABS_RY rest=25, both of 0-255; "
              "hid-generic, so MSC_SCAN per hid-input.c:1787")
    emits_scan = True
    buttons = {
        "a": ecodes.BTN_SOUTH, "b": ecodes.BTN_EAST,
        "x": ecodes.BTN_WEST, "y": ecodes.BTN_NORTH,
        "l": ecodes.BTN_TL, "r": ecodes.BTN_TR,
        "start": ecodes.BTN_START,
        "select": ecodes.BTN_SELECT,
    }
    # hid-generic puts the HID usage in MSC_SCAN. Generic Desktop / Button
    # page usages, 0x00090001 upwards, which is what a button-page control
    # reports.
    scancodes = {
        "a": 0x00090001, "b": 0x00090002, "x": 0x00090003, "y": 0x00090004,
        "l": 0x00090005, "r": 0x00090006,
        "start": 0x00090008, "select": 0x00090007,
    }
    axes = {
        "lx": Axis(code=ecodes.ABS_X, minimum=0, maximum=255, rest=128),
        "ly": Axis(code=ecodes.ABS_Y, minimum=0, maximum=255, rest=128),
        # The C-stick. padmap publishes its Y on ABS_Z, which RetroArch then
        # treats as an analogue trigger -- see FINDINGS.md.
        "cx": Axis(code=ecodes.ABS_RZ, minimum=0, maximum=255, rest=128),
        "cy": Axis(code=ecodes.ABS_Z, minimum=0, maximum=255, rest=131),
        # lt/rt: see the class docstring for why these axes and rest values.
        "lt": Axis(code=ecodes.ABS_RX, minimum=0, maximum=255, rest=24),
        "rt": Axis(code=ecodes.ABS_RY, minimum=0, maximum=255, rest=25),
    }
    dpad_is_hat = True


class SteamControllerPuck(HidrawController):
    """The 2026 Steam Controller, through its receiver.

    The controller padmap has no evdev node for at all: without a `hid-steam`
    that knows this id, the kernel publishes a mouse and a keyboard per slot
    and no joypad. There is nothing to spawn, so this emits the vendor reports
    instead and `triton.Source` decodes them.

    Bit values and the struct layout are read off SDL's driver, and the
    expected evdev codes from what SDL maps each bit to -- `TRITON_LBUTTON_A`
    to `SDL_GAMEPAD_BUTTON_SOUTH` and so on. Declared here rather than
    imported from `triton.py` on purpose: a fixture built from the code under
    test asserts only that the code agrees with itself.
    """

    name = "Steam Controller"
    vid, pid = 0x28DE, 0x1304
    bustype = ecodes.BUS_USB
    source = ("SDL src/joystick/hidapi/SDL_hidapi_steam_triton.c: "
              "TritonButtons line 58, HandleGenericState line 148; "
              "steam/controller_structs.h: TritonMTUNoQuat_t line 631")
    report_id = 0x42

    #: Vocabulary name -> the bit in the report's u32 `buttons` field.
    bits = {
        "a": 0x00000001, "b": 0x00000002, "x": 0x00000004, "y": 0x00000008,
        "home2": 0x00000010,        # QAM
        "r3": 0x00000020, "start": 0x00000040,
        "r4": 0x00000080, "r5": 0x00000100,
        "r": 0x00000200,
        "down": 0x00000400, "right": 0x00000800,
        "left": 0x00001000, "up": 0x00002000,
        "select": 0x00004000, "l3": 0x00008000,
        "home": 0x00010000,
        "l4": 0x00020000, "l5": 0x00040000,
        "l": 0x00080000,
        "rpad": 0x00400000, "r2": 0x00800000,
        "lpad": 0x04000000, "l2": 0x08000000,
    }
    buttons = {
        "a": ecodes.BTN_SOUTH, "b": ecodes.BTN_EAST,
        "x": ecodes.BTN_WEST, "y": ecodes.BTN_NORTH,
        "l": ecodes.BTN_TL, "r": ecodes.BTN_TR,
        "l2": ecodes.BTN_TL2, "r2": ecodes.BTN_TR2,
        "select": ecodes.BTN_SELECT, "start": ecodes.BTN_START,
        "home": ecodes.BTN_MODE,
        "l3": ecodes.BTN_THUMBL, "r3": ecodes.BTN_THUMBR,
        "l4": ecodes.BTN_TRIGGER_HAPPY1, "r4": ecodes.BTN_TRIGGER_HAPPY2,
        "l5": ecodes.BTN_TRIGGER_HAPPY3, "r5": ecodes.BTN_TRIGGER_HAPPY4,
        "home2": ecodes.BTN_TRIGGER_HAPPY5,
        "lpad": ecodes.BTN_TRIGGER_HAPPY6, "rpad": ecodes.BTN_TRIGGER_HAPPY7,
    }
    axes = {
        "lx": Axis(code=ecodes.ABS_X, minimum=-32768, maximum=32767, rest=0),
        "ly": Axis(code=ecodes.ABS_Y, minimum=-32768, maximum=32767, rest=0,
                   inverted=True),
        "rx": Axis(code=ecodes.ABS_RX, minimum=-32768, maximum=32767, rest=0),
        "ry": Axis(code=ecodes.ABS_RY, minimum=-32768, maximum=32767, rest=0,
                   inverted=True),
        "lt": Axis(code=ecodes.ABS_Z, minimum=0, maximum=32767, rest=0),
        "rt": Axis(code=ecodes.ABS_RZ, minimum=0, maximum=32767, rest=0),
    }
    dpad_is_hat = True

    #: Capacitive touch. A finger resting on a stick is not a press, and these
    #: are here so a test can set one and assert padmap publishes nothing.
    touch_bits = {
        "right_stick_touch": 0x00100000, "right_pad_touch": 0x00200000,
        "left_stick_touch": 0x01000000, "left_pad_touch": 0x02000000,
        "right_grip_touch": 0x10000000, "left_grip_touch": 0x20000000,
    }

    def __init__(self) -> None:
        super().__init__()
        self._touching: set[str] = set()
        self._seq = 0

    def controls(self) -> set[str]:
        return set(self.bits) | set(self.axes) | set(DPAD)

    def touch(self, what: str, down: bool = True) -> None:
        """Rest a finger on a stick, pad or grip. Produces no event."""
        if what not in self.touch_bits:
            raise KeyError(what)
        self._touching.add(what) if down else self._touching.discard(what)

    def report(self) -> bytes:
        """One `TritonMTUNoQuat_t`, report id included.

        The Y axes carry what the *controller* reports, which counts upwards.
        Inverting here instead would hide exactly the bug worth catching: a
        decoder that forgot to flip Y would then pass.
        """
        buttons = 0
        for name in self._held:
            buttons |= self.bits.get(name, 0)
        for name in self._touching:
            buttons |= self.touch_bits[name]
        self._seq = (self._seq + 1) & 0xFF
        payload = struct.pack(
            "<BIhhhhhh", self._seq, buttons,
            self._values["lt"], self._values["rt"],
            self._values["lx"], self._values["ly"],
            self._values["rx"], self._values["ry"])
        # Trackpads, pressures and the IMU. Zeroed: padmap reads none of them,
        # and the length is what makes this the report the decoder expects.
        return bytes([self.report_id]) + payload + bytes(36)

    def to_evdev(self, control: str, value: int) -> int:
        """SDL negates the Y axes: `SDL_SendJoystickAxis(..., -sLeftStickY)`.

        Clamped, because -(-32768) is -32768 again in an i16 -- without it a
        stick held fully up would read fully down.
        """
        if not self.axes[control].inverted:
            return value
        return max(-32768, min(32767, -value))

    def _expected(self, control: str, value: int) -> list[Event]:
        if control in ("lpad", "rpad") or control in self.buttons:
            if control in DPAD:
                return super()._expected(control, value)
            return [Event(ecodes.EV_KEY, self.buttons[control], value)]
        return super()._expected(control, value)


class SwitchPro(HidrawController):
    """A Switch Pro Controller in full report mode.

    The other shape padmap has to handle over hidraw, and a useful contrast
    with the Steam Controller: 12-bit sticks packed three-to-two-bytes, a
    d-pad as four bits in a button byte, and a Y axis that also counts the
    wrong way -- but on a 0..4095 range, so the inversion is a subtraction
    rather than a negation.

    Verified against the hardware when the decode was written: pressing A set
    byte 3 to 0x08. `tools/switchprobe.py` is what checked.
    """

    name = "Nintendo Switch Pro Controller"
    vid, pid = 0x057E, 0x2009
    bustype = ecodes.BUS_USB
    source = ("padmap src/padmap/hidraw.py BUTTONS_* tables, verified against "
              "the hardware with tools/switchprobe.py; docs/HIDRAW.md")
    report_id = 0x30

    #: (byte index, mask) per control, in the 0x30 report.
    bits = {
        "y": (3, 0x01), "x": (3, 0x02), "b": (3, 0x04), "a": (3, 0x08),
        "r": (3, 0x40), "r2": (3, 0x80),
        "select": (4, 0x01), "start": (4, 0x02),
        "r3": (4, 0x04), "l3": (4, 0x08),
        "home": (4, 0x10), "capture": (4, 0x20),
        "down": (5, 0x01), "up": (5, 0x02),
        "right": (5, 0x04), "left": (5, 0x08),
        "l": (5, 0x40), "l2": (5, 0x80),
    }
    buttons = {
        # Nintendo's labels are laid out so that A is the *right* face button
        # and B the bottom -- the opposite of an Xbox pad. hid-nintendo
        # publishes them by position, which is what this reproduces: a stored
        # mapping keys on the code, and a pad reporting BTN_SOUTH for the
        # button labelled A would make every other pad's mapping wrong.
        "a": ecodes.BTN_EAST, "b": ecodes.BTN_SOUTH,
        "x": ecodes.BTN_NORTH, "y": ecodes.BTN_WEST,
        "l": ecodes.BTN_TL, "r": ecodes.BTN_TR,
        "l2": ecodes.BTN_TL2, "r2": ecodes.BTN_TR2,
        "select": ecodes.BTN_SELECT, "start": ecodes.BTN_START,
        "home": ecodes.BTN_MODE, "capture": ecodes.BTN_Z,
        "l3": ecodes.BTN_THUMBL, "r3": ecodes.BTN_THUMBR,
    }
    axes = {
        "lx": Axis(code=ecodes.ABS_X, minimum=0, maximum=4095, rest=2048,
                   fuzz=16, flat=128),
        "ly": Axis(code=ecodes.ABS_Y, minimum=0, maximum=4095, rest=2048,
                   fuzz=16, flat=128, inverted=True),
        "rx": Axis(code=ecodes.ABS_RX, minimum=0, maximum=4095, rest=2048,
                   fuzz=16, flat=128),
        "ry": Axis(code=ecodes.ABS_RY, minimum=0, maximum=4095, rest=2048,
                   fuzz=16, flat=128, inverted=True),
    }
    dpad_is_hat = True

    def controls(self) -> set[str]:
        return set(self.bits) | set(self.axes) | set(DPAD)

    def report(self) -> bytes:
        """One 0x30 report.

        Sticks are packed three bytes per pair, low twelve bits first, exactly
        as the controller sends them -- Y increasing upwards. The decoder is
        what has to flip it.
        """
        data = bytearray(64)
        data[0] = self.report_id
        for name in self._held:
            index, mask = self.bits[name]
            data[index] |= mask
        _pack12(data, 6, self._values["lx"], self._values["ly"])
        _pack12(data, 9, self._values["rx"], self._values["ry"])
        return bytes(data)

    def to_evdev(self, control: str, value: int) -> int:
        """hid-nintendo reflects Y about the range rather than negating it.

        The axis is unsigned 0..4095 and the controller counts upwards, so
        full up is 4095 on the wire and 0 in evdev. Same intent as the Steam
        Controller's negation, different arithmetic -- which is why each
        controller states its own rule.
        """
        if not self.axes[control].inverted:
            return value
        axis = self.axes[control]
        return axis.minimum + axis.maximum - value

    def _expected(self, control: str, value: int) -> list[Event]:
        if control in DPAD:
            return super()._expected(control, value)
        return [Event(ecodes.EV_KEY, self.buttons[control], value)]


def _pack12(data: bytearray, at: int, first: int, second: int) -> None:
    """Two 12-bit values into three bytes, low one first."""
    data[at] = first & 0xFF
    data[at + 1] = ((first >> 8) & 0x0F) | ((second & 0x0F) << 4)
    data[at + 2] = (second >> 4) & 0xFF


#: Every controller this framework knows, by the name a test would type.
CONTROLLERS: dict[str, type[Controller]] = {
    "xbox360": Xbox360,
    "xbox-series-x": XboxSeriesX,
    "gamecube": MayflashGameCube,
    "steam-controller": SteamControllerPuck,
    "switch-pro": SwitchPro,
}


def get(key: str) -> Controller:
    """One controller by name, ready to drive."""
    try:
        return CONTROLLERS[key]()
    except KeyError:
        raise KeyError(
            f"no such controller {key!r}; have "
            f"{', '.join(sorted(CONTROLLERS))}") from None


def every() -> list[Controller]:
    """One of each, for a test that should hold for all of them."""
    return [cls() for _, cls in sorted(CONTROLLERS.items())]
