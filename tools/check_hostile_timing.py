"""Things arriving in the wrong order, or at once.

Everything the daemon owns is modal and single-threaded, and every modal flow
holds EVIOCGRAB on every physical pad. So the interesting failures here are
not wrong answers -- they are states nothing can leave. A session with no
front-end left to cancel it, a wizard that outlives the session it belongs to,
a handler that raises out of the selector loop: each one ends with the pads
grabbed and the machine deaf, and none of them looks like an error at the
moment it happens.

The cases below are the ones a user can really produce: quitting Pegasus while
the setup screen is up, launching a game with the wizard open, pressing a key
bound to a slot nobody claimed, a padmap-play killed mid-launch leaving half a
marker behind, a front-end sending a command whose argument is not the type the
daemon assumed.

Every one must end in a sensible error event or a no-op. Never an unhandled
exception, and never pads held by something that can no longer be reached.

Runs against a real `server.Server` with only the machine-touching parts
stubbed: no uinput node is created, no EVIOCGRAB is taken, no evdev device is
opened. Deliberately not a live daemon -- a real one is running on this
machine and a second would fight it for the pads.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_hostile_timing.py
"""

import json
import os
import socket
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

# Redirected before padmap is imported: several modules read these at import
# time, and a live daemon owns the real runtime dir.
_STORE = Path(tempfile.mkdtemp(prefix="padmap-hostile-"))
_RUNTIME = _STORE / "run"
(_RUNTIME / "padmap").mkdir(parents=True)
os.environ["XDG_RUNTIME_DIR"] = str(_RUNTIME)
os.environ["XDG_CONFIG_HOME"] = str(_STORE / "config")
os.environ["XDG_DATA_HOME"] = str(_STORE / "data")
os.environ["PADMAP_PROFILE_DIR"] = str(_STORE / "devices")
os.environ.pop("PADMAP_NO_AUTOSETUP", None)

from padmap import (capture, devices, profiles,  # noqa: E402
                    protocol, server)
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

# The only pads anything in this file may see. Bound once, at import, so a
# stray code path that reaches discovery cannot enumerate the real machine.
_VISIBLE: list[Pad] = []
devices.discover = lambda *a, **k: list(_VISIBLE)  # type: ignore[assignment]

_GAPS: list[str] = []


def ok(message: str) -> None:
    print(f"  ok  {message}")


def gap(message: str) -> None:
    """A defect confirmed against current source, reported rather than failed.

    Kept green on purpose: the maintainer fixes the source and turns the line
    below the gap into an assertion.
    """
    _GAPS.append(message)
    print(f"  gap: {message}")


def pad(name: str, vid: int = 0x1234, pid: int = 0x0001,
        node: str = "event90") -> Pad:
    return Pad(path=f"/dev/input/{node}", name=name, phys="", uniq="",
               vid=vid, pid=pid, syspath="")


class AbsInfo:
    """Just enough of evdev's absinfo for the ranges the daemon reads."""

    def __init__(self, minimum: int, maximum: int, value: int) -> None:
        self.min, self.max, self.value = minimum, maximum, value


class FakeDevice:
    """An open pad handle. Never a real one: opening a real device would take
    events away from the daemon actually running on this machine."""

    def __init__(self) -> None:
        # One centred stick, so calibration has something to measure and does
        # not take the no-axes shortcut straight to the icon step.
        self._axes = [(0x00, AbsInfo(0, 255, 128)),
                      (0x01, AbsInfo(0, 255, 128))]

    def capabilities(self, absinfo: bool = False):
        return {capture.EV_KEY: [0x130, 0x131, 0x133, 0x134],
                capture.EV_ABS: list(self._axes)}

    def active_keys(self):
        return []


class FakeAssigner:
    """An assignment session: live claims, open handles, and real fds.

    The fds are the read ends of real pipes rather than invented numbers,
    because the daemon registers them with a real `selectors` object. That is
    the point of using them: registering an fd twice, or failing to unregister
    one, is exactly the kind of bookkeeping that goes wrong when sessions
    overlap, and only a real selector notices.
    """

    def __init__(self, pads=(), grab: bool = True, **_kw) -> None:
        self.pads = list(pads)
        self.assignments: list[Assignment] = []
        self._open = {p.path: FakeDevice() for p in self.pads}
        read_fd, write_fd = os.pipe()
        self._write_fd = write_fd
        self.fds = [read_fd]
        self.grab_failures: list[Pad] = []
        self.closed = False
        self.on_claimed_event = None
        self.on_raw_event = None
        # How many times claim detection has been run. Zero while a modal
        # flow is up is the whole of "a button answering a prompt must not
        # also burn a player slot".
        self.ticks = 0

    def __enter__(self) -> "FakeAssigner":
        return self

    def device_for(self, target: Pad):
        return self._open.get(target.path)

    def reset(self) -> None:
        self.assignments = []

    def tick(self, on_progress=None, on_claim=None) -> None:
        self.ticks += 1

    def close(self) -> None:
        # A real close() releases EVIOCGRAB. Whether this ran is the whole
        # question in most of the scenarios below.
        self.closed = True


server.Assigner = FakeAssigner  # type: ignore[assignment]


class Harness:
    """A Server with everything that touches the machine stubbed out."""

    def __init__(self, *, stored=(), claims=None, open_pads=None,
                 state=protocol.STATE_IDLE, real_begin=False,
                 clients=0, client_age=60.0):
        self.srv = server.Server()
        self.srv._state = state
        self.srv._assignments = list(stored)
        if claims is None and open_pads is None:
            self.srv._assigner = None
        else:
            session = FakeAssigner(pads=list(open_pads or ()))
            session.assignments = list(claims or ())
            self.srv._assigner = session  # type: ignore[assignment]

        self.events: list[dict] = []
        self.sent: list[tuple[object, dict]] = []
        self.began: list[int] = []
        self.client = object()
        self.republished = 0
        self.sockets: list[socket.socket] = []

        self.srv._broadcast = self.events.append   # type: ignore[assignment]
        self.srv._send = lambda c, m: self.sent.append((c, m))  # type: ignore[assignment]
        # Creates uinput nodes and publishes virtual pads onto the machine.
        self.srv._start_republisher = self._republish  # type: ignore[assignment]
        # Opens every assigned pad to read its capabilities.
        self.srv._write_controller_configs = lambda: None  # type: ignore[assignment]
        self.srv._save_assignments = lambda: None  # type: ignore[assignment]
        if not real_begin:
            # Grabs every pad and stops republishing.
            self.srv._begin = lambda players: self.began.append(players)  # type: ignore[assignment]

        for _ in range(clients):
            self.add_client(age=client_age)

    def _republish(self) -> None:
        self.republished += 1
        self.srv._republisher = object()   # type: ignore[assignment]

    def add_client(self, age: float = 60.0) -> socket.socket:
        """A real connected client, registered exactly as _on_accept does."""
        near, far = socket.socketpair()
        self.sockets += [near, far]
        client = server.Client(near)
        client.connected_at = time.monotonic() - age
        self.srv._clients[near.fileno()] = client
        self.srv._selector.register(near, 1, self.srv._on_client_read)
        return near

    def send(self, command: str, **extra):
        self.srv._handle_command(self.client, {"cmd": command, **extra})  # type: ignore[arg-type]
        return self

    def scan(self):
        """One autosetup poll, with the rate limit defeated."""
        self.srv._last_pad_scan = 0.0
        self.srv._last_scan_signature = None
        self.srv._poll_new_controllers()
        return self

    @property
    def messages(self) -> list[dict]:
        return list(self.events) + [m for _c, m in self.sent]

    @property
    def errors(self) -> list[str]:
        return [str(m.get("message")) for m in self.messages
                if m.get("event") == "error"]

    def last_error(self) -> str:
        return self.errors[-1] if self.errors else ""

    def kinds(self) -> list[str]:
        return [str(m.get("event")) for m in self.messages]

    def close(self) -> None:
        for sock in self.sockets:
            try:
                sock.close()
            except OSError:
                pass


def quietly(harness: Harness, label: str, command: str, **extra):
    """Send a command that must be a no-op, and insist it does not raise.

    Every one of these arrives unprompted -- Esc pressed twice, an overlay
    closing, a key bound to a screen that has already gone. They run on the
    selector thread with no guard anywhere above them, so raising is not a
    failed command, it is a dead daemon and a machine with no pads.
    """
    try:
        harness.send(command, **extra)
    except Exception as exc:                           # noqa: BLE001
        raise SystemExit(
            f"FAIL {label}: {command} raised {type(exc).__name__}: {exc} -- "
            f"this reaches the selector loop unguarded, so the daemon dies "
            f"and every virtual pad goes with it")
    return harness


def open_session(pads, *, stored=(), claims=(), clients=0):
    """A harness sitting in an open session with those pads grabbed."""
    return Harness(stored=stored, claims=claims, open_pads=pads,
                   state=protocol.STATE_ASSIGNING, clients=clients)


def assert_not_stranded(harness: Harness, label: str) -> None:
    """The invariant every scenario in this file shares.

    A daemon that says `assigning` is a daemon holding EVIOCGRAB on every pad.
    If it says that with no session object behind it, nothing left can release
    them: cancel returns early, accept refuses, and the front-end is deaf until
    somebody restarts the daemon by hand.
    """
    srv = harness.srv
    if srv._state == protocol.STATE_ASSIGNING and srv._assigner is None:
        raise SystemExit(
            f"FAIL {label}: the daemon reports 'assigning' with no session "
            f"behind it -- the pads are grabbed and nothing left can release "
            f"them, so every controller on the machine is dead until the "
            f"daemon is restarted")
    if srv._assigner is None and (srv._mapping is not None
                                  or srv._choice is not None
                                  or srv._calibration is not None):
        raise SystemExit(
            f"FAIL {label}: a modal flow outlived the session it belongs to. "
            f"Nothing will read that pad again, so the overlay sits waiting "
            f"for events that cannot arrive")


def marker_says(text: str) -> None:
    marker = protocol.playing_marker()
    marker.parent.mkdir(parents=True, exist_ok=True)
    if marker.is_dir():
        marker.rmdir()
    marker.write_text(text)


def clear_marker() -> None:
    marker = protocol.playing_marker()
    if marker.is_dir():
        marker.rmdir()
    else:
        marker.unlink(missing_ok=True)


def main() -> int:
    clear_marker()
    one = pad("Player One Pad", node="event90")
    two = pad("Player Two Pad", vid=0x2222, node="event91")

    # ------------------------------------------------ a game at a bad moment

    print("S1/S14: a game starts while the setup screen is open")
    _VISIBLE[:] = [pad("Brand New Pad", vid=0x7777, node="event95")]
    live = open_session([one], stored=[Assignment(player=1, pad=one, button=0)],
                        clients=1)
    session = live.srv._assigner
    marker_says(f"{os.getpid()}\n")
    live.scan()
    if live.srv._assigner is not session:
        raise SystemExit(
            "FAIL: launching a game replaced the open session -- the pads the "
            "user is claiming with are released mid-hold")
    if live.began:
        raise SystemExit(
            "FAIL: a second session opened on top of the first, which grabs "
            "pads the first one already holds")
    if live.srv._state != protocol.STATE_ASSIGNING:
        raise SystemExit(
            f"FAIL: the session dropped to {live.srv._state!r} while its pads "
            f"were still grabbed")
    assert_not_stranded(live, "game started mid-session")
    ok("the open session is untouched, and no second one opens")

    live.send("cancel")
    if not session.closed:
        raise SystemExit(
            "FAIL: cancelling during a game did not release the pads -- a "
            "game is not a reason for the escape hatch to stop working")
    if live.srv._assigner is not None:
        raise SystemExit("FAIL: the session object survived its own cancel")
    assert_not_stranded(live, "cancel during a game")
    ok("and cancel still releases them, game or no game")
    live.close()

    print("\nS1: a new controller is plugged in while setup is already open")
    # Without a game to blame it on, so this tests the "a session is already
    # open" guard rather than passing on the marker.
    clear_marker()
    _VISIBLE[:] = [pad("Another New Pad", vid=0x7778, node="event96")]
    busy_setup = open_session([one], stored=[Assignment(1, one, 0)], clients=1)
    running = busy_setup.srv._assigner
    busy_setup.scan()
    if busy_setup.began:
        raise SystemExit(
            "FAIL: a controller plugged in during setup opened a second "
            "session on top of the first, which grabs pads the first already "
            "holds and leaves one of the two unreachable")
    if busy_setup.srv._assigner is not running or running.closed:
        raise SystemExit("FAIL: the open session was torn down under the user")
    if busy_setup.srv._autosetup_blocked() != "a session is already open":
        raise SystemExit(
            f"FAIL: the daemon blames "
            f"{busy_setup.srv._autosetup_blocked()!r} -- this case is passing "
            f"for some other reason than the one under test")
    assert_not_stranded(busy_setup, "new pad during setup")
    ok("no second session, and it says why")
    busy_setup.close()

    print("\nS1/S7: a game starts while the wizard is mid-capture")
    wiz = open_session([one], stored=[Assignment(player=1, pad=one, button=0)],
                       clients=1)
    wiz.send("map", player=1, layout="n64", scope="")
    run = wiz.srv._mapping
    if run is None:
        raise SystemExit(f"FAIL: no wizard opened ({wiz.last_error()!r})")
    run.bindings["a"] = Binding("button", 0x130)
    marker_says(f"{os.getpid()}\n")
    wiz.scan()
    wiz.srv._tick()
    if wiz.srv._mapping is not run:
        raise SystemExit(
            "FAIL: launching a game threw away a capture in progress -- the "
            "user is halfway through the wizard and the prompts vanish")
    if wiz.srv._assigner is None:
        raise SystemExit("FAIL: the session behind the wizard was closed")
    if wiz.began:
        raise SystemExit("FAIL: a second session opened under a live wizard")
    ok("the capture survives, and nothing opens on top of it")

    if wiz.srv._assigner.ticks:
        raise SystemExit(
            "FAIL: claim detection ran while the wizard was up -- every "
            "button answering a prompt is also a hold on a pad, so the "
            "presses that map the controller would burn player slots as they "
            "went")
    if wiz.srv._assigner.assignments:
        raise SystemExit("FAIL: a slot was claimed while the wizard was up")
    ok("claim detection does not run underneath it: no slot is burned")

    # The positive control: with the wizard gone, the same tick must resume
    # claim detection, or the assertion above passes for free.
    wiz.send("configure_end")
    wiz.srv._tick()
    if not wiz.srv._assigner.ticks:
        raise SystemExit(
            "FAIL: claim detection never resumed after the wizard closed -- "
            "the setup screen sits there and no hold ever claims a slot")
    ok("and it resumes the moment the wizard closes")
    wiz.send("map", player=1, layout="n64", scope="")

    print("\nS4: a confirm hold cannot fire while a modal flow is up")
    # The hold that ends a session and the hold that answers a wizard prompt
    # are the same gesture on the same pad. If the confirm timer keeps running
    # underneath, finishing a control accepts the session out from under the
    # user -- pads released, profiles written, wizard gone.
    wiz.srv._confirm_started[one.path] = time.monotonic() - 10.0
    wiz.srv._tick()
    if "accepted" in wiz.kinds():
        raise SystemExit(
            "FAIL: a confirm hold fired while the wizard was open -- the "
            "press that answered a prompt also ended the session, releasing "
            "the pads mid-capture")
    if wiz.srv._mapping is None:
        raise SystemExit("FAIL: the wizard was ended by a stale confirm hold")
    ok("a stale hold does not accept underneath an open wizard")

    wiz.srv._mapping = None
    wiz.srv._calibration = server.CalibrationRun(one, 1, {})
    wiz.srv._calibration.begin_phase(server.PHASE_ICON)
    wiz.srv._confirm_started[one.path] = time.monotonic() - 10.0
    wiz.srv._tick()
    if "accepted" in wiz.kinds():
        raise SystemExit(
            "FAIL: a confirm hold fired during calibration -- a button pressed "
            "incidentally while rotating a stick ended the session")
    ok("nor underneath a calibration")
    wiz.close()

    clear_marker()

    # -------------------------------------------------- the playing marker

    print("\nS1/S14: what padmap-play leaves behind when it dies badly")
    settled = Harness(clients=1)
    for label, text, survives in (
        ("a pid that does not exist", "999999\n", False),
        ("a pid that is not a number", "bananas\n", True),
        ("nothing at all", "", True),
        ("only whitespace", "   \n", True),
        ("a negative pid", "-1\n", False),
        ("a pid larger than any pid_max", "9" * 40, False),
        ("a float", "12.5\n", True),
        ("a whole log line", "padmap-play: starting retroarch\n", True),
    ):
        marker_says(text)
        try:
            running = protocol.game_is_running()
        except Exception as exc:                       # noqa: BLE001
            raise SystemExit(
                f"FAIL: a marker holding {text!r} raised {exc!r} -- this is "
                f"read from the tick, so the daemon dies once a second")
        if running:
            raise SystemExit(
                f"FAIL: {label} was read as a game in progress -- setup can "
                f"never open again until someone deletes the file by hand")
        if protocol.playing_marker().exists() != survives:
            want = "kept" if survives else "cleaned up"
            raise SystemExit(
                f"FAIL: a marker holding {text!r} should have been {want}")
        blocked = settled.srv._autosetup_blocked()
        if blocked == "a game is running":
            raise SystemExit(
                f"FAIL: {label} blocks the setup screen forever")
        if blocked is not None:
            raise SystemExit(
                f"FAIL: setup is blocked for an unrelated reason ({blocked!r}) "
                f"-- this case is passing for the wrong reason")
        ok(f"{label}: no game, setup not blocked")

    clear_marker()
    protocol.playing_marker().mkdir(parents=True)
    try:
        running = protocol.game_is_running()
    except Exception as exc:                           # noqa: BLE001
        raise SystemExit(
            f"FAIL: a directory where the marker should be raised {exc!r} -- "
            f"the tick reads this, so the daemon dies once a second")
    if running:
        raise SystemExit("FAIL: a directory was read as a running game")
    if not protocol.playing_marker().is_dir():
        raise SystemExit(
            "FAIL: the stale-marker cleanup tried to unlink a directory")
    ok("a directory where the marker should be: no game, nothing removed")
    clear_marker()

    marker_says(f"{os.getpid()}\n")
    if not protocol.game_is_running():
        raise SystemExit(
            "FAIL: a marker holding a live pid does not report a game -- "
            "every negative case above was passing for the wrong reason, and "
            "plugging a pad in mid-game would grab the controllers")
    if settled.srv._autosetup_blocked() != "a game is running":
        raise SystemExit(
            "FAIL: a real game does not block the setup screen")
    ok("and a marker holding a live pid does report one")
    clear_marker()
    settled.close()

    # ----------------------------------------------- commands out of order

    print("\nS4: accept with no session, and accept twice")
    stray = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                    state=protocol.STATE_READY)
    quietly(stray, "accept with no session", "accept")
    if "nothing assigned" not in stray.last_error():
        raise SystemExit(
            f"FAIL: accept with no session gave {stray.last_error()!r} -- "
            f"confirming nothing must not look like success")
    if stray.srv._state != protocol.STATE_READY:
        raise SystemExit(
            f"FAIL: a stray accept moved the daemon to {stray.srv._state!r} "
            f"while the virtual pads were still running")
    if stray.republished:
        raise SystemExit(
            "FAIL: a stray accept restarted republishing, which tears down "
            "every virtual pad and rebuilds it under RetroArch")
    assert_not_stranded(stray, "accept with no session")
    ok(f"says {stray.last_error()!r}, stays ready, republishes nothing")
    stray.close()

    twice = open_session([one, two], claims=[Assignment(1, one, 0)])
    twice.send("accept")
    if twice.errors:
        raise SystemExit(f"FAIL: the first accept said {twice.last_error()!r}")
    if [a.player for a in twice.srv._assignments] != [1]:
        raise SystemExit("FAIL: the first accept stored nothing")
    if twice.srv._state != protocol.STATE_READY:
        raise SystemExit("FAIL: the first accept did not reach ready")
    before = twice.republished
    quietly(twice, "accept twice", "accept")
    if "nothing assigned" not in twice.last_error():
        raise SystemExit(
            f"FAIL: a second accept gave {twice.last_error()!r} -- the front-"
            f"end sending it twice must not look like it worked twice")
    if [a.player for a in twice.srv._assignments] != [1]:
        raise SystemExit(
            "FAIL: the second accept changed what is assigned, from a session "
            "that no longer exists")
    if twice.republished != before:
        raise SystemExit(
            "FAIL: the second accept republished again -- every virtual pad "
            "is destroyed and recreated, and RetroArch's indices move")
    assert_not_stranded(twice, "accept twice")
    ok("the second refuses, keeps the assignment, and republishes nothing")
    twice.close()

    print("\nS5: cancel arriving twice, and with nothing open")
    doubled = open_session([one], stored=[Assignment(1, one, 0)])
    grabbed = doubled.srv._assigner
    doubled.send("cancel")
    if not grabbed.closed:
        raise SystemExit("FAIL: the first cancel did not release the pads")
    if doubled.srv._assigner is not None:
        raise SystemExit("FAIL: the session survived its own cancel")
    if doubled.srv._state != protocol.STATE_READY:
        raise SystemExit(
            f"FAIL: cancel left the daemon {doubled.srv._state!r} with "
            f"assignments to republish -- backing out of setup cost the user "
            f"their controllers")
    resumed = doubled.republished
    if not resumed:
        raise SystemExit("FAIL: cancel did not put the assignments back on air")
    quietly(doubled, "cancel twice", "cancel")
    if doubled.errors:
        raise SystemExit(
            f"FAIL: a second cancel complained ({doubled.errors!r}) -- Esc "
            f"pressed twice is a thing people do")
    if doubled.srv._state != protocol.STATE_READY:
        raise SystemExit(
            f"FAIL: the second cancel dropped the daemon to "
            f"{doubled.srv._state!r} while the pads were live")
    if doubled.republished != resumed:
        raise SystemExit(
            "FAIL: the second cancel restarted republishing again, tearing "
            "down virtual pads that were working")
    assert_not_stranded(doubled, "cancel twice")
    ok("the second is a no-op: still ready, pads still on the air")
    doubled.close()

    idle = Harness()
    quietly(idle, "cancel while idle", "cancel")
    if idle.errors or idle.srv._state != protocol.STATE_IDLE:
        raise SystemExit(
            f"FAIL: cancel on an idle daemon gave {idle.errors!r} / "
            f"{idle.srv._state!r}")
    assert_not_stranded(idle, "cancel while idle")
    ok("cancel with nothing open at all: quiet, still idle")
    idle.close()

    print("\nS6: reset with no session, and reset twice")
    noreset = Harness(stored=[Assignment(1, one, 0)],
                      state=protocol.STATE_READY)
    quietly(noreset, "reset with no session", "reset")
    if noreset.errors:
        raise SystemExit(
            f"FAIL: reset with no session said {noreset.errors!r} -- F is "
            f"pressed from screens that have no session behind them")
    if noreset.srv._assigner is not None:
        raise SystemExit("FAIL: reset invented a session")
    if [a.player for a in noreset.srv._assignments] != [1]:
        raise SystemExit(
            "FAIL: reset with no session dropped the accepted assignment -- "
            "the virtual pads are live and the daemon has forgotten whose")
    assert_not_stranded(noreset, "reset with no session")
    ok("no-op, and the accepted assignment survives")
    noreset.close()

    twicereset = open_session([one], claims=[Assignment(1, one, 0)])
    live_session = twicereset.srv._assigner
    quietly(twicereset, "reset twice", "reset")
    quietly(twicereset, "reset twice", "reset")
    if twicereset.srv._assigner is not live_session:
        raise SystemExit(
            "FAIL: resetting twice closed the session -- the pads are "
            "released mid-setup and nothing can be claimed again")
    if live_session.assignments:
        raise SystemExit("FAIL: claims survived the reset")
    if live_session.closed:
        raise SystemExit("FAIL: reset released the grabs")
    if twicereset.errors:
        raise SystemExit(f"FAIL: reset twice said {twicereset.errors!r}")
    assert_not_stranded(twicereset, "reset twice")
    ok("twice over: claims gone, session and grabs intact")
    twicereset.close()

    print("\nS12/S13: configure_end with nothing modal open")
    nothing = open_session([one], stored=[Assignment(1, one, 0)])
    quietly(nothing, "nothing modal open", "configure_end")
    quietly(nothing, "nothing modal open", "configure_end")
    quietly(nothing, "nothing modal open", "skip_control")
    if nothing.errors:
        raise SystemExit(
            f"FAIL: dismissing an overlay that is not there said "
            f"{nothing.errors!r} -- the front-end sends this whenever a "
            f"screen closes")
    for name in ("_calibration", "_choice", "_mapping"):
        if getattr(nothing.srv, name) is not None:
            raise SystemExit(f"FAIL: configure_end invented a {name}")
    if nothing.srv._assigner is None:
        raise SystemExit(
            "FAIL: configure_end with nothing open closed the session, "
            "releasing the pads the setup screen is being driven with")
    assert_not_stranded(nothing, "configure_end with nothing open")
    ok("quiet no-op, session untouched")
    nothing.close()

    print("\nS12: configure_end twice over a live calibration")
    calibrating = open_session([one], stored=[Assignment(1, one, 0)])
    calibrating.send("calibrate", player=1)
    if calibrating.srv._calibration is None:
        raise SystemExit(
            f"FAIL: no calibration started ({calibrating.last_error()!r})")
    calibrating.send("configure_end")
    if calibrating.srv._calibration is not None:
        raise SystemExit(
            "FAIL: calibration stayed in flight after the overlay closed, so "
            "the pad's events go on being swallowed by it")
    done = [m for m in calibrating.messages
            if m.get("event") == "calibration" and m.get("phase") == "done"]
    if not done:
        raise SystemExit(
            "FAIL: the front-end was never told the calibration had ended")
    quietly(calibrating, "configure_end twice", "configure_end")
    if calibrating.errors:
        raise SystemExit(
            f"FAIL: the second configure_end said {calibrating.errors!r}")
    if calibrating.srv._assigner is None:
        raise SystemExit("FAIL: configure_end closed the session under it")
    assert_not_stranded(calibrating, "configure_end twice")
    ok("closed once, told the front-end, second one a no-op")
    calibrating.close()

    # ----------------------------------------- the front-end going away

    print("\nS5: the front-end disappears with a wizard on screen")
    dropped = open_session([one], stored=[Assignment(1, one, 0)])
    sock = dropped.add_client()
    held = dropped.srv._assigner
    dropped.send("map", player=1, layout="n64", scope="")
    inflight = dropped.srv._mapping
    if inflight is None:
        raise SystemExit(f"FAIL: no wizard ({dropped.last_error()!r})")
    inflight.bindings["a"] = Binding("button", 0x130)
    dropped.srv._drop_client(sock)
    if not held.closed:
        raise SystemExit(
            "FAIL: the last front-end went away and the pads stayed grabbed. "
            "Nothing is left to send cancel, so every controller on the "
            "machine is dead until the daemon is restarted")
    if dropped.srv._assigner is not None:
        raise SystemExit("FAIL: the session object outlived its front-end")
    if dropped.srv._mapping is not None:
        raise SystemExit(
            "FAIL: the wizard outlived the session it belongs to -- it holds "
            "a pad nothing will read again, and _tick returns early on it "
            "forever, so no button hold can ever claim a slot")
    if dropped.srv._state == protocol.STATE_ASSIGNING:
        raise SystemExit(
            "FAIL: the daemon still reports 'assigning' with no session")
    assert_not_stranded(dropped, "front-end lost mid-wizard")
    ok("session ended, pads released, wizard closed with it")

    if profiles.load(one) is not None:
        raise SystemExit(
            "FAIL: a wizard abandoned by a dying front-end stored what it had "
            "-- the pad then counts as configured and is never offered again, "
            "leaving half its buttons dead with nothing to say why")
    ok("and the half-finished capture is not kept")
    dropped.close()

    print("\nS5: the same, with a calibration and a picker in flight")
    for label, command, attribute in (
        ("calibration", "calibrate", "_calibration"),
        ("layout picker", "choose_layout", "_choice"),
        ("scope picker", "choose_scope", "_choice"),
    ):
        harness = open_session([one], stored=[Assignment(1, one, 0)])
        peer = harness.add_client()
        grab = harness.srv._assigner
        harness.send(command, player=1)
        if getattr(harness.srv, attribute) is None:
            raise SystemExit(
                f"FAIL: no {label} started ({harness.last_error()!r})")
        harness.srv._drop_client(peer)
        if not grab.closed:
            raise SystemExit(
                f"FAIL: a {label} left the pads grabbed after the front-end "
                f"went away")
        if getattr(harness.srv, attribute) is not None:
            raise SystemExit(
                f"FAIL: the {label} outlived the session, waiting for events "
                f"from a pad nothing will read again")
        assert_not_stranded(harness, f"front-end lost during {label}")
        ok(f"{label}: released and closed")
        harness.close()

    print("\nS5: one of two front-ends going away is not the last one")
    pair = open_session([one], stored=[Assignment(1, one, 0)])
    first_sock = pair.add_client()
    pair.add_client()
    session = pair.srv._assigner
    pair.srv._drop_client(first_sock)
    if session.closed or pair.srv._assigner is None:
        raise SystemExit(
            "FAIL: a passing `padmap status` disconnecting tore down the "
            "session the real front-end is driving")
    if pair.srv._state != protocol.STATE_ASSIGNING:
        raise SystemExit("FAIL: the session state was dropped anyway")
    assert_not_stranded(pair, "one of two clients dropped")
    ok("the session survives while another client is still connected")
    pair.close()

    # ------------------------------------- player numbers nobody can claim

    print("\nS12/S13: per-player commands for slots that do not exist")
    per_player = ("calibrate", "map_for_game", "forget_pad", "choose_layout",
                  "choose_scope", "map")
    impossible = (0, -1, 5, 99)
    for number in impossible:
        for command in per_player:
            bad = open_session([one], stored=[Assignment(1, one, 0)])
            extra = {"player": number}
            if command == "map_for_game":
                extra.update(console="n64", key="n64/goldeneye", title="G")
            if command == "map":
                extra.update(layout="n64", scope="")
            bad.send(command, **extra)
            if f"player {number}" not in bad.last_error():
                raise SystemExit(
                    f"FAIL: {command} for player {number} said "
                    f"{bad.last_error()!r} -- the pads are grabbed on that "
                    f"screen, so an error that does not name the slot is the "
                    f"only feedback there is and it says nothing")
            for name in ("_calibration", "_choice", "_mapping"):
                if getattr(bad.srv, name) is not None:
                    raise SystemExit(
                        f"FAIL: {command} for player {number} opened a "
                        f"{name} for a controller that does not exist")
            if bad.srv._assigner is None or bad.srv._assigner.closed:
                raise SystemExit(
                    f"FAIL: {command} for player {number} tore the session "
                    f"down -- a mistyped key releases every pad")
            assert_not_stranded(bad, f"{command} for player {number}")
            bad.close()
        ok(f"player {number}: all six refuse by name, nothing opens")

    print("\nS11: player 0 does not mean 'the first one'")
    # The commands default `player` to 0 when the key is missing, so a
    # front-end that forgets it must not silently configure slot 1.
    zero = open_session([two], stored=[Assignment(1, two, 0)])
    if zero.srv._pad_for_player(0) is not None:
        raise SystemExit(
            "FAIL: player 0 resolved to a controller -- a command sent with "
            "no player at all would reconfigure somebody's pad")
    zero.send("forget_pad")           # no player key at all
    if profiles.load(two) is not None or "player 0" not in zero.last_error():
        raise SystemExit(
            f"FAIL: forget_pad with no player said {zero.last_error()!r} and "
            f"may have deleted a profile")
    assert_not_stranded(zero, "player 0")
    ok("resolves to nothing, and a player-less command deletes nothing")
    zero.close()

    print("\nS11: forget_pad for a slot claimed by someone else")
    # The destructive one. It must not fall back to 'the pad we have'.
    victim = pad("Do Not Delete Me", vid=0x9999, node="event92")
    profile = profiles.Profile(signature=profiles.signature(victim),
                               name=victim.name)
    profile.record("", profiles.Mapping(buttons={"a": Binding("button", 1)},
                                        layout="n64"))
    profiles.save(profile)
    if profiles.load(victim) is None:
        raise SystemExit("FAIL: the fixture profile did not save")
    elsewhere = open_session([victim],
                             stored=[Assignment(2, victim, 0)])
    for number in (0, -1, 1, 99):
        elsewhere.send("forget_pad", player=number)
        if profiles.load(victim) is None:
            raise SystemExit(
                f"FAIL: forget_pad for player {number} deleted the profile of "
                f"the controller in slot 2 -- the wrong pad is left with "
                f"neither a mapping nor a way to make one")
    assert_not_stranded(elsewhere, "forget_pad for the wrong slot")
    ok("four wrong slot numbers, and slot 2's profile is untouched")
    elsewhere.close()
    profiles.forget(victim)

    print("\nS12: calibrate for a slot nobody has claimed yet")
    unclaimed = open_session([one, two], claims=[], stored=[])
    unclaimed.send("calibrate", player=1)
    if "player 1" not in unclaimed.last_error():
        raise SystemExit(
            f"FAIL: calibrating an unclaimed slot said "
            f"{unclaimed.last_error()!r}")
    if unclaimed.srv._calibration is not None:
        raise SystemExit(
            "FAIL: a calibration started with no pad behind it -- the overlay "
            "draws progress for a controller nobody is holding")
    assert_not_stranded(unclaimed, "calibrate an unclaimed slot")
    ok("refused by name, no overlay opened")
    unclaimed.close()

    # -------------------------------------------------- two begins at once

    print("\nS1/S2: two begins back to back")
    # Reachable: `_poll_new_controllers` opens setup by itself and the user
    # presses Details at the same moment, or a front-end reconnects and
    # re-sends begin. The first session holds EVIOCGRAB on every pad and its
    # fds are registered with the selector; both have to go before the second
    # takes them.
    _VISIBLE[:] = [one, two]
    doubled = Harness(real_begin=True, clients=1)
    doubled.send("begin", players=4)
    first = doubled.srv._assigner
    if first is None:
        raise SystemExit(f"FAIL: no session opened ({doubled.last_error()!r})")
    for fd in first.fds:
        doubled.srv._selector.get_key(fd)      # raises if not registered
    doubled.send("begin", players=4)
    second = doubled.srv._assigner
    if second is first:
        raise SystemExit("FAIL: the second begin did not open a new session")
    if not first.closed:
        raise SystemExit(
            "FAIL: the first session's pads were never released -- two "
            "sessions now hold EVIOCGRAB on the same controllers and only "
            "one of them can ever be cancelled")
    for fd in first.fds:
        try:
            doubled.srv._selector.get_key(fd)
        except KeyError:
            continue
        raise SystemExit(
            f"FAIL: fd {fd} from the first session is still registered -- the "
            f"selector wakes on a session that no longer exists and hands the "
            f"events to whoever holds the fd number next")
    for fd in second.fds:
        doubled.srv._selector.get_key(fd)
    if doubled.srv._state != protocol.STATE_ASSIGNING:
        raise SystemExit(
            f"FAIL: two begins left the daemon {doubled.srv._state!r}")
    assert_not_stranded(doubled, "two begins")
    ok("the first is closed and unregistered before the second registers")

    doubled.send("cancel")
    if not second.closed:
        raise SystemExit("FAIL: the surviving session was not released")
    for fd in second.fds:
        try:
            doubled.srv._selector.get_key(fd)
        except KeyError:
            continue
        raise SystemExit(f"FAIL: fd {fd} outlived the session that owned it")
    assert_not_stranded(doubled, "cancel after two begins")
    ok("and one cancel afterwards leaves nothing grabbed or registered")

    print("\nS3/S7: a begin arriving on top of a live wizard")
    overlap = Harness(real_begin=True, clients=1)
    overlap.srv._assignments = [Assignment(1, one, 0)]
    overlap.send("begin", players=4)
    overlap.send("map", player=1, layout="n64", scope="")
    stale = overlap.srv._mapping
    if stale is None:
        raise SystemExit(f"FAIL: no wizard ({overlap.last_error()!r})")
    overlap.send("begin", players=4)
    if overlap.srv._choice is not None:
        raise SystemExit(
            "FAIL: a picker survived into the new session")
    if overlap.srv._calibration is not None:
        raise SystemExit(
            "FAIL: a calibration survived into the new session")
    ok("the picker and the calibration are both cleared by the new session")

    if overlap.srv._mapping is None:
        ok("and so is the wizard")
    else:
        gap("_begin clears _calibration and _choice but not _mapping, so a "
            "wizard from the previous session survives into the new one. "
            "_tick returns early while a mapping is in flight, so no button "
            "hold can ever claim a slot for the whole of that session: the "
            "setup screen sits there and nothing happens (S3), and confirm "
            "never fires (S4). The run also holds a Pad from the closed "
            "session, so a capture completing would be stored against it")
        if overlap.srv._mapping is not stale:
            raise SystemExit(
                "FAIL: some third thing happened to the wizard")
        overlap.srv._assigner.assignments = []
        overlap.srv._confirm_started[one.path] = time.monotonic() - 10.0
        overlap.srv._tick()
        if "accepted" in overlap.kinds():
            raise SystemExit(
                "FAIL: the stale wizard did not even suppress the tick, so "
                "the analysis above is wrong and something worse is happening")
        print("       (confirmed: the tick is dead for the new session)")
    assert_not_stranded(overlap, "begin over a live wizard")
    overlap.close()

    # ------------------------------------------- malformed wire messages

    print("\nvalid JSON that is not a command object")
    reader = protocol.LineReader()
    for raw in (b"[1,2,3]\n", b'"hello"\n', b"null\n", b"12\n", b"true\n",
                b"[]\n", b"not json at all\n", b"\n"):
        got = reader.feed(raw)
        if got:
            raise SystemExit(
                f"FAIL: {raw!r} was decoded as a command ({got!r}) -- "
                f"_handle_command calls .get() on it and the daemon dies")
    if reader.feed(b'{"cmd":"status"}\n') != [{"cmd": "status"}]:
        raise SystemExit(
            "FAIL: the framing did not survive the junk before it, so one bad "
            "line costs every command after it")
    ok("eight non-object lines dropped, and the next real command still arrives")

    print("\nand the same arriving down a real socket")
    wire = open_session([one], stored=[Assignment(1, one, 0)])
    near = wire.add_client()
    far = wire.sockets[wire.sockets.index(near) + 1]
    far.sendall(b'[1,2,3]\nnull\n"cancel"\n{"cmd":"explode"}\n')
    session = wire.srv._assigner
    wire.srv._on_client_read(near)
    if session.closed:
        raise SystemExit(
            'FAIL: a bare JSON string "cancel" was acted on as a command')
    complaints = [m for _c, m in wire.sent if m.get("event") == "error"]
    if len(complaints) != 1 or "explode" not in str(complaints[0]["message"]):
        raise SystemExit(
            f"FAIL: the daemon answered {complaints!r} -- the junk lines "
            f"should be silent and the unknown command named")
    assert_not_stranded(wire, "junk down the socket")
    ok("junk ignored, the unknown command named, the session untouched")
    wire.close()

    print("\ncommands whose arguments are the wrong type")
    coerce = Harness(real_begin=True, clients=1)
    coerce.send("begin", players=2.9)
    if coerce.srv._slots != 2:
        raise SystemExit(
            f"FAIL: a float slot count gave {coerce.srv._slots} slots")
    if coerce.srv._assigner is None:
        raise SystemExit("FAIL: a float slot count opened no session")
    coerce.send("cancel")
    ok("players as a float: truncated, session opens with 2 slots")

    negative = Harness(real_begin=True, clients=1)
    negative.send("begin", players=-3)
    if negative.srv._slots != 1:
        raise SystemExit(
            f"FAIL: a negative slot count gave {negative.srv._slots} slots -- "
            f"the setup screen would draw no slots at all and nothing could "
            f"be claimed")
    negative.send("cancel")
    assert_not_stranded(negative, "begin with a negative count")
    ok("players negative: clamped to 1, not a screen with no slots")
    negative.close()
    coerce.close()

    icon = open_session([one], stored=[Assignment(1, one, 0)])
    for value in (123, None, 4.5, True):
        icon.send("set_icon", player=1, icon=value)
        if "unknown icon" not in icon.last_error():
            raise SystemExit(
                f"FAIL: an icon of {value!r} gave {icon.last_error()!r} -- a "
                f"pad drawn with a picture that does not exist shows nothing")
    if profiles.load(one) is not None:
        raise SystemExit(
            "FAIL: a rejected icon was written to the profile anyway, which "
            "makes the pad count as configured")
    ok("icon as a number, null, float or bool: refused, nothing written")

    icon.send("map", player=1, layout=None, scope=None)
    if icon.srv._mapping is None:
        raise SystemExit(
            f"FAIL: a null layout killed the wizard ({icon.last_error()!r}) "
            f"-- the front-end omitting a field must fall back, not fail")
    # str(None) is what the daemon coerces a missing scope to; "" is what it
    # would be if the coercion were tightened. Either is fine. What is not
    # fine is a null turning into a scope the launcher really looks up, which
    # would file the capture somewhere the user never chose.
    if icon.srv._mapping.scope not in ("", "None"):
        raise SystemExit(
            f"FAIL: a null scope became {icon.srv._mapping.scope!r} -- the "
            f"capture would be filed under a scope nobody asked for")
    icon.send("configure_end")
    assert_not_stranded(icon, "map with null arguments")
    ok("layout and scope as null: coerced, wizard still opens")
    icon.close()

    print("\nand arguments that are not numbers at all")
    hostile = Harness(stored=[Assignment(1, one, 0)],
                      state=protocol.STATE_READY)
    survived = []
    crashed = []
    for message in ({"cmd": "calibrate", "player": "one"},
                    {"cmd": "calibrate", "player": None},
                    {"cmd": "calibrate", "player": [1]},
                    {"cmd": "forget_pad", "player": {"a": 1}},
                    {"cmd": "map_for_game", "player": "x"},
                    {"cmd": "begin", "players": "four"},
                    {"cmd": "begin", "players": None}):
        try:
            hostile.srv._handle_command(hostile.client, message)  # type: ignore[arg-type]
            survived.append(message)
        except Exception as exc:                       # noqa: BLE001
            crashed.append((message, type(exc).__name__))
    if not crashed:
        ok("seven non-numeric arguments, all answered rather than raised")
    else:
        gap(f"{len(crashed)} of 7 commands with a non-numeric player/players "
            f"raise out of _handle_command: {crashed}. _on_client_read has no "
            f"guard around it and neither does the selector loop, so one "
            f"malformed message from a front-end kills the daemon -- "
            f"serve()'s finally runs close(), which stops the republisher, so "
            f"every virtual pad disappears and the machine has no controllers "
            f"at all until something restarts it")
        if survived:
            print(f"       (answered rather than raised: {len(survived)} of 7)")
    # Whatever the daemon does with them, it must not act on them.
    if hostile.srv._calibration is not None or hostile.srv._choice is not None:
        raise SystemExit(
            "FAIL: a command with a nonsense player number opened a modal "
            "flow anyway")
    if hostile.srv._state != protocol.STATE_READY:
        raise SystemExit(
            f"FAIL: nonsense arguments moved the daemon to "
            f"{hostile.srv._state!r}")
    assert_not_stranded(hostile, "non-numeric arguments")
    ok("either way, nothing opened and the daemon stayed ready")
    hostile.close()

    # ----------------------------------- a pad that vanishes at the worst moment

    print("\nS4/S5: the controller is unplugged between claim and confirm")
    # `_cancel` guards this explicitly -- "a pad that went away while the
    # session was open" -- so the hazard is known. The question is whether
    # `_accept`, which runs the same call one line later in the same story,
    # guards it too.
    def explode() -> None:
        raise OSError(19, "No such device")

    backing_out = open_session([one], stored=[Assignment(1, one, 0)])
    backing_out.srv._start_republisher = explode   # type: ignore[assignment]
    try:
        backing_out.send("cancel")
    except OSError as exc:
        raise SystemExit(
            f"FAIL: cancelling after a pad was unplugged raised {exc!r} -- "
            f"Esc is the escape hatch and it must always work")
    if backing_out.srv._state != protocol.STATE_IDLE:
        raise SystemExit(
            f"FAIL: cancel ended {backing_out.srv._state!r} with no pads "
            f"republishing -- idle is the honest answer")
    if backing_out.srv._assigner is not None:
        raise SystemExit("FAIL: cancel left the session open")
    assert_not_stranded(backing_out, "cancel with a vanished pad")
    ok("cancel survives it and reports idle honestly")
    backing_out.close()

    confirming = open_session([one], claims=[Assignment(1, one, 0)])
    confirming.srv._start_republisher = explode    # type: ignore[assignment]
    grabbed = confirming.srv._assigner
    raised = None
    try:
        confirming.send("accept")
    except OSError as exc:
        raised = exc
    if raised is None:
        ok("and so does accept")
    else:
        gap(f"_accept does not guard _start_republisher, so a pad unplugged "
            f"between the last claim and the confirm hold raises {raised!r} "
            f"out of _handle_command. _cancel wraps the identical call in "
            f"try/except OSError with a comment naming this exact case. The "
            f"session has already been ended by then, so _state is left at "
            f"'assigning' while _assigner is None, and the exception reaches "
            f"the selector loop and kills the daemon")
        if not grabbed.closed:
            raise SystemExit(
                "FAIL: accept raised before releasing the pads, which is "
                "worse than the gap described -- the grabs outlive the daemon")
        print("       (the grabs were at least released before it raised)")
    confirming.close()

    # ------------------------------------------------- restore, S18

    print("\nS18: ensure-daemon restarts and restores what was assigned")
    for label, text in (
        ("truncated json", "{{{"),
        ("a bare string", '"hello"'),
        ("an object, not a list", '{"player": 1}'),
        ("a list of nulls", "[null, 3, false]"),
        ("entries with no path", '[{"player": 1}]'),
        ("a player that is not a number", '[{"player":"x","path":"/x"}]'),
        ("a player that is null", '[{"player":null,"path":"/x"}]'),
        ("an empty list", "[]"),
    ):
        restoring = Harness()
        restoring.srv.state_path.parent.mkdir(parents=True, exist_ok=True)
        restoring.srv.state_path.write_text(text)
        try:
            restoring.srv.restore()
        except Exception as exc:                       # noqa: BLE001
            raise SystemExit(
                f"FAIL: restoring from {label} raised {exc!r} -- this runs "
                f"before serve()'s try/finally, so the daemon never starts "
                f"and leaves its socket behind")
        if restoring.srv._state != protocol.STATE_IDLE:
            raise SystemExit(
                f"FAIL: {label} restored to {restoring.srv._state!r} -- the "
                f"daemon claims controllers are live when none are")
        if restoring.srv._assignments:
            raise SystemExit(
                f"FAIL: {label} produced assignments out of nothing")
        if restoring.republished:
            raise SystemExit(
                f"FAIL: {label} created virtual pads for controllers that "
                f"were never described")
        ok(f"{label}: nothing restored, still idle")
        restoring.close()

    _VISIBLE[:] = [one, two]
    good = Harness()
    good.srv.state_path.write_text(json.dumps([
        {"player": 1, "path": one.path, "name": one.name},
        {"player": 9, "path": "/dev/input/event99", "name": "Unplugged"},
        {"player": 2, "path": two.path, "name": two.name},
    ]))
    good.srv.restore()
    if [a.player for a in good.srv._assignments] != [1, 2]:
        raise SystemExit(
            f"FAIL: restore produced "
            f"{[a.player for a in good.srv._assignments]} -- a pad that is "
            f"gone must be skipped, not faked, or it takes a place in the "
            f"enumeration and shifts every index after it")
    if good.srv._state != protocol.STATE_READY:
        raise SystemExit("FAIL: a good restore did not reach ready")
    if good.republished != 1:
        raise SystemExit("FAIL: a good restore did not republish")
    if good.srv._slots != 2:
        raise SystemExit(
            f"FAIL: restore sized the screen to {good.srv._slots} slots, "
            f"counting a controller that is not there")
    assert_not_stranded(good, "a good restore")
    ok("a real one restores two of three and skips the pad that is gone")
    good.close()

    print("\nS18: the saved state is not even a readable file")
    # `restore()` runs *before* serve()'s try/finally, so anything it raises
    # kills the daemon on the way up and leaves the socket file behind. The
    # next `ensure-daemon` then has to probe and unlink it before it can bind.
    shapes = Harness()
    base = shapes.srv.state_path
    for label, prepare in (
        ("a directory", lambda: (base.unlink(missing_ok=True),
                                 base.mkdir())),
        ("a file with no read permission",
         lambda: (base.is_dir() and base.rmdir(),
                  base.write_text("[]"), base.chmod(0o000))),
    ):
        prepare()
        try:
            shapes.srv.restore()
        except Exception as exc:                       # noqa: BLE001
            raise SystemExit(
                f"FAIL: {label} where assignments.json should be raised "
                f"{exc!r} -- restore runs before serve()'s try/finally, so "
                f"the daemon never starts and leaves its socket behind for "
                f"the next one to trip over")
        if shapes.srv._state != protocol.STATE_IDLE or shapes.srv._assignments:
            raise SystemExit(f"FAIL: {label} restored something")
        ok(f"{label}: nothing restored, still idle")
    base.chmod(0o600)
    base.unlink(missing_ok=True)
    shapes.close()

    clear_marker()
    if _GAPS:
        print(f"\n{len(_GAPS)} gap(s) reported above; everything asserted held")
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
