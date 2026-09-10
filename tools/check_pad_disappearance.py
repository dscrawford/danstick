"""A controller vanishing must not spin, and must not fill the disk.

A Bluetooth pad dropped its connection. Its evdev node went away, and reading
it began returning ENODEV -- which the republisher handled by logging a line
and setting `self._stop`. Neither ended anything: `_stop` only breaks out of
`run()`, and the daemon does not call `run()`, it drives `handle_readable` from
its own selector. A dead node reports readable forever, so the loop re-entered
that branch continuously and wrote the same warning every time.

    /run/user/1000/padmap/padmap.log   3.1 GB   114,694,810 lines
    /run/user/1000                     3.2G  3.2G  0  100%

With the runtime tmpfs full, every later write there failed. What the user saw
was a game exiting instantly with code 1, because the launcher could not write
its autoconfig:

    padmap-play: line 54: printf: write error: No space left on device

Which points nowhere near a controller that quietly disconnected. That distance
between cause and symptom is the reason this file exists.

Three properties, and the second is the one that actually bounds the damage:

  * the disappearance is reported -- silence would be its own bug
  * it is reported ONCE, however many times the descriptor is serviced
  * the descriptor is offered up for unregistration, so the daemon can stop
    waking on it at all

Nothing here opens a real device.

    python3 tools/check_pad_disappearance.py
"""

from __future__ import annotations

import errno
import logging
import os
import sys
import tempfile
from pathlib import Path

_SANDBOX = tempfile.mkdtemp(prefix="padmap-gone-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import virtual  # noqa: E402
from padmap.devices import Pad  # noqa: E402


class GoneSource:
    """A physical pad that has been unplugged: every read is ENODEV."""

    def __init__(self, fd: int = 951) -> None:
        self.fd = fd
        self.reads = 0

    def read(self):
        self.reads += 1
        raise OSError(errno.ENODEV, "No such device")

    def active_keys(self):
        return []

    def ungrab(self):
        return None

    def close(self):
        return None


class FakeUI:
    def __init__(self, fd: int = 952) -> None:
        self.fd = fd

    def write_event(self, event) -> None:
        return None

    def write(self, *_a) -> None:
        return None

    def syn(self) -> None:
        return None

    def close(self) -> None:
        return None


class CountingHandler(logging.Handler):
    def __init__(self) -> None:
        super().__init__()
        self.records: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.records.append(record.getMessage())


def build():
    pad = Pad(path="/dev/input/event951", name="Vanishing Pad", phys="bt-fake",
              uniq="", vid=0x057E, pid=0x2009, syspath="")
    source, ui = GoneSource(), FakeUI()
    vpad = virtual.VirtualPad(player=1, pad=pad, source=source, ui=ui)
    return virtual.Republisher([vpad]), source, vpad


def check_disappearance_is_reported_once() -> None:
    print("\na pad that vanishes is reported, and reported once:")
    rep, source, _vpad = build()

    handler = CountingHandler()
    logger = logging.getLogger("padmap.virtual")
    logger.addHandler(handler)
    logger.setLevel(logging.WARNING)
    try:
        # However hard the selector hammers it.
        for _ in range(500):
            rep.handle_readable(source.fd)
    finally:
        logger.removeHandler(handler)

    if not handler.records:
        raise SystemExit(
            "FAIL: the pad vanished and nothing was logged at all. Silence is "
            "its own bug -- a controller that stops working should say so")
    if len(handler.records) != 1:
        raise SystemExit(
            f"FAIL: {len(handler.records)} log lines for one disappearance "
            f"across 500 wake-ups. A dead evdev node reports readable forever, "
            f"so this is unbounded: in the field it reached 114,694,810 lines "
            f"and 3.1GB, filling the runtime tmpfs. Everything that needed to "
            f"write there then failed -- the visible symptom was a game "
            f"exiting instantly because the launcher could not write its "
            f"autoconfig")
    print(f"  ok  1 line for 500 wake-ups: {handler.records[0]!r}")


def check_the_source_is_not_read_again() -> None:
    print("\n...and the dead source is not read again:")
    rep, source, _vpad = build()
    for _ in range(50):
        rep.handle_readable(source.fd)
    if source.reads > 1:
        raise SystemExit(
            f"FAIL: the vanished source was read {source.reads} times. Each "
            f"one is a syscall that can only fail; the point of noticing is to "
            f"stop asking")
    print(f"  ok  read {source.reads} time(s)")


def check_the_descriptor_is_offered_for_unregistration() -> None:
    print("\nthe descriptor is offered up so the daemon can stop waking on it:")
    rep, source, _vpad = build()
    if rep.dead_fds():
        raise SystemExit(
            "FAIL: the fd was reported dead before anything went wrong")
    rep.handle_readable(source.fd)
    if source.fd not in rep.dead_fds():
        raise SystemExit(
            "FAIL: the vanished pad's descriptor was not reported as dead. "
            "The republisher cannot unregister it itself -- the selector "
            "belongs to the daemon -- so without this it stays registered and "
            "the loop spins on it for as long as the daemon runs")
    print(f"  ok  dead_fds() reports {rep.dead_fds()}")


def check_a_live_pad_is_untouched() -> None:
    print("\na pad that is still there is unaffected:")
    from evdev import ecodes

    class LiveSource(GoneSource):
        def read(self):
            self.reads += 1
            return []

    pad = Pad(path="/dev/input/event952", name="Live Pad", phys="usb-fake",
              uniq="", vid=0x057E, pid=0x2009, syspath="")
    source = LiveSource(fd=953)
    vpad = virtual.VirtualPad(player=2, pad=pad, source=source, ui=FakeUI(954))
    rep = virtual.Republisher([vpad])
    for _ in range(10):
        rep.handle_readable(source.fd)
    if rep.dead_fds():
        raise SystemExit(
            "FAIL: a working pad was reported dead, so an ordinary controller "
            "would be dropped mid-game")
    if source.reads != 10:
        raise SystemExit(
            f"FAIL: a live source was read {source.reads}/10 times")
    print(f"  ok  read {source.reads} times, never marked dead "
          f"(ecodes loaded: {ecodes.EV_KEY == 1})")


def main() -> int:
    check_disappearance_is_reported_once()
    check_the_source_is_not_read_again()
    check_the_descriptor_is_offered_for_unregistration()
    check_a_live_pad_is_untouched()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
