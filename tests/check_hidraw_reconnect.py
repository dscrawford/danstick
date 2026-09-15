"""A pad that reconnects must be picked up again, not silently dropped.

A Bluetooth controller drops and comes back. The kernel gives it a new uhid
instance and usually a new hidraw number, and the descriptor padmap is holding
becomes this:

    fd 6 -> /dev/hidraw9 (deleted)

It is not closed. Reading it does not error. It is simply never readable
again -- so the selector never fires, `_forward` never runs, and the read path
never gets the chance to notice. Input stops with nothing logged, nothing
failed, and the daemon still reporting that it is forwarding, because that line
is written once.

That is what makes it worth a check of its own. The evdev equivalent is
self-announcing: a vanished evdev node reports readable and returns ENODEV,
which `_forward` already handles. Nothing in the event path can ever see the
hidraw case, so detection has to be a poll, and a poll is easy to delete by
accident while tidying.

Three properties:

  * a source whose device is unchanged is not disturbed
  * a source whose node has vanished is reported stale
  * a source whose node number has been reused by a *different* device is also
    reported stale -- existence is not enough, it has to be the same device

Nothing here opens a real controller.

    python3 tests/check_hidraw_reconnect.py
"""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

_SANDBOX = tempfile.mkdtemp(prefix="padmap-reconnect-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import hidraw, virtual  # noqa: E402
from padmap.devices import Pad  # noqa: E402


def pad() -> Pad:
    return Pad(path="/dev/input/event900", name="Fake Pro Controller",
               phys="bt-fake", uniq="", vid=0x057E, pid=0x2009, syspath="")


class FakeSource(hidraw.Source):
    """A hidraw source whose node is an ordinary file we can move about.

    Subclassed rather than mocked so the real `alive` runs -- that method is
    the whole subject.
    """

    def __init__(self, node: Path) -> None:      # noqa: D107
        self.pad = pad()
        self.path = str(node)
        self.name = self.pad.name
        self._fd = -1
        self._buttons, self._axes, self._hat = {}, {}, (0, 0)
        self._counter = 0
        self._rdev = os.stat(node).st_rdev


class FakeUI:
    fd = 902

    def write_event(self, event) -> None: ...
    def write(self, *_a) -> None: ...
    def syn(self) -> None: ...
    def close(self) -> None: ...


def check_an_unchanged_device_is_left_alone() -> None:
    print("\na pad whose device has not changed is not disturbed:")
    node = Path(_SANDBOX) / "hidraw-live"
    node.write_bytes(b"")
    source = FakeSource(node)
    if not source.alive():
        raise SystemExit(
            "FAIL: an untouched source reported itself stale. Every poll would "
            "then tear down and rebuild the republisher, which drops the "
            "virtual pads the front-end is reading")
    vpad = virtual.VirtualPad(player=1, pad=pad(), source=source, ui=FakeUI())
    rep = virtual.Republisher([vpad])
    if rep.stale_sources():
        raise SystemExit("FAIL: republisher reported a healthy pad as stale")
    print("  ok  alive, and not reported stale")


def check_a_vanished_node_is_reported() -> None:
    print("\na pad whose node has gone is reported stale:")
    node = Path(_SANDBOX) / "hidraw-vanishes"
    node.write_bytes(b"")
    source = FakeSource(node)
    node.unlink()                    # the reconnect: old node destroyed

    if source.alive():
        raise SystemExit(
            "FAIL: the node is gone and the source still calls itself alive. "
            "The held descriptor never becomes readable again, so nothing "
            "downstream can notice -- input stops with nothing logged")
    vpad = virtual.VirtualPad(player=1, pad=pad(), source=source, ui=FakeUI())
    rep = virtual.Republisher([vpad])
    if not rep.stale_sources():
        raise SystemExit(
            "FAIL: republisher did not report the vanished pad, so the daemon "
            "never reopens it and the controller stays dead until a restart")
    print("  ok  alive()=False and reported stale")


def check_a_reused_node_number_is_reported() -> None:
    print("\n...and a node number reused by a different device is too:")
    node = Path(_SANDBOX) / "hidraw-reused"
    node.write_bytes(b"")
    source = FakeSource(node)
    # Same path, different device: what happens when the kernel hands the old
    # number to something else. Existence alone would call this healthy.
    source._rdev = (source._rdev or 0) + 1

    if source.alive():
        raise SystemExit(
            "FAIL: the path exists but is a different device, and the source "
            "called itself alive. Checking existence rather than identity "
            "means padmap would happily read another device's reports")
    print("  ok  identity checked, not just existence")


def check_gone_pads_are_not_double_reported() -> None:
    print("\na pad already marked gone is not reported again:")
    node = Path(_SANDBOX) / "hidraw-gone"
    node.write_bytes(b"")
    source = FakeSource(node)
    node.unlink()
    vpad = virtual.VirtualPad(player=1, pad=pad(), source=source, ui=FakeUI())
    vpad.gone = True
    rep = virtual.Republisher([vpad])
    if rep.stale_sources():
        raise SystemExit(
            "FAIL: a pad already torn down was reported stale, so the daemon "
            "would rebuild the republisher on every poll for ever")
    print("  ok  skipped")


def main() -> int:
    check_an_unchanged_device_is_left_alone()
    check_a_vanished_node_is_reported()
    check_a_reused_node_number_is_reported()
    check_gone_pads_are_not_double_reported()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
