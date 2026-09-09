"""The daemon's core: the socket it listens on, and the loop that reads it.

Everything here is about the same failure -- the daemon *exiting*. When it
goes, its uinput nodes go with it, so the machine does not degrade into
something awkward: it simply has no controllers at all, mid-game, with nothing
on screen to say why. Four of the five faults below end the process, and three
of them can be triggered by any local process or by unplugging a pad.

  S3/S4  a line of deeply nested JSON. json.loads recurses once per level, so
         past about 52,000 brackets it raises RecursionError -- a RuntimeError,
         not a ValueError, so the `except ValueError` in LineReader.feed missed
         it. feed() is called from Server._on_client_read, inside the selector
         loop, inside serve(), with no try in between. The socket lives in
         XDG_RUNTIME_DIR, so anything running as this user could end the daemon
         with one write.
  S3/S4  a peer that never sends a newline. Same denial of service by another
         route: without a bound the buffer grows until the OOM killer arrives.
  S4     a confirm hold cancelled by the wrong button. `_confirm_started` was
         keyed on the pad alone and popped on ANY release, so a resting thumb
         going up cancelled the hold still down on the first button -- and a
         held button emits no further events, so nothing restarted it. Reported
         as "I have to reassign controllers twice before they actually get
         assigned".
  S1/S3  a controller unplugged between devices.discover() and open_device().
         Assigner.__enter__ raised FileNotFoundError, _begin did not guard it,
         and _poll_new_controllers calls _begin itself once a second -- so a
         pad unplugged just after being plugged in killed the daemon at the
         moment padmap was trying to be helpful about it.
  S1     something other than a socket in the socket's place. start() raised a
         bare IsADirectoryError with no path in it, and serve() calls start()
         outside its try/finally, so that traceback was the whole diagnosis.

Nothing here goes near real hardware. A live daemon owns the controllers on
this machine: devices.discover is replaced before any Server exists, pads are
backed by pipes rather than evdev nodes, no uinput node is created, and the
only sockets opened are socketpairs and sockets inside a temp dir. The one
subprocess spawned (for the daemon-discovery scenario) is a *fake* padmap
package that sleeps, and the file it resolves to is verified before it is
started -- because `python3 -m padmap.cli serve` resolving to the real package
would start a second daemon and fight the live one for the pads.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \\
        nix develop --command python3 tools/check_daemon_core.py
"""

from __future__ import annotations

import os
import select
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Before padmap is imported: protocol resolves the socket, the prompted file
# and the last-game record out of XDG_RUNTIME_DIR at call time, and several
# modules read these at import time. The real one belongs to the live daemon.
_SANDBOX = Path(tempfile.mkdtemp(prefix="padmap-daemon-core-"))
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    _dir = _SANDBOX / _var.lower()
    _dir.mkdir(parents=True, exist_ok=True)
    os.environ[_var] = str(_dir)
os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "devices")
# Nothing here may open a session on its own initiative.
os.environ["PADMAP_NO_AUTOSETUP"] = "1"

sys.path.insert(0, str(REPO / "src"))

import evdev                                              # noqa: E402,F401
from evdev import ecodes                                  # noqa: E402

from padmap import assign, devices, protocol, server       # noqa: E402
from padmap.devices import Pad                             # noqa: E402

BTN_A = ecodes.BTN_SOUTH
BTN_B = ecodes.BTN_EAST
BTN_START = ecodes.BTN_START


def ok(message: str) -> None:
    print(f"  ok  {message}")


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


# -- the kernel, faked ----------------------------------------------------

# Every pad this file can see. Bound before any Server exists so a code path
# that reaches discovery cannot enumerate the real machine.
_VISIBLE: list[Pad] = []
devices.discover = lambda *a, **k: list(_VISIBLE)   # type: ignore[assignment]

# Paths whose open() raises, as if the controller had been unplugged in the
# window between discover() and open_device().
_VANISHED: set[str] = set()
_OPENED: dict[str, "FakeDevice"] = {}


def pad(node: str, name: str = "Arcade Pad") -> Pad:
    return Pad(path=f"/dev/input/{node}", name=f"{name} {node}", phys=node,
               uniq="", vid=0x1234, pid=0x0001, syspath="")


class FakeDevice:
    """An open pad handle, backed by a real pipe.

    A real fd because Server._begin registers it with a real `selectors`
    object, which will not accept an invented number.
    """

    def __init__(self, path: str) -> None:
        self.path = path
        self._read_fd, self._write_fd = os.pipe()
        self.fd = self._read_fd
        self.grabs = 0
        self.ungrabs = 0
        self.closed = False

    def grab(self) -> None:
        self.grabs += 1

    def ungrab(self) -> None:
        self.ungrabs += 1

    def read_one(self):
        return None

    def read(self):
        return []

    def close(self) -> None:
        self.closed = True
        for handle in (self._read_fd, self._write_fd):
            try:
                os.close(handle)
            except OSError:
                pass


def fake_open(target: Pad):
    if target.path in _VANISHED:
        # Exactly what evdev raises: OSError with errno ENOENT and the node
        # in .filename.
        raise FileNotFoundError(2, "No such file or directory", target.path)
    device = FakeDevice(target.path)
    _OPENED[target.path] = device
    return device


assign.open_device = fake_open   # type: ignore[assignment]


def make_server(name: str) -> server.Server:
    """A Server wired entirely inside the sandbox, never started."""
    root = _SANDBOX / name
    root.mkdir(parents=True, exist_ok=True)
    return server.Server(
        socket_path=root / "padmap.sock",
        state_path=root / "assignments.json",
        launch_config_path=root / "launch.cfg",
    )


# -- S3/S4: a line no client should be able to end the daemon with ---------


def scenario_nested_json_through_the_read_loop() -> None:
    print("\nS3/S4: 200,000 brackets down the socket, the daemon's own path")

    depth = 200000
    poison = (b"[" * depth) + (b"]" * depth) + b"\n"

    reader = protocol.LineReader()
    try:
        got = reader.feed(poison)
    except BaseException as error:                        # noqa: BLE001
        fail(f"LineReader.feed raised {type(error).__name__} on a "
             f"{depth}-deep line. It is called from _on_client_read, inside "
             f"the selector loop, inside serve(), so any process that can "
             f"open the socket in XDG_RUNTIME_DIR ends the daemon -- and "
             f"every virtual pad on the machine stops working mid-game")
    if got:
        fail(f"a {depth}-deep array was handed on as a command: {got!r}")
    ok(f"feed() skips a {depth}-deep line instead of raising")

    # The framing has to survive it too, or a client that is still connected
    # sees the setup screen stop updating and nothing says why.
    if reader.feed(protocol.encode({"cmd": "status"})) != [{"cmd": "status"}]:
        fail("after a poisoned line the reader never resynchronised; every "
             "later command from that front-end is lost")
    ok("the next line after the poisoned one is read normally")

    # And in the shape it actually arrives in: recv(65536) at a time, with a
    # real command ahead of the poison in the same read.
    srv = make_server("nested")
    daemon_side, peer = socket.socketpair()
    daemon_side.setblocking(False)
    srv._clients[daemon_side.fileno()] = server.Client(daemon_side)

    payload = (protocol.encode({"cmd": "status"}) + poison
               + protocol.encode({"cmd": "status"}))

    def write() -> None:
        try:
            peer.sendall(payload)
            peer.shutdown(socket.SHUT_WR)
        except OSError:
            pass

    writer = threading.Thread(target=write)
    writer.start()
    deadline = time.monotonic() + 10.0
    try:
        while srv._clients and time.monotonic() < deadline:
            readable, _, _ = select.select([daemon_side], [], [], 0.25)
            if not readable:
                continue
            try:
                srv._on_client_read(daemon_side)   # the daemon's own handler
            except BaseException as error:                # noqa: BLE001
                fail(f"_on_client_read raised {type(error).__name__} on a "
                     f"{depth}-deep line arriving in recv-sized reads. This "
                     f"is the daemon exiting: every controller on the machine "
                     f"goes dead, mid-game, because a local process wrote to "
                     f"a socket")
    finally:
        writer.join()

    replies = protocol.LineReader().feed(_drain(peer))
    states = [m for m in replies if m.get("event") == "state"]
    if len(states) != 2:
        fail(f"the daemon answered {len(states)} of the 2 status commands "
             f"sent either side of the poisoned line; the good message in "
             f"the same read must not be lost with it")
    ok("both real commands either side of the poison are answered, over a "
       "real socket, in recv(65536) reads")

    peer.close()
    for client in list(srv._clients.values()):
        srv._drop_client(client.sock)


def _drain(sock: socket.socket) -> bytes:
    sock.setblocking(False)
    out = b""
    while True:
        try:
            chunk = sock.recv(65536)
        except OSError:
            break
        if not chunk:
            break
        out += chunk
    return out


def scenario_feed_that_raises_anything() -> None:
    print("\nS4: whatever the reader does, the loop survives it")

    srv = make_server("feedraise")
    daemon_side, peer = socket.socketpair()
    daemon_side.setblocking(False)
    client = server.Client(daemon_side)
    srv._clients[daemon_side.fileno()] = client

    def explode(_data: bytes):
        raise RuntimeError("kaboom")

    client.reader.feed = explode                          # type: ignore[method-assign]
    peer.sendall(b'{"cmd":"status"}\n')
    time.sleep(0.05)
    try:
        srv._on_client_read(daemon_side)
    except BaseException as error:                        # noqa: BLE001
        fail(f"a reader that raises {type(error).__name__} takes the daemon "
             f"with it -- _on_client_read runs inside the selector loop, and "
             f"nothing between here and serve() catches anything")
    ok("a read that raises is logged and dropped, not fatal")
    if daemon_side.fileno() not in srv._clients:
        fail("the front-end was dropped over one unreadable read; the setup "
             "screen would go blank because a stray byte arrived")
    ok("the front-end keeps its connection")

    peer.close()
    srv._drop_client(daemon_side)


def scenario_a_peer_that_never_sends_a_newline() -> None:
    print("\nS4: a peer that sends and sends and never sends a newline")

    reader = protocol.LineReader()
    chunk = b"y" * (1024 * 1024)
    sent = 0
    for _ in range(24):
        if reader.feed(chunk) != []:
            fail("bytes with no newline in them were handed on as a command")
        sent += len(chunk)
    held = len(reader._buffer)
    if held >= sent:
        fail(f"LineReader still holds all {held // (1024 * 1024)} MiB of a "
             f"stream with no newline in it. Any process running as this "
             f"user may connect to the socket, so `yes | nc -U "
             f"$XDG_RUNTIME_DIR/padmap/padmap.sock` grows the daemon until "
             f"the OOM killer takes it -- and every virtual pad goes with it, "
             f"mid-game")
    ok(f"{sent // (1024 * 1024)} MiB with no newline is bounded at "
       f"{held} bytes held")

    # Bounded is only useful if it also resynchronises, and if a real message
    # of the size padmap actually sends still gets through.
    if reader.feed(b"trailing rubbish\n") != []:
        fail("the tail of the over-long line was parsed as a command")
    status = reader.feed(protocol.encode({"cmd": "status"}))
    if status != [{"cmd": "status"}]:
        fail(f"after dropping an over-long line the reader never recovered "
             f"({status!r}); the front-end is connected but nothing it sends "
             f"is ever acted on again")
    ok("framing resynchronises on the next newline after the drop")

    reader = protocol.LineReader()
    big = protocol.encode({"event": "sdl_mapping", "lines": ["x" * 900000]})
    got: list[dict] = []
    for index in range(0, len(big), 65536):
        got += reader.feed(big[index:index + 65536])
    if len(got) != 1 or len(got[0]["lines"][0]) != 900000:
        fail(f"a {len(big)}-byte sdl_mapping did not survive the size bound; "
             f"the pads have no mapping until Pegasus is relaunched")
    ok(f"a legitimate {len(big) // 1024} KiB message still arrives whole")


# -- S4: the confirm hold -------------------------------------------------


class ConfirmHarness:
    """A daemon with one claimed pad and a frozen clock."""

    def __init__(self) -> None:
        self.srv = make_server("confirm")
        self.pad = pad("event90")
        self.sent: list[dict] = []
        self.srv._broadcast = self.sent.append      # type: ignore[assignment]
        self.srv._assignments = []
        self.accepts = 0

        def accept() -> None:
            self.accepts += 1

        self.srv._accept = accept                   # type: ignore[assignment]
        self.srv._assigner = object()               # type: ignore[assignment]
        self.now = 1000.0
        self._real = server.time
        self.srv_time = _Clock(self)
        server.time = self.srv_time                 # type: ignore[assignment]

    def close(self) -> None:
        server.time = self._real                    # type: ignore[assignment]

    def advance(self, seconds: float) -> None:
        self.now += seconds

    def down(self, code: int) -> None:
        self.srv._on_claimed_event(self.pad, code, 1)

    def up(self, code: int) -> None:
        self.srv._on_claimed_event(self.pad, code, 0)

    def tick(self) -> None:
        self.srv._tick_confirm()


class _Clock:
    def __init__(self, harness: ConfirmHarness) -> None:
        self._harness = harness

    def monotonic(self) -> float:
        return self._harness.now


def scenario_second_button_does_not_cancel_the_confirm() -> None:
    print("\nS4: a resting thumb must not cancel the confirm hold")

    h = ConfirmHarness()
    try:
        h.down(BTN_A)
        h.advance(0.2)
        h.down(BTN_B)          # a thumb resting on B, or Start held as well
        h.up(BTN_B)            # that one lets go; A is still down
        h.advance(server.CONFIRM_HOLD_SECONDS)
        h.tick()
        if h.accepts != 1:
            fail("releasing a second button cancelled the confirm hold still "
                 "held down on the first, so the session was never accepted. "
                 "A held button emits no further events, so nothing restarts "
                 "the hold either: the user has to let go and start over, "
                 "which is the report 'I have to reassign controllers twice "
                 "before they actually get assigned'")
        ok("holding A while B is pressed and released still confirms")
    finally:
        h.close()

    h = ConfirmHarness()
    try:
        h.down(BTN_A)
        h.advance(0.2)
        h.up(BTN_A)            # the hold really did end
        h.advance(server.CONFIRM_HOLD_SECONDS)
        h.tick()
        if h.accepts:
            fail("letting go of the button that started the confirm hold "
                 "still accepted the session -- a tap on a claimed pad ends "
                 "setup, releasing the pads before the user has checked the "
                 "player order")
        if h.srv._confirm_started:
            fail("the confirm tracker still holds a hold nobody is holding")
        ok("releasing the button that started it does cancel the hold")
    finally:
        h.close()

    h = ConfirmHarness()
    try:
        h.down(BTN_A)
        h.advance(0.2)
        h.down(BTN_START)
        h.advance(0.2)
        h.up(BTN_A)            # the one that started it goes up
        h.advance(server.CONFIRM_HOLD_SECONDS)
        h.tick()
        if h.accepts:
            fail("the confirm completed after the button that started it was "
                 "released, on the strength of a different button being down")
        ok("a different button still down does not keep a released hold alive")
        # And the tracker is empty, so the next press starts a fresh hold
        # rather than inheriting the old one's start time.
        if h.srv._confirm_started or h.srv._confirm_button:
            fail("the confirm tracker was left half-populated; the next press "
                 "would confirm instantly on a hold nobody made")
        ok("the tracker is emptied, so the next press starts a fresh hold")
    finally:
        h.close()

    h = ConfirmHarness()
    try:
        h.down(BTN_A)
        h.advance(0.2)
        h.srv._clear_confirm()
        if h.srv._confirm_started or h.srv._confirm_button:
            fail("_clear_confirm left half of the hold behind")
        h.advance(server.CONFIRM_HOLD_SECONDS)
        h.tick()
        if h.accepts:
            fail("a hold cleared when the session changed came back and "
                 "accepted a session the user was not confirming")
        ok("_clear_confirm drops both halves of the hold")
    finally:
        h.close()


# -- S1/S3: a controller unplugged at the worst moment --------------------


def scenario_pad_unplugged_between_discovery_and_opening() -> None:
    print("\nS1/S3: a pad unplugged between discover() and open_device()")

    _VISIBLE[:] = [pad("event90"), pad("event91"), pad("event92")]
    _VANISHED.clear()
    _VANISHED.add("/dev/input/event91")
    _OPENED.clear()

    srv = make_server("unplug")
    sent: list[dict] = []
    srv._broadcast = sent.append                    # type: ignore[assignment]
    try:
        srv._begin(4)
    except BaseException as error:                  # noqa: BLE001
        fail(f"_begin raised {type(error).__name__} because one of three "
             f"pads was unplugged between discovery and opening it. Nothing "
             f"guards this -- not _handle_command, not run() -- and "
             f"_poll_new_controllers calls _begin itself once a second, so "
             f"plugging a controller in and pulling it out again ends the "
             f"daemon and every virtual pad on the machine with it")
    ok("_begin survives a pad that vanishes under it")

    if srv._state != protocol.STATE_ASSIGNING:
        fail(f"setup did not open at all (state {srv._state!r}) -- unplugging "
             f"one controller must not stop the other two being assigned")
    if srv._assigner is None or len(srv._assigner.pads) != 2:
        held = None if srv._assigner is None else len(srv._assigner.pads)
        fail(f"the session opened on {held} pad(s) rather than the 2 that "
             f"are still plugged in")
    ok("the session opens on the two pads that are still there")

    counts = [m["count"] for m in sent if m.get("event") == "pads"]
    if counts != [2]:
        fail(f"the front-end was told {counts} pads are present; the setup "
             f"screen would draw a slot for a controller that is not plugged "
             f"in")
    ok("the front-end is told about 2 pads, not the 3 discovery saw")

    if "/dev/input/event91" in _OPENED:
        fail("the vanished node was opened after all; the test is not "
             "reproducing the failure")
    for path in ("/dev/input/event90", "/dev/input/event92"):
        if not _OPENED[path].grabs:
            fail(f"{path} was opened but never grabbed, so every press during "
                 f"setup also reaches the front-end")
    ok("both survivors are open and grabbed")

    srv._end_session(release=True)
    _VANISHED.clear()


def scenario_every_pad_unplugged_at_once() -> None:
    print("\nS1/S3: every pad unplugged between discover() and open_device()")

    _VISIBLE[:] = [pad("event90"), pad("event91")]
    _VANISHED.clear()
    _VANISHED.update({"/dev/input/event90", "/dev/input/event91"})
    _OPENED.clear()

    srv = make_server("unplugall")
    sent: list[dict] = []
    srv._broadcast = sent.append                    # type: ignore[assignment]
    # Something was on the air before setup was asked for. _begin stops
    # republishing before it opens anything, so failing to open must not be a
    # more expensive way to lose your controllers than never having asked.
    srv._assignments = [_stored_assignment()]
    resumed: list[int] = []

    def resume() -> None:
        resumed.append(1)
        srv._republisher = object()                 # type: ignore[assignment]

    srv._start_republisher = resume                 # type: ignore[assignment]

    try:
        srv._begin(4)
    except BaseException as error:                  # noqa: BLE001
        fail(f"_begin raised {type(error).__name__} when every pad had gone; "
             f"the daemon exits and the machine has no controllers at all")
    ok("_begin survives every pad going away at once")

    if srv._assigner is not None:
        fail("a session was left open on no pads: the setup screen sits there "
             "waiting for a press that can never arrive, and the pads stay "
             "grabbed")
    errors = [m for m in sent if m.get("event") == "error"]
    if not errors:
        fail("nothing was said: the setup screen would wait forever for a "
             "session that never opened")
    ok(f"the front-end is told why: {errors[-1]['message']!r}")

    if not resumed or srv._state != protocol.STATE_READY:
        fail(f"the assignments that were on the air before setup was asked "
             f"for were not put back (state {srv._state!r}); asking for a "
             f"screen that could not open cost the user their controllers")
    ok("the previous assignments go back on the air, state is ready")

    _VANISHED.clear()
    _VISIBLE.clear()


def _stored_assignment():
    from padmap.assign import Assignment
    return Assignment(player=1, pad=pad("event90"), button=BTN_A)


# -- S1: something other than a socket in the socket's place --------------


def scenario_socket_path_occupied() -> None:
    print("\nS1: a directory where the socket should be")

    root = _SANDBOX / "occupied"
    root.mkdir(parents=True, exist_ok=True)
    (root / "padmap.sock").mkdir(exist_ok=True)
    srv = server.Server(socket_path=root / "padmap.sock",
                        state_path=root / "assignments.json",
                        launch_config_path=root / "launch.cfg")
    try:
        srv.start()
    except RuntimeError as error:
        if "padmap.sock" not in str(error):
            fail(f"the refusal does not name the path that is in the way: "
                 f"{error}")
        ok(f"refused with a RuntimeError naming it: {error}")
    except BaseException as error:                  # noqa: BLE001
        fail(f"start() raised a bare {type(error).__name__} ({error}). "
             f"serve() calls start() outside its try/finally, so that "
             f"traceback is the entire diagnosis an operator gets for a "
             f"daemon that will not come up -- and it names neither padmap "
             f"nor the path to delete. The live-daemon case one line above "
             f"raises a RuntimeError naming the socket; this must match it")
    else:
        fail("start() claimed to be listening on a directory")

    print("\nS1: a regular file where the socket should be")
    root = _SANDBOX / "occupied2"
    root.mkdir(parents=True, exist_ok=True)
    (root / "padmap.sock").write_text("not a socket\n")
    srv = server.Server(socket_path=root / "padmap.sock",
                        state_path=root / "assignments.json",
                        launch_config_path=root / "launch.cfg")
    # A plain file *can* be unlinked, so this one is expected to start: a
    # stale file is not a reason to refuse to serve.
    try:
        srv.start()
    except BaseException as error:                  # noqa: BLE001
        fail(f"a stale regular file stopped the daemon starting "
             f"({type(error).__name__}: {error}); it can simply be removed, "
             f"as a stale socket already is")
    ok("a stale regular file is removed and the daemon listens")
    srv.close()

    print("\nS1: a regular file where the runtime directory should be")
    root = _SANDBOX / "occupied3"
    root.write_text("not a directory\n")
    srv = server.Server(socket_path=root / "padmap" / "padmap.sock",
                        state_path=_SANDBOX / "occ3.json",
                        launch_config_path=_SANDBOX / "occ3.cfg")
    try:
        srv.start()
    except RuntimeError as error:
        if "padmap.sock" not in str(error):
            fail(f"the refusal does not name the socket: {error}")
        ok(f"refused with a RuntimeError naming it: {error}")
    except BaseException as error:                  # noqa: BLE001
        fail(f"start() raised a bare {type(error).__name__} ({error}) when a "
             f"file sat where the runtime directory should be; an operator "
             f"reading that traceback learns nothing about which path to fix")
    else:
        fail("start() claimed to be listening under a path that is a file")

    print("\nS1: a socket left behind by a daemon that crashed")
    root = _SANDBOX / "stale"
    root.mkdir(parents=True, exist_ok=True)
    dead = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    dead.bind(str(root / "padmap.sock"))
    dead.close()                                   # bound, nothing listening
    srv = server.Server(socket_path=root / "padmap.sock",
                        state_path=root / "assignments.json",
                        launch_config_path=root / "launch.cfg")
    srv.start()
    ok("a stale socket is removed and the daemon listens on it")
    srv.close()


# -- S1: finding the daemon that serves a given runtime dir ---------------


def _fake_padmap_package() -> Path:
    """A `padmap.cli` that sleeps, for a process that only has to look real.

    daemon_pids matches argv structurally -- [python, -m, padmap.cli, serve]
    -- so reproducing it needs a process with exactly that command line. It
    must not be the real one: `padmap serve` grabs controllers, and a live
    daemon owns the ones on this machine.
    """
    root = _SANDBOX / "fakepkg"
    (root / "padmap").mkdir(parents=True, exist_ok=True)
    (root / "padmap" / "__init__.py").write_text("")
    (root / "padmap" / "cli.py").write_text(
        "import time\n"
        "if __name__ == '__main__':\n"
        "    time.sleep(120)\n"
    )
    return root


def scenario_daemon_pids_normalises_the_runtime_dir() -> None:
    print("\nS1: finding the daemon that serves a runtime dir")

    fake = _fake_padmap_package()
    runtime = _SANDBOX / "pids"
    runtime.mkdir(parents=True, exist_ok=True)

    env = dict(os.environ)
    env["PYTHONPATH"] = str(fake)
    # The trailing slash is the whole point: this is a daemon started from a
    # shell where XDG_RUNTIME_DIR happened to carry one. It binds exactly the
    # same socket, because every path here is built with Path(...) / "padmap".
    env["XDG_RUNTIME_DIR"] = str(runtime) + "/"

    # Refuse to spawn anything until `padmap.cli` is proven to be the fake.
    proof = subprocess.run(
        [sys.executable, "-c",
         "import padmap.cli, sys; sys.stdout.write(padmap.cli.__file__)"],
        env=env, cwd=str(fake), capture_output=True, text=True, timeout=60)
    if proof.returncode != 0 or not proof.stdout.startswith(str(fake)):
        fail(f"refusing to run: `python -m padmap.cli` resolves to "
             f"{proof.stdout!r} rather than the fake package in {fake}. "
             f"Starting the real one would grab the controllers the live "
             f"daemon on this machine is using")

    child = subprocess.Popen([sys.executable, "-m", "padmap.cli", "serve"],
                             env=env, cwd=str(fake),
                             stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)
    try:
        # /proc/<pid>/environ is readable as soon as the process exists.
        found: list[int] = []
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            found = protocol.daemon_pids(str(runtime))
            if found:
                break
            time.sleep(0.05)

        if child.pid not in found:
            fail(f"a daemon whose XDG_RUNTIME_DIR is {str(runtime) + '/'!r} "
                 f"is invisible to daemon_pids({str(runtime)!r}) -- the two "
                 f"strings differ by a trailing slash and name the same "
                 f"socket. `ensure-daemon` therefore sees no daemon and "
                 f"starts a second one on a socket that is already in use, "
                 f"and `restart-daemon` leaves the stale one running -- which "
                 f"is exactly the stale-daemon trap the build id exists to "
                 f"catch")
        ok("a daemon started with a trailing slash on XDG_RUNTIME_DIR is found")

        also = protocol.daemon_pids(str(runtime) + "/")
        if child.pid not in also:
            fail("asking with the trailing slash does not find it either, so "
                 "the two spellings still cannot see one another")
        ok("...and asking with the slash finds it too: the two agree")

        other = protocol.daemon_pids(str(runtime) + "-elsewhere")
        if child.pid in other:
            fail("normalising went too far: a daemon serving one runtime dir "
                 "was reported for a different one, so `padmap restart-daemon` "
                 "would signal a daemon serving somebody else's socket")
        ok("a genuinely different runtime dir does not match")
    finally:
        child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=10)


def main() -> int:
    print("daemon core: the socket, the read loop, and the confirm hold")
    print(f"sandbox: {_SANDBOX}")

    scenario_nested_json_through_the_read_loop()
    scenario_feed_that_raises_anything()
    scenario_a_peer_that_never_sends_a_newline()

    scenario_second_button_does_not_cancel_the_confirm()

    scenario_pad_unplugged_between_discovery_and_opening()
    scenario_every_pad_unplugged_at_once()

    scenario_socket_path_occupied()
    scenario_daemon_pids_normalises_the_runtime_dir()

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
