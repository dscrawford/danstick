"""Two of the same controller must resolve to two different hidraw nodes.

Vendor and product do not identify a controller. They identify a *model*. Two
Switch Pro Controllers both report 057e:2009, and `node_for` used to return
the first hidraw node whose uevent carried those ids -- to every pad that
asked.

So both players read the same physical controller. One pad does nothing at
all, while every artefact says the setup is fine: two clones exist, two
profiles are written, and the log says both are forwarding. The only trace is
a line nobody reads closely:

    republisher watching fds [...] for [(1, 6, '/dev/hidraw10'),
                                        (2, 10, '/dev/hidraw10'), ...]

the same node, twice. Observed exactly that way once a second Pro Controller
was paired.

Sorting made it arbitrary rather than merely first-come: `sorted()` on
"hidraw10" and "hidraw8" is a string sort, so a machine that had been working
on hidraw8 for weeks silently moves everybody to hidraw10 the moment a second
pad appears.

Five properties:

  * two identical pads get their own node each, via the sysfs parent link
  * that link wins over anything id-based, since it is the only exact one
  * HID_UNIQ resolves a pad whose sysfs walk fails -- on Bluetooth it is the
    controller's own address
  * a single pad still resolves when nothing else identifies it
  * an ambiguous pad resolves to *nothing*, and says so. Handing back the
    wrong controller cannot be diagnosed from anything the pad reports

Built against a constructed sysfs tree: the real one needs two identical
controllers plugged in, which is exactly why this went unnoticed.

    python3 tests/check_hidraw_node_identity.py
"""

from __future__ import annotations

import logging
import os
import sys
import tempfile
from pathlib import Path

_SANDBOX = tempfile.mkdtemp(prefix="padmap-nodeid-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import hidraw  # noqa: E402
from padmap.devices import Pad  # noqa: E402

VID, PID = 0x057E, 0x2009


class Tree:
    """A sysfs layout shaped like the real one, under a temp directory.

    /sys/class/input/eventN/device -> .../<hid>/input/inputN
    /sys/class/hidraw/hidrawN/device -> .../<hid>
    .../<hid>/hidraw/hidrawN

    The `hidraw` subdirectory hanging off the HID device is the parent-child
    link the resolution relies on, so it has to be real here rather than
    implied.
    """

    def __init__(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="padmap-sysfs-"))
        (self.root / "class" / "input").mkdir(parents=True)
        (self.root / "class" / "hidraw").mkdir(parents=True)
        self.devices = self.root / "devices"
        self.devices.mkdir()

    def add(self, tag: str, event: str, hidraw_name: str, uniq: str,
            link_input: bool = True, link_hidraw: bool = True) -> None:
        hid = self.devices / tag
        (hid / "input" / f"input_{tag}").mkdir(parents=True)
        (hid / "uevent").write_text(
            f"HID_ID=0005:{VID:08X}:{PID:08X}\n"
            f"HID_PHYS=aa:bb:cc:dd:ee:ff\n"
            f"HID_UNIQ={uniq}\n")

        if link_hidraw:
            (hid / "hidraw").mkdir()
            (hid / "hidraw" / hidraw_name).mkdir()

        # /sys/class/hidraw/<name>/device -> the HID device
        node = self.root / "class" / "hidraw" / hidraw_name
        node.mkdir(exist_ok=True)
        if not (node / "device").exists():
            (node / "device").symlink_to(hid)

        # /sys/class/input/<event>/device -> the input device under it
        ev = self.root / "class" / "input" / event
        ev.mkdir(exist_ok=True)
        if link_input:
            (ev / "device").symlink_to(hid / "input" / f"input_{tag}")

    def use(self) -> None:
        hidraw.SYS_CLASS = str(self.root / "class")


def pad(event: str, uniq: str = "") -> Pad:
    return Pad(path=f"/dev/input/{event}", name="Pro Controller",
               phys="aa:bb:cc:dd:ee:ff", uniq=uniq, vid=VID, pid=PID,
               syspath="")


class Captured(logging.Handler):
    def __init__(self) -> None:              # noqa: D107
        super().__init__()
        self.lines: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.lines.append(record.getMessage())


def check_two_identical_pads_get_their_own_node() -> None:
    print("\ntwo identical controllers resolve to two different nodes:")
    t = Tree()
    # hidraw10 deliberately first: it is what a string sort picks, so if the
    # id-based path is taken this check fails in the same way the bug did.
    t.add("devA", "event26", "hidraw10", "98:B6:E9:B1:6B:14")
    t.add("devB", "event30", "hidraw8", "70:F0:88:C4:3C:FD")
    t.use()

    a = hidraw.node_for(pad("event26"))
    b = hidraw.node_for(pad("event30"))
    print(f"  event26 -> {a}")
    print(f"  event30 -> {b}")
    if a is None or b is None:
        raise SystemExit(f"FAIL: a pad resolved to nothing ({a}, {b})")
    if a == b:
        raise SystemExit(
            f"FAIL: both controllers resolved to {a}. Two players then read "
            f"one physical pad: the other does nothing, while two clones "
            f"exist and the log says both are forwarding")
    if a != "/dev/hidraw10" or b != "/dev/hidraw8":
        raise SystemExit(
            f"FAIL: resolved to the wrong nodes ({a}, {b}); each pad must get "
            f"the hidraw hanging off its own HID device")
    print("  ok  each pad got its own")


def check_the_sysfs_link_beats_ids() -> None:
    print("\nthe device link wins over vendor/product:")
    t = Tree()
    # Both claim the same uniq, so only the parent link can separate them.
    t.add("devA", "event26", "hidraw10", "SAME")
    t.add("devB", "event30", "hidraw8", "SAME")
    t.use()
    if hidraw.node_for(pad("event30", "SAME")) != "/dev/hidraw8":
        raise SystemExit(
            "FAIL: an id-based match was preferred to the pad's own device")
    print("  ok  resolved through the device, not the ids")


def check_uniq_resolves_when_the_walk_fails() -> None:
    print("\nHID_UNIQ resolves a pad whose sysfs walk fails:")
    t = Tree()
    t.add("devA", "event26", "hidraw10", "98:B6:E9:B1:6B:14")
    # No /sys/class/input/event30/device at all: nothing to walk up from.
    t.add("devB", "event30", "hidraw8", "70:F0:88:C4:3C:FD",
          link_input=False)
    t.use()
    got = hidraw.node_for(pad("event30", "70:F0:88:C4:3C:FD"))
    if got != "/dev/hidraw8":
        raise SystemExit(
            f"FAIL: resolved to {got}; the pad's own Bluetooth address "
            f"identifies it even when the device tree does not")
    print("  ok  matched on the controller's own address")


def check_a_lone_pad_still_resolves() -> None:
    print("\na single pad resolves even with nothing to identify it:")
    t = Tree()
    t.add("devA", "event26", "hidraw8", "", link_input=False)
    t.use()
    got = hidraw.node_for(pad("event26"))
    if got != "/dev/hidraw8":
        raise SystemExit(
            f"FAIL: resolved to {got}. Refusing to guess must not break the "
            f"ordinary one-controller case, which is nearly everybody")
    print("  ok  unambiguous, so it is taken")


def check_an_ambiguous_pad_refuses_to_guess() -> None:
    print("\n...but two indistinguishable candidates resolve to nothing:")
    t = Tree()
    t.add("devA", "event26", "hidraw10", "", link_input=False)
    t.add("devB", "event30", "hidraw8", "", link_input=False)
    t.use()

    handler = Captured()
    log = logging.getLogger("padmap.hidraw")
    previous = log.level
    log.setLevel(logging.INFO)
    log.addHandler(handler)
    try:
        got = hidraw.node_for(pad("event26"))
    finally:
        log.removeHandler(handler)
        log.setLevel(previous)

    if got is not None:
        raise SystemExit(
            f"FAIL: guessed {got} with no way to tell two controllers apart. "
            f"A pad reading someone else's input reports nothing wrong -- it "
            f"is simply dead, and looks configured")
    if not handler.lines:
        raise SystemExit(
            "FAIL: refused silently. Falling back to evdev for a pad that "
            "needs hidraw means no input, so the reason has to be on record")
    print(f"  ok  {handler.lines[0]}")


def main() -> int:
    check_two_identical_pads_get_their_own_node()
    check_the_sysfs_link_beats_ids()
    check_uniq_resolves_when_the_walk_fails()
    check_a_lone_pad_still_resolves()
    check_an_ambiguous_pad_refuses_to_guess()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
