"""Which pads take the hidraw path must not be a list of product ids.

It used to be exactly that:

    SWITCH_PRO = (0x057E, 0x2009)
    SUPPORTED = {SWITCH_PRO}

One controller. Every other pad that needs hidraw -- a Joy-Con, a SNES or N64
pad for Switch Online, any Nintendo model released after that line was written
-- fell through to the evdev path, where the node opens, grabs and watches
without ever emitting an event. A pad that does nothing, with no error
anywhere, because a number was missing from a set.

A product id names a product. What decides whether this module can read a
device is which *protocol* it speaks, and the kernel already says so: the
driver bound to the HID device. `hid-nintendo` binds the Pro Controller, both
Joy-Cons and the Online pads, and they share the output-report and subcommand
protocol implemented here. One driver name covers the family, including
hardware that did not exist yet.

Five properties:

  * a pad bound to a known driver is taken, with a product id that appears
    nowhere in padmap -- if this passes only for 057e:2009, the allowlist is
    back
  * a pad bound to an unrelated driver is left on evdev, whoever makes it
  * an override can switch a pad on that no driver rule matches
  * an override can switch a matched pad off
  * a pad whose device cannot be read is left on evdev rather than guessed at

Built against a constructed sysfs tree; nothing here needs a controller.

    python3 tools/check_hidraw_supported.py
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

_SANDBOX = tempfile.mkdtemp(prefix="padmap-supported-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import hidraw  # noqa: E402
from padmap.devices import Pad  # noqa: E402

# Deliberately not a Nintendo id, and not one padmap mentions anywhere. A
# driver-based rule must take this pad; an id-based one cannot.
MADE_UP_VID, MADE_UP_PID = 0x1234, 0xABCD


class Tree:
    """A sysfs layout with a driver on the HID device, under a temp dir."""

    def __init__(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="padmap-sysfs-"))
        (self.root / "class" / "input").mkdir(parents=True)
        (self.root / "class" / "hidraw").mkdir(parents=True)
        self.devices = self.root / "devices"
        self.devices.mkdir()

    def add(self, tag: str, event: str, hidraw_name: str, driver: str,
            vid: int, pid: int, link_input: bool = True) -> None:
        hid = self.devices / tag
        (hid / "input" / f"input_{tag}").mkdir(parents=True)
        (hid / "uevent").write_text(
            f"DRIVER={driver}\n"
            f"HID_ID=0005:{vid:08X}:{pid:08X}\n"
            f"HID_UNIQ=aa:bb:cc:dd:ee:{pid & 0xFF:02x}\n")
        (hid / "hidraw").mkdir()
        (hid / "hidraw" / hidraw_name).mkdir()

        node = self.root / "class" / "hidraw" / hidraw_name
        node.mkdir(exist_ok=True)
        if not (node / "device").exists():
            (node / "device").symlink_to(hid)

        ev = self.root / "class" / "input" / event
        ev.mkdir(exist_ok=True)
        if link_input:
            (ev / "device").symlink_to(hid / "input" / f"input_{tag}")

    def use(self) -> None:
        hidraw.SYS_CLASS = str(self.root / "class")


def pad(event: str, vid: int, pid: int) -> Pad:
    return Pad(path=f"/dev/input/{event}", name="Some Pad", phys="p", uniq="",
               vid=vid, pid=pid, syspath="")


def write_overrides(mapping: dict) -> Path:
    path = Path(os.environ["XDG_CONFIG_HOME"]) / "padmap" / "hidraw.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(mapping))
    return path


def check_a_known_driver_is_taken() -> None:
    print("\na pad bound to a known driver takes the hidraw path:")
    t = Tree()
    driver = sorted(hidraw.HID_DRIVERS)[0]
    t.add("dev", "event10", "hidraw0", driver, MADE_UP_VID, MADE_UP_PID)
    t.use()

    p = pad("event10", MADE_UP_VID, MADE_UP_PID)
    if hidraw.driver_for(p) != driver:
        raise SystemExit(
            f"FAIL: driver_for returned {hidraw.driver_for(p)!r}, not "
            f"{driver!r}; the kernel's own answer is not being read")
    if not hidraw.supported(p, overrides={}):
        raise SystemExit(
            f"FAIL: a pad bound to {driver!r} was refused the hidraw path "
            f"because its ids are {MADE_UP_VID:04x}:{MADE_UP_PID:04x}. That "
            f"is the allowlist back: every Nintendo pad that is not the Pro "
            f"Controller goes to evdev and silently does nothing")
    print(f"  ok  driver {driver!r} taken, ids ignored")


def check_an_unrelated_driver_is_not() -> None:
    print("\na pad bound to an unrelated driver stays on evdev:")
    t = Tree()
    t.add("dev", "event11", "hidraw1", "hid-generic", 0x0079, 0x1879)
    t.use()
    if hidraw.supported(pad("event11", 0x0079, 0x1879), overrides={}):
        raise SystemExit(
            "FAIL: a hid-generic pad was routed to hidraw. Nothing here can "
            "decode its reports, so it would be read as a Switch pad and "
            "produce nonsense")
    print("  ok  left alone")


def check_an_override_can_switch_one_on() -> None:
    print("\nan override adds a pad no driver rule matches:")
    t = Tree()
    t.add("dev", "event12", "hidraw2", "some-new-driver", 0x0079, 0x4321)
    t.use()
    p = pad("event12", 0x0079, 0x4321)
    if hidraw.supported(p, overrides={}):
        raise SystemExit("FAIL: matched without an override; this check is "
                         "not testing the override")
    write_overrides({"0079:4321": True})
    if not hidraw.supported(p):
        raise SystemExit(
            "FAIL: hidraw.json did not switch the pad on. Without a working "
            "escape hatch, HID_DRIVERS is just a longer allowlist and every "
            "new controller needs a release")
    print("  ok  switched on from config")


def check_an_override_can_switch_one_off() -> None:
    print("\n...and removes one the driver rule matches:")
    t = Tree()
    driver = sorted(hidraw.HID_DRIVERS)[0]
    t.add("dev", "event13", "hidraw3", driver, 0x057E, 0x2017)
    t.use()
    write_overrides({"057e:2017": False})
    if hidraw.supported(pad("event13", 0x057E, 0x2017)):
        raise SystemExit(
            "FAIL: hidraw.json could not switch a matched pad off. A pad the "
            "family rule gets wrong would have no way back to evdev short of "
            "editing the source")
    print("  ok  switched off from config")


def check_an_unreadable_device_is_left_alone() -> None:
    print("\na pad whose device cannot be read stays on evdev:")
    t = Tree()
    t.add("dev", "event14", "hidraw4", sorted(hidraw.HID_DRIVERS)[0],
          0x057E, 0x2009, link_input=False)
    t.use()
    if hidraw.supported(pad("event14", 0x057E, 0x2009), overrides={}):
        raise SystemExit(
            "FAIL: routed to hidraw without being able to read what the "
            "device is. Guessing here means opening a node for a pad that "
            "may not speak this protocol at all")
    print("  ok  no answer means no")


def main() -> int:
    check_a_known_driver_is_taken()
    check_an_unrelated_driver_is_not()
    check_an_override_can_switch_one_on()
    check_an_override_can_switch_one_off()
    check_an_unreadable_device_is_left_alone()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
