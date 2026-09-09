"""A wizard prompt must not also be delivered to the front-end.

Reported on a Switch Pro: the mapping wizard asks "press B", the user presses
B, and Pegasus exits the configuration screen. The controller was fine and the
grab was fine -- the leak is that the pad is being republished at the same time.

The daemon holds EVIOCGRAB on the *physical* pad, so the front-end genuinely
cannot see that one. But the same pad is cloned as `padmap Player N`, and the
clone is the only thing the front-end ever watches. So every answer to a prompt
arrived at the UI as ordinary controller input, and on any pad whose B sits
where the front-end's cancel is, the step cancelled itself with the button it
had just asked for. Nothing about it is Switch-specific.

Four things, and the third is the one that is easy to get wrong:

  * while a wizard is open, presses must not reach the clone
  * with no wizard open, they must -- a pause that never lifts is just a
    controller that stopped working
  * the source must still be *drained* while paused. An evdev node that is not
    read fills up; the backlog would then arrive at the front-end in one burst
    the moment the wizard closed, which is the same bug with a delay
  * anything held when the pause starts must be released on the clone, or the
    press is forwarded, the release is dropped, and the virtual pad is left
    with a button down and nothing to lift it -- the shape of the stuck-input
    fault that made exiting a game immediately start another

Nothing here opens a real device: the pads are fakes, and the server's
republisher is replaced before any command is dispatched.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \\
        python3 tools/check_wizard_input_leak.py
"""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

_SANDBOX = tempfile.mkdtemp(prefix="padmap-leak-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from evdev import ecodes  # noqa: E402

from padmap import virtual  # noqa: E402
from padmap.devices import Pad  # noqa: E402

BTN_A = 0x130
BTN_B = 0x131


class FakeEvent:
    def __init__(self, type_: int, code: int, value: int) -> None:
        self.type = type_
        self.code = code
        self.value = value


class FakeSource:
    """A physical pad we own entirely: it emits what the test tells it to."""

    def __init__(self, fd: int = 901) -> None:
        self.fd = fd
        self.queued: list[FakeEvent] = []
        self.reads = 0
        self.held: list[int] = []

    def read(self):
        self.reads += 1
        out, self.queued = self.queued, []
        return out

    def active_keys(self):
        return list(self.held)

    def ungrab(self):
        return None

    def close(self):
        return None


class FakeUI:
    """The clone. Records everything written, which is the whole question."""

    def __init__(self, fd: int = 902) -> None:
        self.fd = fd
        self.written: list[tuple[int, int, int]] = []
        self.syns = 0

    def write_event(self, event) -> None:
        self.written.append((event.type, event.code, event.value))

    def write(self, type_: int, code: int, value: int) -> None:
        self.written.append((type_, code, value))

    def syn(self) -> None:
        self.syns += 1

    def close(self) -> None:
        return None


def build() -> tuple[virtual.Republisher, FakeSource, FakeUI]:
    pad = Pad(path="/dev/input/event901", name="Fake Pro Controller",
              phys="usb-fake", uniq="", vid=0x057E, pid=0x2009, syspath="")
    source, ui = FakeSource(), FakeUI()
    vpad = virtual.VirtualPad(player=1, pad=pad, source=source, ui=ui)
    return virtual.Republisher([vpad]), source, ui


def press(source: FakeSource, code: int) -> None:
    source.queued.append(FakeEvent(ecodes.EV_KEY, code, 1))


def check_presses_reach_the_clone_normally() -> None:
    print("\nwith no wizard open, presses reach the clone:")
    rep, source, ui = build()
    press(source, BTN_A)
    rep.handle_readable(source.fd)
    if (ecodes.EV_KEY, BTN_A, 1) not in ui.written:
        raise SystemExit(
            "FAIL: an ordinary press did not reach the virtual pad, so the "
            "controller does not work at all outside a wizard")
    print(f"  ok  {len(ui.written)} event(s) forwarded")


def check_presses_are_withheld_while_paused() -> None:
    print("\nwhile a wizard is open, presses do NOT reach the clone:")
    rep, source, ui = build()
    rep.set_paused(True)
    press(source, BTN_B)
    rep.handle_readable(source.fd)
    leaked = [w for w in ui.written if w[:2] == (ecodes.EV_KEY, BTN_B)]
    if leaked:
        raise SystemExit(
            f"FAIL: B reached the front-end while a wizard was open "
            f"({leaked!r}). This is the reported bug: the wizard asks for B, "
            f"Pegasus reads the same B as 'back', and the configuration screen "
            f"exits on the button it just requested")
    print("  ok  nothing forwarded")


def check_the_source_is_still_drained_while_paused() -> None:
    print("\n...but the source is still read, so no backlog builds up:")
    rep, source, ui = build()
    rep.set_paused(True)
    press(source, BTN_A)
    rep.handle_readable(source.fd)
    if source.reads == 0:
        raise SystemExit(
            "FAIL: the source was not read while paused. An evdev node that "
            "is not drained fills up, and every withheld press would then "
            "arrive at the front-end at once when the wizard closed -- the "
            "same bug, delayed")
    if source.queued:
        raise SystemExit(
            f"FAIL: {len(source.queued)} event(s) left queued on the source")
    print(f"  ok  source read {source.reads} time(s), queue empty")


def check_held_buttons_are_released_on_pause() -> None:
    print("\na button held as the wizard opens is released on the clone:")
    rep, source, ui = build()
    source.held = [BTN_A]
    rep.set_paused(True)
    releases = [w for w in ui.written
                if w == (ecodes.EV_KEY, BTN_A, 0)]
    if not releases:
        raise SystemExit(
            "FAIL: a held button was not released on the virtual pad. Its "
            "press was already forwarded and its release will be dropped "
            "during the pause, so the clone is left with a button down and "
            "nothing to lift it -- a stuck input for as long as the daemon "
            "runs")
    if ui.syns == 0:
        raise SystemExit("FAIL: releases were written but never synced, so "
                         "no consumer ever sees them")
    print(f"  ok  {len(releases)} release(s) written and synced")


def check_resuming_restores_forwarding() -> None:
    print("\nclosing the wizard restores forwarding:")
    rep, source, ui = build()
    rep.set_paused(True)
    press(source, BTN_A)
    rep.handle_readable(source.fd)
    rep.set_paused(False)
    before = len(ui.written)
    press(source, BTN_A)
    rep.handle_readable(source.fd)
    if len(ui.written) == before:
        raise SystemExit(
            "FAIL: presses still do not reach the clone after the wizard "
            "closed. A pause that never lifts is a controller that stopped "
            "working, which is worse than the bug it fixes")
    print("  ok  forwarding resumed")


def check_the_daemon_pauses_for_each_modal_flow() -> None:
    print("\nthe daemon pauses the clone for every modal flow:")
    from padmap import devices, server
    devices.discover = lambda *a, **k: []    # type: ignore[assignment]

    srv = server.Server()
    srv._broadcast = lambda message: None    # type: ignore[assignment]
    rep, _source, _ui = build()
    srv._republisher = rep                   # type: ignore[assignment]

    for attribute in ("_mapping", "_calibration", "_choice"):
        setattr(srv, attribute, object())
        srv._sync_republish_pause()
        if not rep.paused:
            raise SystemExit(
                f"FAIL: the republisher was not paused while {attribute} was "
                f"open, so that wizard's prompts still reach the front-end")
        setattr(srv, attribute, None)
        srv._sync_republish_pause()
        if rep.paused:
            raise SystemExit(
                f"FAIL: the republisher stayed paused after {attribute} "
                f"closed -- the controller would be dead from then on")
        print(f"  ok  {attribute}: paused while open, resumed after")


def main() -> int:
    check_presses_reach_the_clone_normally()
    check_presses_are_withheld_while_paused()
    check_the_source_is_still_drained_while_paused()
    check_held_buttons_are_released_on_pause()
    check_resuming_restores_forwarding()
    check_the_daemon_pauses_for_each_modal_flow()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
