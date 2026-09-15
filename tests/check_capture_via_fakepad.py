"""The mapping wizard, driven by the real controllers instead of ideal ones.

`capture.py` exists largely because of one adapter: a GameCube adapter whose
analogue triggers rest at 24 and 25 of 0-255 -- 81% deflected, untouched --
which broke the wizard three separate ways. A capture recorded the direction
the trigger was travelling *away* from; a resting report answered a prompt
nobody had touched; and re-arming waited for a return to a midpoint a trigger
never reaches, so after one press the trigger was dead and every later press
was dropped in silence.

`check_capture.py` and `check_axis_rest.py` test the *shape* of that against
hand-built spans -- a trigger resting at 0. `fakepad.MayflashGameCube` carries
the numbers the hardware actually reports, and this drives the wizard with
them. A regression that only appears at rest=24 would pass every other check
in the repository.

    nix develop --command python3 tests/check_capture_via_fakepad.py
"""

from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from evdev import ecodes  # noqa: E402

from padmap import fakepad, layouts, mapping  # noqa: E402
from padmap.capture import (CAPTURE_GAP_SECONDS, EV_ABS,  # noqa: E402
                            EV_KEY, MappingRun)

failures: list[str] = []


def check(name: str, condition: bool, detail: str = "") -> None:
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{(': ' + detail) if detail else ''}")
        failures.append(name)


class Incoming:
    """An event as it reaches capture, which reads only these three fields."""

    def __init__(self, type_: int, code: int, value: int) -> None:
        self.type, self.code, self.value = type_, code, value


class Clock:
    """A hand-wound clock, so a test never waits and never races."""

    def __init__(self) -> None:
        self.t = 0.0

    def __call__(self) -> float:
        return self.t

    def tick(self, seconds: float = 0.05) -> None:
        self.t += seconds


def spans(pad) -> dict[int, tuple[int, int, int]]:
    """{code: (minimum, maximum, rest)}, straight from the fixture."""
    return {axis.code: (axis.minimum, axis.maximum, axis.rest)
            for axis in pad.axes.values()}


def run_for(pad, layout_id: str) -> tuple[MappingRun, Clock]:
    clock = Clock()
    run = MappingRun(
        pad=None, player=1, layout=layouts.get(layout_id),
        keys=sorted(set(pad.buttons.values())), axes=spans(pad), now=clock)
    return run, clock


def feed(run: MappingRun, clock: Clock, events) -> bool:
    """Everything in a frame, as separate events.

    SYN and MSC are dropped: the daemon's poll loop is what turns a device's
    frames into the individual events that reach capture, and it forwards
    neither.
    """
    answered = False
    for event in events:
        if event.type in (EV_KEY, EV_ABS):
            if run.feed(Incoming(event.type, event.code, event.value)):
                answered = True
        clock.tick()
    return answered


def check_resting_frame_answers_nothing() -> None:
    """The literal reproduction: the wizard opens, the pad is untouched."""
    print("the GameCube adapter, untouched")
    cube = fakepad.get("gamecube")
    run, clock = run_for(cube, "gamecube")
    answered = feed(run, clock, cube.rest())
    check("its resting report answers no prompt", not answered,
          f"the wizard advanced to index {run.index} with nothing touched")
    check("and nothing was bound", not run.bindings, str(run.bindings))


def check_every_fixture_rests_quietly() -> None:
    """The same property, for every controller in the framework.

    A fixture that declared the wrong rest value would otherwise only be
    caught by its own coherence checks, never by the code that consumes it.
    """
    print("every fixture, untouched")
    for pad in fakepad.every():
        tag = type(pad).__name__
        layout = "gamecube" if isinstance(pad, fakepad.MayflashGameCube) \
            else "generic"
        run, clock = run_for(pad, layout)
        if isinstance(pad, fakepad.EvdevController):
            neutral = pad.rest()
        else:
            neutral = [fakepad.Event(ecodes.EV_ABS, axis.code, axis.rest)
                       for axis in pad.axes.values()]
        check(f"{tag}: answers nothing at rest",
              not feed(run, clock, neutral), f"advanced to {run.index}")


def check_a_trigger_can_be_pressed_twice() -> None:
    """The re-arm. After one press the trigger used to be dead for good.

    Walks the wizard to the layout's shoulder prompts -- the GameCube layout
    has `leftshoulder` then `rightshoulder` -- and presses the real trigger
    axis for each. The second is the one that matters: re-arming wanted the
    axis back near centre, and a trigger at rest is a full range away from it.
    """
    print("the GameCube adapter's triggers, pressed one after another")
    cube = fakepad.get("gamecube")
    run, clock = run_for(cube, "gamecube")
    feed(run, clock, cube.rest())
    clock.tick(CAPTURE_GAP_SECONDS)

    guard = 0
    while run.current is not None and guard < 40 and \
            run.current.canonical not in ("leftshoulder", "rightshoulder"):
        run.skip()
        clock.tick(CAPTURE_GAP_SECONDS)
        guard += 1
    check("the wizard reaches a shoulder prompt", run.current is not None
          and run.current.canonical in ("leftshoulder", "rightshoulder"),
          str(run.current.canonical if run.current else None))
    if run.current is None:
        return

    for control, axis in (("lt", ecodes.ABS_RX), ("rt", ecodes.ABS_RY)):
        prompt = run.current.canonical if run.current else None
        if prompt not in ("leftshoulder", "rightshoulder"):
            break
        # Pushed to the stop, then released back to its resting deflection --
        # not to zero, which is the value it never returns to.
        feed(run, clock, [fakepad.Event(ecodes.EV_ABS, axis, 255)])
        clock.tick(CAPTURE_GAP_SECONDS)
        answered = feed(run, clock, [fakepad.Event(
            ecodes.EV_ABS, axis, cube.axes[control].rest)])
        clock.tick(CAPTURE_GAP_SECONDS)
        check(f"pressing {control} answers {prompt!r}",
              answered or run.current is None
              or run.current.canonical != prompt,
              f"still on {prompt!r} at index {run.index}")


def check_stick_guessing_refuses_the_triggers() -> None:
    """`ABS_RX`/`ABS_RY` on this adapter are triggers, not the right stick.

    Guessing a stick from the axis code alone declared them one, and the pad
    read as permanently pushed to the left. The guard is that a stick rests
    centred and a trigger does not -- checked here against the fixture's real
    rest values rather than a hand-picked span.
    """
    print("mapping.stick_fields, against the adapter's real axes")
    cube = fakepad.get("gamecube")
    axes = spans(cube)
    check("the adapter's trigger axes do not rest centred",
          not mapping.rests_centred(axes[ecodes.ABS_RX])
          and not mapping.rests_centred(axes[ecodes.ABS_RY]),
          str((axes[ecodes.ABS_RX], axes[ecodes.ABS_RY])))
    fields = mapping.stick_fields(sorted(axes), {}, axes)
    check("so they are not declared as the right stick",
          "rightx" not in fields and "righty" not in fields, str(fields))

    xbox = fakepad.get("xbox360")
    xaxes = spans(xbox)
    check("an ordinary pad's sticks do rest centred, for contrast",
          mapping.rests_centred(xaxes[ecodes.ABS_RX]))
    xfields = mapping.stick_fields(sorted(xaxes), {}, xaxes)
    check("and are still declared",
          "rightx" in xfields and "righty" in xfields, str(xfields))


def main() -> int:
    for fn in (check_resting_frame_answers_nothing,
               check_every_fixture_rests_quietly,
               check_a_trigger_can_be_pressed_twice,
               check_stick_guessing_refuses_the_triggers):
        fn()
    if failures:
        print(f"\n{len(failures)} failure(s): {', '.join(failures)}")
        return 1
    print("\nall capture-via-fakepad checks pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
