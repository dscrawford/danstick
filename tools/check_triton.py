"""The 2026 Steam Controller decode, against SDL's published protocol.

Everything here is checkable without the hardware, and that is the point: the
controller is a wireless receiver, so "no reports arrived" is the *normal*
state of a slot with nothing paired into it, and is indistinguishable from a
decoder that is wrong. So the byte layout is tested directly.

The numbers come from SDL's `SDL_hidapi_steam_triton.c` and its two headers
(zlib, upstream 2025-11-12), read rather than guessed -- see
`docs/STEAM-CONTROLLER-SDL3.md`. What would go wrong without each check:

  * **the wrong six bytes.** Lizard mode is left by one feature report. Get a
    field wrong and the controller stays a keyboard, silently -- the ioctl
    still succeeds.
  * **a shifted offset.** The state report is a packed C struct. One byte out
    and the sticks read as triggers, which looks like a miscalibrated pad
    rather than a misread one.
  * **publishing capacitive touch as buttons.** Six of the 30 bits are
    "a finger is resting here". Bound by the mapping wizard they fire
    constantly.
  * **the dock.** The receiver's fifth interface sends nothing, ever. Treating
    it as a slot gives a pad that is silent and looks asleep.

    nix develop --command python3 tools/check_triton.py
"""

import struct
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from evdev import ecodes  # noqa: E402

from padmap import triton  # noqa: E402

failures: list[str] = []


def check(name: str, condition: bool, detail: str = "") -> None:
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{(': ' + detail) if detail else ''}")
        failures.append(name)


def state_report(buttons=0, tl=0, tr=0, lx=0, ly=0, rx=0, ry=0, seq=0,
                 tail=28) -> bytes:
    """A state report payload -- everything after the report id."""
    return (struct.pack("<BIhhhhhh", seq & 0xFF, buttons, tl, tr, lx, ly, rx, ry)
            + bytes(tail))


def check_lizard_packet() -> None:
    print("the packet that leaves lizard mode")
    packet = triton.lizard_off_packet()
    check("64 bytes, the feature report size", len(packet) == 64, str(len(packet)))
    check("report id 1", packet[0] == 0x01)
    check("type is ID_SET_SETTINGS_VALUES (0x87)", packet[1] == 0x87)
    check("length is one packed ControllerSetting (3)", packet[2] == 0x03)
    check("setting is SETTING_LIZARD_MODE (9)", packet[3] == 0x09)
    check("value is LIZARD_MODE_OFF, u16 little-endian",
          packet[4:6] == b"\x00\x00")
    check("the rest is zero", packet[6:] == bytes(58))
    check("the whole packet, byte for byte",
          packet == bytes([1, 0x87, 3, 9, 0, 0]) + bytes(58), packet[:8].hex(" "))


def check_ioctl_number() -> None:
    print("HIDIOCSFEATURE")
    # _IOC(_IOC_READ|_IOC_WRITE, 'H', 0x06, len): dir 3 at bit 30, 'H' at
    # bit 8, number 6, size at bit 16. A wrong number does not fail safely --
    # it is a different ioctl.
    check("64-byte feature report", triton.HIDIOCSFEATURE(64) == 0xC0404806,
          hex(triton.HIDIOCSFEATURE(64)))
    check("the size field really is the size",
          triton.HIDIOCSFEATURE(32) == 0xC0204806)
    check("the low half never changes",
          triton.HIDIOCSFEATURE(1) & 0xFFFF == 0x4806)


def check_offsets() -> None:
    print("the state report's field offsets")
    payload = state_report(buttons=0xDEADBEEF, tl=111, tr=222,
                           lx=1111, ly=-2222, rx=3333, ry=-4444, seq=7)
    got = triton.decode_state(payload)
    for field, want in (("seq", 7), ("buttons", 0xDEADBEEF),
                        ("trigger_left", 111), ("trigger_right", 222),
                        ("left_x", 1111), ("left_y", -2222),
                        ("right_x", 3333), ("right_y", -4444)):
        check(f"{field} reads back", got.get(field) == want,
              f"{got.get(field)} != {want}")


def check_short_payload() -> None:
    print("a truncated report")
    check("a payload shorter than the prefix decodes to nothing",
          triton.decode_state(state_report()[:10]) == {})
    check("an empty payload does not raise",
          triton.decode_state(b"") == {})
    check("exactly the prefix is enough",
          bool(triton.decode_state(state_report()[:triton.STATE_PREFIX_BYTES])))


def check_button_table() -> None:
    print("the button table")
    codes = list(triton.BUTTONS.values())
    check("no evdev code is used twice", len(codes) == len(set(codes)),
          str([c for c in codes if codes.count(c) > 1]))
    bits = list(triton.BUTTONS)
    check("no bit is listed twice", len(bits) == len(set(bits)))
    check("every entry is a single bit",
          all(bit and not (bit & (bit - 1)) for bit in bits))
    # The four face buttons follow SDL's mapping, not the printed labels.
    for bit, code, label in ((triton.BTN_A, ecodes.BTN_SOUTH, "A->SOUTH"),
                             (triton.BTN_B, ecodes.BTN_EAST, "B->EAST"),
                             (triton.BTN_X, ecodes.BTN_WEST, "X->WEST"),
                             (triton.BTN_Y, ecodes.BTN_NORTH, "Y->NORTH")):
        check(label, triton.BUTTONS.get(bit) == code)
    check("the d-pad is a hat, not four buttons",
          not any(bit in triton.BUTTONS for bit in (
              triton.BTN_DPAD_UP, triton.BTN_DPAD_DOWN,
              triton.BTN_DPAD_LEFT, triton.BTN_DPAD_RIGHT)))


def check_touch_is_not_a_button() -> None:
    print("capacitive touch")
    for name in ("RIGHT_STICK_TOUCH", "LEFT_STICK_TOUCH", "RIGHT_PAD_TOUCH",
                 "LEFT_PAD_TOUCH", "RIGHT_GRIP_TOUCH", "LEFT_GRIP_TOUCH"):
        bit = getattr(triton, name)
        check(f"{name} is not published as a button",
              bit not in triton.BUTTONS)
        check(f"{name} is in TOUCH_ONLY", bool(triton.TOUCH_ONLY & bit))
    # The pad *clicks* are real presses and must survive the cull.
    check("a trackpad click is still a button",
          triton.LEFT_PAD_CLICK in triton.BUTTONS
          and triton.RIGHT_PAD_CLICK in triton.BUTTONS)


def check_hat() -> None:
    print("the d-pad")
    check("neutral", triton.hat_for(0) == (0, 0))
    check("up is negative Y", triton.hat_for(triton.BTN_DPAD_UP) == (0, -1))
    check("down is positive Y", triton.hat_for(triton.BTN_DPAD_DOWN) == (0, 1))
    check("left is negative X", triton.hat_for(triton.BTN_DPAD_LEFT) == (-1, 0))
    check("right is positive X", triton.hat_for(triton.BTN_DPAD_RIGHT) == (1, 0))
    check("diagonals combine",
          triton.hat_for(triton.BTN_DPAD_UP | triton.BTN_DPAD_RIGHT) == (1, -1))
    # A real hat cannot report both. Left+right held must not walk anywhere.
    check("opposites cancel rather than picking one",
          triton.hat_for(triton.BTN_DPAD_LEFT | triton.BTN_DPAD_RIGHT) == (0, 0))
    check("and on the other axis too",
          triton.hat_for(triton.BTN_DPAD_UP | triton.BTN_DPAD_DOWN) == (0, 0))


class FakeSource(triton.Source):
    """A Source with no device behind it, for decoding alone."""

    def __init__(self) -> None:
        self.pad = None
        self.path = "/dev/null"
        self.name = "Steam Controller"
        self._fd = -1
        self._rdev = None
        self._buttons = {}
        self._axes = {}
        self._hat = (0, 0)
        self._pending = []
        self._counter = 0
        self._simple_seen = 0
        self._last_lizard = 0.0
        self.connected = True


def check_decoding_emits_changes() -> None:
    print("decoding to events")
    src = FakeSource()
    events = src._decode_state(state_report(buttons=triton.BTN_A))
    pressed = [(e.code, e.value) for e in events if e.type == ecodes.EV_KEY]
    check("A down is emitted", (ecodes.BTN_SOUTH, 1) in pressed, str(pressed))

    again = src._decode_state(state_report(buttons=triton.BTN_A))
    check("holding it emits nothing more",
          not [e for e in again if e.type == ecodes.EV_KEY], str(len(again)))

    release = src._decode_state(state_report(buttons=0))
    ups = [(e.code, e.value) for e in release if e.type == ecodes.EV_KEY]
    check("releasing emits the up", (ecodes.BTN_SOUTH, 0) in ups, str(ups))


def check_stick_orientation() -> None:
    print("stick orientation")
    src = FakeSource()
    events = src._decode_state(state_report(ly=20000, ry=20000, lx=20000))
    axes = {e.code: e.value for e in events if e.type == ecodes.EV_ABS}
    # The controller reports up as positive; evdev's convention is the
    # opposite, and SDL negates for the same reason.
    check("left Y is inverted", axes.get(ecodes.ABS_Y) == -20000,
          str(axes.get(ecodes.ABS_Y)))
    check("right Y is inverted", axes.get(ecodes.ABS_RY) == -20000)
    check("X is not inverted", axes.get(ecodes.ABS_X) == 20000)


def check_extreme_negative_is_clamped() -> None:
    print("the edge of an i16")
    src = FakeSource()
    events = src._decode_state(state_report(ly=-32768))
    axes = {e.code: e.value for e in events if e.type == ecodes.EV_ABS}
    # -(-32768) is -32768 again in two's complement; without a clamp the
    # stick reads fully *down* when it is held fully up.
    check("negating the minimum does not wrap",
          axes.get(ecodes.ABS_Y) == 32767, str(axes.get(ecodes.ABS_Y)))


def check_triggers() -> None:
    print("triggers")
    src = FakeSource()
    events = src._decode_state(state_report(tl=32767, tr=0))
    axes = {e.code: e.value for e in events if e.type == ecodes.EV_ABS}
    check("left trigger on ABS_Z", axes.get(ecodes.ABS_Z) == 32767)
    caps = src.capabilities()[ecodes.EV_ABS]
    ranges = {code: info for code, info in caps}
    check("triggers rest at zero on a 0.. range",
          ranges[ecodes.ABS_Z].min == 0 and ranges[ecodes.ABS_Z].value == 0,
          f"min={ranges[ecodes.ABS_Z].min}")
    check("sticks are centred on a signed range",
          ranges[ecodes.ABS_X].min == -32768 and ranges[ecodes.ABS_X].value == 0)
    check("no axis has a zero range",
          all(info.max > info.min for info in ranges.values()),
          "a zero range is a divide-by-zero in RetroArch's udev_compute_axis")


def check_capabilities_without_absinfo() -> None:
    print("capabilities(absinfo=False)")
    src = FakeSource()
    plain = src.capabilities(absinfo=False)[ecodes.EV_ABS]
    check("returns bare codes", all(isinstance(c, int) for c in plain))
    rich = {c for c, _ in src.capabilities()[ecodes.EV_ABS]}
    check("the same axes either way", set(plain) == rich)


def check_disconnect_releases_everything() -> None:
    print("a controller that vanishes mid-press")
    src = FakeSource()
    src._decode_state(state_report(buttons=triton.BTN_A | triton.BTN_B))
    src._note_connected(False)
    ups = [(e.code, e.value) for e in src._pending]
    check("held buttons are released",
          (ecodes.BTN_SOUTH, 0) in ups and (ecodes.BTN_EAST, 0) in ups, str(ups))
    check("nothing is left held", not any(src._buttons.values()))


def check_report_ids() -> None:
    print("report ids")
    check("0x42 is a state report", 0x42 in triton.STATE_REPORTS)
    check("0x45 is too (the BLE variant)", 0x45 in triton.STATE_REPORTS)
    check("0x47 is too (the timestamped variant)", 0x47 in triton.STATE_REPORTS)
    check("battery is not decoded as state", 0x43 not in triton.STATE_REPORTS)
    check("wireless status is not either", 0x79 not in triton.STATE_REPORTS)


def check_descriptor_walk() -> None:
    print("telling a slot from the dock")
    # The real descriptors, captured from this machine's receiver.
    slot = bytes.fromhex(
        "05010902a1018540090"  # mouse, report 0x40
        "1") + bytes.fromhex("a100c0c0") + bytes.fromhex(
        "05010906a1018541c0") + bytes.fromhex(
        "0600ff0901a1018542c0")
    dock = bytes.fromhex("0600ff0902a1018542c0")
    check("a slot's vendor collection is found past the mouse and keyboard",
          0xFF000001 in triton._vendor_collections(slot),
          str([hex(u) for u in triton._vendor_collections(slot)]))
    check("the dock is exactly the usage mainline ignores",
          triton._vendor_collections(dock) == [triton.DOCK_USAGE],
          str([hex(u) for u in triton._vendor_collections(dock)]))
    check("a truncated descriptor does not raise",
          isinstance(triton._vendor_collections(slot[:9]), list))


def check_live_slots() -> None:
    print("this machine")
    pads = triton.slots()
    if not pads:
        print("  --   no Steam Controller attached; skipping")
        return
    check("every slot is a hidraw node",
          all(p.path.startswith("/dev/hidraw") for p in pads))
    check("no two slots share a uniq",
          len({p.uniq for p in pads}) == len(pads),
          str([p.uniq for p in pads]))
    check("none is claimed to be visible to RetroArch",
          not any(p.retroarch_visible for p in pads))
    check("padmap owns them", all(triton.owns(p) for p in pads))


def main() -> int:
    for fn in (check_lizard_packet, check_ioctl_number, check_offsets,
               check_short_payload, check_button_table,
               check_touch_is_not_a_button, check_hat,
               check_decoding_emits_changes, check_stick_orientation,
               check_extreme_negative_is_clamped, check_triggers,
               check_capabilities_without_absinfo,
               check_disconnect_releases_everything, check_report_ids,
               check_descriptor_walk, check_live_slots):
        fn()
    if failures:
        print(f"\n{len(failures)} failure(s): {', '.join(failures)}")
        return 1
    print("\nall Triton checks pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
