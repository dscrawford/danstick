#!/usr/bin/env python3
"""The republisher, measured with real devices instead of reasoned about.

`virtual.py` is the hot path. Every button press, every stick movement, every
frame boundary a game ever sees has been read off a physical pad, decided
about, and written into a uinput clone by `Republisher._forward`. Nothing
downstream can tell a mistake here from a mistake anywhere else: an event
dropped, delayed, mis-scaled or emitted without its `EV_SYN` looks exactly
like a bad mapping to the person holding the controller. That is not
hypothetical -- "there's a lag on the controllers... it feels more like it's
just arriving at a very slow rate" was a real report, and the same session
records the instinct it produced, which was to go hunting for a persistence
bug that did not exist.

So this file does not stub evdev. It builds its own controller.

    a synthetic source pad  ->  virtual.create()  ->  the published clone
      (evdev.UInput)             (grab + uinput)       (read back here)

Everything checked is read off the clone as a real event, or read out of the
clone's real capability bitmaps. What is claimed about latency is timed.

WHAT THIS TOUCHES, AND WHAT IT DOES NOT
---------------------------------------
This is the one area that cannot be tested without uinput, so it creates
uinput nodes -- its own, never the machine's. A live daemon owns the real
controllers on this machine and a game may be running:

  * `XDG_RUNTIME_DIR`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and
    `PADMAP_PROFILE_DIR` are redirected to a temp dir *before* padmap is
    imported, so no real profile, socket or assignment file is read or
    written.
  * `devices.discover()` is never called. Every `Pad` here is constructed by
    hand around a node this file created.
  * Every synthetic source carries a phys under `devices.VIRTUAL_PHYS_PREFIX`
    ("padmap/"), which is precisely the prefix `discover()` filters out. The
    live daemon therefore cannot see these pads even when it rescans, so it
    can neither offer a setup screen for them nor grab them.
  * Player numbers start at 90, so the published clones are called
    "padmap Player 90" and up -- no collision with the daemon's real pads or
    with the `input_playerN_reserved_device` names RetroArch matches on.
  * Every device is closed in a `finally`, and the run leaves nothing behind.

The one thing that cannot be hidden: a joystick-shaped uinput node is visible
to SDL and to RetroArch's udev driver while it exists. It has to be -- a node
without buttons in the BTN_JOYSTICK range is not tagged `uaccess` by udev and
this test could not even open the device it just created. The nodes live for
a fraction of a second each and answer to no reservation.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tests/check_republisher.py
"""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

# Redirect every path padmap might touch BEFORE importing it. A live daemon
# owns the real controllers; nothing below may reach its state.
_SANDBOX = tempfile.mkdtemp(prefix="padmap-check-republisher-")
os.environ["XDG_RUNTIME_DIR"] = _SANDBOX
os.environ["XDG_CONFIG_HOME"] = _SANDBOX
os.environ["XDG_DATA_HOME"] = _SANDBOX
os.environ["PADMAP_PROFILE_DIR"] = os.path.join(_SANDBOX, "profiles")
# The identity switches decide what the clone advertises, so the scenarios
# that care set them explicitly. Start from a known default either way.
os.environ.pop("PADMAP_PAD_IDENTITY", None)
os.environ.pop("PADMAP_ONLY_VIRTUAL", None)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

import gc  # noqa: E402
import json  # noqa: E402
import logging  # noqa: E402
import re  # noqa: E402
import select  # noqa: E402
import statistics  # noqa: E402
import threading  # noqa: E402
import time  # noqa: E402
from typing import NoReturn  # noqa: E402

import evdev  # noqa: E402
from evdev import ecodes  # noqa: E402

from padmap import profiles, virtual  # noqa: E402
from padmap.devices import VIRTUAL_PHYS_PREFIX, Pad  # noqa: E402

# padmap's own warnings belong in the transcript, in order, rather than
# arriving on stderr wherever the terminal happens to flush them.
logging.basicConfig(stream=sys.stdout, level=logging.WARNING,
                    format="      [%(name)s] %(message)s")


# -- reporting ---------------------------------------------------------------

def heading(text: str) -> None:
    print(f"\n=== {text} ===")


def ok(text: str) -> None:
    print(f"  ok  {text}")


def gap(text: str) -> None:
    print(f"  gap: {text}")


def fail(message: str) -> NoReturn:
    raise SystemExit(f"FAIL: {message}")


# -- the synthetic controller ------------------------------------------------

# Buttons must sit in the BTN_JOYSTICK range (0x120-0x13f). Not for padmap's
# benefit -- nothing here calls discover() -- but for udev's: only a node it
# classifies as a joystick is tagged `uaccess`, and without that ACL this
# process cannot open the device it just created.
BTN_A, BTN_B, BTN_C = ecodes.BTN_SOUTH, ecodes.BTN_EAST, ecodes.BTN_C

# A plain pad: two buttons, one stick axis, a relative axis some adapters
# really do expose, a scancode channel, and a switch. The switch is the
# control: EV_SW is not in FORWARD_TYPES and must never reach the clone.
PAD_CAPS: dict[int, list] = {
    ecodes.EV_KEY: [BTN_A, BTN_B, BTN_C],
    ecodes.EV_ABS: [
        (ecodes.ABS_X, evdev.AbsInfo(174, 0, 255, 0, 0, 0)),
        (ecodes.ABS_Y, evdev.AbsInfo(128, 0, 255, 0, 0, 0)),
    ],
    ecodes.EV_REL: [ecodes.REL_X],
    ecodes.EV_MSC: [ecodes.MSC_SCAN],
    ecodes.EV_SW: [ecodes.SW_HEADPHONE_INSERT],
}

_next_product = [0x9F00]
_next_player = [90]

_EVENT_DIR = re.compile(r"event\d+")


def _wait_for_node(ui: evdev.UInput) -> evdev.InputDevice | None:
    """The event node behind a uinput handle, once it is usable.

    python-evdev's own lookup gives up quickly, and there are two separate
    delays to ride out: the node appearing under /dev/input, and udev applying
    the `uaccess` ACL that lets this process open it. Missing either one
    reports as `UInput.device is None`, which reads like "uinput is broken"
    when it only means "not yet".
    """
    if ui.device is not None:
        return ui.device
    import evdev._uinput as _uinput  # noqa: PLC0415 - only needed on the slow path

    try:
        syspath = f"/sys/devices/virtual/input/{_uinput.get_sysname(ui.fd)}"
    except OSError:
        return None
    for _ in range(40):
        try:
            nodes = [e for e in os.listdir(syspath) if _EVENT_DIR.fullmatch(e)]
        except OSError:
            nodes = []
        if nodes:
            try:
                return evdev.InputDevice(f"/dev/input/{nodes[0]}")
            except (FileNotFoundError, PermissionError):
                pass
        time.sleep(0.05)
    return None


class Source:
    """A controller this file owns end to end.

    The uinput end is ours to write into; the event node it produces is what
    padmap will open, grab and republish. Its phys is under "padmap/" so the
    live daemon's discovery skips it entirely.
    """

    def __init__(self, label: str, caps: dict | None = None,
                 version: int = 0x0102, max_effects: int = 0) -> None:
        _next_product[0] += 1
        self.product = _next_product[0]
        self.name = f"padmap selftest {os.getpid()} {label}"
        self.phys = f"{VIRTUAL_PHYS_PREFIX}selftest-{os.getpid()}-{self.product:04x}"
        self.version = version
        self.ui = evdev.UInput(
            events=dict(PAD_CAPS if caps is None else caps),
            name=self.name, phys=self.phys,
            vendor=virtual.PADMAP_VID, product=self.product,
            version=version, bustype=ecodes.BUS_USB,
            max_effects=max_effects,
        )
        device = _wait_for_node(self.ui)
        if device is None:
            self.ui.close()
            raise Unavailable(
                f"udev never gave this process access to the node behind "
                f"{self.name!r}")
        self.device = device
        self.pad = Pad(path=self.device.path, name=self.name, phys=self.phys,
                       uniq="", vid=virtual.PADMAP_VID, pid=self.product,
                       syspath="")

    def emit(self, etype: int, code: int, value: int) -> None:
        self.ui.write(etype, code, value)

    def syn(self) -> None:
        self.ui.syn()

    def frame(self, *events: tuple[int, int, int]) -> None:
        for etype, code, value in events:
            self.ui.write(etype, code, value)
        self.ui.syn()

    def store_profile(self, axes: dict, raw: dict | None = None) -> Path:
        """Write a profile for this pad into the sandboxed profile store."""
        directory = profiles.profile_dir()
        directory.mkdir(parents=True, exist_ok=True)
        signature = profiles.signature(self.pad)
        path = directory / profiles._filename(signature)
        if raw is not None:
            path.write_text(json.dumps(raw))
        else:
            profiles.save(profiles.Profile(signature=signature, axes=axes))
        return path

    def close(self) -> None:
        if self.device is not self.ui.device:
            try:
                self.device.close()
            except OSError:
                pass
        try:
            self.ui.close()
        except OSError:
            pass


class Unavailable(Exception):
    """uinput is not usable here; skip rather than fail."""


class Bench:
    """Everything one scenario creates, closed in one place."""

    def __init__(self) -> None:
        self._closers: list = []

    def source(self, label: str, **kwargs) -> Source:
        src = Source(label, **kwargs)
        self._closers.append(src.close)
        return src

    def publish(self, src: Source, grab: bool = True,
                allow_failure: bool = False) -> virtual.VirtualPad:
        _next_player[0] += 1
        try:
            vpad = virtual.create(src.pad, _next_player[0], grab=grab)
        except Exception as error:  # noqa: BLE001 - report, do not traceback
            if allow_failure:
                raise
            fail(f"virtual.create could not publish {src.name!r}: "
                 f"{type(error).__name__}: {error} -- a pad that cannot be "
                 f"published is a player left holding a dead controller")
        self._closers.append(vpad.close)
        return vpad

    def republisher(self, *vpads: virtual.VirtualPad) -> virtual.Republisher:
        return virtual.Republisher(list(vpads))

    def reader(self, vpad: virtual.VirtualPad) -> evdev.InputDevice:
        """A second view of the clone: what a game would read."""
        dev = evdev.InputDevice(vpad.ui.device.path)
        self._closers.append(dev.close)
        return dev

    def close(self) -> None:
        for closer in reversed(self._closers):
            try:
                closer()
            except Exception:  # noqa: BLE001 - teardown must not mask a FAIL
                pass
        self._closers = []


def settle(rep: virtual.Republisher, readers: list, seconds: float = 0.25):
    """Pump the republisher for a while and collect what reached the clones.

    Deliberately runs the whole window rather than stopping at the first
    event: half the scenarios here assert that something did *not* arrive,
    and an early return would make those pass by not looking.
    """
    got: dict[int, list] = {r.fd: [] for r in readers}
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        rep.pump(timeout=0.01)
        ready, _, _ = select.select([r.fd for r in readers], [], [], 0.0)
        for reader in readers:
            if reader.fd in ready:
                try:
                    got[reader.fd].extend(reader.read())
                except BlockingIOError:
                    pass
    return got


def triples(events) -> list[tuple[int, int, int]]:
    return [(e.type, e.code, e.value) for e in events]


def one_clone(rep, reader, seconds: float = 0.25) -> list:
    return settle(rep, [reader], seconds)[reader.fd]


# -- scenarios ---------------------------------------------------------------

def check_forward_types_reach_the_clone() -> None:
    """S17 — every type in FORWARD_TYPES arrives, unchanged and in order."""
    heading("S17 — every forwarded event type reaches the clone unchanged")
    bench = Bench()
    try:
        src = bench.source("forward")
        vpad = bench.publish(src)
        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)

        src.frame(
            (ecodes.EV_KEY, BTN_A, 1),
            (ecodes.EV_ABS, ecodes.ABS_X, 200),
            (ecodes.EV_REL, ecodes.REL_X, -3),
            (ecodes.EV_MSC, ecodes.MSC_SCAN, 0x90001),
        )
        got = triples(one_clone(rep, reader))

        expected = [
            (ecodes.EV_KEY, BTN_A, 1),
            (ecodes.EV_ABS, ecodes.ABS_X, 200),
            (ecodes.EV_REL, ecodes.REL_X, -3),
            (ecodes.EV_MSC, ecodes.MSC_SCAN, 0x90001),
            (ecodes.EV_SYN, ecodes.SYN_REPORT, 0),
        ]
        if got != expected:
            fail(f"the clone did not reproduce the frame: sent {expected}, "
                 f"got {got} -- a game reads the clone, so anything missing "
                 f"or altered here is a control that does not work")
        ok("EV_KEY, EV_ABS, EV_REL, EV_MSC and EV_SYN all arrive")
        ok("their values are byte-identical to what the pad sent")
        ok("they arrive in the order the pad sent them")

        for etype in virtual.FORWARD_TYPES:
            if etype not in {t for t, _, _ in expected}:
                fail(f"FORWARD_TYPES contains {etype} and this scenario never "
                     f"exercises it; the claim 'every forwarded type arrives' "
                     f"is not being checked")
        ok(f"all {len(virtual.FORWARD_TYPES)} types in FORWARD_TYPES were exercised")
    finally:
        bench.close()


def check_unforwarded_types_are_dropped() -> None:
    """S17 — a type outside FORWARD_TYPES must not reach the clone."""
    heading("S17 — an event type outside FORWARD_TYPES never reaches the clone")
    bench = Bench()
    try:
        src = bench.source("drop")
        vpad = bench.publish(src)
        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)

        if ecodes.EV_SW in virtual.FORWARD_TYPES:
            fail("EV_SW is in FORWARD_TYPES, so this scenario checks nothing")

        # The clone *advertises* EV_SW, because _capabilities_for copies the
        # source's bitmaps wholesale. Advertising it and forwarding it are
        # different decisions, and only the first one is made.
        if ecodes.EV_SW not in vpad.ui.device.capabilities():
            fail("the clone does not advertise EV_SW, so a dropped EV_SW "
                 "event proves nothing about the type filter")
        ok("the clone advertises EV_SW, so nothing but the filter can drop it")

        src.frame(
            (ecodes.EV_KEY, BTN_B, 1),
            (ecodes.EV_SW, ecodes.SW_HEADPHONE_INSERT, 1),
            (ecodes.EV_KEY, BTN_C, 1),
        )
        got = triples(one_clone(rep, reader))

        if any(t == ecodes.EV_SW for t, _, _ in got):
            fail(f"EV_SW reached the clone: {got}")
        ok("the EV_SW event is dropped")
        if got != [(ecodes.EV_KEY, BTN_B, 1), (ecodes.EV_KEY, BTN_C, 1),
                   (ecodes.EV_SYN, ecodes.SYN_REPORT, 0)]:
            fail(f"dropping one event disturbed the rest of the frame: {got}")
        ok("the events either side of it still arrive, in order")
    finally:
        bench.close()


def check_syn_completes_every_frame() -> None:
    """S17 — without EV_SYN nothing downstream ever sees a complete frame."""
    heading("S17 — EV_SYN completes every frame")
    bench = Bench()
    try:
        src = bench.source("syn")
        vpad = bench.publish(src)
        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)

        frames = 8
        for i in range(frames):
            # Press first, then alternate: the kernel discards an EV_KEY event
            # that does not change state, so a leading release would silently
            # make the first frame two events long.
            src.frame((ecodes.EV_ABS, ecodes.ABS_X, 10 + i),
                      (ecodes.EV_KEY, BTN_A, 1 - (i % 2)))
        got = triples(one_clone(rep, reader, seconds=0.4))

        syns = [e for e in got if e[0] == ecodes.EV_SYN]
        reports = [e for e in syns if e[1] == ecodes.SYN_REPORT]
        if len(reports) != frames:
            fail(f"{len(reports)} SYN_REPORT of {frames} frames reached the "
                 f"clone -- a frame without one is a press a game never "
                 f"finishes reading")
        ok(f"all {frames} SYN_REPORT boundaries arrive")

        # Every SYN must be *last* in its frame, not merely present.
        boundaries = [i for i, e in enumerate(got) if e[0] == ecodes.EV_SYN]
        if boundaries != [i * 3 + 2 for i in range(frames)]:
            fail(f"the frame boundaries landed in the wrong places: {got}")
        ok("each one arrives after the events it closes, not before")

        if any(e[1] == ecodes.SYN_DROPPED for e in syns):
            fail("the kernel reported SYN_DROPPED: the clone's reader "
                 "overflowed, so events were lost")
        ok("no SYN_DROPPED: nothing overflowed on the way through")
    finally:
        bench.close()


def check_capabilities_are_mirrored() -> None:
    """S15 — the clone claims what the source claims, less uinput's own."""
    heading("S15 — the clone advertises the source's capabilities, minus what "
            "uinput supplies itself")
    bench = Bench()
    try:
        src = bench.source("caps")
        vpad = bench.publish(src)

        computed = virtual._capabilities_for(vpad.source)
        if ecodes.EV_SYN in computed:
            fail("_capabilities_for kept EV_SYN; uinput supplies it itself "
                 "and passing it through confuses UInput's setup")
        ok("EV_SYN is dropped from the capability set handed to uinput")

        source_caps = vpad.source.capabilities()
        clone_caps = vpad.ui.device.capabilities()
        if set(clone_caps) != set(source_caps):
            fail(f"the clone advertises {sorted(clone_caps)} where the source "
                 f"advertises {sorted(source_caps)}; SDL and libretro classify "
                 f"a pad from these bits")
        ok("the clone's event types match the source's exactly")

        for etype in (ecodes.EV_KEY, ecodes.EV_REL, ecodes.EV_MSC,
                      ecodes.EV_SW, ecodes.EV_ABS):
            if clone_caps.get(etype) != source_caps.get(etype):
                fail(f"type {etype}: clone has {clone_caps.get(etype)}, "
                     f"source has {source_caps.get(etype)}")
        ok("every button, axis, relative axis, scancode and switch code matches")

        if clone_caps.get(ecodes.EV_KEY) != [BTN_A, BTN_B, BTN_C]:
            fail(f"the buttons are not the ones the pad has: "
                 f"{clone_caps.get(ecodes.EV_KEY)}")
        ok("the button list is the source's three buttons and nothing else")
    finally:
        bench.close()


def check_absinfo_survives() -> None:
    """S12 — RetroArch scales against the declared range, so it must survive."""
    heading("S12 — absinfo survives the clone exactly")
    bench = Bench()
    try:
        caps = {
            ecodes.EV_KEY: [BTN_A],
            ecodes.EV_ABS: [
                # A worn N64 stick: rests at 174 on a 0-255 axis.
                (ecodes.ABS_X, evdev.AbsInfo(174, 0, 255, 3, 7, 11)),
                # A GameCube trigger on the code a right stick usually uses.
                (ecodes.ABS_RY, evdev.AbsInfo(25, -32768, 32767, 16, 128, 0)),
                # A hat, whose range is three values wide.
                (ecodes.ABS_HAT0X, evdev.AbsInfo(0, -1, 1, 0, 0, 0)),
            ],
        }
        src = bench.source("absinfo", caps=caps)
        vpad = bench.publish(src)

        for code in (ecodes.ABS_X, ecodes.ABS_RY, ecodes.ABS_HAT0X):
            want = vpad.source.absinfo(code)
            have = vpad.ui.device.absinfo(code)
            for field in ("min", "max", "fuzz", "flat"):
                if getattr(want, field) != getattr(have, field):
                    fail(f"axis {code}: {field} is {getattr(have, field)} on "
                         f"the clone and {getattr(want, field)} on the pad -- "
                         f"RetroArch scales a stick against the declared "
                         f"range, so a wrong one is a stick that cannot reach "
                         f"its own extreme")
            if want.resolution != have.resolution:
                fail(f"axis {code}: resolution {have.resolution} != "
                     f"{want.resolution}")
        ok("min, max, fuzz and flat are preserved on all three axes")
        ok("resolution is preserved too")

        wide = vpad.ui.device.absinfo(ecodes.ABS_RY)
        if (wide.min, wide.max, wide.flat) != (-32768, 32767, 128):
            fail(f"the 16-bit axis was not preserved: {wide}")
        ok("a 16-bit signed axis is not squeezed into the 0-255 of its neighbour")

        hat = vpad.ui.device.absinfo(ecodes.ABS_HAT0X)
        if (hat.min, hat.max) != (-1, 1):
            fail(f"the hat's range became {hat.min}..{hat.max}")
        ok("a three-value hat keeps its -1..1 range")
    finally:
        bench.close()


def check_force_feedback_is_not_invented() -> None:
    """S15 — "uinput advertises force feedback it does not have"."""
    heading("S15 — a pad that cannot rumble does not claim 96 effects")
    bench = Bench()
    try:
        src = bench.source("noff", max_effects=0)
        if ecodes.EV_FF in src.device.capabilities():
            fail("the synthetic source advertises EV_FF, so this scenario is "
                 "not testing a pad that cannot rumble")
        vpad = bench.publish(src)

        if ecodes.EV_FF in vpad.ui.device.capabilities():
            fail("the clone of a pad with no force feedback advertises EV_FF")
        ok("the clone of a pad with no rumble does not advertise EV_FF")

        count = vpad.ui.device.ff_effects_count
        if count != 0:
            fail(f"the clone reports {count} force feedback effects for a pad "
                 f"that cannot rumble at all; RetroArch reads that number "
                 f"back and reports it to the user")
        ok("it reports 0 effects, not uinput's default 96")
    finally:
        bench.close()


def check_force_feedback_is_mirrored() -> None:
    """S15 — a pad that can rumble keeps its own effect count."""
    heading("S15 — a pad that can rumble keeps its own effect count")
    bench = Bench()
    try:
        caps = dict(PAD_CAPS)
        caps[ecodes.EV_FF] = [ecodes.FF_RUMBLE, ecodes.FF_PERIODIC]
        src = bench.source("ff", caps=caps, max_effects=8)
        if src.device.ff_effects_count != 8:
            fail(f"the synthetic source reports "
                 f"{src.device.ff_effects_count} effects, not the 8 asked for; "
                 f"this scenario cannot check mirroring")
        vpad = bench.publish(src)

        if ecodes.EV_FF not in vpad.ui.device.capabilities():
            fail("the clone of a rumbling pad does not advertise EV_FF, so "
                 "nothing can ever upload an effect to it -- rumble is dead "
                 "for N64 and GameCube")
        ok("the clone advertises EV_FF")

        clone_ff = vpad.ui.device.capabilities()[ecodes.EV_FF]
        if ecodes.FF_RUMBLE not in clone_ff:
            fail(f"FF_RUMBLE is missing from the clone: {clone_ff}")
        ok("FF_RUMBLE survives to the clone")

        if vpad.ui.device.ff_effects_count != 8:
            fail(f"the clone reports {vpad.ui.device.ff_effects_count} effects "
                 f"where the pad can play 8; padmap can only proxy effects "
                 f"the physical device is able to play")
        ok("the clone reports exactly the 8 effects the pad can play")

        if not vpad.effects:
            ok("no effect ids are mapped until something uploads one")
        else:
            fail(f"the effect map starts non-empty: {vpad.effects}")
    finally:
        bench.close()


def check_identity_is_mirrored_by_default() -> None:
    """S15 — mirror mode, measured off the published node."""
    heading("S15 — the clone mirrors the source's identity by default")
    bench = Bench()
    try:
        os.environ.pop("PADMAP_PAD_IDENTITY", None)
        os.environ.pop("PADMAP_ONLY_VIRTUAL", None)
        if virtual.identity_mode() != virtual.IDENTITY_MIRROR:
            fail(f"identity_mode() is {virtual.identity_mode()} with both "
                 f"switches unset")

        src = bench.source("mirror", version=0x0102)
        vpad = bench.publish(src)
        info = vpad.ui.device.info

        if info.vendor != src.device.info.vendor:
            fail(f"vendor {info.vendor:#06x} != {src.device.info.vendor:#06x}")
        if info.product != src.device.info.product:
            fail(f"product {info.product:#06x} != "
                 f"{src.device.info.product:#06x}")
        ok("the clone carries the source's vendor and product ids")

        if info.version != 0x0102:
            fail(f"version {info.version:#06x} != 0x0102")
        ok("the clone carries the source's version")

        if info.bustype != ecodes.BUS_USB:
            fail(f"bus {info.bustype} != BUS_USB; SDL's database is keyed on a "
                 f"GUID whose first field is the bus, so a USB pad published "
                 f"on BUS_VIRTUAL matches no entry it has")
        if info.bustype == ecodes.BUS_VIRTUAL:
            fail("the clone reports BUS_VIRTUAL in mirror mode")
        ok("the clone carries the source's bus, not uinput's BUS_VIRTUAL")
    finally:
        bench.close()


def check_padmap_identity_mode() -> None:
    """S15 — 1209:0001 on BUS_VIRTUAL is what PADMAP_ONLY_VIRTUAL hides by."""
    heading("S15 — PADMAP_PAD_IDENTITY=padmap publishes 1209:0001 on BUS_VIRTUAL")
    bench = Bench()
    try:
        os.environ["PADMAP_PAD_IDENTITY"] = virtual.IDENTITY_PADMAP
        src = bench.source("padmapident", version=0x0102)
        vpad = bench.publish(src)
        info = vpad.ui.device.info

        if (info.vendor, info.product) != (virtual.PADMAP_VID,
                                           virtual.PADMAP_PID):
            fail(f"padmap identity published {info.vendor:#06x}:"
                 f"{info.product:#06x}, not "
                 f"{virtual.PADMAP_VID:#06x}:{virtual.PADMAP_PID:#06x} -- "
                 f"SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT names those ids, "
                 f"so a mismatch leaves the machine with no controller at all")
        ok("vendor and product are padmap's own 1209:0001")

        if info.bustype != ecodes.BUS_VIRTUAL:
            fail(f"bus is {info.bustype}, not BUS_VIRTUAL")
        ok("the bus is BUS_VIRTUAL")

        if info.version != virtual.PADMAP_VERSION:
            fail(f"version {info.version} != {virtual.PADMAP_VERSION}")
        ok("the version is padmap's own, not the source's 0x0102")
    finally:
        os.environ.pop("PADMAP_PAD_IDENTITY", None)
        bench.close()


def check_name_and_phys() -> None:
    """S15 — the name RetroArch pins to, and the phys discovery skips."""
    heading("S15 — the clone is named for its player and phys'd out of discovery")
    bench = Bench()
    try:
        src = bench.source("naming")
        vpad = bench.publish(src)
        player = vpad.player

        if vpad.ui.device.name != virtual.virtual_name(player):
            fail(f"the published node is called {vpad.ui.device.name!r}, not "
                 f"{virtual.virtual_name(player)!r}; RetroArch's reservation "
                 f"matcher compares device names exactly")
        ok(f"the node is really called {vpad.ui.device.name!r}")

        if not vpad.ui.device.name.startswith(virtual.VIRTUAL_PREFIX):
            fail("the name does not start with VIRTUAL_PREFIX, so the code "
                 "that cleans padmap values out of retroarch.cfg will miss it")
        ok("its name starts with VIRTUAL_PREFIX")

        if vpad.ui.device.phys != virtual.virtual_phys(player):
            fail(f"phys is {vpad.ui.device.phys!r}, not "
                 f"{virtual.virtual_phys(player)!r}")
        if not vpad.ui.device.phys.startswith(VIRTUAL_PHYS_PREFIX):
            fail("the clone's phys is not under VIRTUAL_PHYS_PREFIX, so "
                 "discovery would pick up padmap's own output and republish "
                 "it, one layer deeper on every restart")
        ok("its phys is under VIRTUAL_PHYS_PREFIX, so discovery skips it")

        if vpad.name != virtual.virtual_name(player):
            fail(f"VirtualPad.name says {vpad.name!r} where the device says "
                 f"{vpad.ui.device.name!r}")
        ok("VirtualPad.name agrees with the device that was actually created")
    finally:
        bench.close()


def check_calibration_is_applied() -> None:
    """S12 — a stored calibration corrects the axis as it passes through."""
    heading("S12 — calibration is applied to EV_ABS when the profile has axes")
    bench = Bench()
    try:
        src = bench.source("calibrated")
        # The N64 stick from FINDINGS: rests at 174 on a 0-255 axis.
        src.store_profile({ecodes.ABS_X: profiles.AxisCalibration(
            center=174, minimum=0, maximum=255, flat=8)})
        vpad = bench.publish(src)

        if ecodes.ABS_X not in vpad.axes:
            fail("the stored calibration was not loaded onto the virtual pad")
        ok("the stored calibration is loaded onto the virtual pad")

        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)

        cal = vpad.axes[ecodes.ABS_X]
        raw_value = 200
        corrected = cal.apply(raw_value)
        if corrected == raw_value:
            fail(f"the calibration is a no-op at {raw_value}, so this "
                 f"scenario cannot tell correction from pass-through")

        src.frame((ecodes.EV_ABS, ecodes.ABS_X, raw_value))
        got = triples(one_clone(rep, reader))
        values = [v for t, c, v in got
                  if t == ecodes.EV_ABS and c == ecodes.ABS_X]
        if values != [corrected]:
            fail(f"the pad reported {raw_value} and the clone emitted "
                 f"{values}, not [{corrected}] -- an uncorrected stick reads "
                 f"as permanently deflected and a front-end acts on that "
                 f"immediately")
        ok(f"raw {raw_value} leaves the clone as {corrected}")

        # Resting position: the whole point. The stick sitting at its worn
        # rest must read as centred, not as 46% deflected.
        src.frame((ecodes.EV_ABS, ecodes.ABS_X, 255))
        one_clone(rep, reader, seconds=0.15)
        src.frame((ecodes.EV_ABS, ecodes.ABS_X, 174))
        got = triples(one_clone(rep, reader))
        values = [v for t, c, v in got
                  if t == ecodes.EV_ABS and c == ecodes.ABS_X]
        mid = (cal.minimum + cal.maximum) // 2
        if values != [mid]:
            fail(f"the stick at its measured rest (174) emitted {values} "
                 f"instead of the declared midpoint {mid}: runaway menu "
                 f"navigation is what that looks like to the user")
        ok(f"the stick at its worn rest of 174 reads as the midpoint {mid}")
    finally:
        bench.close()


def check_uncalibrated_pads_pass_through() -> None:
    """S12 — no profile means verbatim, which is right for a good pad."""
    heading("S12 — without a profile the axis is forwarded verbatim")
    bench = Bench()
    try:
        src = bench.source("verbatim")
        vpad = bench.publish(src)
        if vpad.axes:
            fail(f"a pad with no stored profile came up with calibration "
                 f"{vpad.axes}")
        ok("a pad with no stored profile has no calibration")

        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)
        for value in (200, 5, 174, 255, 0):
            src.frame((ecodes.EV_ABS, ecodes.ABS_X, value))
        got = [v for t, c, v in triples(one_clone(rep, reader, seconds=0.35))
               if t == ecodes.EV_ABS and c == ecodes.ABS_X]
        if got != [200, 5, 174, 255, 0]:
            fail(f"an uncalibrated pad's axis was altered on the way through: "
                 f"{got}")
        ok("every raw value reaches the clone unchanged")

        # And the axis it declares is untouched too: no centre seed happened.
        info = vpad.ui.device.absinfo(ecodes.ABS_Y)
        if info.value != 128:
            fail(f"an untouched axis on an uncalibrated pad was seeded to "
                 f"{info.value}; nothing should write to a pad nobody has "
                 f"measured")
        ok("no centre seed is written for a pad with no profile")
    finally:
        bench.close()


def check_only_calibrated_axes_are_touched() -> None:
    """S12 — junk in one axis slot costs that axis, not the whole pad."""
    heading("S12 — an axis with no calibration is forwarded verbatim on a "
            "calibrated pad")
    bench = Bench()
    try:
        src = bench.source("partial")
        src.store_profile({ecodes.ABS_X: profiles.AxisCalibration(
            center=174, minimum=0, maximum=255, flat=8)})
        vpad = bench.publish(src)
        if set(vpad.axes) != {ecodes.ABS_X}:
            fail(f"expected only ABS_X calibrated, got {sorted(vpad.axes)}")
        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)

        src.frame((ecodes.EV_ABS, ecodes.ABS_Y, 200))
        got = [v for t, c, v in triples(one_clone(rep, reader))
               if t == ecodes.EV_ABS and c == ecodes.ABS_Y]
        if got != [200]:
            fail(f"ABS_Y has no calibration but came out as {got}, not [200]; "
                 f"correcting an axis nobody measured is a guess applied to "
                 f"a stick that was fine")
        ok("an axis outside the profile is forwarded untouched")

        src.frame((ecodes.EV_ABS, ecodes.ABS_X, 200))
        got = [v for t, c, v in triples(one_clone(rep, reader))
               if t == ecodes.EV_ABS and c == ecodes.ABS_X]
        if got == [200]:
            fail("the calibrated axis was not corrected either, so this "
                 "scenario proves nothing about selectivity")
        ok("the calibrated axis on the same pad still is corrected")
    finally:
        bench.close()


def check_calibration_is_keyed_on_type_not_code() -> None:
    """S12 — ABS_X and REL_X are both code 0; only one is an axis."""
    heading("S12 — calibration is keyed on EV_ABS, not on the code alone")
    bench = Bench()
    try:
        if ecodes.ABS_X != ecodes.REL_X:
            fail("ABS_X and REL_X no longer share a code, so this scenario "
                 "cannot show that the type is what is being matched")
        src = bench.source("typed")
        src.store_profile({ecodes.ABS_X: profiles.AxisCalibration(
            center=174, minimum=0, maximum=255, flat=8)})
        vpad = bench.publish(src)
        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)

        # A relative movement of 200 counts is a mouse-wheel-sized shove, not
        # a stick position. Rescaling it would be nonsense, and the code it
        # arrives under is the same 0 the calibration is filed against.
        src.frame((ecodes.EV_REL, ecodes.REL_X, 200))
        got = [v for t, c, v in triples(one_clone(rep, reader))
               if t == ecodes.EV_REL]
        if got != [200]:
            fail(f"a relative movement of 200 came out as {got}: the axis "
                 f"calibration filed under code 0 was applied to REL_X")
        ok("a REL event sharing the calibrated axis's code is untouched")

        src.frame((ecodes.EV_ABS, ecodes.ABS_X, 200))
        got = [v for t, c, v in triples(one_clone(rep, reader))
               if t == ecodes.EV_ABS and c == ecodes.ABS_X]
        if got == [200]:
            fail("EV_ABS code 0 was not corrected, so the comparison above "
                 "shows nothing")
        ok(f"the EV_ABS event under the same code is corrected, to {got[0]}")
    finally:
        bench.close()


def check_calibrated_pads_start_centred() -> None:
    """S12 — seed at centre, not at the adapter's stale power-on value."""
    heading("S12 — a calibrated pad starts at centre, not at the stale rest")
    bench = Bench()
    try:
        plain = bench.source("stale")
        plain_pad = bench.publish(plain)
        stale = plain_pad.ui.device.absinfo(ecodes.ABS_X).value
        if stale != 174:
            fail(f"the synthetic pad's declared rest is {stale}, not the 174 "
                 f"this scenario is built around")
        ok("an uncalibrated clone inherits the adapter's declared rest of 174")

        src = bench.source("seeded")
        src.store_profile({ecodes.ABS_X: profiles.AxisCalibration(
            center=174, minimum=0, maximum=255, flat=8)})
        vpad = bench.publish(src)
        seeded = vpad.ui.device.absinfo(ecodes.ABS_X).value
        mid = (0 + 255) // 2
        if seeded != mid:
            fail(f"a calibrated clone came up declaring {seeded} instead of "
                 f"the midpoint {mid}; a pad that looks slammed to a corner "
                 f"from the moment it appears is runaway menu navigation")
        ok(f"a calibrated clone comes up declaring the midpoint {mid}")

        untouched = vpad.ui.device.absinfo(ecodes.ABS_Y).value
        if untouched != 128:
            fail(f"ABS_Y has no calibration but was seeded to {untouched}")
        ok("an axis with no calibration is not seeded")
    finally:
        bench.close()


def check_grab_is_exclusive() -> None:
    """S3 — a republished pad is grabbed, so nothing else sees it raw."""
    heading("S3 — the source is grabbed while republishing")
    bench = Bench()
    try:
        src = bench.source("grabbed")
        vpad = bench.publish(src, grab=True)
        probe = evdev.InputDevice(src.pad.path)
        try:
            probe.grab()
            probe.ungrab()
            fail("the physical pad is not grabbed while it is being "
                 "republished, so RetroArch would see the pad twice: once "
                 "raw and once as the clone")
        except OSError:
            ok("a second reader cannot grab the pad while it is republished")
        finally:
            probe.close()
    finally:
        bench.close()

    bench = Bench()
    try:
        src = bench.source("ungrabbed")
        bench.publish(src, grab=False)
        probe = evdev.InputDevice(src.pad.path)
        try:
            probe.grab()
            probe.ungrab()
            ok("grab=False leaves the pad open to everyone else")
        except OSError as error:
            fail(f"create(grab=False) grabbed the pad anyway: {error}")
        finally:
            probe.close()
    finally:
        bench.close()


def check_close_releases_both_ends() -> None:
    """S5 — cancelling has to give the controllers back."""
    heading("S5 — close() ungrabs the physical pad and removes the virtual one")
    bench = Bench()
    try:
        src = bench.source("release")
        vpad = bench.publish(src)
        clone_path = vpad.ui.device.path
        source_path = src.pad.path

        if not os.path.exists(clone_path):
            fail(f"the clone node {clone_path} was never created")
        ok(f"the clone exists at {clone_path} while republishing")

        vpad.close()

        probe = evdev.InputDevice(source_path)
        try:
            probe.grab()
            probe.ungrab()
            ok("the physical pad can be grabbed again after close()")
        except OSError as error:
            fail(f"close() left EVIOCGRAB held on {source_path} "
                 f"({error}); every controller on the machine stays dead "
                 f"until the daemon is restarted")
        finally:
            probe.close()

        if os.path.exists(clone_path):
            fail(f"close() left the virtual node {clone_path} behind; a stale "
                 f"'padmap Player N' is a pad RetroArch will still reserve a "
                 f"slot for")
        ok("the virtual node is gone after close()")

        try:
            os.fstat(vpad.source.fd)
            fail("close() left the source file descriptor open")
        except OSError:
            ok("the source descriptor is closed, not merely ungrabbed")
    finally:
        bench.close()


def check_republisher_close_releases_every_pad() -> None:
    """S5 — releasing 'every device', not just the first one."""
    heading("S5 — Republisher.close() releases every pad, not just the first")
    bench = Bench()
    try:
        sources = [bench.source(f"multi{i}") for i in range(3)]
        vpads = [bench.publish(s) for s in sources]
        rep = bench.republisher(*vpads)
        clone_paths = [v.ui.device.path for v in vpads]

        rep.close()

        for src, path in zip(sources, clone_paths):
            probe = evdev.InputDevice(src.pad.path)
            try:
                probe.grab()
                probe.ungrab()
            except OSError as error:
                fail(f"{src.name} is still grabbed after Republisher.close() "
                     f"({error})")
            finally:
                probe.close()
            if os.path.exists(path):
                fail(f"the virtual node {path} outlived Republisher.close()")
        ok("all three physical pads are ungrabbed")
        ok("all three virtual nodes are removed")
    finally:
        bench.close()


def check_stop_ends_run() -> None:
    """S5 — the loop the daemon runs has to be stoppable."""
    heading("S5 — stop() ends run()")
    bench = Bench()
    try:
        src = bench.source("stoppable")
        vpad = bench.publish(src)
        rep = bench.republisher(vpad)

        thread = threading.Thread(target=rep.run, daemon=True)
        thread.start()
        time.sleep(0.1)
        if not thread.is_alive():
            fail("run() returned before anything asked it to stop")
        ok("run() keeps pumping until asked to stop")

        rep.stop()
        thread.join(timeout=3.0)
        if thread.is_alive():
            fail("run() did not return within 3s of stop(); the daemon would "
                 "never shut down and its uinput nodes would outlive it")
        ok("run() returns promptly after stop()")
    finally:
        bench.close()


def check_vanished_source_stops_the_loop() -> None:
    """S1 — an unplugged pad must end the loop, not spin on a dead fd."""
    heading("S1 — a source that disappears mid-stream stops the republisher")
    bench = Bench()
    try:
        src = bench.source("vanishing")
        vpad = bench.publish(src)
        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)

        src.frame((ecodes.EV_KEY, BTN_A, 1))
        if not triples(one_clone(rep, reader, seconds=0.2)):
            fail("nothing was forwarded before the pad was unplugged, so the "
                 "disappearance below proves nothing")
        ok("the pad forwards normally before it is unplugged")

        src.close()  # the kernel destroys the node: reads now give ENODEV

        thread = threading.Thread(target=rep.run, daemon=True)
        started = time.monotonic()
        thread.start()
        thread.join(timeout=5.0)
        if thread.is_alive():
            fail("run() never returned after the source disappeared -- a "
                 "select() that always reports a dead descriptor readable is "
                 "a busy loop on the thread that forwards controller events")
        ok(f"run() returns {time.monotonic() - started:.3f}s after the pad "
           f"disappears")

        # And it stays stopped: a second run() must not resume on a dead fd.
        again = threading.Thread(target=rep.run, daemon=True)
        again.start()
        again.join(timeout=5.0)
        if again.is_alive():
            fail("a second run() after the source vanished did not return")
        ok("it stays stopped rather than resuming on the dead descriptor")
    finally:
        bench.close()


def check_two_pads_stay_separate() -> None:
    """S17 — two players, neither stealing the other's inputs."""
    heading("S17 — two pads are forwarded independently")
    bench = Bench()
    try:
        one = bench.source("p1")
        two = bench.source("p2")
        vone = bench.publish(one)
        vtwo = bench.publish(two)
        read_one = bench.reader(vone)
        read_two = bench.reader(vtwo)
        rep = bench.republisher(vone, vtwo)

        if vone.ui.device.path == vtwo.ui.device.path:
            fail("both players were published onto the same node")
        if vone.ui.device.name == vtwo.ui.device.name:
            fail(f"both clones are called {vone.ui.device.name!r}, so "
                 f"RetroArch cannot reserve a slot per player")
        ok("the two players get distinct nodes with distinct names")

        one.frame((ecodes.EV_KEY, BTN_A, 1))
        two.frame((ecodes.EV_ABS, ecodes.ABS_Y, 42))
        got = settle(rep, [read_one, read_two], seconds=0.35)

        first = triples(got[read_one.fd])
        second = triples(got[read_two.fd])
        if first != [(ecodes.EV_KEY, BTN_A, 1),
                     (ecodes.EV_SYN, ecodes.SYN_REPORT, 0)]:
            fail(f"player 1's clone carried {first}")
        if second != [(ecodes.EV_ABS, ecodes.ABS_Y, 42),
                      (ecodes.EV_SYN, ecodes.SYN_REPORT, 0)]:
            fail(f"player 2's clone carried {second}")
        ok("each press arrives on its own player's clone")
        ok("neither clone carries the other player's events")
    finally:
        bench.close()


def check_descriptors_cover_both_directions() -> None:
    """S15 — inbound presses and outbound rumble are both watched."""
    heading("S15 — fds cover both directions, and an unknown fd is a no-op")
    bench = Bench()
    try:
        one = bench.source("fds1")
        two = bench.source("fds2")
        vone = bench.publish(one)
        vtwo = bench.publish(two)
        rep = bench.republisher(vone, vtwo)

        watched = set(rep.fds)
        for vpad in (vone, vtwo):
            if vpad.source.fd not in watched:
                fail(f"player {vpad.player}'s physical pad is not watched, so "
                     f"its button presses never reach anything")
            if vpad.ui.fd not in watched:
                fail(f"player {vpad.player}'s uinput node is not watched, so "
                     f"force feedback requests are never answered")
        ok("every physical source descriptor is watched")
        ok("every uinput descriptor is watched, so rumble can travel back")
        if len(rep.fds) != 4:
            fail(f"fds returned {len(rep.fds)} descriptors for two pads: "
                 f"{rep.fds}")
        ok("two pads produce exactly four descriptors")

        # A descriptor the republisher does not own must be ignored, not
        # crashed on: the daemon shares one selector with its socket.
        try:
            rep.handle_readable(max(watched) + 1000)
        except Exception as error:  # noqa: BLE001
            fail(f"handle_readable on a descriptor the republisher does not "
                 f"own raised {type(error).__name__}: {error}")
        ok("a descriptor it does not own is ignored rather than crashed on")
    finally:
        bench.close()


def check_burst_latency_and_loss() -> None:
    """S1 — "there's a lag on the controllers", measured rather than argued."""
    heading("S1 — forwarding latency and loss over a burst of 3000 events")
    bench = Bench()
    try:
        # The default pad shape, whose axes declare fuzz 0. With a fuzz the
        # kernel's own noise filter would swallow small changes, and that is
        # indistinguishable here from a republisher that dropped them.
        if dict(PAD_CAPS[ecodes.EV_ABS])[ecodes.ABS_X].fuzz:
            fail("the burst axis declares a fuzz, so the kernel would filter "
                 "some of the burst and the loss figure below would be a lie")
        src = bench.source("burst")
        vpad = bench.publish(src)
        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)

        frames, batch = 1000, 25
        sent = frames * 3
        latencies: list[float] = []
        counts = {ecodes.EV_KEY: 0, ecodes.EV_ABS: 0, ecodes.EV_SYN: 0}
        dropped = 0
        frame = 0
        started = time.monotonic()
        while frame < batch * (frames // batch):
            marker = time.time()
            for i in range(batch):
                index = frame + i
                # Alternate starting with a press: the kernel discards an
                # EV_KEY event that does not change state, and a discarded
                # source event would read here as republisher loss.
                src.emit(ecodes.EV_KEY, BTN_A, 1 - (index % 2))
                src.emit(ecodes.EV_ABS, ecodes.ABS_X, 1 + (index % 254))
                src.syn()
            frame += batch
            expect = batch * 3
            seen = 0
            deadline = time.monotonic() + 2.0
            while seen < expect and time.monotonic() < deadline:
                rep.pump(timeout=0.01)
                ready, _, _ = select.select([reader.fd], [], [], 0.0)
                if not ready:
                    continue
                try:
                    events = list(reader.read())
                except BlockingIOError:
                    continue
                for event in events:
                    seen += 1
                    latencies.append(event.timestamp() - marker)
                    if event.type in counts:
                        counts[event.type] += 1
                    if (event.type == ecodes.EV_SYN
                            and event.code == ecodes.SYN_DROPPED):
                        dropped += 1
        elapsed = time.monotonic() - started

        received = sum(counts.values())
        milliseconds = sorted(value * 1000.0 for value in latencies)
        median = statistics.median(milliseconds)
        p99 = milliseconds[min(len(milliseconds) - 1,
                               int(len(milliseconds) * 0.99))]
        worst = milliseconds[-1]
        print(f"      {received}/{sent} events in {elapsed:.2f}s; "
              f"latency ms median {median:.3f} p99 {p99:.3f} max {worst:.3f}")

        if dropped:
            fail(f"the kernel reported SYN_DROPPED {dropped} times: the clone "
                 f"overflowed and a game lost input")
        ok("no SYN_DROPPED anywhere in the burst")

        if received != sent:
            fail(f"{sent - received} of {sent} events never reached the "
                 f"clone; input that arrives incomplete is indistinguishable "
                 f"from input mapped wrongly when you are holding the pad")
        ok(f"all {sent} events reached the clone -- none lost")
        if counts[ecodes.EV_KEY] != frames:
            fail(f"{counts[ecodes.EV_KEY]} button events of {frames}")
        if counts[ecodes.EV_ABS] != frames:
            fail(f"{counts[ecodes.EV_ABS]} axis events of {frames}")
        if counts[ecodes.EV_SYN] != frames:
            fail(f"{counts[ecodes.EV_SYN]} frame boundaries of {frames}")
        ok("the counts split evenly: 1000 presses, 1000 axis moves, 1000 frames")

        # Generous by two orders of magnitude against what this measures
        # (~0.1ms median). They are set to catch the shape of the real report:
        # a stall of 80-176ms about once a second, caused by a device scan on
        # the thread that forwards controller events.
        if median > 10.0:
            fail(f"median forwarding latency is {median:.2f}ms; the pad's own "
                 f"cadence is about 8ms, so this is felt as lag")
        ok(f"median forwarding latency {median:.3f}ms (bound 10ms)")
        if p99 > 40.0:
            fail(f"99th percentile latency is {p99:.2f}ms")
        ok(f"99th percentile {p99:.3f}ms (bound 40ms)")
        if worst > 150.0:
            fail(f"the worst single event took {worst:.1f}ms to cross; that "
                 f"is the shape of the reported stall -- 'it feels more like "
                 f"it's just arriving at a very slow rate'")
        ok(f"worst single event {worst:.3f}ms (bound 150ms)")
    finally:
        bench.close()


def check_hostile_calibration() -> None:
    """A stored profile is user data, and it is read straight into the pump.

    Was a printed gap until the bound went in. The failure it describes was
    measured, not imagined: a profile declaring a range of +-2**40 made
    `apply(150)` return 1099511627776, `write_event` raise OverflowError --
    which is not an OSError, so nothing between here and the daemon's selector
    callback caught it -- and the process ended mid-game with every player's
    controller going dead at once. "The controllers just died."
    """
    heading("S12 — a hand-edited calibration is read straight onto the hot path")
    bench = Bench()
    try:
        # Valid JSON, a real object, every field the right type -- and the
        # declared range is wider than an evdev value can be.
        huge = {"center": 128, "min": -(2 ** 40), "max": 2 ** 40,
                "flat": 0, "reach_min": 100, "reach_max": 150}
        sane = {"center": 128, "min": 0, "max": 255, "flat": 4,
                "reach_min": 20, "reach_max": 240}

        forward = bench.source("hostilefwd")
        forward.store_profile({}, raw={
            "signature": profiles.signature(forward.pad),
            "name": "hand edited",
            "axes": {str(ecodes.ABS_X): huge, str(ecodes.ABS_Y): sane},
        })
        loaded = profiles.load(forward.pad)
        if loaded is None:
            fail("profiles.load threw the whole profile away over one bad "
                 "axis; the controller's name, icon and every mapping went "
                 "with it, so the pad reads as one nobody has ever set up")
        if ecodes.ABS_X in loaded.axes:
            fail(f"profiles.load kept a calibration declaring a range of "
                 f"{huge['min']}..{huge['max']}; no evdev value can carry "
                 f"that, so it is carried to the hot path where writing it "
                 f"raises OverflowError and the daemon exits mid-game -- "
                 f"every controller dead at once")
        ok("an axis whose declared range cannot fit an evdev value is "
           "rejected when the profile is read")
        if ecodes.ABS_Y not in loaded.axes:
            fail("the sane axis beside it was dropped too; one bad number "
                 "must cost one axis, not every calibrated stick on the pad")
        ok("the sane axis in the same file survives, and so does the profile")
        if loaded.name != "hand edited":
            fail("the rest of the profile did not survive the rejected axis")
        ok("name, icon and mappings are untouched by the rejection")

        # The rejection has to reach `virtual`, not just `profiles`: what the
        # user gets is an *uncorrected* axis, which is exactly what an
        # uncalibrated pad already does and is the one behaviour here known to
        # work.
        vpad = bench.publish(forward)
        if ecodes.ABS_X in vpad.axes:
            fail("the republisher is applying the rejected calibration anyway")
        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)
        forward.frame((ecodes.EV_ABS, ecodes.ABS_X, 150),
                      (ecodes.EV_KEY, BTN_A, 1))
        try:
            got = triples(one_clone(rep, reader, seconds=0.2))
        except Exception as error:  # noqa: BLE001 - the bug itself
            fail(f"Republisher.pump died forwarding one axis event "
                 f"({type(error).__name__}: {error}); this call is three "
                 f"frames below the daemon's selector callback with no guard "
                 f"in between, so the daemon exits and takes every player's "
                 f"controller with it")
        if (ecodes.EV_ABS, ecodes.ABS_X, 150) not in got:
            fail(f"the uncalibrated axis did not arrive verbatim: {got}; a "
                 f"rejected calibration must leave the stick working, not "
                 f"silence it")
        ok("the axis is forwarded uncorrected, and the raw 150 arrives as 150")
        if (ecodes.EV_KEY, BTN_A, 1) not in got:
            fail("the button pressed in the same frame never arrived, so the "
                 "pad went quiet even though the daemon survived")
        ok("the rest of the frame still arrives -- the pad keeps working")

        # The same value reached through create()'s centre seed, which runs
        # before the loop ever starts. Its failure is worse than the hot
        # path's: server catches OSError only, so it escapes after the
        # assignments are written and the setup screen waits forever.
        seeding = bench.source("hostileseed")
        seeding.store_profile({}, raw={
            "signature": profiles.signature(seeding.pad),
            "axes": {str(ecodes.ABS_X): {"center": 0, "min": 2 ** 40,
                                         "max": 2 ** 40 + 2, "flat": 0}},
        })
        try:
            seeded = bench.publish(seeding, allow_failure=True)
        except Exception as error:  # noqa: BLE001 - the bug itself
            raise SystemExit(
                f"FAIL: virtual.create died seeding an axis at the midpoint of "
                f"a stored range ({type(error).__name__}: {error}); the daemon "
                f"catches OSError only, so this ends it after the assignments "
                f"have been written and the setup screen waits forever") \
                from None
        if seeded.axes:
            fail("create() published with an unwritable calibration attached")
        ok("virtual.create publishes the pad instead of dying on the centre "
           "seed")
    finally:
        bench.close()


def check_live_calibration_cannot_kill_create() -> None:
    """S12 — the store is not the only source of a calibration.

    `calibrate` builds one from a live measurement and hands it to `create`
    without a round trip through JSON, so the bound `profiles.from_json`
    enforces on read is not reached at all on that path. An absinfo read off a
    lying adapter is exactly the sort of thing that produces one, and the cost
    is not a bad axis -- the centre seed raises OverflowError, which is not an
    OSError, so it escapes `create` past every caller's guard.
    """
    heading("S12 — a calibration that never came from a file cannot kill create")
    bench = Bench()
    try:
        src = bench.source("liveinsane")
        insane = profiles.AxisCalibration(
            center=0, minimum=2 ** 40, maximum=2 ** 40 + 2, flat=0)
        if insane.fits_evdev():
            fail("AxisCalibration.fits_evdev() calls a range of 2**40 "
                 "writable; the bound is not being enforced at all")
        ok("fits_evdev() rejects a range no evdev value can carry")

        real_load = virtual.load_profile

        def measured(pad, directory=None):
            return profiles.Profile(signature=profiles.signature(pad),
                                    axes={ecodes.ABS_X: insane})

        virtual.load_profile = measured
        try:
            vpad = bench.publish(src, allow_failure=True)
        except Exception as error:  # noqa: BLE001 - the bug itself
            raise SystemExit(
                f"FAIL: virtual.create died on a calibration handed to it "
                f"directly ({type(error).__name__}: {error}); a measurement "
                f"that never touched the profile store still ends the daemon "
                f"with the setup screen waiting") from None
        finally:
            virtual.load_profile = real_load

        if ecodes.ABS_X in vpad.axes:
            fail("create() kept the unwritable calibration, so the first "
                 "stick movement takes the daemon down instead")
        ok("create() drops it and publishes the pad uncorrected")
    finally:
        bench.close()


def check_one_refused_event_is_not_the_daemon() -> None:
    """S1 — whatever the cause, one bad write must not end the process.

    The bound in `profiles` stops the value that was actually reported ever
    being stored. This is the other half: `_forward` sits three frames below
    the daemon's selector callback, and *any* exception from the clone write
    -- not just the one overflow already seen -- reaches it. A controller
    missing an input recovers; a daemon exiting does not.
    """
    heading("S1 — one event the clone refuses does not take the daemon down")
    bench = Bench()
    try:
        src = bench.source("refused")
        vpad = bench.publish(src)
        reader = bench.reader(vpad)
        rep = bench.republisher(vpad)

        # Injected past create()'s filter deliberately: the point is that the
        # write is guarded regardless of how a bad value got here.
        vpad.axes[ecodes.ABS_X] = profiles.AxisCalibration(
            center=128, minimum=-(2 ** 40), maximum=2 ** 40, flat=0,
            reach_min=100, reach_max=150)

        src.frame((ecodes.EV_ABS, ecodes.ABS_X, 150))
        src.frame((ecodes.EV_KEY, BTN_A, 1))
        try:
            got = triples(one_clone(rep, reader, seconds=0.2))
        except Exception as error:  # noqa: BLE001 - the bug itself
            fail(f"the write escaped _forward as {type(error).__name__}: "
                 f"{error} -- it escapes pump(), escapes run(), and ends the "
                 f"daemon mid-game, which the user reports as 'the "
                 f"controllers just died'")
        ok("the republisher survived an event the clone would not take")
        if vpad.dropped != 1:
            fail(f"the refused event was counted {vpad.dropped} times, not "
                 f"once; the count is what keeps the log from writing a line "
                 f"per event for as long as the game runs")
        ok("the refused event is counted once, not logged per event")
        if (ecodes.EV_KEY, BTN_A, 1) not in got:
            fail("the next press never arrived, so the pad went quiet -- "
                 "dropping one event must not stop the ones after it")
        ok("the very next press still crosses, so the pad keeps playing")
        if rep._stop:
            fail("the republisher stopped itself over one refused event")
        ok("the republisher is still running")
    finally:
        bench.close()


def check_profile_numbers_that_are_not_numbers() -> None:
    """S12 — JSON can hold a number Python cannot make an int of.

    `1e400` parses to `inf`, and `int(inf)` raises OverflowError. That escaped
    `Profile.from_json` entirely -- `load`'s except wraps only the read and
    the parse -- and since `is_known` is `load() is not None` and discovery
    calls it for every pad, one damaged file stopped the whole controller
    list rather than costing one axis.
    """
    heading("S12 — an axis value JSON can hold but int() cannot take")
    for label, value in (("1e400 (inf)", 1e400), ("-1e400 (-inf)", -1e400)):
        raw = {"signature": "sig", "name": "kept",
               "axes": {"0": {"center": 0, "min": 0, "max": value, "flat": 0},
                        "1": {"center": 128, "min": 0, "max": 255,
                              "flat": 0}}}
        try:
            profile = profiles.Profile.from_json(json.loads(json.dumps(raw)))
        except Exception as error:  # noqa: BLE001 - the bug itself
            fail(f"a profile holding {label} raised {type(error).__name__} "
                 f"out of from_json; is_known() calls load() for every pad "
                 f"during discovery, so one such file leaves the user with "
                 f"an empty controller list")
        if 0 in profile.axes:
            fail(f"the axis declaring {label} was kept")
        if 1 not in profile.axes:
            fail(f"the sane axis beside {label} was dropped with it")
        if profile.name != "kept":
            fail(f"{label} cost the whole profile, not one axis")
        ok(f"an axis declaring {label} costs that axis and nothing else")


def check_failed_publish_does_not_keep_the_pad() -> None:
    """S5 — if publishing fails the physical pad must not stay grabbed."""
    heading("S5 — a publish that fails does not keep the physical pad")
    bench = Bench()
    try:
        src = bench.source("failedpublish")
        real = virtual.evdev.UInput

        def refuse(*args, **kwargs):
            # What a machine with /dev/uinput unloaded or mis-permissioned
            # does, which is a real deployment failure rather than a fiction.
            raise OSError(13, "Permission denied: /dev/uinput")

        virtual.evdev.UInput = refuse
        try:
            virtual.create(src.pad, 99)
            fail("create() returned even though uinput refused")
        except OSError:
            ok("create() reports the failure as an OSError, which the daemon "
               "catches")
        finally:
            virtual.evdev.UInput = real

        # The traceback keeps create()'s frame -- and with it the open source
        # device -- alive until the except block above has been left.
        gc.collect()

        probe = evdev.InputDevice(src.pad.path)
        try:
            probe.grab()
            probe.ungrab()
            ok("the physical pad is not left grabbed by the failed publish")
        except OSError as error:
            fail(f"a failed publish left EVIOCGRAB held on {src.pad.path} "
                 f"({error}); a retry can never succeed and the machine has "
                 f"no controllers until the daemon is restarted")
        finally:
            probe.close()
    finally:
        bench.close()


# -- entry point -------------------------------------------------------------

SCENARIOS = [
    check_forward_types_reach_the_clone,
    check_unforwarded_types_are_dropped,
    check_syn_completes_every_frame,
    check_capabilities_are_mirrored,
    check_absinfo_survives,
    check_force_feedback_is_not_invented,
    check_force_feedback_is_mirrored,
    check_identity_is_mirrored_by_default,
    check_padmap_identity_mode,
    check_name_and_phys,
    check_calibration_is_applied,
    check_uncalibrated_pads_pass_through,
    check_only_calibrated_axes_are_touched,
    check_calibration_is_keyed_on_type_not_code,
    check_calibrated_pads_start_centred,
    check_grab_is_exclusive,
    check_close_releases_both_ends,
    check_republisher_close_releases_every_pad,
    check_stop_ends_run,
    check_vanished_source_stops_the_loop,
    check_two_pads_stay_separate,
    check_descriptors_cover_both_directions,
    check_burst_latency_and_loss,
    check_hostile_calibration,
    check_live_calibration_cannot_kill_create,
    check_one_refused_event_is_not_the_daemon,
    check_profile_numbers_that_are_not_numbers,
    check_failed_publish_does_not_keep_the_pad,
]


def uinput_available() -> str:
    """Empty if uinput works here, otherwise why it does not."""
    try:
        probe = Source("probe")
    except Unavailable as error:
        return str(error)
    except PermissionError as error:
        return f"no permission for /dev/uinput ({error})"
    except OSError as error:
        return f"/dev/uinput is not usable ({error})"
    except Exception as error:  # noqa: BLE001 - evdev raises its own class
        return f"{type(error).__name__}: {error}"
    probe.close()
    return ""


def main() -> int:
    reason = uinput_available()
    if reason:
        print("skipped: the republisher can only be measured through uinput, "
              f"and {reason}")
        print("all checks passed")
        return 0

    for scenario in SCENARIOS:
        scenario()

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
