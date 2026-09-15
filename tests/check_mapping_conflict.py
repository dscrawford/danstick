"""A press the wizard refuses must say why, not just do nothing.

One input may only answer one prompt. That rule is necessary -- an axis
springing back through centre, or a button still travelling, would otherwise
fill in several controls from a single push -- and it is enforced by a silent
`return False`.

Silence is fine when the refusal is the machine correcting an accident. It is
not fine when the user meant it, and they mean it whenever the pad has fewer
inputs than the layout has controls. Mapping an N64 pad against the GameCube
layout is the case that prompted this: the pad's only spare axis halves are
the C directions, whichever prompt reaches them first takes them, and every
later prompt for that same stick is refused with no output of any kind. The
button is not dead, the control is not unsupported, the wizard has not hung --
but those are the only three conclusions available to someone watching.

Four properties:

  * a re-used input is refused, and names the control already holding it
  * the refusal is visible in the event the front-end renders, so the screen
    can say it rather than sitting unchanged
  * a fresh input is still accepted, and clears any conflict left behind --
    otherwise a stale name is pinned under a later, unrelated control
  * the refusal does not advance the wizard or record a binding

Nothing here opens a device.

    python3 tests/check_mapping_conflict.py
"""

from __future__ import annotations

import logging
import os
import sys
import tempfile
from pathlib import Path

_SANDBOX = tempfile.mkdtemp(prefix="padmap-conflict-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import capture, layouts  # noqa: E402


class Event:
    """The two fields MappingRun reads off an evdev event."""

    def __init__(self, type_: int, code: int, value: int) -> None:
        self.type, self.code, self.value = type_, code, value


class Clock:
    def __init__(self) -> None:
        self.t = 1000.0

    def __call__(self) -> float:
        return self.t

    def advance(self, by: float = 1.0) -> None:
        self.t += by


class Captured(logging.Handler):
    def __init__(self) -> None:              # noqa: D107
        super().__init__()
        self.lines: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.lines.append(record.getMessage())


def run_for(layout_id: str, clock: Clock) -> capture.MappingRun:
    # Buttons the pad reports, in evdev order. BTN_SOUTH upwards, which is
    # what `sdl_button_index` expects to find.
    keys = [0x130 + n for n in range(16)]
    return capture.MappingRun(
        pad=None, player=1, layout=layouts.get(layout_id), keys=keys,
        now=clock,
    )


def press(run: capture.MappingRun, code: int, clock: Clock) -> bool:
    """A full press and release, long enough to bind but not to skip."""
    run.feed(Event(capture.EV_KEY, code, 1))
    clock.advance(0.10)
    accepted = run.feed(Event(capture.EV_KEY, code, 0))
    clock.advance(capture.CAPTURE_GAP_SECONDS + 0.05)
    return accepted


def check_a_reused_input_is_named() -> None:
    print("\na re-used input is refused, and names what holds it:")
    clock = Clock()
    run = run_for("gamecube", clock)
    first = run.layout.controls[0].canonical

    handler = Captured()
    log = logging.getLogger("padmap.capture")
    # setLevel, not `log.level = ...`: assigning the attribute
    # leaves Logger._cache holding an isEnabledFor answer worked
    # out under the old level, so a logger that has already
    # declined an INFO record goes on declining it.
    previous = log.level
    log.setLevel(logging.INFO)
    log.addHandler(handler)
    try:
        if not press(run, 0x130, clock):
            raise SystemExit("FAIL: the first press was not accepted at all; "
                             "this check cannot test anything further")
        index_before = run.index
        bindings_before = dict(run.bindings)

        # The same physical button again, now for the second control.
        if press(run, 0x130, clock):
            raise SystemExit(
                "FAIL: one button answered two prompts. A single press would "
                "walk several controls and the mapping would be nonsense")
    finally:
        log.removeHandler(handler)
        log.setLevel(previous)

    if not handler.lines:
        raise SystemExit(
            "FAIL: the press was refused and nothing was logged. From the "
            "sofa that is indistinguishable from a dead button, an "
            "unsupported control, or a hung wizard -- and it is the exact "
            "report this check exists for: 'it doesn't accept it'")
    said = " ".join(handler.lines)
    if first not in said:
        raise SystemExit(
            f"FAIL: the refusal did not name {first!r}, the control already "
            f"holding that input. Without the name there is no way to know "
            f"which earlier prompt to answer differently.\n  logged: {said}")
    if run.index != index_before:
        raise SystemExit("FAIL: a refused press advanced the wizard")
    if run.bindings != bindings_before:
        raise SystemExit("FAIL: a refused press recorded a binding")
    print(f"  ok  {said}")


def check_the_refusal_reaches_the_front_end() -> None:
    print("\nthe refusal is visible in the event the theme renders:")
    clock = Clock()
    run = run_for("gamecube", clock)
    first = run.layout.controls[0].canonical
    press(run, 0x131, clock)
    press(run, 0x131, clock)              # refused

    event = run.to_event()
    if "conflict" not in event:
        raise SystemExit(
            "FAIL: to_event() carries no 'conflict' field, so the screen has "
            "nothing to show and stays exactly as it was")
    if event["conflict"] != first:
        raise SystemExit(
            f"FAIL: conflict was {event['conflict']!r}, expected {first!r}")
    print(f"  ok  conflict={event['conflict']!r}")


def check_a_fresh_input_clears_it() -> None:
    print("\na fresh input is accepted and clears the conflict:")
    clock = Clock()
    run = run_for("gamecube", clock)
    press(run, 0x132, clock)
    press(run, 0x132, clock)              # refused, sets conflict
    if not run.conflict:
        raise SystemExit("FAIL: the refusal did not set a conflict to clear")

    if not press(run, 0x133, clock):      # a different button: must bind
        raise SystemExit(
            "FAIL: a previously unused button was refused. The wizard would "
            "be unfinishable")
    if run.conflict:
        raise SystemExit(
            f"FAIL: conflict {run.conflict!r} survived a successful capture, "
            f"so the screen keeps warning about a clash that is not "
            f"happening under a control that has nothing to do with it")
    print("  ok  accepted, conflict cleared")


def check_skipping_clears_it() -> None:
    print("\n...and so does skipping past a control:")
    clock = Clock()
    run = run_for("n64", clock)
    press(run, 0x134, clock)
    press(run, 0x134, clock)
    if not run.conflict:
        raise SystemExit("FAIL: the refusal did not set a conflict to clear")
    run.skip()
    if run.conflict:
        raise SystemExit(f"FAIL: conflict {run.conflict!r} survived a skip")
    print("  ok  cleared")


def main() -> int:
    check_a_reused_input_is_named()
    check_the_refusal_reaches_the_front_end()
    check_a_fresh_input_clears_it()
    check_skipping_clears_it()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
