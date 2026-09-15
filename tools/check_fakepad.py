"""Every controller in the framework, driven through padmap's real code.

The framework exists so that padmap can be tested against controllers nobody
has plugged in, and the value is entirely in whether the fixtures are *real*.
So this does two separate jobs, and the second is the one that matters:

  * the fixtures are self-consistent -- no two controls share an evdev code,
    every axis has a usable range, every declared control can be driven;
  * **padmap's own decoders turn each controller's reports into the events the
    fixture says they should.** The fixture declares its expectations from the
    hardware's documentation and padmap decodes independently, so agreement is
    evidence rather than tautology.

That second part is how the Steam Controller decode gets tested at all. It is
a wireless receiver: an unpaired slot reads nothing, forever, which is
indistinguishable from a decoder that is wrong.

    XDG_RUNTIME_DIR=$(mktemp -d) nix develop --command \
        python3 tools/check_fakepad.py
"""

import os
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from evdev import ecodes  # noqa: E402

from padmap import fakepad, hidraw, triton  # noqa: E402
from padmap.fakepad import Event  # noqa: E402

failures: list[str] = []


def check(name: str, condition: bool, detail: str = "") -> None:
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{(': ' + detail) if detail else ''}")
        failures.append(name)


# -- the fixtures themselves ----------------------------------------------
def check_fixtures_are_coherent() -> None:
    print("every fixture")
    for pad in fakepad.every():
        tag = type(pad).__name__
        codes = list(pad.buttons.values())
        check(f"{tag}: no evdev code is claimed by two controls",
              len(codes) == len(set(codes)),
              str([c for c in codes if codes.count(c) > 1]))
        axis_codes = [axis.code for axis in pad.axes.values()]
        check(f"{tag}: no axis code is claimed twice",
              len(axis_codes) == len(set(axis_codes)))
        check(f"{tag}: every axis has a usable range",
              all(a.maximum > a.minimum for a in pad.axes.values()),
              "a zero range is a divide-by-zero in RetroArch")
        check(f"{tag}: every rest value is inside its range",
              all(a.minimum <= a.rest <= a.maximum
                  for a in pad.axes.values()))
        check(f"{tag}: it says where its numbers came from",
              bool(pad.source) and len(pad.source) > 20)
        check(f"{tag}: it has the four face buttons",
              all(pad.has(name) for name in fakepad.FACE))
        check(f"{tag}: it has a d-pad",
              all(pad.has(name) for name in fakepad.DPAD))


def check_unknown_controls_are_refused() -> None:
    print("asking for something that is not there")
    pad = fakepad.get("xbox360")
    for call, what in ((lambda: pad.press("paddle4"), "press"),
                       (lambda: pad.move("gyro", 1), "move")):
        try:
            call()
        except KeyError:
            check(f"{what} of an unknown control raises", True)
        else:
            check(f"{what} of an unknown control raises", False,
                  "it silently did nothing, which a test would read as a pass")
    try:
        fakepad.get("nintendo-64")
    except KeyError as error:
        check("an unknown controller names the ones that exist",
              "xbox360" in str(error), str(error))
    else:
        check("an unknown controller names the ones that exist", False)


def check_registry() -> None:
    print("the registry")
    check("every() returns one of each",
          len(fakepad.every()) == len(fakepad.CONTROLLERS))
    check("get() builds a fresh instance each time",
          fakepad.get("xbox360") is not fakepad.get("xbox360"))
    pad = fakepad.get("xbox360")
    pad.press("a")
    check("and that instance carries no state from the last one",
          not fakepad.get("xbox360").held())


# -- the shapes that differ between real controllers ----------------------
def check_scancodes() -> None:
    print("MSC_SCAN, which only some drivers send")
    xbox = fakepad.get("xbox360")
    cube = fakepad.get("gamecube")
    check("xpad sends no scancode",
          not any(e.type == ecodes.EV_MSC for e in xbox.press("a")))
    scan = [e for e in cube.press("a") if e.type == ecodes.EV_MSC]
    check("hid-generic sends one before the key", len(scan) == 1, str(scan))
    frame = cube.press("b")
    check("and it comes before the key, as the driver emits it",
          frame[0].type == ecodes.EV_MSC and frame[1].type == ecodes.EV_KEY,
          str(frame))


def check_trigger_ranges_differ() -> None:
    print("trigger ranges")
    check("the 360's triggers are 0..255",
          fakepad.get("xbox360").axes["lt"].maximum == 255)
    check("the Series X's are 0..1023",
          fakepad.get("xbox-series-x").axes["lt"].maximum == 1023,
          "anything hard-coding 255 is wrong by a factor of four")


def check_gamecube_resting_triggers() -> None:
    print("the GameCube adapter's resting triggers")
    cube = fakepad.get("gamecube")
    check("the left trigger is on ABS_RX, not ABS_Z",
          cube.axes["lt"].code == ecodes.ABS_RX)
    check("the right trigger is on ABS_RY",
          cube.axes["rt"].code == ecodes.ABS_RY)
    check("and it rests at 24 of 0-255, not at zero",
          cube.axes["lt"].rest == 24,
          "81% deflected, untouched -- three separate faults came from this")
    rest = {e.code: e.value for e in cube.rest()
            if e.type == ecodes.EV_ABS}
    check("the neutral frame reports that, rather than zeros",
          rest[ecodes.ABS_RX] == 24 and rest[ecodes.ABS_RY] == 25, str(rest))
    check("no axis on it rests at zero",
          all(a.rest != 0 for a in cube.axes.values()),
          "0..255 has no value that normalises to zero")


def check_face_button_positions() -> None:
    print("A is not in the same place on every pad")
    xbox = fakepad.get("xbox360")
    switch = fakepad.get("switch-pro")
    check("Xbox A is the bottom face button",
          xbox.buttons["a"] == ecodes.BTN_A)
    # BTN_A and BTN_SOUTH are the same code; the Switch is the interesting one.
    check("Switch A is the *right* face button",
          switch.buttons["a"] == ecodes.BTN_EAST,
          "Nintendo's labels are mirrored, and hid-nintendo publishes by "
          "position")
    check("Switch B is the bottom one",
          switch.buttons["b"] == ecodes.BTN_SOUTH)


def check_hat_behaviour() -> None:
    print("d-pads")
    for pad in fakepad.every():
        tag = type(pad).__name__
        pad.press("left")
        frame = pad.press("right")
        hat_x = [e for e in frame
                 if e.type == ecodes.EV_ABS and e.code == ecodes.ABS_HAT0X]
        check(f"{tag}: holding left and right cancels",
              bool(hat_x) and hat_x[-1].value == 0,
              str(frame))


# -- statefulness ---------------------------------------------------------
def check_double_press_and_bare_release() -> None:
    print("pressing twice, and releasing what was never pressed")
    for pad in fakepad.every():
        tag = type(pad).__name__
        pad.press("a")
        pad.press("a")
        check(f"{tag}: pressing an already-held button does not raise",
              "a" in pad.held())
        pad.release("a")
        check(f"{tag}: one release clears it even after two presses",
              "a" not in pad.held())
        try:
            pad.release("b")
        except Exception as error:                      # noqa: BLE001
            check(f"{tag}: releasing an unheld button does not raise", False,
                  repr(error))
        else:
            check(f"{tag}: releasing an unheld button does not raise", True)
        check(f"{tag}: and does not leave it held", "b" not in pad.held())


def check_interleaved_dpad() -> None:
    print("d-pad: a second direction while the first is still held")
    for pad in fakepad.every():
        tag = type(pad).__name__
        pad.press("up")
        pad.press("left")
        check(f"{tag}: up and left are both held",
              {"up", "left"} <= pad.held())
        pad.release("left")
        check(f"{tag}: releasing left leaves up held",
              "up" in pad.held() and "left" not in pad.held())
        frame = pad.press("down")
        hat_y = [e for e in frame
                 if e.type == ecodes.EV_ABS and e.code == ecodes.ABS_HAT0Y]
        # The other axis of the same cancellation check_hat_behaviour makes.
        check(f"{tag}: holding up and down cancels",
              bool(hat_y) and hat_y[-1].value == 0, str(frame))


def check_unknown_controls_are_refused_on_hidraw() -> None:
    print("asking a hidraw pad for something that is not there")
    for key in ("steam-controller", "switch-pro"):
        pad = fakepad.get(key)
        for call, what in ((lambda p=pad: p.press("paddle9"), "press"),
                           (lambda p=pad: p.move("gyro", 1), "move")):
            try:
                call()
            except KeyError:
                check(f"{key}: {what} of an unknown control raises", True)
            else:
                check(f"{key}: {what} of an unknown control raises", False,
                      "it silently did nothing, which reads as a pass")


def check_axis_extremes_and_clamping() -> None:
    print("axes: travel past either end")
    for pad in fakepad.every():
        tag = type(pad).__name__
        for name, axis in pad.axes.items():
            pad.move(name, axis.minimum - 1000)
            check(f"{tag}.{name}: below minimum clamps",
                  pad.value(name) == axis.minimum, str(pad.value(name)))
            pad.move(name, axis.maximum + 1000)
            check(f"{tag}.{name}: above maximum clamps",
                  pad.value(name) == axis.maximum, str(pad.value(name)))


# -- the wire encoding, not only the codes it produces --------------------
def check_hidraw_bits_do_not_collide() -> None:
    print("hidraw wire encoding")
    steam = fakepad.get("steam-controller")
    seen: dict[int, list[str]] = {}
    for name, bit in steam.bits.items():
        seen.setdefault(bit, []).append(name)
    clash = {bit: names for bit, names in seen.items() if len(names) > 1}
    check("Steam Controller: every button bit belongs to one control",
          not clash, str(clash))
    overlap = set(steam.bits.values()) & set(steam.touch_bits.values())
    check("Steam Controller: button bits and touch bits do not overlap",
          not overlap, str(overlap))

    switch = fakepad.get("switch-pro")
    slots: dict[tuple[int, int], list[str]] = {}
    for name, slot in switch.bits.items():
        slots.setdefault(slot, []).append(name)
    clash2 = {slot: names for slot, names in slots.items() if len(names) > 1}
    check("Switch Pro: every (byte, mask) belongs to one control",
          not clash2, str(clash2))


# -- padmap's decoders, against the reports ------------------------------
class PuckSource(triton.Source):
    """A Triton source with no device behind it."""

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


class SwitchSource(hidraw.Source):
    """A Switch Pro source with no device behind it."""

    def __init__(self) -> None:
        self.pad = None
        self.path = "/dev/null"
        self.name = "Nintendo Switch Pro Controller"
        self._fd = -1
        self._rdev = None
        self._buttons = {}
        self._axes = {}
        self._hat = (0, 0)
        self._counter = 0
        self._simple_seen = 0
        self._pending = []


def _keys(events) -> set[tuple[int, int]]:
    return {(e.code, e.value) for e in events if e.type == ecodes.EV_KEY}


def check_steam_controller_decode() -> None:
    """The decode that had never seen a report until this framework.

    triton.py was written from SDL's source and tested against constructed
    reports built by hand in its own check. This is stronger: the report comes
    from a fixture that declares the bit layout independently, and the
    expectation comes from what SDL maps that bit to.
    """
    print("Steam Controller: reports through triton.py")
    pad = fakepad.get("steam-controller")
    src = PuckSource()
    src._decode_state(pad.report()[1:])          # settle at rest

    missed: list[str] = []
    for name in sorted(pad.bits):
        if name in fakepad.DPAD:
            continue
        want = pad.press(name)
        got = src._decode_state(pad.report()[1:])
        expected = _keys(want)
        if not expected <= _keys(got):
            missed.append(f"{name}: wanted {expected}, got {_keys(got)}")
        pad.release(name)
        src._decode_state(pad.report()[1:])
    check(f"every one of its {len(pad.bits)} buttons decodes to the right code",
          not missed, "; ".join(missed[:3]))

    check("the report is the length the descriptor declares",
          len(pad.report()) == 54,
          "report 0x42 is 1 + 53 bytes on the real receiver")


def check_steam_controller_axes() -> None:
    print("Steam Controller: sticks and triggers")
    pad = fakepad.get("steam-controller")
    src = PuckSource()
    src._decode_state(pad.report()[1:])

    pad.move("ly", 20000)
    got = {e.code: e.value for e in src._decode_state(pad.report()[1:])
           if e.type == ecodes.EV_ABS}
    # The fixture writes what the controller reports -- up is positive -- so
    # the decoder is what has to invert it.
    check("pushing the left stick up gives a negative ABS_Y",
          got.get(ecodes.ABS_Y) == -20000, str(got.get(ecodes.ABS_Y)))

    pad.move("lt", 32767)
    got = {e.code: e.value for e in src._decode_state(pad.report()[1:])
           if e.type == ecodes.EV_ABS}
    check("a full left trigger is ABS_Z at its maximum",
          got.get(ecodes.ABS_Z) == 32767, str(got.get(ecodes.ABS_Z)))


def check_steam_controller_touch_is_silent() -> None:
    print("Steam Controller: a finger resting on a stick")
    pad = fakepad.get("steam-controller")
    src = PuckSource()
    src._decode_state(pad.report()[1:])
    for what in pad.touch_bits:
        pad.touch(what)
    events = src._decode_state(pad.report()[1:])
    check("publishes nothing", not _keys(events),
          f"capacitive touch bound as a button would fire constantly: "
          f"{_keys(events)}")


def check_switch_pro_decode() -> None:
    print("Switch Pro: reports through hidraw.py")
    pad = fakepad.get("switch-pro")
    src = SwitchSource()
    src._decode(pad.report())

    missed: list[str] = []
    for name in sorted(pad.bits):
        if name in fakepad.DPAD:
            continue
        want = pad.press(name)
        got = src._decode(pad.report())
        if not _keys(want) <= _keys(got):
            missed.append(f"{name}: wanted {_keys(want)}, got {_keys(got)}")
        pad.release(name)
        src._decode(pad.report())
    check(f"every one of its {len(pad.bits)} bits decodes to the right code",
          not missed, "; ".join(missed[:3]))


def check_switch_pro_sticks() -> None:
    print("Switch Pro: 12-bit sticks")
    pad = fakepad.get("switch-pro")
    src = SwitchSource()
    src._decode(pad.report())
    pad.move("lx", 3000)
    pad.move("ly", 4095)
    got = {e.code: e.value for e in src._decode(pad.report())
           if e.type == ecodes.EV_ABS}
    check("X survives the three-byte packing", got.get(ecodes.ABS_X) == 3000,
          str(got.get(ecodes.ABS_X)))
    # The controller counts Y upwards on 0..4095, so full up is 4095 there and
    # 0 in evdev -- a subtraction, where the Steam Controller's is a negation.
    check("full up reads as zero, not 4095", got.get(ecodes.ABS_Y) == 0,
          str(got.get(ecodes.ABS_Y)))


def check_switch_dpad() -> None:
    print("Switch Pro: the d-pad through the decoder")
    pad = fakepad.get("switch-pro")
    src = SwitchSource()
    src._decode(pad.report())
    pad.press("up")
    got = {e.code: e.value for e in src._decode(pad.report())
           if e.type == ecodes.EV_ABS}
    check("up is ABS_HAT0Y = -1", got.get(ecodes.ABS_HAT0Y) == -1, str(got))


def check_releases_decode() -> None:
    """The existing decode checks throw the release away; this asserts it.

    A decoder that set a button and never cleared it would pass every
    press-only check and leave a game with a button held down for ever.
    """
    print("releases, through both decoders")
    steam, src = fakepad.get("steam-controller"), PuckSource()
    src._decode_state(steam.report()[1:])
    missed = []
    for name in sorted(steam.bits):
        if name in fakepad.DPAD:
            continue
        steam.press(name)
        src._decode_state(steam.report()[1:])
        want = steam.release(name)
        got = src._decode_state(steam.report()[1:])
        if not _keys(want) <= _keys(got):
            missed.append(f"{name}: wanted {_keys(want)}, got {_keys(got)}")
    check("every Steam Controller button decodes its release",
          not missed, "; ".join(missed[:3]))

    switch, ssrc = fakepad.get("switch-pro"), SwitchSource()
    ssrc._decode(switch.report())
    missed = []
    for name in sorted(switch.bits):
        if name in fakepad.DPAD:
            continue
        switch.press(name)
        ssrc._decode(switch.report())
        want = switch.release(name)
        got = ssrc._decode(switch.report())
        if not _keys(want) <= _keys(got):
            missed.append(f"{name}: wanted {_keys(want)}, got {_keys(got)}")
    check("every Switch Pro bit decodes its release",
          not missed, "; ".join(missed[:3]))


def check_release_without_press_claims_nothing_extra() -> None:
    """A fixture must not claim an event the hardware would not produce.

    `HidrawController.release` declares a key-up unconditionally, while both
    decoders only emit on a transition. Releasing something never pressed
    changes no byte in the report, so a correct decoder emits nothing -- and
    a fixture claiming otherwise would be an expectation no real controller
    can meet, quietly weakening every test that uses it.
    """
    print("hidraw: releasing something never pressed")
    pad, src = fakepad.get("steam-controller"), PuckSource()
    src._decode_state(pad.report()[1:])
    want = pad.release("b")
    got = src._decode_state(pad.report()[1:])
    check("the fixture claims no more than the decoder emits",
          _keys(want) <= _keys(got),
          f"fixture claims {_keys(want)}, decoder emits {_keys(got)}")


def check_combined_state_decodes() -> None:
    """Several controls in one report, which nothing else here exercises.

    Every other decode check flips one control at a time. The Switch Pro
    packs six of these into three bytes, so a mask that clobbers its
    neighbour is only visible when two share a byte.
    """
    print("several controls held in one report")
    pad, src = fakepad.get("steam-controller"), PuckSource()
    src._decode_state(pad.report()[1:])
    held = ["a", "start", "l", "r2", "home", "l4"]
    for name in held:
        pad.press(name)
    pad.move("lx", 15000)
    pad.move("rt", 32767)
    events = src._decode_state(pad.report()[1:])
    want = {(pad.buttons[n], 1) for n in held}
    check("Steam Controller: every simultaneous press decodes",
          want <= _keys(events), str(want - _keys(events)))
    axes = {e.code: e.value for e in events if e.type == ecodes.EV_ABS}
    check("Steam Controller: axes moved in the same frame decode too",
          axes.get(ecodes.ABS_X) == 15000
          and axes.get(ecodes.ABS_RZ) == 32767,
          str(axes))

    switch, ssrc = fakepad.get("switch-pro"), SwitchSource()
    ssrc._decode(switch.report())
    # a/y/r2 share byte 3, l3/capture share byte 4, l2 is byte 5.
    held = ["a", "y", "r2", "l3", "capture", "l2"]
    for name in held:
        switch.press(name)
    got = _keys(ssrc._decode(switch.report()))
    want = {(switch.buttons[n], 1) for n in held}
    check("Switch Pro: no bit clobbers a neighbour in the same byte",
          want <= got, str(want - got))


def check_steam_controller_negative_y_clamps() -> None:
    print("Steam Controller: the stick held fully down")
    pad, src = fakepad.get("steam-controller"), PuckSource()
    src._decode_state(pad.report()[1:])
    # -32768 has no positive counterpart in an i16, so a naive negation
    # returns it unchanged and the stick reads fully down when held fully up.
    check("the fixture's own rule clamps",
          pad.to_evdev("ly", -32768) == 32767,
          str(pad.to_evdev("ly", -32768)))
    pad.move("ly", -32768)
    got = {e.code: e.value for e in src._decode_state(pad.report()[1:])
           if e.type == ecodes.EV_ABS}
    check("and the decoder agrees, rather than wrapping",
          got.get(ecodes.ABS_Y) == 32767, str(got.get(ecodes.ABS_Y)))


def check_switch_pro_axis_extremes() -> None:
    print("Switch Pro: both ends of the 12-bit stick")
    pad, src = fakepad.get("switch-pro"), SwitchSource()
    src._decode(pad.report())
    pad.move("ly", 0)
    got = {e.code: e.value for e in src._decode(pad.report())
           if e.type == ecodes.EV_ABS}
    check("fully down on the wire (0) is fully positive in evdev (4095)",
          got.get(ecodes.ABS_Y) == 4095, str(got.get(ecodes.ABS_Y)))


# -- the whole pipeline, with a real device node --------------------------
def check_spawn_is_discovered() -> None:
    print("spawning a real device")
    if not os.access("/dev/uinput", os.W_OK):
        print("  --   /dev/uinput not writable; skipping")
        return
    from padmap import devices

    pad = fakepad.get("xbox360")
    ui = pad.spawn()
    try:
        import time
        time.sleep(0.4)
        found = [p for p in devices.discover()
                 if p.vid == pad.vid and p.pid == pad.pid]
        check("padmap discovers it", bool(found))
        if found:
            check("under exactly the driver's name", found[0].name == pad.name,
                  f"{found[0].name!r} != {pad.name!r}")
            dev = devices.open_device(found[0])
            caps = dev.capabilities(absinfo=True)
            keys = set(caps.get(ecodes.EV_KEY, []))
            check("with every button it declares",
                  set(pad.buttons.values()) <= keys,
                  str(set(pad.buttons.values()) - keys))
            ranges = {c: i for c, i in caps.get(ecodes.EV_ABS, [])}
            check("and the trigger range the driver would give",
                  ranges[ecodes.ABS_Z].max == 255,
                  str(ranges.get(ecodes.ABS_Z)))
            dev.close()
    finally:
        ui.close()


def check_spawned_gamecube_keeps_its_rest_values() -> None:
    print("spawning the awkward one")
    if not os.access("/dev/uinput", os.W_OK):
        print("  --   /dev/uinput not writable; skipping")
        return
    from padmap import devices

    pad = fakepad.get("gamecube")
    ui = pad.spawn()
    try:
        import time
        time.sleep(0.4)
        found = [p for p in devices.discover()
                 if p.vid == pad.vid and p.pid == pad.pid]
        check("padmap discovers it", bool(found))
        if found:
            dev = devices.open_device(found[0])
            ranges = {c: i for c, i in
                      dev.capabilities(absinfo=True).get(ecodes.EV_ABS, [])}
            # The whole reason this fixture exists: a test that reads the rest
            # value off a real node sees 24, not the midpoint.
            check("the resting trigger survives into the device node",
                  ranges[ecodes.ABS_RX].value == 24,
                  str(ranges.get(ecodes.ABS_RX)))
            dev.close()
    finally:
        ui.close()


def main() -> int:
    for fn in (check_fixtures_are_coherent, check_unknown_controls_are_refused,
               check_unknown_controls_are_refused_on_hidraw,
               check_registry, check_scancodes, check_trigger_ranges_differ,
               check_gamecube_resting_triggers, check_face_button_positions,
               check_hat_behaviour, check_interleaved_dpad,
               check_double_press_and_bare_release,
               check_axis_extremes_and_clamping,
               check_hidraw_bits_do_not_collide,
               check_steam_controller_decode, check_steam_controller_axes,
               check_steam_controller_touch_is_silent,
               check_steam_controller_negative_y_clamps,
               check_switch_pro_decode, check_switch_pro_sticks,
               check_switch_pro_axis_extremes, check_switch_dpad,
               check_releases_decode,
               check_release_without_press_claims_nothing_extra,
               check_combined_state_decodes,
               check_spawn_is_discovered,
               check_spawned_gamecube_keeps_its_rest_values):
        fn()
    if failures:
        print(f"\n{len(failures)} failure(s): {', '.join(failures)}")
        return 1
    print("\nall fakepad checks pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
