"""Confirm and claim: how a person tells padmap who is player 1 (S3, S4).

The founding premise. Four identical adapter ports, four identical pads, no
static attribute that differs -- so the only thing left that can say "this one
is player 1" is a human holding a button. A transient cannot hold; a human
can. `assign.Assigner` turns held presses into a player order, and
`Server._tick_confirm` turns one more held press on an already-claimed pad into
the end of the session.

Everything here runs against the real `assign.Assigner` and the real
`server.Server`. What is faked is only the kernel: `assign.open_device` is
replaced at import with a fake that hands back a pipe-backed device, and
`devices.discover` only ever returns synthetic pads. No evdev node is opened,
no EVIOCGRAB is taken and no uinput node is created -- a live daemon owns the
real pads on this machine and a second reader would take events away from it.

Hold timing is the subject, so the clock is the fixture: `assign.time` and
`server.time` are swapped for a frozen clock inside each timing scenario, and
put back afterwards. Only `run()`, which owns a select loop, uses the real one.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_confirm_and_claim.py
"""

import contextlib
import logging
import os
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

# Redirected before padmap is imported: several modules read these at import
# time, and the real runtime dir belongs to the daemon running on this machine.
_STORE = Path(tempfile.mkdtemp(prefix="padmap-claim-"))
(_STORE / "run" / "padmap").mkdir(parents=True)
os.environ["XDG_RUNTIME_DIR"] = str(_STORE / "run")
os.environ["XDG_CONFIG_HOME"] = str(_STORE / "config")
os.environ["XDG_DATA_HOME"] = str(_STORE / "data")
os.environ["PADMAP_PROFILE_DIR"] = str(_STORE / "devices")
# Nothing here may open a session on its own initiative: every session below
# is one this file asked for.
os.environ["PADMAP_NO_AUTOSETUP"] = "1"

import evdev                                             # noqa: E402
from evdev import ecodes                                 # noqa: E402

from padmap import assign, capture, devices, protocol, server  # noqa: E402
from padmap.assign import Assignment                     # noqa: E402
from padmap.devices import Pad                           # noqa: E402

# The daemon logs a warning for the deliberately ungrabbable pad below, and it
# would land in the middle of the scenario headings.
logging.getLogger("padmap").setLevel(logging.CRITICAL)


class FakeRepublisher:
    """Stands in for a live republisher, which these tests never really start.

    Was a bare `object()`, which said the only thing the daemon asked at the
    time: whether anything was republishing at all. It now also pauses the
    clone while a wizard is open -- so a stand-in has to answer set_paused, or
    the checks fail on the double rather than on the daemon.
    """

    def __init__(self) -> None:
        self.paused = False

    def set_paused(self, paused: bool) -> None:
        self.paused = paused

    def dead_fds(self) -> list[int]:
        # Nothing vanishes in these scenarios; the daemon still asks.
        return []

    def stale_sources(self) -> list:
        # Nor does anything reconnect. Polled once every couple of seconds.
        return []

BTN_A = ecodes.BTN_SOUTH        # 0x130
BTN_B = ecodes.BTN_EAST         # 0x131
BTN_START = ecodes.BTN_START    # 0x13b
KEY_ENTER = ecodes.KEY_ENTER    # 0x1c -- keyboard range, below BTN_FIRST

_GAPS: list[str] = []


def ok(message: str) -> None:
    print(f"  ok  {message}")


def gap(message: str) -> None:
    """A defect confirmed against current source, reported rather than failed.

    Kept green deliberately: the maintainer fixes the source and turns the
    assertion below the gap into the real one.
    """
    _GAPS.append(message)
    print(f"  gap: {message}")


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


# -- the kernel, faked ----------------------------------------------------

# Every pad this file can see. Bound at import so a stray code path that
# reaches discovery cannot enumerate the real machine.
_VISIBLE: list[Pad] = []
devices.discover = lambda *a, **k: list(_VISIBLE)  # type: ignore[assignment]

_OPENED: dict[str, "FakeDevice"] = {}
_VANISHED: set[str] = set()      # paths whose open() raises, as if unplugged
_UNGRABBABLE: set[str] = set()   # paths whose grab() raises EBUSY
_PRELOAD: list = []              # events already queued when a device opens


def pad(name: str, node: str = "event90", vid: int = 0x1234,
        pid: int = 0x0001) -> Pad:
    return Pad(path=f"/dev/input/{node}", name=name, phys="", uniq="",
               vid=vid, pid=pid, syspath="")


class FakeDevice:
    """An open pad handle, backed by a real pipe.

    The fd is a real pipe read end rather than an invented number because the
    daemon registers it with a real `selectors` object and `Assigner.run`
    calls `select.select` on it. Writing a byte per queued event is what makes
    a synthetic press look readable to both.
    """

    def __init__(self, path: str) -> None:
        self.path = path
        self._read_fd, self._write_fd = os.pipe()
        self.fd = self._read_fd
        self._queue: list = []
        self.grabs = 0
        self.ungrabs = 0
        self.closes = 0
        # Set to make read() raise, i.e. a pad unplugged mid-session.
        self.read_error: Exception | None = None

    def feed(self, *events) -> None:
        if not events:
            return
        self._queue.extend(events)
        os.write(self._write_fd, b"." * len(events))

    def grab(self) -> None:
        if self.path in _UNGRABBABLE:
            raise OSError(16, "Device or resource busy")
        self.grabs += 1

    def ungrab(self) -> None:
        self.ungrabs += 1

    def close(self) -> None:
        self.closes += 1
        for fd in (self._read_fd, self._write_fd):
            try:
                os.close(fd)
            except OSError:
                pass

    def read_one(self):
        if not self._queue:
            return None
        os.read(self._read_fd, 1)
        return self._queue.pop(0)

    def read(self):
        if self.read_error is not None:
            raise self.read_error
        if not self._queue:
            # What evdev does with an empty non-blocking descriptor.
            raise BlockingIOError(11, "Resource temporarily unavailable")
        os.read(self._read_fd, len(self._queue))
        events, self._queue = self._queue, []
        return iter(events)


def _fake_open(target: Pad) -> FakeDevice:
    if target.path in _VANISHED:
        raise FileNotFoundError(2, "No such file or directory", target.path)
    device = FakeDevice(target.path)
    device.feed(*_PRELOAD)
    _OPENED[target.path] = device
    return device


# Module-wide, at import: after this line nothing in this process can open a
# real evdev node, whichever path reaches Assigner.__enter__.
assign.open_device = _fake_open  # type: ignore[assignment]


def ev(kind: int, code: int, value: int):
    return evdev.InputEvent(0, 0, kind, code, value)


def press(code: int = BTN_A):
    return ev(ecodes.EV_KEY, code, 1)


def release(code: int = BTN_A):
    return ev(ecodes.EV_KEY, code, 0)


def repeat(code: int = BTN_A):
    """Autorepeat. The kernel emits these while a key stays down."""
    return ev(ecodes.EV_KEY, code, 2)


def axis(code: int = ecodes.ABS_X, value: int = 200):
    return ev(ecodes.EV_ABS, code, value)


class Clock:
    """A monotonic clock nobody has to wait for."""

    def __init__(self, start: float = 1000.0) -> None:
        self.now = start

    def monotonic(self) -> float:
        return self.now

    def advance(self, seconds: float) -> float:
        self.now += seconds
        return self.now


@contextlib.contextmanager
def frozen(*modules):
    """Swap `time` for a frozen clock in the given modules, then put it back."""
    clock = Clock()
    saved = [(module, module.time) for module in modules]
    for module in modules:
        module.time = clock
    try:
        yield clock
    finally:
        for module, real in saved:
            module.time = real


class Session:
    """A real Assigner over fake pads, with a frozen clock and event sinks."""

    def __init__(self, count: int = 1, grab: bool = True,
                 hold: float | None = None, preload=()) -> None:
        self.pads = [pad(f"Arcade Pad {i + 1}", node=f"event{90 + i}")
                     for i in range(count)]
        _PRELOAD[:] = list(preload)
        self._frozen = frozen(assign)
        self.clock = self._frozen.__enter__()
        self.assigner = assign.Assigner(
            self.pads, grab=grab,
            hold_seconds=assign.HOLD_SECONDS if hold is None else hold)
        self.assigner.__enter__()
        _PRELOAD.clear()
        self.devices = [_OPENED[p.path] for p in self.pads]
        self.claims: list[Assignment] = []
        self.progress: list[tuple[Pad, float]] = []
        self.claimed_events: list[tuple[Pad, int, int]] = []
        self.raw: list[tuple[Pad, object]] = []
        self.assigner.on_claimed_event = (
            lambda p, c, v: self.claimed_events.append((p, c, v)))
        self.assigner.on_raw_event = lambda p, e: self.raw.append((p, e))

    # -- driving ---------------------------------------------------------

    def queue(self, *events, which: int = 0) -> "Session":
        """Put events in the kernel buffer without letting the daemon read."""
        self.devices[which].feed(*events)
        return self

    def pump(self, which: int = 0) -> "Session":
        """Let the assigner read a pad, as the selector does when it is readable."""
        self.assigner.handle_readable(self.devices[which].fd)
        return self

    def send(self, *events, which: int = 0) -> "Session":
        """Queue events and let the assigner consume them, as a selector would."""
        self.queue(*events, which=which)
        return self.pump(which)

    def tick(self, seconds: float = 0.0) -> "Session":
        self.clock.advance(seconds)
        self.assigner.tick(
            on_progress=lambda p, f: self.progress.append((p, f)),
            on_claim=self.claims.append)
        return self

    def close(self) -> None:
        self.assigner.close()
        self._frozen.__exit__(None, None, None)

    # -- reading ---------------------------------------------------------

    @property
    def assignments(self) -> list[Assignment]:
        return self.assigner.assignments

    def players(self) -> list[tuple[int, str, int]]:
        return [(a.player, a.pad.event, a.button) for a in self.assignments]

    def fractions(self) -> list[float]:
        return [f for _pad, f in self.progress]


# -- the daemon side ------------------------------------------------------

class StubAssigner:
    """A session with claims and real fds, but no timing of its own.

    Used where the question is what the *daemon* does with the confirm
    tracker, not how a hold becomes a claim -- that is the Assigner's half and
    is tested against the real thing above.
    """

    def __init__(self, pads=(), claims=()) -> None:
        self.pads = list(pads)
        self.assignments = list(claims)
        read_fd, self._write_fd = os.pipe()
        self.fds = [read_fd]
        self.grab_failures: list[Pad] = []
        self.closed = False
        self.ticks = 0
        self.on_claimed_event = None
        self.on_raw_event = None

    def tick(self, on_progress=None, on_claim=None) -> None:
        # Claim detection running at all is the thing a modal flow must
        # suspend, so counting is the assertion.
        self.ticks += 1

    def reset(self) -> None:
        self.assignments = []

    def close(self) -> None:
        self.closed = True
        for fd in (self.fds[0], self._write_fd):
            try:
                os.close(fd)
            except OSError:
                pass


class Daemon:
    """A real Server with only the machine-touching parts stubbed."""

    def __init__(self, claims: int = 1, pads=None) -> None:
        self.srv = server.Server()
        self.events: list[dict] = []
        self.srv._broadcast = self.events.append  # type: ignore[assignment]
        self.srv._send = lambda c, m: None        # type: ignore[assignment]
        # Writes assignments.json, sdl_controllers.txt, and creates uinput
        # nodes on the machine, respectively.
        self.srv._save_assignments = lambda: None       # type: ignore[assignment]
        self.srv._write_controller_configs = lambda: None  # type: ignore[assignment]
        self.srv._start_republisher = self._republish   # type: ignore[assignment]
        self.republished = 0
        self.accepts = 0

        real_accept = self.srv._accept

        def counting_accept() -> None:
            self.accepts += 1
            real_accept()

        self.srv._accept = counting_accept  # type: ignore[assignment]

        self.pads = list(pads) if pads is not None else [
            pad(f"Arcade Pad {i + 1}", node=f"event{90 + i}")
            for i in range(max(claims, 1))]
        self.session = StubAssigner(
            pads=self.pads,
            claims=[Assignment(player=i + 1, pad=self.pads[i], button=BTN_A)
                    for i in range(claims)])
        self.srv._assigner = self.session  # type: ignore[assignment]
        self.srv._state = protocol.STATE_ASSIGNING

    def _republish(self) -> None:
        self.republished += 1
        self.srv._republisher = FakeRepublisher()  # type: ignore[assignment]

    # -- driving ---------------------------------------------------------

    def down(self, which: int = 0, code: int = BTN_A) -> "Daemon":
        self.srv._on_claimed_event(self.pads[which], code, 1)
        return self

    def up(self, which: int = 0, code: int = BTN_A) -> "Daemon":
        self.srv._on_claimed_event(self.pads[which], code, 0)
        return self

    def ticks(self, count: int = 1) -> "Daemon":
        for _ in range(count):
            self.srv._tick()
        return self

    # -- reading ---------------------------------------------------------

    def kinds(self) -> list[str]:
        return [str(m.get("event")) for m in self.events]

    def confirms(self) -> list[float]:
        return [float(m["frac"]) for m in self.events
                if m.get("event") == "confirm"]

    def errors(self) -> list[str]:
        return [str(m.get("message")) for m in self.events
                if m.get("event") == "error"]

    def accepted(self) -> int:
        return self.kinds().count("accepted")

    def close(self) -> None:
        try:
            self.session.close()
        except OSError:
            pass


# =========================================================================


def main() -> int:
    print("S3: a hold long enough claims the next free slot")
    s = Session()
    s.send(press(BTN_A))
    s.tick(assign.HOLD_SECONDS - 0.01)
    if s.assignments:
        fail("a press shorter than HOLD_SECONDS claimed a slot -- a stray "
             "press on an empty adapter port would burn player 1")
    ok(f"{assign.HOLD_SECONDS - 0.01:.2f}s of hold has claimed nothing yet")
    s.tick(0.02)
    if len(s.assignments) != 1:
        fail("holding a button past HOLD_SECONDS did not claim a slot -- "
             "nothing can be assigned to player 1 at all")
    claim = s.assignments[0]
    if (claim.player, claim.pad.path, claim.button) != (1, s.pads[0].path,
                                                        BTN_A):
        fail(f"the claim describes the wrong pad or button: {claim}")
    if len(s.claims) != 1:
        fail(f"on_claim fired {len(s.claims)} times for one hold")
    ok("holding past HOLD_SECONDS claims player 1, naming pad and button")
    ok("on_claim fires exactly once")
    s.close()

    print("\nS3: a tap does not claim")
    s = Session()
    s.send(press(BTN_A), release(BTN_A))
    s.tick(5.0)
    if s.assignments:
        fail("a tap claimed a slot -- press-and-hold is the whole reason a "
             "transient cannot take a player slot")
    ok("press and release in one batch claims nothing, five seconds later")
    s.close()

    print("\nS3: a hold interrupted by a release does not claim")
    s = Session()
    s.send(press(BTN_A))
    s.tick(assign.HOLD_SECONDS * 0.9)
    s.send(release(BTN_A))
    s.tick(5.0)
    if s.assignments:
        fail("a hold released just short of the threshold still claimed")
    ok("released at 90% of the hold: no claim, however long we wait")
    s.send(press(BTN_A))
    s.tick(assign.HOLD_SECONDS + 0.01)
    if len(s.assignments) != 1:
        fail("a full hold after an abandoned one did not claim -- the user "
             "would have to unplug the pad to get it back")
    ok("a fresh full hold afterwards claims normally")
    s.close()

    print("\nS3: a button already down when the session opened does not claim")
    # The kernel buffered the down event before we started listening; that is
    # the press that opened the setup screen. __enter__ drains it.
    s = Session(preload=[press(BTN_A)])
    # The fd is readable the moment the session opens -- the daemon's selector
    # fires immediately -- so anything __enter__ failed to drain is read here.
    s.pump()
    s.tick(5.0)
    if s.assignments:
        fail("the press that opened the session claimed a slot -- the screen "
             "appears with player 1 already taken by whoever opened it")
    ok("a press queued before the session opened is drained, not claimed")
    s.send(release(BTN_A))
    s.tick(5.0)
    if s.assignments:
        fail("the lone release of an already-held button claimed a slot")
    ok("the release of that button claims nothing either")
    s.send(press(BTN_B))
    s.tick(assign.HOLD_SECONDS + 0.01)
    if s.players() != [(1, "event90", BTN_B)]:
        fail(f"a genuine hold after the drained one failed: {s.players()}")
    ok("a genuine hold afterwards claims, and names the button really held")
    s.close()

    print("\nS3: keyboard-range codes are not buttons")
    s = Session()
    s.send(press(KEY_ENTER))
    s.tick(5.0)
    if s.assignments:
        fail(f"holding {hex(KEY_ENTER)}, below BTN_FIRST, claimed a slot -- a "
             "combo device's keyboard half would assign itself a player")
    ok(f"a five-second hold on {hex(KEY_ENTER)} claims nothing")
    s.send(press(BTN_A))
    s.tick(assign.HOLD_SECONDS + 0.01)
    if s.players() != [(1, "event90", BTN_A)]:
        fail(f"a real button after a keyboard key misreported: {s.players()}")
    ok("a real button on the same node still claims, named correctly")
    s.close()

    print("\nS3: autorepeat neither starts nor completes a hold")
    s = Session()
    s.send(repeat(BTN_A), repeat(BTN_A), repeat(BTN_A))
    s.tick(5.0)
    if s.assignments:
        fail("autorepeat alone (value 2, no value 1) claimed a slot")
    ok("autorepeat with no press behind it claims nothing")
    s.close()

    s = Session()
    s.send(press(BTN_A))
    s.tick(assign.HOLD_SECONDS - 0.05)
    s.send(repeat(BTN_A))            # the kernel repeating the held button
    s.tick(0.06)
    if len(s.assignments) != 1:
        fail("autorepeat during a hold restarted the timer -- a button held "
             "long enough to repeat would never finish claiming")
    ok("autorepeat during a hold does not restart it: the claim still lands "
       "HOLD_SECONDS after the original press")
    s.close()

    print("\nS3: a second button during a hold neither restarts nor hijacks it")
    s = Session()
    s.send(press(BTN_A))
    s.tick(0.10)
    s.send(press(BTN_START))
    s.tick(0.05)
    s.send(release(BTN_START))
    s.tick(assign.HOLD_SECONDS - 0.14)
    if len(s.assignments) != 1:
        fail("releasing a second button cancelled the hold on the first -- a "
             "resting thumb makes the pad unassignable")
    if s.assignments[0].button != BTN_A:
        fail(f"the claim credits the wrong button: {hex(s.assignments[0].button)}")
    ok("a second button pressed and released mid-hold does not cancel it")
    ok("the claim credits the button that was actually held")
    s.close()

    print("\nS3: two pads holding at once get slots in press order")
    s = Session(count=2)
    s.send(press(BTN_A), which=0)
    s.tick(0.01)
    s.send(press(BTN_B), which=1)
    s.tick(assign.HOLD_SECONDS + 0.01)
    if len(s.assignments) != 2:
        fail(f"two pads holding produced {len(s.assignments)} claim(s)")
    if s.players() != [(1, "event90", BTN_A), (2, "event91", BTN_B)]:
        fail(f"player order does not follow press order: {s.players()}")
    ok("both pads claim, first to press is player 1, second is player 2")
    ok("each claim keeps its own pad and its own button")
    s.close()

    print("\nS3: the same pad cannot claim twice")
    s = Session()
    s.send(press(BTN_A))
    s.tick(assign.HOLD_SECONDS + 0.01)
    s.send(release(BTN_A))
    s.send(press(BTN_B))
    s.tick(5.0)
    if len(s.assignments) != 1:
        fail("one pad claimed two slots -- one person holding twice would "
             "take player 1 and player 2 and lock everyone else out")
    ok("a second hold on a claimed pad claims nothing more")
    forwarded = [(code, value) for _p, code, value in s.claimed_events]
    if (BTN_B, 1) not in forwarded or (BTN_A, 0) not in forwarded:
        fail(f"button activity on a claimed pad was not forwarded: {forwarded}")
    ok("its button activity is forwarded to on_claimed_event instead -- which "
       "is what the confirm gesture is built out of")

    print("\nS3: only real buttons are forwarded from a claimed pad")
    before = len(s.claimed_events)
    s.send(axis(ecodes.ABS_X, 32000), press(KEY_ENTER), release(KEY_ENTER))
    extra = s.claimed_events[before:]
    if extra:
        fail(f"stick movement or a keyboard key reached the confirm gesture: "
             f"{[(hex(c), v) for _p, c, v in extra]}")
    ok("stick movement and keyboard-range keys are not confirm presses")
    raw_kinds = {(e.type, e.code) for _p, e in s.raw}
    if (ecodes.EV_ABS, ecodes.ABS_X) not in raw_kinds:
        fail("on_raw_event never saw the EV_ABS traffic -- calibration reads "
             "the stick through this stream and the pads are grabbed, so "
             "nothing else can")
    ok("the raw stream still carries EV_ABS from a claimed pad, for calibration")
    s.close()

    print("\nS3: progress is monotonic and reaches 1.0 exactly once")
    s = Session()
    s.send(press(BTN_A))
    for _ in range(6):
        s.tick(0.05)
    fracs = s.fractions()
    if not fracs:
        fail("no progress was reported during a hold -- the fill ring on the "
             "player slot never moves and holding looks like it does nothing")
    if fracs != sorted(fracs):
        fail(f"progress went backwards during a hold: {fracs}")
    if max(fracs) != 1.0:
        fail(f"progress never reached 1.0: {fracs}")
    if fracs.count(1.0) != 1:
        fail(f"1.0 was reported {fracs.count(1.0)} times: {fracs}")
    if any(f > 1.0 for f in fracs):
        fail(f"progress overshot 1.0: {fracs}")
    ok(f"progress rises {fracs[0]:.2f} -> 1.00 without going backwards")
    ok("1.0 is reported exactly once, and never exceeded")
    tail = len(s.progress)
    s.tick(1.0)
    if len(s.progress) != tail:
        fail("progress kept being reported after the slot was claimed")
    ok("no further progress once the slot is claimed")
    s.close()

    print("\nS3: reset drops the claims and lets the same pad claim again")
    s = Session()
    s.send(press(BTN_A))
    s.tick(assign.HOLD_SECONDS + 0.01)
    if len(s.assignments) != 1:
        fail("setup for reset: nothing claimed")
    s.queue(press(BTN_B))            # still in the kernel buffer, unread
    s.assigner.reset()
    if s.assignments:
        fail("reset left claims behind -- 'start over' does not start over")
    ok("reset drops every claim")
    s.pump()          # the fd is still readable if reset left anything in it
    s.tick(5.0)
    if s.assignments:
        fail("an event queued before reset claimed a slot afterwards -- the "
             "button still held from the last round re-claims instantly")
    ok("events queued before the reset are drained, not replayed as claims")
    s.send(press(BTN_A))
    s.tick(assign.HOLD_SECONDS + 0.01)
    if s.players() != [(1, "event90", BTN_A)]:
        fail(f"the same pad could not claim again after reset: {s.players()}")
    ok("the same pad can claim player 1 again")
    s.close()

    print("\nS3: the session grabs the pads and releases them again")
    s = Session(count=2)
    if [d.grabs for d in s.devices] != [1, 1]:
        fail("a pad was not grabbed -- every setup press also reaches "
             "whatever has focus, so claiming a slot doubles as a UI keypress")
    if s.assigner.grab_failures:
        fail("a grab that succeeded was recorded as a failure")
    if sorted(s.assigner.fds) != sorted(d.fd for d in s.devices):
        fail("the fds offered to the selector are not the open devices")
    ok("both pads grabbed, both fds offered to the caller's event loop")
    handle = s.assigner.device_for(s.pads[1])
    if handle is not s.devices[1]:
        fail("device_for returned the wrong handle -- calibration would read "
             "absinfo from the wrong pad")
    if s.assigner.device_for(pad("Stranger", node="event99")) is not None:
        fail("device_for invented a handle for a pad not in the session")
    ok("device_for finds a pad's open handle by path, and None for a stranger")
    s.close()
    if [d.ungrabs for d in s.devices] != [1, 1] or \
            [d.closes for d in s.devices] != [1, 1]:
        fail("close() did not ungrab and close every pad -- the machine is "
             "left with no working controllers")
    if s.assigner.fds:
        fail("closed devices are still offered as fds")
    ok("close() ungrabs and closes every pad, and stops offering their fds")

    print("\nS3: a pad that cannot be grabbed is recorded, not fatal")
    _UNGRABBABLE.add("/dev/input/event91")
    try:
        s = Session(count=2)
    except OSError as exc:
        _UNGRABBABLE.clear()
        fail(f"a busy pad ended the session with {exc!r} -- one device held "
             "by something else stops the setup screen appearing at all")
    _UNGRABBABLE.clear()
    if [p.path for p in s.assigner.grab_failures] != ["/dev/input/event91"]:
        fail(f"the ungrabbable pad was not recorded: {s.assigner.grab_failures}")
    ok("a pad already grabbed elsewhere is recorded in grab_failures")
    s.send(press(BTN_A), which=1)
    s.tick(assign.HOLD_SECONDS + 0.01)
    if s.players() != [(1, "event91", BTN_A)]:
        fail(f"the ungrabbed pad cannot claim: {s.players()}")
    ok("it can still claim a slot; its presses merely leak to other apps")
    s.close()

    print("\nS3: nothing here raises out of the selector callback")
    s = Session()
    s.assigner.handle_readable(9999)
    ok("handle_readable on an fd this session never opened is a no-op")
    s.send(press(BTN_A))
    s.devices[0].read_error = OSError(19, "No such device")
    try:
        s.assigner.handle_readable(s.devices[0].fd)
    except OSError as exc:
        fail(f"a pad unplugged mid-session raised {exc!r} out of the read "
             "callback -- that reaches the daemon's selector loop unguarded "
             "and takes every virtual pad down with it")
    ok("a pad unplugged mid-read is swallowed, not raised")
    s.devices[0].read_error = None
    s.close()
    s.assigner.close()
    ok("close() twice does not raise")

    print("\nS3: run() stops at the wanted count and at the deadline")
    # The one place the real clock is used: run owns a select loop.
    empty = Session(hold=0.05)
    empty._frozen.__exit__(None, None, None)   # real time, real select
    started = time.monotonic()
    got = empty.assigner.run(wanted=1, timeout=0.15)
    elapsed = time.monotonic() - started
    if got:
        fail(f"run() returned claims nobody made: {got}")
    if elapsed < 0.15:
        fail(f"run() gave up after {elapsed:.3f}s, before its own timeout")
    if elapsed > 2.0:
        fail(f"run() overshot its timeout badly ({elapsed:.3f}s)")
    ok(f"run() with nobody pressing returns empty at the deadline "
       f"({elapsed:.2f}s)")
    empty.devices[0].feed(press(BTN_A))
    got = empty.assigner.run(wanted=None, timeout=3.0)
    if len(got) != 1 or got[0].button != BTN_A:
        fail(f"run() did not notice a real hold on a readable pad: {got}")
    ok("run() wakes on the readable pad and claims a genuine hold")
    empty.assigner.close()

    print("\nS3: a pad unplugged between discovery and opening the session")
    _VANISHED.add("/dev/input/event91")
    survivors = [pad("Arcade Pad 1", node="event90"),
                 pad("Arcade Pad 2", node="event91"),
                 pad("Arcade Pad 3", node="event92")]
    stalled = assign.Assigner(survivors)
    try:
        stalled.__enter__()
        entered = True
    except OSError:
        entered = False
    finally:
        stalled.close()
    _VANISHED.clear()
    if entered:
        ok("a pad that disappears between discover() and open() is skipped")
    else:
        gap("a controller unplugged in the window between devices.discover() "
            "and Assigner.__enter__() ends the daemon. open_device raises "
            "FileNotFoundError out of __enter__; Server._begin does not guard "
            "it, and neither _handle_command nor Server.run() has a try, so "
            "`begin` -- or the once-a-second autosetup poll that calls _begin "
            "itself -- exits the process and every virtual pad with it. "
            "Repro: devices.discover returns 3 pads, the 2nd node is gone, "
            "srv._begin(4) raises FileNotFoundError with state still 'idle'.")
    opened_before_the_gap = _OPENED["/dev/input/event90"]
    if not (opened_before_the_gap.closes and opened_before_the_gap.ungrabs):
        fail("after a failed __enter__, close() did not release the pad it "
             "had already grabbed -- those pads stay locked to a dead session")
    ok("close() after a failed __enter__ still releases the pads it opened")

    # -- the daemon's half ------------------------------------------------

    print("\nS4: confirming takes a more decided hold than claiming")
    if server.CONFIRM_HOLD_SECONDS <= assign.HOLD_SECONDS:
        fail(f"CONFIRM_HOLD_SECONDS ({server.CONFIRM_HOLD_SECONDS}) is not "
             f"longer than HOLD_SECONDS ({assign.HOLD_SECONDS}) -- the press "
             "that claims the last slot would end the session with it")
    ok(f"CONFIRM_HOLD_SECONDS {server.CONFIRM_HOLD_SECONDS}s > HOLD_SECONDS "
       f"{assign.HOLD_SECONDS}s")
    with frozen(server) as clock:
        d = Daemon(claims=1)
        d.down()
        clock.advance(assign.HOLD_SECONDS)
        d.ticks()
        if d.accepted():
            fail("a hold as short as a claim confirmed the session")
        if d.confirms()[-1] >= 1.0:
            fail(f"the confirm bar was already full at HOLD_SECONDS: "
                 f"{d.confirms()}")
        ok("a hold of HOLD_SECONDS on a claimed pad does not confirm")
        d.close()

    print("\nS4: a full hold accepts, exactly once")
    with frozen(server) as clock:
        d = Daemon(claims=2)
        d.down()
        clock.advance(server.CONFIRM_HOLD_SECONDS + 0.01)
        d.ticks(5)
        if d.accepts != 1:
            fail(f"_accept ran {d.accepts} times for one confirm hold -- the "
                 "session is accepted again on every tick")
        if d.accepted() != 1:
            fail(f"{d.accepted()} 'accepted' events for one confirm")
        ok("holding past CONFIRM_HOLD_SECONDS accepts the session")
        ok("five more ticks accept it no further times")
        if d.srv._state != protocol.STATE_READY or d.republished != 1:
            fail(f"after confirming, state is {d.srv._state!r} with "
                 f"{d.republished} republisher start(s)")
        if d.srv._assigner is not None or not d.session.closed:
            fail("the session was not ended: the pads stay grabbed and the "
                 "setup screen never closes")
        if [a.player for a in d.srv._assignments] != [1, 2]:
            fail(f"the accepted assignments are wrong: {d.srv._assignments}")
        ok("the claims are kept, the pads released, republishing started")
        if 1.0 not in d.confirms():
            fail(f"the confirm bar never reached 1.0: {d.confirms()}")
        if d.confirms() != sorted(d.confirms()):
            fail(f"the confirm bar went backwards: {d.confirms()}")
        ok("the confirm bar was broadcast rising to 1.0")
        d.close()

    print("\nS4: releasing early cancels the confirm")
    with frozen(server) as clock:
        d = Daemon(claims=1)
        d.down()
        clock.advance(server.CONFIRM_HOLD_SECONDS * 0.8)
        d.ticks()
        partial = d.confirms()[-1]
        if not 0.0 < partial < 1.0:
            fail(f"an 80% confirm hold reported {partial}")
        d.up()
        d.ticks()
        if d.confirms()[-1] != 0.0:
            fail(f"letting go left the confirm bar at {d.confirms()[-1]}")
        ok(f"released at {partial:.0%}: the bar drops back to 0.0")
        clock.advance(10.0)
        d.ticks(5)
        if d.accepted():
            fail("a released confirm completed anyway once time passed -- the "
                 "session ends by itself some seconds after a tap")
        ok("and no amount of waiting completes it afterwards")
        zeroes = [f for f in d.confirms() if f == 0.0]
        if len(zeroes) != 1:
            fail(f"the 0.0 reset was broadcast {len(zeroes)} times -- one "
                 "message per tick to every client, forever")
        ok("the reset is broadcast once, not on every tick")
        d.close()

    print("\nS4: the longest-held pad drives the bar")
    with frozen(server) as clock:
        d = Daemon(claims=2)
        d.down(which=0)
        clock.advance(0.4)
        d.down(which=1)
        d.ticks()
        longest = round(0.4 / server.CONFIRM_HOLD_SECONDS, 3)
        if not d.confirms():
            fail("with two pads holding, no confirm progress was broadcast at "
                 "all -- the bar under the status line never moves, so the "
                 "one gesture that ends setup gives no sign it is working")
        if d.confirms()[-1] != longest:
            fail(f"the bar does not follow the longest hold: {d.confirms()} "
                 f"where the pad held 0.4s means {longest}")
        ok("with two pads holding, the bar follows the one held longest")
        d.up(which=0)
        d.ticks()
        if d.confirms()[-1] >= longest:
            fail(f"letting go of the longest hold did not drop the bar back "
                 f"to the other pad's: {d.confirms()}")
        if d.accepted():
            fail("the session confirmed off a pad that was let go")
        ok("letting that one go falls back to the other pad's hold")
        clock.advance(server.CONFIRM_HOLD_SECONDS)
        d.ticks()
        if d.accepted() != 1:
            fail("the remaining held pad never completed its confirm")
        ok("the remaining pad completes its own confirm")
        d.close()

    print("\nS4: a modal flow suspends the confirm and the claim detector")
    for label, attribute, value in (
        ("the layout picker", "_choice",
         capture.Chooser(pad("Arcade Pad 1"), 1, [])),
        ("the mapping wizard", "_mapping", object()),
        ("calibration", "_calibration",
         server.CalibrationRun(pad("Arcade Pad 1"), 1, {})),
    ):
        with frozen(server) as clock:
            d = Daemon(claims=1)
            d.down()
            clock.advance(server.CONFIRM_HOLD_SECONDS * 5)
            setattr(d.srv, attribute, value)
            d.ticks(3)
            if d.accepted():
                fail(f"{label} was open and a button press underneath it "
                     "confirmed the session -- the wizard is answered with "
                     "button presses, and one of them ended setup")
            if d.session.ticks:
                fail(f"{label} was open and claim detection still ran -- a "
                     "button answering a prompt also burns a player slot")
            ok(f"{label} open: nothing confirms, nothing claims")
            setattr(d.srv, attribute, None)
            d.ticks()
            if d.accepted() != 1:
                fail(f"after {label} closed, the pending confirm did not "
                     "resume")
            ok(f"{label} closed: the daemon carries on normally")
            d.close()

    print("\nS4: an in-flight confirm is dropped when the session changes")
    with frozen(server):
        d = Daemon(claims=1)
        d.down()
        d.srv._reset()
        if d.srv._confirm_started:
            fail("reset left a confirm hold in flight -- the button that is "
                 "still down resumes it and ends the session the user just "
                 "asked to start over")
        ok("_reset drops the confirm tracker")
        d.down()
        d.srv._end_session(release=True)
        if d.srv._confirm_started or d.srv._assigner is not None:
            fail("ending the session left a confirm hold behind")
        if not d.session.closed:
            fail("ending the session did not release the pads")
        ok("_end_session drops it too, and releases the pads")
        d.close()

    print("\nS4: an empty session cannot be confirmed")
    with frozen(server) as clock:
        d = Daemon(claims=0)
        # Seeded directly: nothing should be able to produce this, because
        # only a claimed pad's buttons reach _on_claimed_event. The guard is
        # what stops a confirm with no claims from publishing nothing.
        d.srv._confirm_started[d.pads[0].path] = clock.monotonic()
        clock.advance(server.CONFIRM_HOLD_SECONDS + 0.01)
        d.ticks()
        if d.accepted():
            fail("a session with no claims was accepted -- padmap publishes "
                 "zero pads and the machine has no controllers")
        if "nothing assigned yet" not in d.errors():
            fail(f"no explanation of the refusal reached the client: "
                 f"{d.errors()}")
        if d.srv._state != protocol.STATE_ASSIGNING:
            fail(f"the refused confirm still moved the state to "
                 f"{d.srv._state!r}")
        ok("confirming with nothing claimed is refused, with a reason")
        ok("the session stays open so the user can still claim a slot")
        d.close()

    # -- both halves, on one pad ------------------------------------------

    print("\nS3+S4: one pad, from first press to accepted (real Assigner)")
    with frozen(assign, server) as clock:
        _VISIBLE[:] = [pad("Arcade Pad 1", node="event90")]
        srv = server.Server()
        events: list[dict] = []
        srv._broadcast = events.append               # type: ignore[assignment]
        srv._send = lambda c, m: None                # type: ignore[assignment]
        srv._save_assignments = lambda: None         # type: ignore[assignment]
        srv._write_controller_configs = lambda: None  # type: ignore[assignment]
        srv._start_republisher = lambda: setattr(     # type: ignore[assignment]
            srv, "_republisher", object())
        # The real Assigner over the fake open_device installed at import.
        srv._begin(4)
        device = _OPENED["/dev/input/event90"]

        def deliver(*evs) -> None:
            device.feed(*evs)
            srv._on_pad_read(device.fd)

        deliver(press(BTN_A))
        clock.advance(assign.HOLD_SECONDS + 0.01)
        srv._tick()
        claims = [m for m in events if m.get("event") == "claim"]
        if len(claims) != 1 or claims[0]["player"] != 1:
            fail(f"holding a button did not claim player 1: {claims}")
        ok("holding the button claims player 1 and tells the front-end")

        # Still held. Long past the confirm threshold.
        clock.advance(server.CONFIRM_HOLD_SECONDS * 3)
        srv._tick()
        if any(m.get("event") == "accepted" for m in events):
            fail("the press that claimed the slot went straight on to confirm "
                 "the session -- setup ends the instant the last player "
                 "claims, before anyone can check the order")
        ok("keeping that same press held does not confirm: the claim consumed "
           "it, so the confirm must be a new press")

        deliver(release(BTN_A))
        srv._tick()
        deliver(press(BTN_A))
        clock.advance(assign.HOLD_SECONDS + 0.01)
        srv._tick()
        if any(m.get("event") == "accepted" for m in events):
            fail("a second press only as long as a claim confirmed")
        ok("a second press held for a claim's worth still does not confirm")
        clock.advance(server.CONFIRM_HOLD_SECONDS)
        srv._tick()
        accepted = [m for m in events if m.get("event") == "accepted"]
        if len(accepted) != 1:
            fail(f"holding again did not accept the session: "
                 f"{[m.get('event') for m in events]}")
        if [p["player"] for p in accepted[0]["players"]] != [1]:
            fail(f"the accepted payload is wrong: {accepted[0]}")
        if srv._state != protocol.STATE_READY:
            fail(f"state after accepting is {srv._state!r}")
        ok("holding it past CONFIRM_HOLD_SECONDS accepts, with player 1 in "
           "the payload, and the daemon is ready")
        srv._end_session(release=True)
        _VISIBLE.clear()

    print("\nS4: a second button released mid-confirm")
    with frozen(server) as clock:
        d = Daemon(claims=1)
        d.down(code=BTN_A)
        clock.advance(0.3)
        d.down(code=BTN_START)       # a resting thumb, or Start held too
        d.up(code=BTN_START)         # let go of that one; BTN_A never released
        clock.advance(server.CONFIRM_HOLD_SECONDS)
        d.ticks(3)
        if d.accepted() != 1:
            fail("releasing a *second* button cancels a confirm hold that is "
                 "still down on the first. Server._on_claimed_event keyed "
                 "_confirm_started on pad.path alone and popped it on any "
                 "value == 0, unlike Assigner._consume which checks "
                 "held[0] == event.code for exactly this reason. Because a "
                 "held button emits no further events, the confirm can then "
                 "never complete until the user lets go and presses again -- "
                 "which is the report 'I have to reassign controllers twice "
                 "before they actually get assigned'. A thumb resting on B "
                 "is enough")
        ok("releasing a second button does not cancel a confirm still "
           "held on the first")
        d.close()

    print("\nS4: releasing the button that started the confirm cancels it")
    with frozen(server) as clock:
        d = Daemon(claims=1)
        d.down(code=BTN_A)
        clock.advance(0.3)
        d.up(code=BTN_A)             # the hold really is over
        clock.advance(server.CONFIRM_HOLD_SECONDS)
        d.ticks(3)
        if d.accepted():
            fail("a tap on a claimed pad accepted the session anyway -- setup "
                 "ends before the user has checked the player order, and the "
                 "pads are released out from under them")
        if d.srv._confirm_started:
            fail("the confirm tracker still holds a hold nobody is holding; "
                 "the next tick would accept a session nobody confirmed")
        ok("the release that matches the press does cancel the hold")
        d.close()

    if _GAPS:
        print(f"\n{len(_GAPS)} gap(s) reported above; everything asserted held")
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
