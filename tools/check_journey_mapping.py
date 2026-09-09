#!/usr/bin/env python3
"""The mapping journey, walked end to end: prompt, press, store, launch.

Stories S7-S13. Somebody holds a controller, is shown a picture of a pad with
an arrow on it, presses a button per prompt, and expects the game they then
start to use those buttons. Everything between is silent when it goes wrong: a
control bound to nothing looks exactly like a control bound to the wrong
thing, and RetroArch reports a pad as configured either way.

So this file walks *every* layout in `layouts.ALL` the way a person would --
one input per prompt, skipping what the pad does not have -- and then asks the
two consumers what they were told:

  * SDL, by parsing the generated database line back and comparing it, field
    by field, against the bindings that were captured.
  * RetroArch, by reading the autoconfig keys, which are not the canonical
    ones. mupen64plus-next reads N64 B from RetroPad **Y**, and the dolphin
    core reads GameCube A from RetroPad **A** where the global gamepad table
    crosses a/b. That divergence is the entire reason a GameCube controller
    needs a separate N64 mapping, and it is only visible in the emitted keys.

And the pickers, which are the other half of S9 and S10. A picker draws a pad
beside every entry, and the wizard then walks a layout. When those two are not
the same pad the user is asked to press an X on a controller that has none,
and the natural response -- pressing the stick, since nothing else is left --
binds a face button to an axis. That is how a real mapping ended up with
cancel on `-a3`, and it is what the layout/id pairing checks below exist for.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_journey_mapping.py
"""

from __future__ import annotations

import atexit
import os
import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

# Redirected before padmap is imported: a LIVE daemon runs on this machine and
# keeps its assignments, its autoconfig profiles and the recently-played list
# under XDG_RUNTIME_DIR, and the user's real controller profiles under
# XDG_DATA_HOME. Nothing here may reach any of them.
_SANDBOX = Path(tempfile.mkdtemp(prefix="padmap-check-journey-mapping-"))
os.environ["XDG_RUNTIME_DIR"] = str(_SANDBOX / "run")
os.environ["XDG_CONFIG_HOME"] = str(_SANDBOX / "config")
os.environ["XDG_DATA_HOME"] = str(_SANDBOX / "data")
os.environ["RETROARCH_CONFIG_DIR"] = str(_SANDBOX / "retroarch")
os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "devices")
# Pinned so nothing here has to open a device to learn what identity the
# virtual pad advertises. The pads below are fictional, so `mirror` -- the
# default -- would fall back to exactly these ids anyway, after a warning
# about an event node that does not exist.
os.environ["PADMAP_PAD_IDENTITY"] = "padmap"
for _name in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    Path(os.environ[_name]).mkdir(parents=True, exist_ok=True)
atexit.register(shutil.rmtree, _SANDBOX, ignore_errors=True)

from padmap import (capture, controllercfg, layouts,  # noqa: E402
                    mapping, profiles, protocol, retroarch)
from padmap.assign import Assignment  # noqa: E402
from padmap.capture import (CAPTURE_GAP_SECONDS, EV_ABS,  # noqa: E402
                            EV_KEY, SKIP_HOLD_SECONDS, Chooser, MappingRun)
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

# `find_profile` otherwise globs /nix/store and reads every libretro autoconfig
# on the machine. What libretro happens to ship must not decide what this file
# asserts -- and a capture is meant to beat the database anyway.
retroarch.autoconfig_dirs = lambda: []  # type: ignore[assignment]


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


# -- a pad, and a clock ------------------------------------------------------

CLOCK = {"t": 0.0}


def now() -> float:
    return CLOCK["t"]


def tick(seconds: float = 0.05) -> None:
    CLOCK["t"] += seconds


class Event:
    """One evdev event, in the three fields capture actually reads."""

    def __init__(self, type_: int, code: int, value: int) -> None:
        self.type = type_
        self.code = code
        self.value = value


# Sixteen buttons, more than the largest layout has controls, so a walk can
# answer every prompt with a distinct one.
FULL_KEYS = list(range(0x130, 0x140))

# A pad with two centred sticks, two analogue triggers that rest at their
# minimum (measured on the GameCube adapter here), and a hat.
FULL_AXES = {
    0x00: (0, 255, 128),    # ABS_X    left stick
    0x01: (0, 255, 128),    # ABS_Y
    0x02: (0, 255, 0),      # ABS_Z    left trigger, rests at zero
    0x03: (0, 255, 128),    # ABS_RX   right stick
    0x04: (0, 255, 128),    # ABS_RY
    0x05: (0, 255, 0),      # ABS_RZ   right trigger
    0x10: (-1, 1, 0),       # ABS_HAT0X
    0x11: (-1, 1, 0),       # ABS_HAT0Y
}

# The Mayflash adapter as measured: its analogue triggers are ABS_RX/ABS_RY,
# the codes a right stick usually occupies. Declaring those a stick is what
# left the front-end "stuck to the left".
TRIGGERS_ON_STICK_AXES = {
    0x00: (0, 255, 128), 0x01: (0, 255, 128),
    0x03: (0, 255, 0), 0x04: (0, 255, 0),
    0x10: (-1, 1, 0), 0x11: (-1, 1, 0),
}


def run(layout_id: str, keys: list[int] | None = None,
        axes: dict[int, tuple[int, int, int]] | None = None,
        held: set[int] | None = None, scope: str = "") -> MappingRun:
    CLOCK["t"] = 0.0
    return MappingRun(
        pad=None, player=1, layout=layouts.get(layout_id),
        keys=list(FULL_KEYS if keys is None else keys),
        axes=dict(FULL_AXES if axes is None else axes),
        held=set(held or ()), scope=scope, now=now,
    )


def tap(r: MappingRun, code: int) -> bool:
    """Press and release a button quickly enough to be a binding.

    Waits out the capture gap first, as a person moving between prompts
    unavoidably does.
    """
    tick(CAPTURE_GAP_SECONDS)
    r.feed(Event(EV_KEY, code, 1))
    tick(0.05)
    return r.feed(Event(EV_KEY, code, 0))


def hold(r: MappingRun, code: int) -> bool:
    """Hold a button long enough to skip the control being asked for."""
    tick(CAPTURE_GAP_SECONDS)
    r.feed(Event(EV_KEY, code, 1))
    tick(SKIP_HOLD_SECONDS + 0.05)
    return r.feed(Event(EV_KEY, code, 0))


def poke(r: MappingRun, code: int, value: int, rest: int) -> bool:
    """Push an axis or hat, then let it go so it re-arms."""
    tick(CAPTURE_GAP_SECONDS)
    answered = r.feed(Event(EV_ABS, code, value))
    r.feed(Event(EV_ABS, code, rest))
    return answered


def chooser(options: list[capture.Option], index: int = 0,
            kind: str = capture.KIND_SCOPE) -> Chooser:
    CLOCK["t"] = 0.0
    return Chooser(pad=None, player=1, options=options, kind=kind, index=index,
                   axes=dict(FULL_AXES), now=now)


# -- walking a layout --------------------------------------------------------

def button_walk(layout_id: str, keys: list[int] | None = None):
    """Answer every prompt of a layout with a distinct button.

    Returns (run, controls asked in order, code used per control).
    """
    codes = list(FULL_KEYS if keys is None else keys)
    r = run(layout_id, keys=codes)
    asked: list[str] = []
    used: dict[str, int] = {}
    for step in range(len(r.layout.controls)):
        control = r.current
        if control is None:
            fail(f"{layout_id}: the wizard ran out of prompts at step {step}, "
                 f"with {len(r.layout.controls)} controls to ask about")
        asked.append(control.canonical)
        code = codes[step]
        if not tap(r, code):
            fail(f"{layout_id}: pressing a button nothing else has used did "
                 f"not answer {control.label!r} -- the wizard is stuck on a "
                 f"prompt the user cannot get past")
        used[control.canonical] = code
    return r, asked, used


def sdl_fields_of(r: MappingRun, sticks: dict[str, str] | None = None
                  ) -> dict[str, str]:
    """The SDL line a capture produces, parsed back the way SDL reads it."""
    line = mapping.sdl_mapping(
        controllercfg.virtual_guid(1), "padmap Player 1", r.bindings,
        sticks=sticks)
    parsed = mapping.parse_sdl_line(line)
    if parsed is None:
        fail(f"the generated SDL line does not parse as one: {line!r}")
    guid, name, fields = parsed
    if guid != controllercfg.virtual_guid(1) or name != "padmap Player 1":
        fail(f"the SDL line round-tripped as {guid!r}/{name!r}")
    if fields.pop("platform", None) != "Linux":
        fail("the SDL line carries no platform, so SDL will ignore it")
    return fields


def retroarch_settings(r: MappingRun) -> dict[str, str]:
    """key -> value for the autoconfig lines a capture produces."""
    lines = mapping.retroarch_lines(r.bindings, r.layout.retroarch_keys())
    out: dict[str, str] = {}
    for line in lines:
        key, _, value = line.partition(" = ")
        if key in out:
            fail(f"{r.layout.id}: {key} is emitted twice, so two controls "
                 f"collapse onto one and only the last one works")
        out[key] = value.strip('"')
    return out


# -- S7: the wizard walks the layout ----------------------------------------

def check_every_layout_is_asked_in_order() -> None:
    print("S7: every layout asks for each of its controls once, in order:")
    for layout_id, layout in layouts.ALL.items():
        r, asked, _ = button_walk(layout_id)
        if asked != list(layout.order()):
            fail(f"{layout_id}: asked {asked}, layout is {layout.order()} -- "
                 f"the prompts and the picture disagree about the pad")
        if len(set(asked)) != len(asked):
            fail(f"{layout_id}: asked for {asked} -- a control asked twice "
                 f"overwrites its own binding and the first press is lost")
        if not r.finished:
            fail(f"{layout_id}: stopped at {r.index} of "
                 f"{len(layout.controls)} after answering every prompt")
        print(f"  ok  {layout_id}: {len(asked)} controls, in layout order")


def check_arrow_points_at_the_prompt() -> None:
    print("\nS7: the arrow points at the control being asked for:")
    # The front-end draws `layout.controls[index]` with an arrow and prints
    # `label`. If those two ever name different controls the user is pointed
    # at one button and asked for another, and every answer after that is
    # filed against the wrong control.
    for layout_id in layouts.ALL:
        r = run(layout_id)
        for step in range(len(r.layout.controls)):
            payload = r.to_event()
            if payload["index"] != step:
                fail(f"{layout_id}: step {step} reported index "
                     f"{payload['index']}")
            drawn = payload["layout"]["controls"]
            if payload["index"] >= len(drawn):
                fail(f"{layout_id}: the arrow index {payload['index']} is off "
                     f"the end of the {len(drawn)} controls drawn")
            here = drawn[payload["index"]]
            if here["canonical"] != payload["control"]:
                fail(f"{layout_id}: the arrow sits on {here['canonical']!r} "
                     f"while the prompt asks for {payload['control']!r}")
            if here["label"] != payload["label"]:
                fail(f"{layout_id}: the prompt reads {payload['label']!r} and "
                     f"the control drawn is labelled {here['label']!r}")
            if payload["done"]:
                fail(f"{layout_id}: reported done at step {step}")
            if payload["total"] != len(r.layout.controls):
                fail(f"{layout_id}: total is {payload['total']}")
            tap(r, FULL_KEYS[step])
        print(f"  ok  {layout_id}: {len(r.layout.controls)} prompts, arrow on "
              f"the control named every time")


def check_a_full_walk_is_complete() -> None:
    print("\nS7: a full walk produces a binding per control and a done event:")
    for layout_id, layout in layouts.ALL.items():
        r, _, used = button_walk(layout_id)
        if sorted(r.bindings) != sorted(layout.order()):
            missing = set(layout.order()) - set(r.bindings)
            fail(f"{layout_id}: {len(r.bindings)} bindings for "
                 f"{len(layout.controls)} controls; missing {sorted(missing)}")
        for control, code in used.items():
            index = mapping.sdl_button_index(FULL_KEYS, code)
            if r.bindings[control].sdl() != f"b{index}":
                fail(f"{layout_id}: {control} was answered with "
                     f"{hex(code)} but recorded "
                     f"{r.bindings[control].sdl()!r}")
        payload = r.to_event()
        if not payload["done"]:
            fail(f"{layout_id}: the wizard never reported done, so the "
                 f"front-end has no moment to move on to calibration")
        if payload["index"] != payload["total"] or payload["control"] != "":
            fail(f"{layout_id}: finished event still names a control")
        if len(payload["captured"]) != len(layout.controls):
            fail(f"{layout_id}: the finished event shows "
                 f"{len(payload['captured'])} captured controls")
        print(f"  ok  {layout_id}: {len(r.bindings)} bindings, done, and every "
              f"binding is the button that answered it")


def check_layout_drawn_is_layout_walked() -> None:
    print("\nS7: the pad drawn beside the prompt is the pad being walked:")
    for layout_id, layout in layouts.ALL.items():
        r = run(layout_id)
        payload = r.to_event()
        if payload["layout"]["id"] != layout_id:
            fail(f"asked to walk {layout_id} and drew "
                 f"{payload['layout']['id']}")
        drawn = [c["canonical"] for c in payload["layout"]["controls"]]
        if drawn != list(layout.order()):
            fail(f"{layout_id}: drew {drawn}, asks for {layout.order()}")
        for control in payload["layout"]["controls"]:
            if not (0.0 <= control["x"] <= 1.0 and 0.0 <= control["y"] <= 1.0):
                fail(f"{layout_id}: {control['canonical']} sits at "
                     f"({control['x']}, {control['y']}), off the canvas, so "
                     f"the arrow points at nothing")
        print(f"  ok  {layout_id}: drawn and walked from one set of controls")


# -- S8: skipping ------------------------------------------------------------

def check_skip_needs_no_named_button() -> None:
    print("\nS8: holding any button skips, from the very first prompt:")
    # It cannot be a *named* button: the daemon grabs the pads for the whole
    # session, so the front-end sees no controller input, and nothing is
    # mapped yet anyway. A hold works before a single control is known.
    for layout_id in layouts.ALL:
        r = run(layout_id)
        first = r.current
        if not hold(r, FULL_KEYS[7]):
            fail(f"{layout_id}: a long hold on the first prompt did nothing")
        if r.index != 1:
            fail(f"{layout_id}: the hold did not move past {first.label!r}")
        if r.bindings:
            fail(f"{layout_id}: the skip bound something anyway: {r.bindings}")
        print(f"  ok  {layout_id}: {first.label!r} skipped, nothing bound")


def check_skip_works_with_a_claimed_button() -> None:
    print("\nS8: even a button already used for another control can skip:")
    # A one-button pad has nothing else to hold. If the "one input cannot
    # answer two prompts" rule were checked before the hold, such a pad could
    # answer exactly one prompt and then never move again.
    r = run("snes", keys=[0x130])
    first = r.current.canonical
    tap(r, 0x130)
    if first not in r.bindings:
        fail("the only button on the pad did not answer the first prompt")
    second = r.current.canonical
    if tap(r, 0x130):
        fail("the same button answered two prompts")
    if not hold(r, 0x130):
        fail("holding the only button on the pad did not skip -- a pad with "
             "one button could answer one prompt and then never move again")
    if second in r.bindings:
        fail(f"{second} was bound by the hold that skipped it")
    print(f"  ok  {first} bound, {second} skipped, both with one button")


def check_skipping_leaves_a_hole() -> None:
    print("\nS8: a skipped control is unbound and reaches neither consumer:")
    r = run("generic")
    order = r.layout.order()
    for step, control in enumerate(order):
        if control == "lefttrigger":
            hold(r, FULL_KEYS[step])
        else:
            tap(r, FULL_KEYS[step])
    if "lefttrigger" in r.bindings:
        fail("the skipped trigger was bound anyway")
    if len(r.bindings) != len(order) - 1:
        fail(f"{len(r.bindings)} bindings for {len(order) - 1} answered "
             f"controls")
    if "lefttrigger" in sdl_fields_of(r):
        fail("SDL was told about a control the pad does not have; the "
             "front-end will then wait for a trigger nobody can press")
    if "input_l2_btn" in retroarch_settings(r):
        fail("RetroArch was given a binding for a skipped control -- it will "
             "report the pad as configured and the button does nothing")
    print("  ok  lefttrigger skipped: absent from the capture, from SDL and "
          "from the autoconfig")


def check_skipping_everything_stores_nothing() -> None:
    print("\nS8: a wizard that skips every control has nothing worth storing:")
    # `Server._finish_mapping` only stores when `run.bindings` is non-empty,
    # and this is why: a profile with no bindings still marks the pad as
    # configured, so it is never offered the wizard again and every button is
    # dead with nothing to say why.
    r = run("snes")
    for _ in range(len(r.layout.controls)):
        hold(r, FULL_KEYS[0])
    if not r.finished:
        fail("holding through every prompt did not finish the walk")
    if r.bindings:
        fail(f"skipping everything still produced {r.bindings}")
    if sdl_fields_of(r):
        fail("an all-skipped run still writes SDL fields")
    if mapping.retroarch_lines(r.bindings, r.layout.retroarch_keys()):
        fail("an all-skipped run still writes autoconfig bindings")
    print("  ok  finished, nothing captured, nothing emitted")


# -- S7: refusing the input that is not an answer ----------------------------

def check_the_opening_press_answers_nothing() -> None:
    print("\nS7: the press that opened the wizard does not answer a prompt:")
    for layout_id in layouts.ALL:
        r = run(layout_id, held={0x131})
        CLOCK["t"] = 5.0                # it has been down a long time
        r.feed(Event(EV_KEY, 0x131, 0))
        if r.index != 0 or r.bindings:
            fail(f"{layout_id}: the opening press answered the first prompt "
                 f"({r.bindings})")
        if not tap(r, 0x131):
            fail(f"{layout_id}: the same button is now ignored for good")
        print(f"  ok  {layout_id}: release swallowed, next press binds")


def check_one_input_cannot_answer_two_prompts() -> None:
    print("\nS7: one input cannot answer two prompts:")
    r = run("snes")
    tap(r, 0x130)
    after = r.index
    if tap(r, 0x130):
        fail("the same button answered a second control -- a stuck button "
             "would fill in the whole layout")
    if r.index != after:
        fail("a refused press advanced the wizard anyway")
    print("  ok  a button is refused a second time")

    # Walk to the d-pad, where an axis is an acceptable answer.
    while r.current is not None and r.current.kind != "dpad":
        r.skip()
        tick(CAPTURE_GAP_SECONDS)
    at = r.index
    if not poke(r, 0x00, 255, 128):
        fail("a full deflection was refused on a d-pad prompt")
    if poke(r, 0x00, 255, 128):
        fail("the same axis direction answered a second control")
    if r.index != at + 1:
        fail("a refused axis push advanced the wizard")
    if not poke(r, 0x00, 0, 128):
        fail("the other direction of the same stick was refused; left and "
             "right are two answers, not one")
    print("  ok  an axis direction is refused a second time, the opposite "
          "direction is still a fresh answer")

    at = r.index
    if not poke(r, 0x10, 1, 0):
        fail("a hat push was refused")
    if poke(r, 0x10, 1, 0):
        fail("the same hat direction answered a second control")
    if r.index != at + 1:
        fail("a refused hat push advanced the wizard")
    print("  ok  a hat direction is refused a second time")


# -- S7: the round trip to both consumers ------------------------------------

def check_sdl_round_trip() -> None:
    print("\nS7: a capture survives the round trip to SDL:")
    for layout_id, layout in layouts.ALL.items():
        r, _, _ = button_walk(layout_id)
        fields = sdl_fields_of(r)
        wanted = {
            mapping.SDL_FIELDS[control]: r.bindings[control].sdl()
            for control in layout.order()
        }
        if fields != wanted:
            fail(f"{layout_id}: SDL is told {fields}, the capture says "
                 f"{wanted}")
        if len(set(fields.values())) != len(fields):
            fail(f"{layout_id}: two SDL fields point at the same button: "
                 f"{fields}")
        print(f"  ok  {layout_id}: {len(fields)} fields, parsed back "
              f"identical to the capture")


def check_retroarch_round_trip() -> None:
    print("\nS7: a capture survives the round trip to RetroArch:")
    for layout_id, layout in layouts.ALL.items():
        r, _, _ = button_walk(layout_id)
        settings = retroarch_settings(r)
        if len(settings) != len(layout.controls):
            fail(f"{layout_id}: {len(settings)} autoconfig keys for "
                 f"{len(layout.controls)} captured controls")
        overrides = layout.retroarch_keys()
        for control in layout.order():
            key = overrides.get(control) or mapping.RETROARCH_KEYS[control]
            if settings.get(key) != r.bindings[control].retroarch():
                fail(f"{layout_id}: {control} was captured as "
                     f"{r.bindings[control].retroarch()!r} and emitted as "
                     f"{key} = {settings.get(key)!r}")
        if len(set(settings.values())) != len(settings):
            fail(f"{layout_id}: two RetroArch keys carry the same button, so "
                 f"one physical button drives two controls: {settings}")
        print(f"  ok  {layout_id}: {len(settings)} keys, each the button that "
              f"answered its control")


def check_n64_keys_are_crossed() -> None:
    print("\nS9: N64 A is RetroPad B, which is why a GameCube pad needs its "
          "own N64 mapping:")
    n64, _, used = button_walk("n64")
    gc, _, gc_used = button_walk("gamecube")
    n64_keys = retroarch_settings(n64)
    gc_keys = retroarch_settings(gc)

    a_press = mapping.sdl_button_index(FULL_KEYS, used["a"])
    if n64_keys.get("input_b_btn") != str(a_press):
        fail(f"the button pressed for N64 'A' reached RetroArch as "
             f"{n64_keys.get('input_b_btn')!r}, not input_b_btn = "
             f"{a_press!r}; mupen64plus-next reads N64 A from RetroPad B")
    if "input_a_btn" in n64_keys:
        fail("the N64 mapping emits input_a_btn, which mupen64plus-next never "
             "reads in its default mapping -- that button would be dead")
    b_press = mapping.sdl_button_index(FULL_KEYS, used["b"])
    if n64_keys.get("input_y_btn") != str(b_press):
        fail(f"N64 'B' reached RetroArch as "
             f"{n64_keys.get('input_y_btn')!r}, not {b_press!r}")
    if "input_x_btn" in n64_keys:
        fail("the N64 mapping emits input_x_btn; the N64 pad has no X")
    if "input_r2_btn" in n64_keys:
        fail("the N64 mapping binds RetroPad R2, which is the core's "
             "cbuttons_mode -- holding it turns A and B into C-buttons")

    gc_a_press = mapping.sdl_button_index(FULL_KEYS, gc_used["a"])
    if gc_keys.get("input_a_btn") != str(gc_a_press):
        fail(f"GameCube 'A' reached RetroArch as "
             f"{gc_keys.get('input_a_btn')!r}; the dolphin core binds GC A to "
             f"RetroPad A, which is exactly what the global table does not do")
    if gc_keys.get("input_r_btn") != str(
            mapping.sdl_button_index(FULL_KEYS, gc_used["righttrigger"])):
        fail("GameCube Z is not on RetroPad R; left alone it presses the "
             "Triforce test switch instead")
    if "input_l_btn" in gc_keys:
        fail("the GameCube mapping emits input_l_btn, which is not a "
             "GameCube button at all")

    if n64_keys.get("input_b_btn") == gc_keys.get("input_b_btn") and \
            used["a"] == gc_used["a"]:
        # Both layouts were answered with the same physical button for 'a';
        # the keys they emit must still differ, or one mapping would do.
        fail("the same button emits the same key under both layouts, so a "
             "per-console mapping would buy nothing")
    print("  ok  N64: A -> input_b_btn, B -> input_y_btn, no a/x/r2 keys")
    print("  ok  GameCube: A -> input_a_btn, Z -> input_r_btn, no l key")


def check_n64_c_buttons_are_a_stick() -> None:
    print("\nS9: the N64 C-buttons reach the core as its right stick:")
    r, _, used = button_walk("n64")
    fields = sdl_fields_of(r)
    for control, field in (("rightstick_up", "-righty"),
                           ("rightstick_down", "+righty"),
                           ("rightstick_left", "-rightx"),
                           ("rightstick_right", "+rightx")):
        want = f"b{mapping.sdl_button_index(FULL_KEYS, used[control])}"
        if fields.get(field) != want:
            fail(f"{control} reached SDL as {field}:{fields.get(field)!r}, "
                 f"wanted {want} -- SDL has no C-buttons, only a right stick")
    settings = retroarch_settings(r)
    for control, key in (("rightstick_up", "input_r_y_minus_btn"),
                         ("rightstick_down", "input_r_y_plus_btn"),
                         ("rightstick_left", "input_r_x_minus_btn"),
                         ("rightstick_right", "input_r_x_plus_btn")):
        if key not in settings:
            fail(f"{control} did not reach RetroArch as {key}; the core reads "
                 f"the right analog stick for C, with no button fallback")
    print("  ok  four C-buttons, as half-axes to SDL and stick keys to "
          "RetroArch")


def check_arcade_buttons_are_mame_numbers() -> None:
    print("\nS9: the arcade panel reads across as MAME buttons 1-6:")
    # mame2010 numbers MAME's buttons off the RetroPad in plain order, so the
    # panel's six buttons have to emit a, b, x, y, l, r reading across -- not
    # the Nintendo-crossed order the global table would give them.
    r, asked, used = button_walk("arcade")
    settings = retroarch_settings(r)
    panel = ["x", "y", "leftshoulder", "a", "b", "rightshoulder"]
    expected = ["input_a_btn", "input_b_btn", "input_x_btn",
                "input_y_btn", "input_l_btn", "input_r_btn"]
    if asked[:6] != panel:
        fail(f"the wizard asks for the panel in the order {asked[:6]}")
    for control, key in zip(panel, expected):
        want = str(mapping.sdl_button_index(FULL_KEYS, used[control]))
        if settings.get(key) != want:
            fail(f"panel button {control!r} reached RetroArch as "
                 f"{key}={settings.get(key)!r}, wanted {want} -- a scrambled "
                 f"panel gives a fighting game a punch where it wants a kick")
    print(f"  ok  {expected} in panel order, top row then bottom")


def check_axis_bindings_go_under_the_axis_key() -> None:
    print("\nS7: a d-pad wired to an analogue stick is emitted as an axis:")
    # RetroArch parses a `_btn` value with strtoull, so "-0" and "+0" both
    # come out as button 0: the two directions of one stick collapse and
    # pressing either activates both. Only the `_axis` parser reads the sign.
    r = run("snes")
    while r.current is not None and r.current.kind != "dpad":
        r.skip()
        tick(CAPTURE_GAP_SECONDS)
    poke(r, 0x01, 0, 128)       # up
    poke(r, 0x01, 255, 128)     # down
    poke(r, 0x00, 0, 128)       # left
    poke(r, 0x00, 255, 128)     # right
    settings = retroarch_settings(r)
    wanted = {"input_up_axis": "-1", "input_down_axis": "+1",
              "input_left_axis": "-0", "input_right_axis": "+0"}
    for key, value in wanted.items():
        if settings.get(key) != value:
            fail(f"expected {key} = {value!r}, got {settings.get(key)!r} -- "
                 f"an axis under a _btn key silently parses as button 0")
    if any(key.endswith("_btn") for key in settings):
        fail(f"an axis binding reached RetroArch under a _btn key: {settings}")
    fields = sdl_fields_of(r)
    if fields != {"dpup": "-a1", "dpdown": "+a1",
                  "dpleft": "-a0", "dpright": "+a0"}:
        fail(f"SDL was told {fields}")
    print("  ok  four directions, four signed axis keys, none under _btn")


def check_a_captured_axis_is_not_also_a_stick() -> None:
    print("\nS7: an axis the capture claims is not also declared a stick:")
    # The GameCube adapter's analogue triggers sit on ABS_RX/ABS_RY. Declaring
    # those a stick told SDL the right stick was jammed 80% to the upper left
    # and held there -- reported as the front-end being "stuck to the left".
    r = run("gamecube", axes=TRIGGERS_ON_STICK_AXES)
    while r.current is not None and r.current.canonical != "leftshoulder":
        tap(r, FULL_KEYS[r.index])
    poke(r, 0x03, 255, 0)       # L, an analogue trigger travelling up
    poke(r, 0x04, 255, 0)       # R
    if r.bindings["leftshoulder"].sdl() != "+a2":
        fail(f"L recorded as {r.bindings['leftshoulder'].sdl()!r}, wanted "
             f"'+a2' -- ABS_RX is code 3 but the third axis on this pad")
    codes = sorted(TRIGGERS_ON_STICK_AXES)
    sticks = mapping.stick_fields(codes, r.bindings, TRIGGERS_ON_STICK_AXES)
    if "rightx" in sticks or "righty" in sticks:
        fail(f"the triggers were declared a stick anyway ({sticks}); SDL then "
             f"reports a stick permanently pushed over")
    if sticks.get("leftx") != "a0" or sticks.get("lefty") != "a1":
        fail(f"the real stick was dropped ({sticks}); a front-end with no "
             f"stick and no d-pad cannot be navigated at all")
    fields = sdl_fields_of(r, sticks=sticks)
    if fields.get("leftshoulder") != "+a2" or "righty" in fields:
        fail(f"the SDL line disagrees with the capture: {fields}")
    print(f"  ok  L and R captured as +a2/+a3, sticks are {sorted(sticks)}")


def check_a_pad_with_too_few_buttons() -> None:
    print("\nS8: a pad with fewer buttons than the layout has controls still "
          "produces something usable:")
    # Four buttons and a hat, walked through the generic pad's fourteen
    # controls. Everything the pad does not have is skipped.
    keys = [0x130, 0x131, 0x132, 0x133]
    r = run("generic", keys=keys)
    for control in r.layout.order():
        if control in ("a", "b", "back", "start"):
            index = ("a", "b", "back", "start").index(control)
            if not tap(r, keys[index]):
                fail(f"{control} could not be answered on a four-button pad")
        elif control == "dpup":
            poke(r, 0x11, -1, 0)
        elif control == "dpdown":
            poke(r, 0x11, 1, 0)
        elif control == "dpleft":
            poke(r, 0x10, -1, 0)
        elif control == "dpright":
            poke(r, 0x10, 1, 0)
        else:
            hold(r, keys[0])
    if not r.finished:
        fail(f"stopped at {r.index} of {len(r.layout.controls)}")
    if sorted(r.bindings) != sorted(
            ["a", "b", "back", "start", "dpup", "dpdown", "dpleft", "dpright"]):
        fail(f"captured {sorted(r.bindings)}")
    settings = retroarch_settings(r)
    for key in ("input_b_btn", "input_a_btn", "input_select_btn",
                "input_start_btn", "input_up_btn", "input_down_btn",
                "input_left_btn", "input_right_btn"):
        if key not in settings:
            fail(f"{key} is missing, so a pad that can be navigated in the "
                 f"wizard cannot be navigated in a game")
    if "input_x_btn" in settings or "input_l_btn" in settings:
        fail("a control the pad does not have was bound anyway; RetroArch "
             "will report the pad configured with dead buttons")
    fields = sdl_fields_of(r, sticks={"leftx": "a0", "lefty": "a1"})
    for field in ("a", "b", "dpup", "dpdown", "dpleft", "dpright", "leftx"):
        if field not in fields:
            fail(f"SDL was not told about {field}; without it the front-end "
                 f"has no way to move or confirm")
    print(f"  ok  8 of 14 controls captured, 6 skipped, and both consumers "
          f"get a pad that can confirm, cancel and move")


# -- S9 / S10: the pickers ---------------------------------------------------

def controls_of(option: capture.Option) -> list[str]:
    return [c["canonical"] for c in option.to_json()["layout"]["controls"]]


# What `icons.for_pad` would answer for the controller in hand. Deliberately
# not `generic` in the checks below: `Server._begin_mapping` falls back to this
# guess whenever the picker hands it an empty layout id, so a check that used
# the generic pad as the fallback could not tell "the picker said generic"
# apart from "the picker said nothing".
PAD_GUESS = "arcade"


def wizard_asks(layout_id: str, guess: str = PAD_GUESS) -> list[str]:
    """What the wizard would ask, given the layout id a picker hands over.

    Reproduces `Server._begin_mapping` exactly: `layout_id or
    icons.for_pad(pad)`, resolved through `layouts.for_icon`. The `or` is the
    interesting half -- an empty id from a picker does not mean the generic
    pad, it means "whatever this controller is guessed to be".
    """
    return list(layouts.for_icon(layout_id or guess).order())


def check_layout_picker_offers_every_layout() -> None:
    print("\nS9: the layout picker offers every layout padmap knows:")
    options = capture.layout_options({"n64"})
    if [o.id for o in options] != list(layouts.ALL):
        fail(f"offered {[o.id for o in options]}, layouts.ALL is "
             f"{list(layouts.ALL)} -- a console added to padmap would simply "
             f"never appear")
    for option in options:
        if option.layout != option.id:
            fail(f"{option.id!r} draws {option.layout!r}")
        if controls_of(option) != wizard_asks(option.id):
            fail(f"{option.id!r} draws {controls_of(option)} and the wizard "
                 f"would ask for {wizard_asks(option.id)}")
    marked = [o.id for o in options if o.mapped]
    if marked != ["n64"]:
        fail(f"marked {marked} as already mapped, expected ['n64'] -- "
             f"re-mapping a layout replaces it, and there is no other way to "
             f"tell which ones that would destroy")
    print(f"  ok  {len(options)} layouts, each drawing the pad the wizard "
          f"then walks, n64 marked")


def check_scope_picker_promises_the_pad_it_walks() -> None:
    print("\nS9: every console scope draws the console the wizard walks:")
    options = capture.scope_options(
        scopes={"console:n64"}, default_layout="gamecube",
        recent=[("n64", "n64/goldeneye-007", "GoldenEye 007")])
    ids = [o.id for o in options]
    if ids[0] != profiles.SCOPE_UNIVERSAL:
        fail(f"the strip opens on {ids[0]!r}; the commonest answer must be "
             f"one hold away")
    consoles = [o for o in options if profiles.scope_console(o.id)]
    if [profiles.scope_console(o.id) for o in consoles] != layouts.CONSOLES:
        fail(f"offered consoles {[o.id for o in consoles]}, padmap knows "
             f"{layouts.CONSOLES}")
    for option in consoles:
        named = profiles.scope_console(option.id)
        if option.layout != named:
            fail(f"{option.id!r} draws {option.layout!r} -- the scope and the "
                 f"picture name different consoles")
        if controls_of(option) != wizard_asks(option.layout):
            fail(f"{option.id!r} draws {controls_of(option)} and the wizard "
                 f"then asks for {wizard_asks(option.layout)}; a picker that "
                 f"promises one pad and asks about another is how a mapping "
                 f"ended up with cancel bound to an axis")
        if named not in layouts.ALL:
            fail(f"{option.id!r} names a console no core ever reports")
    if any(o.id == "console:generic" for o in options):
        fail("'generic' was offered as a console; no core reports it, so "
             "that scope could never resolve")
    if not next(o for o in consoles if o.id == "console:n64").mapped:
        fail("the scope that already has a capture is unmarked, so re-mapping "
             "it would silently destroy what is there")
    print(f"  ok  {len(consoles)} consoles, each drawn as the pad the wizard "
          f"walks, n64 marked")

    games = [o for o in options if profiles.scope_game(o.id)]
    if len(games) != 1 or games[0].label != "GoldenEye 007":
        fail(f"the game just played is not offered ({[o.id for o in games]})")
    if controls_of(games[0]) != wizard_asks("n64"):
        fail("a per-game scope is not drawn as its console's pad; a mapping "
             "for one N64 game is still a capture of the N64 control set")
    print(f"  ok  {games[0].label!r} -> {games[0].id!r}, drawn and walked as "
          f"n64")


def check_scope_picker_deduplicates_and_marks() -> None:
    print("\nS9: several recent games, each offered once:")
    options = capture.scope_options(
        scopes={"game:n64/smash"}, default_layout="n64",
        recent=[("n64", "n64/goldeneye", "GoldenEye"),
                ("n64", "n64/smash", "Smash"),
                ("n64", "n64/goldeneye", "GoldenEye")])
    games = [o for o in options if profiles.scope_game(o.id)]
    if [o.id for o in games] != ["game:n64/goldeneye", "game:n64/smash"]:
        fail(f"offered {[o.id for o in games]} -- a replayed game must not "
             f"take two places on a strip worked one step at a time")
    if not next(o for o in games if o.id == "game:n64/smash").mapped:
        fail("an existing per-game mapping is unmarked")
    plain = capture.scope_options(scopes=set(), default_layout="")
    if any(profiles.scope_game(o.id) for o in plain):
        fail("a per-game scope was offered with no game to attach it to")
    print(f"  ok  {[o.label for o in games]}, Smash marked, and none offered "
          f"when nothing has been played")


def check_scope_picker_unknown_console() -> None:
    print("\nS9: a recently played game whose console padmap cannot name:")
    # Reachable: `padmap.launch` records every launch, deliberately including
    # one whose core it does not recognise -- "a launch with an unknown core
    # is exactly the one whose controls are most likely to have felt wrong".
    # `layouts.for_core` returns "" for such a core, and that "" travelled
    # into the strip as the layout to draw.
    #
    # It used to be offered. The entry was drawn as the generic pad, because
    # Option.to_json falls back to it for an empty layout, and confirming it
    # handed _begin_mapping an empty layout id -- which falls back to the
    # pad's own guess. So the strip promised one controller and the wizard
    # asked about another, which is exactly how a mapping once ended up with
    # cancel bound to an axis. game_scope_options already refused the same
    # thing; scope_options now does too.
    options = capture.scope_options(
        scopes=set(), default_layout="gamecube",
        recent=[("", "unknown/mystery-blob", "Mystery Blob")])
    games = [o for o in options if profiles.scope_game(o.id)]
    if games:
        fail(f"a game with no console is still offered ({[o.id for o in games]}). "
             f"The strip draws the generic pad for it and the wizard then walks "
             f"whatever the controller is guessed to be, so the picture promises "
             f"one pad and the prompts ask about another")
    print("  ok  not offered at all")

    print("\n...while a recent game whose console IS known is still offered:")
    options = capture.scope_options(
        scopes=set(), default_layout="gamecube",
        recent=[("", "unknown/blob", "Blob"), ("n64", "n64/mario", "Mario")])
    games = [o for o in options if profiles.scope_game(o.id)]
    if [o.id for o in games] != [profiles.game_scope("n64/mario")]:
        fail(f"expected only the N64 game, got {[o.id for o in games]} -- "
             f"refusing the unknown one must not cost the known ones")
    if games[0].layout != "n64":
        fail(f"the offered entry draws {games[0].layout!r}, not the console "
             f"whose control set the capture is made against")
    print(f"  ok  {games[0].id!r} offered, drawn as {games[0].layout!r}")

def check_game_scope_picker_is_two_entries() -> None:
    print("\nS10: asked from a game, the question is two entries wide:")
    pair = capture.game_scope_options(
        "n64", "n64/goldeneye-007-usa", "GoldenEye 007 (USA)",
        {"console:n64"})
    if [o.id for o in pair] != ["console:n64", "game:n64/goldeneye-007-usa"]:
        fail(f"offered {[o.id for o in pair]}; the console must come first, "
             f"since it is the answer that is right more often and the first "
             f"entry is the one a hurried user confirms")
    for option in pair:
        if option.layout != "n64":
            fail(f"{option.id!r} draws {option.layout!r}; both entries are "
                 f"captured against the N64 control set")
        if controls_of(option) != wizard_asks("n64"):
            fail(f"{option.id!r} draws a pad the wizard does not then walk")
    if not pair[0].mapped or pair[1].mapped:
        fail("the existing console mapping is unmarked while the fresh "
             "per-game one is marked")
    if pair[1].label != "GoldenEye 007 (USA)":
        fail(f"the game entry reads {pair[1].label!r}")
    print(f"  ok  {[o.label for o in pair]}, console first, both drawn and "
          f"walked as n64")

    print("\nS10: no console known for the game, nothing offered:")
    if capture.game_scope_options("", "unknown/mystery", "Mystery", set()):
        fail("a scope was offered for a game whose console is unknown; both "
             "entries are captured against the console's control set, so "
             "without one there is nothing coherent to draw or to walk")
    if not capture.game_scope_options("n64", "", "", set()):
        fail("a console-only mapping was refused for a game with no key")
    print("  ok  refused without a console, still offered without a key")


def check_scope_picker_is_worked_from_the_pad() -> None:
    print("\nS9: the scope strip is moved and confirmed from the pad itself:")
    # It has to be: the daemon holds EVIOCGRAB for the whole session, so the
    # front-end receives no controller input at all, and nothing is mapped
    # yet, so no gesture may name a button.
    options = capture.scope_options(scopes=set(), default_layout="n64")
    c = chooser(options)
    target = "console:n64"
    steps = 0
    while c.chosen != target:
        if not c.feed(Event(EV_ABS, 0x10, 1)):
            fail(f"the strip stopped moving at {c.chosen!r}")
        c.feed(Event(EV_ABS, 0x10, 0))
        steps += 1
        if steps > len(options):
            fail("the strip never reached the N64 scope")
    c.feed(Event(EV_KEY, 0x130, 1))
    tick(0.1)
    c.feed(Event(EV_KEY, 0x130, 0))
    if c.confirmed:
        fail("a tap confirmed a scope; the press that opened the picker is "
             "often still travelling when it appears")
    c.feed(Event(EV_KEY, 0x131, 1))
    tick(SKIP_HOLD_SECONDS + 0.05)
    if not c.feed(Event(EV_KEY, 0x131, 0)):
        fail("holding a button did not confirm the scope")
    if c.chosen != target:
        fail(f"confirmed {c.chosen!r} rather than {target!r}")
    if c.chosen_layout != "n64":
        fail(f"the daemon would walk {c.chosen_layout!r} for a scope the "
             f"picker drew as n64")
    if c.to_event()["kind"] != capture.KIND_SCOPE:
        fail("the front-end is not told which question this is, so it cannot "
             "title the strip")
    payload = c.to_event()
    if payload["choices"][payload["index"]]["id"] != c.chosen:
        fail("the entry the theme highlights is not the one the daemon acts "
             "on")
    print(f"  ok  {steps} pushes to {c.chosen!r}, tap ignored, hold confirmed, "
          f"and the daemon will walk {c.chosen_layout}")


# -- S9 + S14: from the wizard to the file RetroArch reads -------------------

# A GameCube adapter used to play N64 games: the case that forced scopes to
# exist. Twelve buttons, a hat, two sticks, two analogue triggers.
GC_KEYS = list(range(0x130, 0x13C))
GC_AXES = {
    0x00: (0, 255, 128), 0x01: (0, 255, 128),   # left stick
    0x02: (0, 255, 0), 0x05: (0, 255, 0),       # L and R, resting at zero
    0x03: (0, 255, 128), 0x04: (0, 255, 128),   # the C-stick
    0x10: (-1, 1, 0), 0x11: (-1, 1, 0),
}
GC_PAD = Pad(path="/dev/input/event901", name="padmap-check GameCube Adapter",
             phys="check/901", uniq="", vid=0x057E, pid=0x0337, syspath="",
             retroarch_visible=True)

# Which physical input answers each N64 prompt, in layout order. This is the
# script a person follows: press A, press B, press Start, waggle the d-pad,
# the two shoulders, Z, then the C-stick four ways.
N64_ON_GAMECUBE = [
    ("a", ("button", 0x130)),
    ("b", ("button", 0x131)),
    ("start", ("button", 0x137)),
    ("dpup", ("hat", 0x11, -1)),
    ("dpdown", ("hat", 0x11, 1)),
    ("dpleft", ("hat", 0x10, -1)),
    ("dpright", ("hat", 0x10, 1)),
    ("leftshoulder", ("button", 0x134)),
    ("rightshoulder", ("button", 0x135)),
    ("lefttrigger", ("button", 0x136)),         # the Z button
    ("rightstick_up", ("axis", 0x04, 0)),
    ("rightstick_down", ("axis", 0x04, 255)),
    ("rightstick_left", ("axis", 0x03, 0)),
    ("rightstick_right", ("axis", 0x03, 255)),
]


def walk_n64_on_a_gamecube_pad() -> MappingRun:
    r = run("n64", keys=GC_KEYS, axes=GC_AXES, scope="console:n64")
    for control, press in N64_ON_GAMECUBE:
        if r.current is None or r.current.canonical != control:
            fail(f"the N64 wizard asked for "
                 f"{r.current.canonical if r.current else None!r} where the "
                 f"script expects {control!r}")
        if press[0] == "button":
            answered = tap(r, press[1])
        elif press[0] == "hat":
            answered = poke(r, press[1], press[2], 0)
        else:
            answered = poke(r, press[1], press[2], 128)
        if not answered:
            fail(f"{control!r} could not be answered by {press}")
    if not r.finished:
        fail(f"the N64 walk stopped at {r.index}")
    return r


def store(pad: Pad, scope: str, run_: MappingRun) -> None:
    profile = profiles.load(pad) or profiles.Profile(
        signature=profiles.signature(pad), name=pad.name)
    profile.record(scope, profiles.Mapping(
        buttons=dict(run_.bindings), layout=run_.layout.id))
    profiles.save(profile)


def check_capture_reaches_retroarch() -> None:
    print("\nS9+S14: what RetroArch is handed for an N64 launch is the "
          "buttons that were pressed:")
    profiles.forget(GC_PAD)

    # The pad's own default, captured under the GameCube layout first.
    default = run("gamecube", keys=GC_KEYS, axes=GC_AXES)
    for step in range(len(default.layout.controls)):
        tap(default, GC_KEYS[step])
    store(GC_PAD, profiles.SCOPE_UNIVERSAL, default)

    n64 = walk_n64_on_a_gamecube_pad()
    store(GC_PAD, profiles.console_scope("n64"), n64)

    dest = _SANDBOX / "autoconfig-n64"
    written = retroarch.install_profiles(
        [Assignment(player=1, pad=GC_PAD, button=0)], dest=dest,
        console="n64", game="", context="GoldenEye 007 (USA)")
    if len(written) != 1:
        fail(f"{len(written)} profiles written for one player")
    text = written[0].read_text()
    emitted = {}
    for line in text.splitlines():
        if line.startswith("#") or " = " not in line:
            continue
        key, _, value = line.partition(" = ")
        emitted[key.strip()] = value.strip().strip('"')

    wanted = {
        "input_b_btn": str(mapping.sdl_button_index(GC_KEYS, 0x130)),  # A
        "input_y_btn": str(mapping.sdl_button_index(GC_KEYS, 0x131)),  # B
        "input_start_btn": str(mapping.sdl_button_index(GC_KEYS, 0x137)),
        "input_l_btn": str(mapping.sdl_button_index(GC_KEYS, 0x134)),
        "input_r_btn": str(mapping.sdl_button_index(GC_KEYS, 0x135)),
        "input_l2_btn": str(mapping.sdl_button_index(GC_KEYS, 0x136)),  # Z
        "input_up_btn": "h0up", "input_down_btn": "h0down",
        "input_left_btn": "h0left", "input_right_btn": "h0right",
        "input_r_y_minus_axis": "-4", "input_r_y_plus_axis": "+4",
        "input_r_x_minus_axis": "-3", "input_r_x_plus_axis": "+3",
    }
    for key, value in wanted.items():
        if emitted.get(key) != value:
            fail(f"an N64 launch was handed {key} = {emitted.get(key)!r}, "
                 f"and the user pressed the input that means {value!r}")
    if "input_a_btn" in emitted:
        fail("the N64 profile binds RetroPad A, which the core never reads")
    if "input_r2_btn" in emitted:
        fail("the N64 profile binds RetroPad R2, the core's cbuttons_mode")
    if "Layout: Nintendo 64 (n64)" not in text:
        fail("the profile does not name the layout it was captured under, so "
             "input_y_btn on an N64 pad reads as a mistake")
    if "Mapping scope: console:n64" not in text:
        fail("the profile does not name the scope it came from, so 'why is "
             "player 1 bound like this' cannot be answered from the file")
    print(f"  ok  {len(wanted)} keys, every one the input that answered its "
          f"prompt, under the N64 key table")

    # ...and a launch that is not an N64 game gets the pad's own mapping,
    # captured under the GameCube layout, which uses a different key table.
    dest2 = _SANDBOX / "autoconfig-default"
    plain = retroarch.install_profiles(
        [Assignment(player=1, pad=GC_PAD, button=0)], dest=dest2)[0].read_text()
    if 'input_a_btn = "0"' not in plain:
        fail("the pad's own mapping does not put GameCube A on RetroPad A")
    if "input_y_btn" in plain and "input_r_x_plus_axis" in plain:
        fail("the N64 capture leaked into a launch with no console")
    if "Layout: GameCube (gamecube)" not in plain:
        fail("a launch with no console did not fall back to the pad's own "
             "capture")
    print("  ok  a launch with no console still gets the GameCube capture, "
          "under the GameCube key table")


def check_per_game_scope_survives_to_the_launcher() -> None:
    print("\nS9+S14: a per-game scope offered by the picker is the one the "
          "launcher looks up:")
    rom_dir = _SANDBOX / "roms" / "n64"
    rom_dir.mkdir(parents=True, exist_ok=True)
    rom = rom_dir / "GoldenEye 007 (U).z64"
    rom.write_text("not really a rom")
    key = profiles.game_key("n64", str(rom))
    protocol.write_last_game("n64", key, "GoldenEye 007 (U)")

    recent = protocol.read_recent_games()
    options = capture.scope_options(
        scopes=set(), default_layout="gamecube",
        recent=[(g["console"], g["key"], g["title"]) for g in recent])
    offered = [o for o in options if profiles.scope_game(o.id)]
    if [o.id for o in offered] != [profiles.game_scope(key)]:
        fail(f"the picker offers {[o.id for o in offered]} for a game the "
             f"launcher will look up as {profiles.game_scope(key)!r}")

    for_this_game = walk_n64_on_a_gamecube_pad()
    # One button moved, so the per-game capture is distinguishable from the
    # console one by its value rather than by "something was written".
    for_this_game.bindings["a"] = Binding("button", 9, ra_index=9)
    store(GC_PAD, offered[0].id, for_this_game)

    dest = _SANDBOX / "autoconfig-game"
    text = retroarch.install_profiles(
        [Assignment(player=1, pad=GC_PAD, button=0)], dest=dest,
        console="n64", game=key, context="GoldenEye 007 (U)")[0].read_text()
    if 'input_b_btn = "9"' not in text:
        fail("the per-game capture did not win for the game it was made for; "
             "a wizard that completes and changes nothing is the worst "
             "possible failure here")
    scope, _ = controllercfg.resolved_mapping(GC_PAD, "n64", key)
    if scope != offered[0].id:
        fail(f"the launcher resolved {scope!r}, the picker offered "
             f"{offered[0].id!r}")
    other = retroarch.install_profiles(
        [Assignment(player=1, pad=GC_PAD, button=0)],
        dest=_SANDBOX / "autoconfig-other", console="n64", game="",
        context="another n64 game")[0].read_text()
    if 'input_b_btn = "9"' in other:
        fail("the per-game mapping applied to a different N64 game")
    print(f"  ok  {offered[0].id!r} offered, stored and resolved; another N64 "
          f"game still gets the console mapping")


# -- S12 / S13: what happens either side of the capture ----------------------

def check_done_event_hands_over_to_calibration() -> None:
    print("\nS12: the finished wizard names the player, so the sticks can be "
          "measured before accepting:")
    # The theme starts calibration for `mappingPlayer` when the wizard reports
    # done. Accept writes the profiles, ends the session and releases the
    # pads, so calibrating afterwards would measure a controller nobody is
    # holding: capture buttons -> measure sticks -> accept.
    r = run("snes")
    r.player = 3
    for step in range(len(r.layout.controls)):
        tap(r, FULL_KEYS[step])
    payload = r.to_event()
    if payload["player"] != 3:
        fail(f"the done event names player {payload['player']}; calibration "
             f"would then be started for the wrong slot")
    if not payload["done"] or payload["event"] != "mapping":
        fail("the front-end is never told the wizard finished")
    if len(payload["captured"]) != len(r.layout.controls):
        fail("the done event drops the capture the front-end just drew")
    print(f"  ok  player 3, done, {len(payload['captured'])} controls "
          f"captured")


def check_recalibration_keeps_the_capture() -> None:
    print("\nS13: recalibrating the last assigned pad keeps its mappings:")
    from padmap import server

    pad = Pad(path="/dev/input/event902", name="padmap-check Recal Pad",
              phys="check/902", uniq="", vid=0x1111, pid=0x2222, syspath="")
    profiles.forget(pad)
    daemon = server.Server()
    n64 = walk_n64_on_a_gamecube_pad()
    daemon._store_mapping(pad, "n64", dict(n64.bindings), scope="console:n64")
    before = profiles.load(pad)
    if before is None or "console:n64" not in before.mappings:
        fail("the capture was not filed under the scope it was made for")
    if before.icon:
        fail(f"a capture made *for N64 games* set the pad's icon to "
             f"{before.icon!r}; a GameCube controller mapped for N64 is still "
             f"a GameCube controller")

    daemon._store_profile(pad, {
        0: profiles.AxisCalibration(center=128, minimum=0, maximum=255,
                                    flat=4, reach_min=12, reach_max=243),
    })
    after = profiles.load(pad)
    if after is None or "console:n64" not in after.mappings:
        fail("measuring the sticks again forgot where every button is")
    if after.mappings["console:n64"].layout != "n64":
        fail("the layout was dropped by recalibration, so the capture would "
             "be re-emitted under the generic key table and the "
             "console-specific buttons would go dead")
    if after.mappings["console:n64"].buttons != before.mappings[
            "console:n64"].buttons:
        fail("the bindings changed across a recalibration")
    if not after.axes:
        fail("the calibration was not stored")
    print(f"  ok  {len(after.mappings)} scope(s) and the n64 layout survive; "
          f"{len(after.axes)} axis calibrated")

    print("\nS9: an unscoped capture is what may name the controller:")
    plain = Pad(path="/dev/input/event903", name="padmap-check Plain Pad",
                phys="check/903", uniq="", vid=0x3333, pid=0x4444, syspath="")
    profiles.forget(plain)
    daemon._store_mapping(plain, "n64", {"a": Binding("button", 0)}, scope="")
    stored = profiles.load(plain)
    if stored is None or stored.icon != "n64":
        fail(f"'this is what my controller is' left the icon "
             f"{stored.icon if stored else None!r}")
    print("  ok  the unscoped answer sets the icon; the scoped one does not")


# -- the numbering the two consumers do not share ----------------------------

def check_button_numbering_is_kept_for_both() -> None:
    print("\nS7: both button numberings are carried, because they differ:")
    # SDL walks BTN_JOYSTICK..KEY_MAX first and only then 0..BTN_JOYSTICK;
    # RetroArch's udev driver counts plainly upwards from BTN_MISC. On a pad
    # carrying any code below 0x120 they disagree, and the difference is
    # invisible on every other pad.
    keys = [0x100, 0x120, 0x121]
    r = run("snes", keys=keys, axes={})
    tap(r, 0x100)
    binding = r.bindings["a"]
    if binding.sdl() != "b2":
        fail(f"SDL was told {binding.sdl()!r}, and SDL numbers 0x100 as b2")
    if binding.retroarch() != "0":
        fail(f"RetroArch was told {binding.retroarch()!r}, and its udev "
             f"driver numbers 0x100 as 0 -- one number for both silently "
             f"shifts every binding on the one pad that differs")
    print("  ok  b2 to SDL and 0 to RetroArch, from a single press")

    # Below BTN_MISC, RetroArch's driver has no number for the code at all.
    # Plenty of pads advertise one -- KEY_HOMEPAGE (0xAC) is a common Home
    # button -- and the daemon passes every EV_KEY code the device reports
    # into the wizard unfiltered.
    home = [0xAC, 0x120, 0x121]
    r = run("snes", keys=home, axes={})
    tap(r, 0xAC)
    binding = r.bindings["a"]
    if binding.sdl() != "b2":
        fail(f"SDL was told {binding.sdl()!r}; SDL numbers a sub-BTN_JOYSTICK "
             f"code after the joystick ones, so 0xAC is b2 here")
    if mapping.retroarch_button_index(home, 0xAC) != mapping.RA_INVISIBLE:
        fail("RetroArch now numbers a code below BTN_MISC, or no longer says "
             "so distinctly -- ra_index=None means 'the two agree'")
    # It used to be stored with ra_index=None, which means "both consumers
    # agree", and was emitted to RetroArch as button 2 -- the number its udev
    # driver gives 0x121, a different button entirely. The Home key pressed
    # the wrong control in every game while Pegasus behaved, with nothing said.
    if binding.retroarch_visible():
        fail(f"the wizard stored {binding!r} for KEY_HOMEPAGE as though "
             f"RetroArch could name it; its udev driver cannot see any code "
             f"below BTN_MISC, so the number written would press whichever "
             f"real button happens to hold that index")
    if mapping.retroarch_lines(r.bindings, r.layout.retroarch_keys()):
        fail("an autoconfig line was written for a button RetroArch cannot "
             "see; the pad reads as configured and the control is dead")
    print("  ok  a code below BTN_MISC is marked as one RetroArch cannot see, "
          "and no autoconfig line is invented for it")


def main() -> int:
    check_every_layout_is_asked_in_order()
    check_arrow_points_at_the_prompt()
    check_a_full_walk_is_complete()
    check_layout_drawn_is_layout_walked()

    check_skip_needs_no_named_button()
    check_skip_works_with_a_claimed_button()
    check_skipping_leaves_a_hole()
    check_skipping_everything_stores_nothing()

    check_the_opening_press_answers_nothing()
    check_one_input_cannot_answer_two_prompts()

    check_sdl_round_trip()
    check_retroarch_round_trip()
    check_n64_keys_are_crossed()
    check_n64_c_buttons_are_a_stick()
    check_arcade_buttons_are_mame_numbers()
    check_axis_bindings_go_under_the_axis_key()
    check_a_captured_axis_is_not_also_a_stick()
    check_a_pad_with_too_few_buttons()

    check_layout_picker_offers_every_layout()
    check_scope_picker_promises_the_pad_it_walks()
    check_scope_picker_deduplicates_and_marks()
    check_scope_picker_unknown_console()
    check_game_scope_picker_is_two_entries()
    check_scope_picker_is_worked_from_the_pad()

    check_capture_reaches_retroarch()
    check_per_game_scope_survives_to_the_launcher()

    check_done_event_hands_over_to_calibration()
    check_recalibration_keeps_the_capture()
    check_button_numbering_is_kept_for_both()

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
