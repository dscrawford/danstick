#!/usr/bin/env python3
"""Does SDL really re-bind a controller that is already open?

The reported bug: "the new controller config isn't immediately loaded into
Pegasus, and it just uses the old SDL default." Pegasus reads
sdl_controllers.txt exactly once, in GamepadManagerSDL2::start, so a mapping
padmap writes mid-session does nothing until the frontend is relaunched. The
fix hands the line to SDL_GameControllerAddMapping instead.

That fix rests on an assumption worth measuring rather than believing:
that adding a mapping for a GUID SDL already has *replaces* the bindings of
controllers that are open right now, rather than only affecting ones opened
afterwards. If it did the latter the fix would compile, run, log success, and
change nothing -- which is exactly the shape of failure this project has hit
repeatedly.

So this opens a real uinput pad through real SDL, gives it a deliberately
wrong mapping, opens it, replaces the mapping, and then presses a button to
see which SDL button comes out. Nothing here is self-reported: the final
check is an event.

Both of SDL's before-states are exercised, because they are different code
paths inside SDL and only one of them is what really happens: a pad with a
*stored* line (what Pegasus writes for a controller it does not recognise),
and a pad with none, which SDL still calls a game controller because it
manufactures a mapping from the standard BTN_SOUTH/BTN_EAST/... codes the
device advertises. Measured against the real frontend, that second one is the
path padmap's virtual pads take -- they clone their source's evdev key set --
and SDL reports it as *adding* an entry rather than replacing one.

Needs /dev/uinput. Creating a joystick node while the live daemon is watching
is not a neutral act, so the pads' signatures are written into the daemon's
'already asked' file first and removed afterwards -- without that, the machine
opens a setup screen on the user's television and grabs every pad.
"""

from __future__ import annotations

import ctypes
import os
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

import evdev  # noqa: E402
from evdev import ecodes  # noqa: E402

from padmap import mapping, protocol  # noqa: E402

# Two scenarios, two pads, because a uinput node cannot change its identity
# and the two must not share a GUID.
#
#   "pegasus-default" -- SDL already holds a stored line for this GUID, the
#       one Pegasus writes for a pad it does not recognise.
#   "auto-generated"  -- SDL holds no *stored* line at all, but still reports
#       the pad as a game controller because it manufactures a mapping from
#       the standard BTN_SOUTH/BTN_EAST/... codes the device advertises.
#       Measured on the real frontend: this is what actually happens to
#       padmap's virtual pads, which clone their source's evdev key set, and
#       it is a different code path inside SDL -- adding rather than
#       replacing an entry. Whether an open controller follows *that* is the
#       question the whole fix rests on.
PADS = [
    ("PADMAP SDLTEST", 0x0003, True),
    ("PADMAP SDLTEST2", 0x0004, False),
]
PAD_VID, PAD_VERSION = 0x1209, 1

FIRST_KEY = 0x130
KEY_COUNT = 12
CAPABILITIES = {
    ecodes.EV_KEY: list(range(FIRST_KEY, FIRST_KEY + KEY_COUNT)),
    ecodes.EV_ABS: [
        (0x00, evdev.AbsInfo(128, 0, 255, 0, 0, 0)),
        (0x01, evdev.AbsInfo(128, 0, 255, 0, 0, 0)),
    ],
}

# The button SDL must call `a` *after* the new mapping is applied. Nothing
# starts on it -- both of SDL's before-states put A on b0 -- so a fix that
# quietly did nothing would leave the old answer in place and be caught.
RIGHT_A_BUTTON = 7


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def signatures() -> list[str]:
    return [f"{PAD_VID:04x}:{pid:04x}:{name}" for name, pid, _ in PADS]


def guard_live_daemon(add: bool) -> None:
    path = protocol.prompted_path()
    try:
        lines = [line for line in path.read_text().splitlines() if line.strip()]
    except OSError:
        lines = []
    ours = set(signatures())
    lines = [line for line in lines if line.strip() not in ours]
    if add:
        lines.extend(sorted(ours))
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("".join(f"{line}\n" for line in lines))
    except OSError:
        pass


def line_for(guid: str, name: str, a_button: int) -> str:
    """A database line binding `a` to a chosen raw button."""
    return mapping.sdl_line(guid, name, {
        "a": f"b{a_button}",
        "b": "b1", "x": "b2", "y": "b3",
        "leftx": "a0", "lefty": "a1",
    })


def pegasus_default_line(guid: str, name: str) -> str:
    """Byte for byte what Pegasus registers for a pad SDL does not know.

    Copied from GamepadManagerSDL2.cpp, try_register_default_mapping. Using
    padmap's own line-writer for the "before" state would have modelled the
    wrong thing: it emits `platform:Linux` and Pegasus's does not, and it is
    exactly such differences that decide whether SDL treats a later mapping
    as a replacement or as a second competing entry.
    """
    return (guid + "," + name + ","
            "a:b0,b:b1,x:b2,y:b3,"
            "dpup:b12,dpdown:b13,dpleft:b14,dpright:b15,"
            "leftshoulder:b4,rightshoulder:b5,lefttrigger:b6,righttrigger:b7,"
            "back:b8,start:b9,guide:b16,"
            "leftstick:b10,rightstick:b11,"
            "leftx:a0,lefty:a1,rightx:a2,righty:a3")


def scenario(sdl2, name: str, pid: int, pre_register: bool) -> None:
    """Open a pad, replace its mapping while it is open, and press a button.

    `pre_register` picks which of SDL's two paths gets exercised. With it,
    SDL holds a stored line (the one Pegasus writes for an unknown pad) and
    the new mapping *replaces* an entry. Without it, SDL has no stored line
    but still calls the pad a game controller, because it manufactures a
    mapping from the standard BTN_SOUTH/BTN_EAST/... codes the device
    advertises -- and the new mapping is then *added* alongside. Measured on
    the real frontend, that second path is the one padmap's virtual pads take,
    and it is not obviously the same code inside SDL.
    """
    print(f"\n=== {name} ({'a stored default' if pre_register else 'SDL auto-generated'}) ===")
    ui = evdev.UInput(events=CAPABILITIES, name=name, phys="sdltest/0",
                      vendor=PAD_VID, product=pid, version=PAD_VERSION,
                      bustype=ecodes.BUS_USB, max_effects=0)
    controller = None
    try:
        time.sleep(0.6)
        index = -1
        for _ in range(50):
            sdl2.SDL_PumpEvents()
            for candidate in range(sdl2.SDL_NumJoysticks()):
                found = sdl2.SDL_JoystickNameForIndex(candidate)
                if found and found.decode(errors="replace") == name:
                    index = candidate
                    break
            if index >= 0:
                break
            time.sleep(0.1)
        if index < 0:
            fail(f"SDL never saw {name}")

        raw = ctypes.create_string_buffer(33)
        sdl2.SDL_JoystickGetGUIDString(
            sdl2.SDL_JoystickGetDeviceGUID(index), raw, 33)
        guid = raw.value.decode()
        print(f"  index {index}, GUID {guid}")

        # Free check while we are here: padmap computes GUIDs itself, before
        # SDL has ever seen the device, and a line under the wrong one is
        # never looked up and never complained about.
        computed = mapping.sdl_guid(
            bus=ecodes.BUS_USB, vendor=PAD_VID, product=pid,
            version=PAD_VERSION, name=name)
        if computed != guid:
            fail(f"padmap computes {computed}, SDL says {guid}")
        print("  ok  padmap computes the same GUID, unprompted")

        if pre_register:
            if sdl2.SDL_GameControllerAddMapping(
                    pegasus_default_line(guid, name).encode()) < 0:
                fail(f"SDL rejected the default line: {sdl2.SDL_GetError()}")
        elif not sdl2.SDL_IsGameController(index):
            fail("SDL does not consider this pad a controller at all, so the "
                 "auto-generated path is not being exercised")

        controller = sdl2.SDL_GameControllerOpen(index)
        if not controller:
            fail(f"could not open the controller: {sdl2.SDL_GetError()}")
        bind = sdl2.SDL_GameControllerGetBindForButton(
            controller, sdl2.SDL_CONTROLLER_BUTTON_A)
        was = bind.value.button
        if was == RIGHT_A_BUTTON:
            fail("A already sits on the button the new mapping uses, so this "
                 "run could not tell a working fix from a no-op")
        print(f"  ok  open, and A is b{was}")

        print("  replacing the mapping WITHOUT closing or reopening:")
        result = sdl2.SDL_GameControllerAddMapping(
            line_for(guid, name, RIGHT_A_BUTTON).encode())
        if result < 0:
            fail(f"SDL rejected the new mapping: {sdl2.SDL_GetError()}")
        # Deliberately not asserted on. SDL answers 0 when it overwrote an
        # entry and 1 when it added one, and which happens depends on whether
        # a *stored* line existed -- not on whether the live pad changed,
        # which is the only thing that matters and is checked below.
        print(f"    (SDL says {'added an entry' if result else 'replaced in place'})")

        bind = sdl2.SDL_GameControllerGetBindForButton(
            controller, sdl2.SDL_CONTROLLER_BUTTON_A)
        if bind.value.button != RIGHT_A_BUTTON:
            fail(f"the already-open controller still reports A as "
                 f"b{bind.value.button}. Applying a mapping at runtime cannot "
                 f"work this way, and the frontend really would need a restart.")
        print(f"  ok  the same open handle now reports A as b{bind.value.button}")

        print("  and a real press comes out as the new binding:")
        # The part that is not self-reported. Everything above is SDL
        # describing its own state; this is SDL delivering an event for a
        # button the kernel actually saw.
        event = sdl2.SDL_Event()
        sdl2.SDL_PumpEvents()
        while sdl2.SDL_PollEvent(ctypes.byref(event)):
            pass

        def press(offset: int) -> set:
            ui.write(ecodes.EV_KEY, FIRST_KEY + offset, 1)
            ui.syn()
            time.sleep(0.15)
            ui.write(ecodes.EV_KEY, FIRST_KEY + offset, 0)
            ui.syn()
            time.sleep(0.15)
            seen = set()
            deadline = time.monotonic() + 1.5
            while time.monotonic() < deadline:
                sdl2.SDL_PumpEvents()
                got = False
                while sdl2.SDL_PollEvent(ctypes.byref(event)):
                    got = True
                    if event.type == sdl2.SDL_CONTROLLERBUTTONDOWN:
                        seen.add(event.cbutton.button)
                if got and seen:
                    break
                time.sleep(0.05)
            return seen

        seen = press(RIGHT_A_BUTTON)
        if sdl2.SDL_CONTROLLER_BUTTON_A not in seen:
            fail(f"pressing raw button {RIGHT_A_BUTTON} produced {seen}, not "
                 f"A. The mapping was accepted but is not what SDL is using.")
        print(f"  ok  raw b{RIGHT_A_BUTTON} arrives as SDL 'A'")

        seen = press(was)
        if sdl2.SDL_CONTROLLER_BUTTON_A in seen:
            fail(f"raw button {was} STILL arrives as A, so the old mapping is "
                 f"live alongside the new one")
        print(f"  ok  raw b{was} no longer does")
    finally:
        if controller:
            sdl2.SDL_GameControllerClose(controller)
        ui.close()


def main() -> int:
    import sdl2

    guard_live_daemon(add=True)
    try:
        os.environ["SDL_VIDEODRIVER"] = "dummy"
        if sdl2.SDL_Init(sdl2.SDL_INIT_GAMECONTROLLER) != 0:
            fail(f"SDL would not start: {sdl2.SDL_GetError()}")
        for name, pid, pre_register in PADS:
            scenario(sdl2, name, pid, pre_register)
        print("\nall checks passed")
        return 0
    finally:
        try:
            sdl2.SDL_Quit()
        except Exception:  # noqa: BLE001
            pass
        guard_live_daemon(add=False)


if __name__ == "__main__":
    sys.exit(main())
