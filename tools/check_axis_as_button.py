"""A pad whose only spare inputs are axes must still be able to answer.

The GameCube layout asks for X and Y. An N64 pad has neither, and it does not
report its C cluster as buttons either -- on the adapter here they arrive as
ABS_Z and ABS_RZ. So the only thing left to answer those prompts with is an
axis.

The wizard used to refuse every axis for a `kind="button"` control, flatly and
with no output. Pressing C-up when asked for Y did nothing: no binding, no log
line, no change on screen. Indistinguishable from a dead button or a hung
wizard, and unanswerable -- the prompt could not be satisfied by any input the
pad had.

The refusal was not arbitrary. Binding a face button to an axis by accident is
worse than any other misbinding, because the axis is usually also the stick:
every later stick movement then presses that button. A mapping really did end
up with cancel on `-a3` that way.

So the rule is now about deliberateness rather than kind. Four properties:

  * an axis pushed to the stop answers a face button -- the N64-on-GameCube
    case, which is the entire reason this changed
  * a nudge does not, at a deflection that would satisfy any other control.
    This is the original bug and it must stay fixed
  * a refused nudge says so, instead of vanishing
  * a stick control still takes the ordinary threshold, so raising the bar for
    face buttons did not quietly raise it for everything

Nothing here opens a device.

    python3 tools/check_axis_as_button.py
"""

from __future__ import annotations

import logging
import os
import sys
import tempfile
from pathlib import Path

_SANDBOX = tempfile.mkdtemp(prefix="padmap-axisbutton-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import capture, layouts  # noqa: E402

# The N64 adapter's C axis, as it actually reports: 0..255, resting mid.
ABS_Z = 0x02
SPAN = (0, 255, 128)


class Event:
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


def run_at(layout_id: str, canonical: str, clock: Clock) -> capture.MappingRun:
    """A run parked on one named control, so the prompt under test is current."""
    layout = layouts.get(layout_id)
    run = capture.MappingRun(
        pad=None, player=1, layout=layout,
        keys=[0x130 + n for n in range(16)],
        axes={ABS_Z: SPAN}, now=clock,
    )
    for i, control in enumerate(layout.controls):
        if control.canonical == canonical:
            run.index = i
            return run
    raise SystemExit(f"FAIL: {layout_id} has no control {canonical!r}; this "
                     f"check is testing something that no longer exists")


def push(run: capture.MappingRun, value: int, clock: Clock) -> bool:
    accepted = run.feed(Event(capture.EV_ABS, ABS_Z, value))
    clock.advance(capture.CAPTURE_GAP_SECONDS + 0.05)
    return accepted


def deflection_of(value: int) -> float:
    return abs(capture.deflection(SPAN, value))


def check_a_full_push_answers_a_face_button() -> None:
    print("\nan axis pushed to the stop answers a face button:")
    clock = Clock()
    run = run_at("gamecube", "y", clock)
    if run.current.kind != "button":
        raise SystemExit("FAIL: GameCube 'y' is no longer kind='button'; this "
                         "check is no longer testing the reported bug")

    if not push(run, 255, clock):
        raise SystemExit(
            "FAIL: C-up held against the stop did not answer the Y prompt. "
            "An N64 pad has nothing else to offer for Y, so the GameCube "
            "mapping cannot be completed at all -- which is the exact report: "
            "'when i press the n64 c up for gamecube y ... it doesn't respond'")
    if "y" not in run.bindings:
        raise SystemExit("FAIL: the push was accepted but bound nothing to y")
    bound = run.bindings["y"]
    if bound.kind != "axis":
        raise SystemExit(f"FAIL: y was bound as {bound.kind!r}, not an axis")
    print(f"  ok  y -> axis {bound.index} sign {bound.value} "
          f"(deflection {deflection_of(255):.2f})")


def check_a_nudge_still_does_not() -> None:
    print("\na nudge does not -- the original bug stays fixed:")
    clock = Clock()
    run = run_at("gamecube", "y", clock)

    # Chosen to clear the ordinary threshold and miss the face-button one, so
    # this fails the moment the two are collapsed back into one number.
    nudge = 128 + int(0.70 * (255 - 0) / 2)
    got = deflection_of(nudge)
    if not (capture.AXIS_THRESHOLD <= got < capture.AXIS_AS_BUTTON_THRESHOLD):
        raise SystemExit(
            f"FAIL: the test nudge deflects {got:.2f}, which is not between "
            f"AXIS_THRESHOLD ({capture.AXIS_THRESHOLD}) and "
            f"AXIS_AS_BUTTON_THRESHOLD ({capture.AXIS_AS_BUTTON_THRESHOLD}). "
            f"This check can no longer tell the two apart")

    if push(run, nudge, clock):
        raise SystemExit(
            f"FAIL: a {got:.2f} nudge bound the face button Y to an axis. "
            f"That axis is also the stick, so every later stick movement "
            f"would press Y for the rest of the session")
    if run.bindings:
        raise SystemExit("FAIL: a refused nudge recorded a binding")
    print(f"  ok  refused at {got:.2f}")


def check_the_refusal_is_logged() -> None:
    print("\n...and says so rather than vanishing:")
    clock = Clock()
    run = run_at("gamecube", "y", clock)
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
        push(run, 128 + int(0.70 * 255 / 2), clock)
    finally:
        log.removeHandler(handler)
        log.setLevel(previous)

    if not handler.lines:
        raise SystemExit(
            "FAIL: the nudge was refused silently. A user who does not know "
            "about the threshold sees a prompt that ignores them, which is "
            "the failure mode this whole change is about")
    print(f"  ok  {handler.lines[0]}")


def check_a_stick_keeps_the_ordinary_threshold() -> None:
    print("\na stick control still takes the ordinary threshold:")
    clock = Clock()
    run = run_at("gamecube", "rightstick_up", clock)
    if run.current.kind != "stick":
        raise SystemExit("FAIL: GameCube 'rightstick_up' is not kind='stick'")

    nudge = 128 + int(0.70 * 255 / 2)
    if not push(run, nudge, clock):
        raise SystemExit(
            f"FAIL: {deflection_of(nudge):.2f} was refused for a stick "
            f"control. Raising the bar for face buttons must not raise it for "
            f"everything -- every stick prompt would now need a push to the "
            f"stop")
    print(f"  ok  accepted at {deflection_of(nudge):.2f}")


def main() -> int:
    check_a_full_push_answers_a_face_button()
    check_a_nudge_still_does_not()
    check_the_refusal_is_logged()
    check_a_stick_keeps_the_ordinary_threshold()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
