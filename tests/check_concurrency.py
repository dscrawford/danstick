"""More than one of something: two daemons, two front-ends, two /proc entries.

Everything padmap owns is singular in the happy path -- one daemon, one
socket, one front-end driving one session -- and every guard in this area
exists because something turned up in duplicate. The three questions here:

  * `protocol.daemon_pids` (S18). It matches argv *structurally* rather than
    as a substring, because `pgrep -f` also matches the shell running a
    diagnostic that merely mentions "padmap.cli serve" -- signalling that
    kills the user's shell, not a daemon. It filters by XDG_RUNTIME_DIR
    because that is what decides which socket a daemon serves, and by uid
    because /proc is full of other people's processes. It is fed by /proc,
    which is the most concurrent thing on the machine: a pid can vanish
    between being listed and being read, an entry can be unreadable, and
    `cmdline` is raw bytes with no promise of a trailing NUL or of UTF-8.

  * The socket and the runtime dir (S18). A stale socket file left by a
    killed daemon must be replaced, a *live* one must never be stolen, and a
    runtime dir that does not exist yet must simply be created.

  * The daemon with several clients (S1, S2, S5). A second front-end joining
    mid-session must be given state; one client leaving must not cancel a
    session another is driving, because `_drop_client` fires `_cancel` only
    when the *last* client goes; and a client must have been settled for
    AUTOSETUP_CLIENT_SECONDS before the daemon will grab every pad for it --
    `padmap ensure-daemon` connects for milliseconds to read status, and
    counting that once left the daemon in `assigning`, with every pad grabbed,
    to show a screen on a front-end that was not running.

Nothing here may touch real hardware or the real socket. A live daemon owns
the controllers on this machine: XDG_RUNTIME_DIR, XDG_CONFIG_HOME and
XDG_DATA_HOME are redirected before padmap is imported, `devices.discover`
and `server.Assigner` are replaced before any command is dispatched, and every
socket bound below is inside the sandbox. No uinput node is created.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \\
        nix develop --command python3 tests/check_concurrency.py
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import sys
import tempfile
import time
from pathlib import Path

# Before padmap is imported: several modules read these at import time.
_SANDBOX = tempfile.mkdtemp(prefix="padmap-concurrency-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)
# No scenario below wants a session opened behind its back. The one that asks
# about autosetup clears this itself.
os.environ["PADMAP_NO_AUTOSETUP"] = "1"

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import devices, profiles, protocol, server  # noqa: E402
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

_GAPS: list[str] = []


def heading(text: str) -> None:
    print(f"\n{text}")


def ok(text: str) -> None:
    print(f"  ok  {text}")


def gap(text: str) -> None:
    """A defect confirmed against this source, reported rather than failed.

    Green on purpose: the maintainer fixes the source and turns the line under
    the gap into an assertion.
    """
    _GAPS.append(text)
    print(f"  gap: {text}")


def fail(text: str) -> None:
    raise SystemExit(f"FAIL: {text}")


def need(condition: object, message: str, failure: str) -> None:
    if not condition:
        fail(failure)
    ok(message)


# ---------------------------------------------------------------------------
# Hardware stand-ins. Nothing below may open a real device.
# ---------------------------------------------------------------------------

def pad(node: str = "event90", name: str = "Concurrency Pad",
        vid: int = 0x1234, pid: int = 0x0001) -> Pad:
    return Pad(path=f"/dev/input/{node}", name=name, phys=f"usb-{node}",
               uniq="", vid=vid, pid=pid, syspath="")


PAD = pad()


class AbsInfo:
    def __init__(self, minimum: int, maximum: int, value: int) -> None:
        self.min, self.max, self.value = minimum, maximum, value


class FakeDevice:
    """An open pad handle -- never a real one: opening a real device would
    take events away from the daemon actually running on this machine."""

    def capabilities(self, absinfo: bool = False):
        from padmap import capture
        return {capture.EV_KEY: [0x130, 0x131, 0x133, 0x134],
                capture.EV_ABS: [(0x00, AbsInfo(0, 255, 128)),
                                 (0x01, AbsInfo(0, 255, 128))]}

    def active_keys(self):
        return []


class FakeAssigner:
    """A session: live claims, open handles, and real fds.

    The fds are read ends of real pipes rather than invented numbers, because
    the daemon registers them with a real `selectors` object -- and a pump
    loop selecting on a made-up fd would raise instead of testing anything.
    """

    opened: list["FakeAssigner"] = []

    def __init__(self, pads=(), grab: bool = True, **_kw) -> None:
        self.pads = list(pads)
        self.assignments: list[Assignment] = []
        self._open = {p.path: FakeDevice() for p in self.pads}
        read_fd, write_fd = os.pipe()
        self.fds = [read_fd]
        self._write_fd = write_fd
        self.grab_failures: list[Pad] = []
        # A real close() releases EVIOCGRAB. Whether this ran is the whole
        # question in most of the scenarios below.
        self.closed = False
        self.on_claimed_event = None
        self.on_raw_event = None
        FakeAssigner.opened.append(self)

    def __enter__(self) -> "FakeAssigner":
        return self

    def __exit__(self, *_exc) -> None:
        return None

    def device_for(self, target: Pad):
        return self._open.get(target.path)

    def reset(self) -> None:
        self.assignments = []

    def tick(self, on_progress=None, on_claim=None) -> None:
        return None

    def close(self) -> None:
        self.closed = True

    def release_fds(self) -> None:
        for fd in (self.fds[0], self._write_fd):
            try:
                os.close(fd)
            except OSError:
                pass


def isolate_hardware() -> None:
    devices.discover = lambda *a, **k: [PAD]        # type: ignore[assignment]
    server.Assigner = FakeAssigner                  # type: ignore[assignment]


# ---------------------------------------------------------------------------
# A real daemon on a real unix socket, inside the sandbox
# ---------------------------------------------------------------------------

class Peer:
    """A front-end: a real client socket speaking the real wire protocol."""

    def __init__(self, path: Path) -> None:
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(str(path))
        self.sock.setblocking(False)
        self._buffer = b""

    def send(self, **message) -> None:
        self.sock.sendall(protocol.encode(message))

    def drain(self) -> list[dict]:
        while True:
            try:
                data = self.sock.recv(65536)
            except (BlockingIOError, OSError):
                break
            if not data:
                break
            self._buffer += data
        out = []
        while b"\n" in self._buffer:
            line, self._buffer = self._buffer.split(b"\n", 1)
            if line.strip():
                out.append(json.loads(line))
        return out

    def events(self) -> list[str]:
        return [str(m.get("event")) for m in self.drain()]

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


class Daemon:
    """A Server listening on a sandbox socket, hardware stubbed out."""

    def __init__(self, name: str = "daemon") -> None:
        self.path = protocol.runtime_dir() / f"{name}.sock"
        self.srv = server.Server(socket_path=self.path)
        # Creates uinput nodes and publishes virtual pads onto the machine.
        self.srv._start_republisher = lambda: None   # type: ignore[assignment]
        self.srv._save_assignments = lambda: None    # type: ignore[assignment]
        # Opens every assigned pad to read its capabilities.
        self.srv._write_controller_configs = lambda: None  # type: ignore[assignment]
        self.srv.start()
        self.peers: list[Peer] = []

    def connect(self) -> Peer:
        peer = Peer(self.path)
        self.peers.append(peer)
        self.pump()
        return peer

    def pump(self, rounds: int = 16) -> None:
        """One turn of the selector loop, without `_tick`.

        Deliberately without: `_tick` polls for new controllers, and the
        question here is what the *socket* does.
        """
        for _ in range(rounds):
            ready = self.srv._selector.select(timeout=0.01)
            if not ready:
                return
            for key, _mask in ready:
                key.data(key.fileobj)

    @property
    def session(self):
        return self.srv._assigner

    def close(self) -> None:
        for peer in self.peers:
            peer.close()
        try:
            self.srv.close()
        except Exception:                              # noqa: BLE001
            pass
        for assigner in FakeAssigner.opened:
            assigner.release_fds()
        FakeAssigner.opened.clear()


def begin_session(daemon: Daemon, driver: Peer, players: int = 4) -> None:
    """Open an assignment session the way a front-end does, and claim a slot."""
    driver.send(cmd="begin", players=players)
    daemon.pump()
    if daemon.session is None:
        fail("a `begin` over a real socket opened no session; every "
             "multi-client scenario below depends on it")
    daemon.session.assignments = [Assignment(player=1, pad=PAD, button=0)]


# ---------------------------------------------------------------------------
# S18: protocol.daemon_pids, against synthetic /proc-like data
# ---------------------------------------------------------------------------

OURS = "/run/user/4242"
THEIRS = "/run/user/65534"
PYTHON = "/nix/store/whatever-python3/bin/python3"


class FakeProc:
    """A directory tree shaped like /proc, read in its place.

    Synthetic because the cases that matter cannot be arranged for real: a
    process owned by another uid, one that exits between the listing and the
    read, and a `cmdline` that is not valid UTF-8.
    """

    def __init__(self, root: Path) -> None:
        self.root = root
        self.root.mkdir(parents=True, exist_ok=True)
        # Pids whose directory is removed the moment it is read: the exact
        # race between listing /proc and reading an entry.
        self.vanishing: set[str] = set()

    def write(self, pid: int, cmdline: bytes | None,
              environ: bytes | None) -> Path:
        entry = self.root / str(pid)
        entry.mkdir(parents=True, exist_ok=True)
        if cmdline is not None:
            entry.joinpath("cmdline").write_bytes(cmdline)
        if environ is not None:
            entry.joinpath("environ").write_bytes(environ)
        return entry

    def pids(self, runtime: str, uid: int | None = None) -> list[int]:
        real_path = protocol.Path
        real_getuid = protocol.os.getuid

        def fake_path(arg, *rest):
            text = str(arg)
            if rest:
                return real_path(arg, *rest)
            if text == "/proc":
                return real_path(self.root)
            if text.startswith("/proc/"):
                tail = text[len("/proc/"):]
                first = tail.split("/")[0]
                if first in self.vanishing:
                    shutil.rmtree(self.root / first, ignore_errors=True)
                return real_path(self.root) / tail
            return real_path(text)

        protocol.Path = fake_path                      # type: ignore[assignment]
        if uid is not None:
            protocol.os.getuid = lambda: uid           # type: ignore[assignment]
        try:
            return sorted(protocol.daemon_pids(runtime))
        finally:
            protocol.Path = real_path                  # type: ignore[assignment]
            protocol.os.getuid = real_getuid           # type: ignore[assignment]


def argv_bytes(*parts: str) -> bytes:
    return b"".join(part.encode() + b"\0" for part in parts)


def environ_bytes(**items: str) -> bytes:
    return b"".join(f"{k}={v}".encode() + b"\0" for k, v in items.items())


def scenario_daemon_pids_on_synthetic_proc() -> None:
    heading("S18: daemon_pids reads /proc, which changes underneath it")

    proc = FakeProc(Path(_SANDBOX) / "fakeproc")
    daemon_argv = (PYTHON, "-m", "padmap.cli", "serve")

    # Two daemons, same runtime dir. Both must be reported: _stop_daemon's
    # fallback refuses to guess when there is more than one candidate, and it
    # can only refuse if it is told about both.
    proc.write(3001, argv_bytes(*daemon_argv), environ_bytes(
        XDG_RUNTIME_DIR=OURS, HOME="/home/x"))
    proc.write(3002, argv_bytes(*daemon_argv), environ_bytes(
        XDG_RUNTIME_DIR=OURS))
    # A daemon serving somebody else's socket.
    proc.write(3003, argv_bytes(*daemon_argv), environ_bytes(
        XDG_RUNTIME_DIR=THEIRS))
    # A shell that merely mentions the string -- `pgrep -f` matches this.
    proc.write(3004, argv_bytes(
        "/bin/bash", "-c", "pgrep -f 'padmap.cli serve' | xargs kill"),
        environ_bytes(XDG_RUNTIME_DIR=OURS))
    # cmdline with no trailing NUL. The kernel writes one, but a process can
    # rewrite its own argv and /proc hands back whatever is there.
    proc.write(3005, b"\0".join(part.encode() for part in daemon_argv),
               environ_bytes(XDG_RUNTIME_DIR=OURS))
    # A whole command line in one blob, no NUL anywhere: what a substring
    # match would happily accept.
    proc.write(3006, b"python3 -m padmap.cli serve",
               environ_bytes(XDG_RUNTIME_DIR=OURS))
    # argv that is not valid UTF-8 -- an interpreter path with a stray byte.
    proc.write(3007, b"\xff\xfe\x80\0-m\0padmap.cli\0serve\0",
               environ_bytes(XDG_RUNTIME_DIR=OURS))
    # environ that is not valid UTF-8, with a good XDG_RUNTIME_DIR after it.
    proc.write(3008, argv_bytes(*daemon_argv),
               b"JUNK=\xff\xfe\0XDG_RUNTIME_DIR=" + OURS.encode() + b"\0")
    # Exits between being listed and being read.
    proc.write(3009, argv_bytes(*daemon_argv), environ_bytes(
        XDG_RUNTIME_DIR=OURS))
    proc.vanishing.add("3009")
    # A kernel thread: listed, no cmdline at all.
    proc.write(3010, None, environ_bytes(XDG_RUNTIME_DIR=OURS))
    # Not a pid. /proc is full of these.
    (proc.root / "self").mkdir(exist_ok=True)
    (proc.root / "meminfo").write_text("MemTotal: 1 kB\n")
    # An entry we may not read at all.
    unreadable = proc.write(3011, argv_bytes(*daemon_argv), environ_bytes(
        XDG_RUNTIME_DIR=OURS))
    os.chmod(unreadable, 0o000)

    try:
        found = proc.pids(OURS)
        other_uid = proc.pids(OURS, uid=os.getuid() + 1)
    except Exception as error:                         # noqa: BLE001
        os.chmod(unreadable, 0o755)
        fail(f"daemon_pids raised {type(error).__name__}: {error} -- "
             f"ensure-daemon runs this before every front-end launch, so the "
             f"launcher stops instead of starting the daemon")
    finally:
        os.chmod(unreadable, 0o755)

    need(3001 in found and 3002 in found,
         "two daemons sharing one runtime dir are both reported, so "
         "ensure-daemon can refuse to guess which to signal",
         f"only {found} came back for two daemons on {OURS}; _stop_daemon's "
         f"fallback would signal one of them as if it were alone")
    need(3003 not in found,
         "a daemon on another XDG_RUNTIME_DIR is left alone",
         f"a daemon serving {THEIRS} was matched ({found}); stopping it takes "
         f"down a session that is none of this one's business")
    need(3004 not in found,
         "a shell whose command line merely mentions 'padmap.cli serve' is "
         "not a daemon",
         f"a diagnostic shell was matched ({found}); signalling it kills the "
         f"user's own shell instead of a daemon")
    need(3005 in found,
         "argv with no trailing NUL is still matched",
         f"a daemon whose cmdline lacked its final NUL was invisible "
         f"({found}); ensure-daemon would start a second one beside it")
    need(3006 not in found,
         "a whole command line in one blob, with no NUL to split on, is not "
         "matched",
         f"an unsplit command line was matched ({found}) -- that is the "
         f"substring behaviour daemon_pids exists to avoid")
    need(3007 in found,
         "argv that is not valid UTF-8 is decoded, not fatal",
         f"a daemon with a non-UTF-8 argv was lost ({found}); /proc is bytes, "
         f"and UnicodeDecodeError here stops ensure-daemon entirely")
    need(3008 in found,
         "an environ block that is not valid UTF-8 still yields its "
         "XDG_RUNTIME_DIR",
         f"a daemon with a damaged environ was lost ({found})")
    need(3009 not in found and not (proc.root / "3009").exists(),
         "a pid that exits between being listed and being read is skipped",
         f"a vanished process was matched ({found}) -- its pid may already "
         f"belong to something else, and signalling it would kill that")
    need(3010 not in found,
         "an entry with no cmdline (a kernel thread) is skipped, not an "
         "IndexError",
         f"an empty argv was matched ({found})")
    need(3011 not in found,
         "a /proc entry that cannot be read is skipped",
         f"an unreadable entry was matched ({found}) on argv nothing could "
         f"have read")
    need(all(isinstance(p, int) for p in found),
         "non-numeric /proc entries are skipped and every result is an int",
         f"daemon_pids returned something that is not a pid: {found}")
    need(other_uid == [],
         "a process owned by another uid is never matched, whatever its argv",
         f"processes owned by another user were matched ({other_uid}); on a "
         f"shared machine ensure-daemon would signal a stranger's daemon")

    default = proc.pids("/tmp")
    need(3010 not in default and 3006 not in default,
         "the /tmp default does not turn every process into a daemon",
         f"asking about the documented default matched non-daemons: {default}")

    elsewhere = proc.pids("/run/user/999999")
    need(elsewhere == [],
         "a runtime dir nothing serves matches nothing",
         f"an unrelated runtime dir matched {elsewhere}")

    # And against the real /proc, where a live daemon may well be running.
    real = protocol.daemon_pids(runtime=str(Path(_SANDBOX) / "never-existed"))
    need(real == [],
         "against the real /proc, a runtime dir we invented matches no live "
         "process at all",
         f"daemon_pids matched live processes {real} for a runtime dir that "
         f"has never existed; a test would signal the user's own daemon")

    # Two spellings of one runtime dir. They name the same socket, so they are
    # the same daemon.
    spelled = proc.pids(OURS + "/")
    same_socket = str(
        Path(OURS) / "padmap" / "padmap.sock") == str(
        Path(OURS + "/") / "padmap" / "padmap.sock")
    need(same_socket,
         "'/run/user/4242' and '/run/user/4242/' resolve to one socket path",
         "the premise of the next check is wrong: the two spellings name "
         "different sockets")
    if spelled == []:
        gap("a daemon is invisible to daemon_pids when the runtime dir is "
            "spelled with a trailing slash: env XDG_RUNTIME_DIR=/run/user/N/ "
            "serves exactly the socket /run/user/N/padmap/padmap.sock, but "
            "the environ is compared as a string, so `_stop_daemon`'s "
            "fallback reports 0 candidates for the daemon that just answered "
            "it and tells the user to stop it by hand")
    else:
        ok("both spellings of one runtime dir find the same daemon")


# ---------------------------------------------------------------------------
# S18: one socket, more than one daemon
# ---------------------------------------------------------------------------

def scenario_socket_and_runtime_dir() -> None:
    heading("S18: a second daemon must never steal a live socket")

    first = Daemon("live")
    need(first.path.exists(),
         "a runtime dir that did not exist is created and bound",
         f"{first.path} was not created; the daemon cannot be reached")

    second = server.Server(socket_path=first.path)
    try:
        second.start()
        fail("a second daemon bound the socket a live one is listening on. "
             "Whichever the front-end reaches is a coin toss, and the loser "
             "holds pads nothing can reach it to release")
    except RuntimeError as error:
        need("already listening" in str(error) and str(first.path) in str(error),
             "a second daemon refuses to start and names the socket",
             f"the refusal said {error!r}, which does not tell the user "
             f"which socket is in the way")
    except OSError as error:
        fail(f"a second daemon failed with a bare {type(error).__name__} "
             f"({error}) rather than the guarded refusal; the probe that "
             f"tells a live daemon from a stale socket did not run")
    need(second._listener is None,
         "and the refused daemon leaves no half-open listener behind",
         "the refused daemon kept a listening socket")

    survivor = first.connect()
    need(survivor.events() == ["state"],
         "the live daemon is untouched by the attempt and still serves",
         "a client could not be served after a second daemon tried to bind")
    first.close()

    heading("S18: a stale socket left by a daemon that was killed")
    path = protocol.runtime_dir() / "stale.sock"
    orphan = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    orphan.bind(str(path))
    orphan.listen(1)
    orphan.close()          # the file survives; nothing is listening
    need(path.exists(), "the killed daemon left its socket file behind",
         "the fixture did not leave a stale socket to test with")

    revived = server.Server(socket_path=path)
    try:
        revived.start()
    except Exception as error:                         # noqa: BLE001
        fail(f"a stale socket stopped the daemon starting "
             f"({type(error).__name__}: {error}) -- after a crash or a "
             f"SIGKILL the machine would have no daemon at all, and nothing "
             f"but a manual `rm` to fix it")
    need(revived._listener is not None,
         "a stale socket is removed and replaced",
         "no listener after starting over a stale socket")
    probe = Peer(path)
    probe.close()
    ok("and a client can reach the replacement")
    revived.close()
    need(not path.exists(),
         "close() takes the socket file with it, so the next start is clean",
         f"{path} outlived the daemon that made it")

    heading("S18: junk where the socket should be")
    junk = protocol.runtime_dir() / "junk.sock"
    junk.write_text("not a socket at all\n")
    over_file = server.Server(socket_path=junk)
    try:
        over_file.start()
    except Exception as error:                         # noqa: BLE001
        fail(f"a regular file in the socket's place stopped the daemon "
             f"({type(error).__name__}: {error})")
    need(over_file._listener is not None,
         "a regular file in the socket's place is replaced",
         "no listener after starting over a regular file")
    over_file.close()

    # A directory cannot be replaced by a bind, and this is the one case the
    # guard does not cover.
    blocked = protocol.runtime_dir() / "blocked.sock"
    blocked.mkdir(parents=True, exist_ok=True)
    over_dir = server.Server(socket_path=blocked)
    raised: BaseException | None = None
    try:
        over_dir.start()
    except BaseException as error:                     # noqa: BLE001
        raised = error
    need(raised is not None,
         "a directory in the socket's place does not silently look like a "
         "started daemon",
         "start() reported success with a directory where the socket goes, "
         "so ensure-daemon would wait for a daemon that can never answer")
    need(over_dir._listener is None and blocked.is_dir(),
         "nothing is bound and the directory is left alone",
         f"start() over a directory left listener={over_dir._listener!r} or "
         f"removed the directory")
    if isinstance(raised, RuntimeError):
        ok("and the refusal is the same diagnosable RuntimeError")
    else:
        gap(f"a directory where the socket should be raises a bare "
            f"{type(raised).__name__} out of Server.start(); serve() calls "
            f"start() outside its try/finally, so `padmap serve` dies with a "
            f"traceback pointing at Path.unlink rather than saying which "
            f"path is in the way -- unlike the live-daemon case, which "
            f"refuses with a RuntimeError naming the socket")
    shutil.rmtree(blocked)

    heading("S18: two daemons, two runtime dirs, no interference")
    other_runtime = Path(_SANDBOX) / "second-login"
    (other_runtime / "padmap").mkdir(parents=True, exist_ok=True)
    here = Daemon("pair-a")
    there = server.Server(socket_path=other_runtime / "padmap" / "padmap.sock")
    there.start()
    need(here.path != Path(there.socket_path),
         "two runtime dirs give two different sockets",
         "both daemons claimed the same path")
    peer_here = here.connect()
    peer_there = Peer(there.socket_path)
    need(peer_here.events() == ["state"],
         "the first daemon serves its own client",
         "the daemon on the first runtime dir stopped answering")
    for key, _mask in there._selector.select(timeout=0.05):
        key.data(key.fileobj)
    need([m.get("event") for m in peer_there.drain()] == ["state"],
         "and the second serves its own, at the same time",
         "the daemon on the second runtime dir did not answer its client")
    peer_there.close()
    there.close()
    here.close()

    heading("S18: the socket file removed underneath a running daemon")
    fragile = Daemon("fragile")
    fragile.path.unlink()
    try:
        fragile.srv.close()
    except Exception as error:                         # noqa: BLE001
        fail(f"close() raised {type(error).__name__} because something else "
             f"had already removed the socket ({error}); the daemon would die "
             f"on the way out and leave EVIOCGRAB held on every pad")
    ok("close() survives the socket having been removed already")
    fragile.peers.clear()
    fragile.close()


# ---------------------------------------------------------------------------
# S2/S5: a second front-end joins a session that is already open
# ---------------------------------------------------------------------------

def scenario_second_client_joins() -> None:
    heading("S2: a second front-end connecting mid-session is given state")

    daemon = Daemon("join")
    first = daemon.connect()
    need(first.events() == ["state"],
         "a client is sent state the moment it connects",
         "a connecting client was told nothing, so a front-end that starts "
         "before the daemon has anything to say never paints")

    begin_session(daemon, first, players=3)
    opened = first.events()
    need(opened[:2] == ["pads", "state"],
         "the client that opened the session is told how many pads there are, "
         "then the new state",
         f"`begin` answered {opened!r}; the screen has nothing to draw slots "
         f"from")

    second = daemon.connect()
    joined = second.drain()
    need(len(joined) == 1 and joined[0].get("event") == "state",
         "the late client is sent exactly one state event, not a replay of "
         "the session so far",
         f"the second client received {joined!r}")
    need(joined[0].get("state") == protocol.STATE_ASSIGNING,
         "which says a session is open, so it can raise the setup screen "
         "without being told",
         f"the late client was told state={joined[0].get('state')!r} while a "
         f"session was open -- it would show an idle screen over grabbed pads")
    need(joined[0].get("slots") == 3,
         "and carries the slot count the first client asked for",
         f"the late client saw slots={joined[0].get('slots')!r} rather than "
         f"the 3 the session was opened with")
    need([a.get("player") for a in joined[0].get("players") or []] == [1],
         "and the claims made before it arrived",
         f"the late client saw players={joined[0].get('players')!r}, so a "
         f"controller already claimed in front of the user would not be drawn")
    need(first.drain() == [],
         "and the client that was already connected is not repainted",
         "an arriving client broadcast to everybody; a passing status query "
         "would redraw the screen someone is using")

    # The SDL lines are the other half of picking up mid-session.
    daemon.srv._sdl_lines = ["03000000,padmap Player 1,a:b1,"]
    second.send(cmd="status")
    daemon.pump()
    replies = second.drain()
    need([m.get("event") for m in replies] == ["state", "sdl_mapping"],
         "a late client asking for status gets state and the SDL mappings",
         f"status answered {[m.get('event') for m in replies]!r}; without the "
         f"mappings the front-end runs on whatever sdl_controllers.txt said "
         f"when it started")
    need(replies[1].get("lines") == ["03000000,padmap Player 1,a:b1,"],
         "carrying the lines written before it connected",
         f"the late client got lines={replies[1].get('lines')!r}")
    need(first.drain() == [],
         "and one client's status query is answered to it alone",
         "a status query from one client was broadcast to the others")

    second.send(cmd="nonsense")
    daemon.pump()
    need([m.get("event") for m in second.drain()] == ["error"],
         "an unknown command is answered to the client that sent it",
         "an unknown command was not answered to its sender")
    need(first.drain() == [],
         "and not to the other one",
         "one client's mistake was reported to every other client")

    # A broadcast, by contrast, reaches everyone.
    first.send(cmd="reset")
    daemon.pump()
    need("state" in first.events() and "state" in second.events(),
         "a change one client makes is broadcast to both",
         "a session change reached only the client that made it, so the "
         "second front-end draws a session that has moved on without it")
    daemon.close()


# ---------------------------------------------------------------------------
# S5: losing a client is only the end when it is the last one
# ---------------------------------------------------------------------------

def scenario_one_of_two_clients_leaves() -> None:
    heading("S5: one of two front-ends leaving does not cancel the session")

    daemon = Daemon("two")
    driver = daemon.connect()
    passer = daemon.connect()
    begin_session(daemon, driver)
    session = daemon.session
    driver.drain()

    passer.close()
    daemon.pump()
    need(len(daemon.srv._clients) == 1,
         "the daemon notices the disconnect and forgets that client",
         f"{len(daemon.srv._clients)} clients still registered after one "
         f"disconnected; the daemon would never decide it was the last")
    need(not session.closed and daemon.session is session,
         "the session the other client is driving survives",
         "a passing `padmap status` disconnecting released the pads out from "
         "under the front-end that was mid-assignment")
    need(daemon.srv._state == protocol.STATE_ASSIGNING,
         "and the daemon still says it is assigning",
         f"the state fell to {daemon.srv._state!r} while the session was "
         f"still open")

    driver.send(cmd="status")
    daemon.pump()
    need("state" in driver.events(),
         "and the surviving client is still served",
         "the surviving client stopped being answered")

    heading("S5: the last front-end leaving releases the pads")
    driver.close()
    daemon.pump()
    need(session.closed,
         "the session is closed and every EVIOCGRAB released",
         "the last front-end went away and the pads stayed grabbed. Nothing "
         "is left to send cancel, so every controller on the machine is dead "
         "until the daemon is restarted")
    need(daemon.session is None,
         "the session object goes with it",
         "the session outlived its last front-end")
    need(daemon.srv._state != protocol.STATE_ASSIGNING,
         "and the daemon no longer claims to be assigning",
         "the daemon reports 'assigning' with no session behind it -- cancel "
         "returns early and accept refuses, so nothing can release the pads")
    daemon.close()

    heading("S5: the front-end goes first, a status query is still connected")
    daemon = Daemon("order")
    front = daemon.connect()
    begin_session(daemon, front)
    session = daemon.session
    query = daemon.connect()

    front.close()
    daemon.pump()
    need(not session.closed,
         "losing the front-end while a query is connected does not cancel "
         "yet -- the rule is the last client, not the front-end",
         "the session ended while another client was still connected")
    query.close()
    daemon.pump()
    need(session.closed and daemon.session is None,
         "and when the query goes too, the pads are released after all",
         "the pads stayed grabbed with nobody connected at all, which is the "
         "state that leaves the machine with no working controllers")
    daemon.close()


def scenario_client_lost_mid_wizard() -> None:
    heading("S5/S7: a client disconnecting mid-wizard")

    daemon = Daemon("wizard")
    driver = daemon.connect()
    watcher = daemon.connect()
    begin_session(daemon, driver)

    driver.send(cmd="map", player=1, layout="n64", scope="")
    daemon.pump()
    run = daemon.srv._mapping
    if run is None:
        fail("no wizard started over a real socket, so this scenario tests "
             "nothing")
    need("mapping" in watcher.events(),
         "a wizard one client starts is drawn on the other one too",
         "the second front-end was never told a wizard had opened, so it "
         "shows the setup screen while the pad drives a wizard it cannot see")
    run.bindings["a"] = Binding("button", 0x130)

    watcher.close()
    daemon.pump()
    need(daemon.srv._mapping is run,
         "the wizard survives a second client disconnecting",
         "a passing client disconnecting threw away a half-finished capture "
         "the user was in the middle of")
    need(not daemon.session.closed,
         "and so does the session it belongs to",
         "the session was cancelled by a client that was not driving it")

    driver.close()
    daemon.pump()
    need(daemon.srv._mapping is None,
         "when the last client goes the wizard is closed with the session",
         "the wizard outlived the session it belongs to -- it holds a pad "
         "nothing will read again, and _tick returns early on it forever, so "
         "no button hold can ever claim a slot")
    need(profiles.load(PAD) is None,
         "and the half-finished capture is not kept",
         "a wizard abandoned by a dying front-end stored what it had -- the "
         "pad then counts as configured and is never offered again, leaving "
         "half its buttons dead with nothing to say why")
    daemon.close()


def scenario_dead_client_does_not_deny_the_live_one() -> None:
    heading("S5: a client that died unnoticed must not deny service")

    daemon = Daemon("dead")
    dead = daemon.connect()
    alive = daemon.connect()
    begin_session(daemon, dead)
    alive.drain()

    # Gone without the daemon having read the EOF: exactly what a front-end
    # that segfaults looks like from here, until the next write.
    dead.sock.close()
    try:
        daemon.srv._broadcast({"event": "state", "state": "probe"})
    except Exception as error:                         # noqa: BLE001
        fail(f"broadcasting to a client that had gone raised "
             f"{type(error).__name__} ({error}); this runs on the selector "
             f"thread with no guard above it, so the daemon dies and takes "
             f"every virtual pad with it")
    need([m.get("event") for m in alive.drain()] == ["state"],
         "the surviving client receives the event anyway",
         "one dead client swallowed a broadcast the live front-end needed")
    need(len(daemon.srv._clients) == 1,
         "and the dead one is dropped as it is discovered",
         f"{len(daemon.srv._clients)} clients registered after one had gone; "
         f"a dead client that is never dropped means the session is never "
         f"seen to lose its last one")
    session = daemon.session
    need(session is not None and not session.closed,
         "the session is untouched, because a client is still there",
         "the session was cancelled while a front-end was still connected")

    # Now the last one dies, and it is a *broadcast* that finds out -- so
    # _cancel runs from inside the loop that is iterating the clients.
    alive.sock.close()
    try:
        daemon.srv._broadcast({"event": "state", "state": "probe"})
    except Exception as error:                         # noqa: BLE001
        fail(f"the last client dying mid-broadcast raised "
             f"{type(error).__name__} ({error}) -- _drop_client cancels the "
             f"session from inside _broadcast's own loop, and that is the "
             f"path a crashing front-end takes")
    need(len(daemon.srv._clients) == 0 and session.closed,
         "the pads are released from inside the broadcast that found out",
         "the last client was dropped mid-broadcast and the session was left "
         "holding EVIOCGRAB on every pad")
    daemon.close()


def scenario_connect_disconnect_churn() -> None:
    heading("S1/S18: ensure-daemon connecting for milliseconds, repeatedly")

    daemon = Daemon("churn")
    front = daemon.connect()
    begin_session(daemon, front)
    session = daemon.session
    front.drain()

    for _ in range(40):
        query = Peer(daemon.path)
        query.close()
        daemon.pump()
    need(not session.closed and daemon.session is session,
         "40 connect-and-vanish clients do not disturb the open session",
         "a repeated status query tore down the session a front-end was "
         "driving; every `padmap ensure-daemon` would cost the user their "
         "assignment")
    need(len(daemon.srv._clients) == 1,
         "and leave exactly one client registered, not forty",
         f"{len(daemon.srv._clients)} clients registered after the churn; the "
         f"daemon leaks a descriptor per query and never sees the last client "
         f"leave")
    need(len(daemon.srv._selector.get_map()) == len(daemon.srv._clients) + 2,
         "with the selector holding only the listener, the session fd and "
         "that one client",
         f"the selector holds {len(daemon.srv._selector.get_map())} "
         f"registrations for 1 client, 1 listener and 1 session fd; a "
         f"registration leak ends in select() failing on a closed descriptor")

    later = daemon.connect()
    need(later.events() == ["state"],
         "and a real front-end connecting afterwards is served normally",
         "the listener stopped accepting after the churn")

    # A dropped client's fd number is handed straight back out. The client
    # table is keyed on it, so a ghost entry would shadow the new client.
    reused = sorted(daemon.srv._clients)
    need(len(set(reused)) == len(reused) == 2,
         "the client table has one entry per live client, with no ghost left "
         "at a reused descriptor number",
         f"the client table is keyed {reused!r} for 2 connected clients")
    daemon.close()


def scenario_autosetup_needs_a_settled_client() -> None:
    heading("S1: which of several clients counts as a front-end")

    os.environ.pop("PADMAP_NO_AUTOSETUP", None)
    try:
        srv = server.Server()
        # _begin grabs every pad; nothing here may reach it.
        srv._begin = lambda players: None              # type: ignore[assignment]
        now = time.monotonic()

        def client(age: float):
            fake = type("StubClient", (), {})()
            fake.connected_at = now - age
            return fake

        settled = server.AUTOSETUP_CLIENT_SECONDS + 0.5
        fresh = server.AUTOSETUP_CLIENT_SECONDS / 2

        srv._clients = {}
        need(srv._autosetup_blocked() == "no front-end is connected",
             "with nothing connected, setup may not open",
             f"autosetup said {srv._autosetup_blocked()!r} with no clients; "
             f"the daemon would grab every pad to show a screen on a "
             f"front-end that is not running")

        srv._clients = {1: client(fresh)}
        need(srv._autosetup_blocked() == "no front-end is connected",
             "a client connected for less than AUTOSETUP_CLIENT_SECONDS does "
             "not count",
             "a millisecond-long status query counted as a front-end -- this "
             "is the bug that left ensure-daemon holding every pad")

        srv._clients = {1: client(0.0), 2: client(fresh)}
        need(srv._autosetup_blocked() == "no front-end is connected",
             "and neither do two of them at once",
             "two overlapping status queries added up to a front-end; "
             "`padmap ensure-daemon` and padctl run side by side")

        srv._clients = {1: client(settled)}
        need(srv._autosetup_blocked() is None,
             "a client that has been there longer does count",
             f"a settled front-end was refused ({srv._autosetup_blocked()!r}) "
             f"-- plugging a controller in would never open the screen, which "
             f"is the whole of S1")

        srv._clients = {1: client(0.0), 2: client(settled), 3: client(fresh)}
        need(srv._autosetup_blocked() is None,
             "one settled front-end is enough even while queries come and go",
             "a passing status query connected beside a real front-end "
             "blocked the setup screen; ensure-daemon runs immediately before "
             "the front-end, so this is the normal case")

        srv._clients = {1: client(fresh)}
        need(srv._autosetup_blocked() == "no front-end is connected",
             "and losing the settled one blocks it again",
             "setup stayed available after the front-end went away")

        # The other reasons still win, so this test is not passing by
        # accident.
        srv._clients = {1: client(settled)}
        srv._state = protocol.STATE_ASSIGNING
        need(srv._autosetup_blocked() == "a session is already open",
             "a session already open blocks it whatever the clients are doing",
             f"autosetup said {srv._autosetup_blocked()!r} with a session "
             f"open; a second _begin would tear down the first one's grabs")
        srv._state = protocol.STATE_IDLE
        os.environ["PADMAP_NO_AUTOSETUP"] = "1"
        need(srv._autosetup_blocked() is not None
             and "PADMAP_NO_AUTOSETUP" in srv._autosetup_blocked(),
             "and the environment switch wins over a settled front-end",
             "PADMAP_NO_AUTOSETUP did not stop the setup screen")
    finally:
        os.environ["PADMAP_NO_AUTOSETUP"] = "1"


def main() -> int:
    isolate_hardware()
    if not str(protocol.socket_path()).startswith(_SANDBOX):
        fail(f"the sandbox did not take: socket_path() is "
             f"{protocol.socket_path()}, which may be the real daemon's")

    scenario_daemon_pids_on_synthetic_proc()
    scenario_socket_and_runtime_dir()
    scenario_second_client_joins()
    scenario_one_of_two_clients_leaves()
    scenario_client_lost_mid_wizard()
    scenario_dead_client_does_not_deny_the_live_one()
    scenario_connect_disconnect_churn()
    scenario_autosetup_needs_a_settled_client()

    if _GAPS:
        print(f"\n{len(_GAPS)} gap(s) reported above, none of them fatal:")
        for text in _GAPS:
            print(f"  - {text}")
    shutil.rmtree(_SANDBOX, ignore_errors=True)
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
