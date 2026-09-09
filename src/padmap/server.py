"""The padmap daemon.

One process owning everything with a lifetime longer than a single command:
the assignment session, the virtual pads, and the generated RetroArch launch
config. Front-ends connect over a unix socket and drive it (see protocol.py).

Why a daemon rather than a per-launch command: the virtual pads must exist
continuously. If they came and went per game, RetroArch would see a different
set of devices each launch and pad indices would move underneath it -- which
is the exact failure this project exists to fix.

Single-threaded, one selector loop over three kinds of descriptor:

  * the listening socket, plus each connected client
  * the physical pads, while a session is assigning
  * the physical pads and our uinput nodes, while republishing

A periodic tick drives hold timers, which nothing else would notice: a held
button emits no further events.
"""

from __future__ import annotations

import errno
import json
import logging
import os
import selectors
import signal
import socket
import time
from pathlib import Path
from typing import Any

from evdev import ecodes

from . import (calibrate, capture, controllercfg, devices, icons, layouts,
               profiles, protocol, retroarch, virtual)
from .assign import Assigner, Assignment
from .devices import Pad
from .protocol import STATE_ASSIGNING, STATE_IDLE, STATE_READY, LineReader

log = logging.getLogger("padmap.server")

TICK_SECONDS = 0.02

# How often to look for a controller model that has never been set up. Well
# above the tick rate: discover() globs /sys/class/input and reads capability
# bits, which is cheap but not free, and a second's delay before the setup
# screen appears is imperceptible next to plugging a cable in.
PAD_SCAN_SECONDS = 1.0

# Set to "1" to keep the setup screen from opening by itself.
ENV_NO_AUTOSETUP = "PADMAP_NO_AUTOSETUP"

# How long a client must have been connected before the daemon will grab the
# pads on its behalf. Comfortably longer than a status query, which is what
# `padmap ensure-daemon` and padctl do, and far shorter than a front-end's
# lifetime.
AUTOSETUP_CLIENT_SECONDS = 3.0
# Longer than the claim hold: confirming ends the session, so it should take a
# deliberate press rather than the same flick that claims a slot.
CONFIRM_HOLD_SECONDS = 0.7


# Configuration is a modal flow the *daemon* owns, not the front-end. While it
# runs, claim detection and the confirm gesture are both suspended: the user is
# pressing buttons to drive the wizard, and those must not also claim slots or
# end the session. It stays modal through the icon step too -- that was the
# "button press passed through to the confirmation underneath" bug.
PHASE_AWAIT_REST = "await_rest"
PHASE_REST = "rest"
PHASE_AWAIT_REACH = "await_reach"
PHASE_REACH = "reach"
PHASE_ICON = "icon"

# Only the sampling phases are timed. The await phases wait for a button, so
# the user sets the pace and is never measured before they are ready.
PHASE_SECONDS = {PHASE_REST: 0.8}

# The reach phase ends on a button press, but not before this -- otherwise the
# same press that started it could end it immediately.
REACH_MINIMUM_SECONDS = 1.2


class Client:
    def __init__(self, sock: socket.socket) -> None:
        self.sock = sock
        self.reader = LineReader()
        # Used to tell a front-end from a passing query -- see
        # AUTOSETUP_CLIENT_SECONDS.
        self.connected_at = time.monotonic()


class CalibrationRun:
    """A calibration in flight, advanced from the daemon's tick.

    Sampling cannot call calibrate.sample_rest/sample_reach here: those own a
    loop and block for up to five seconds, which would stall every other pad
    and every connected client. This collects the same samples incrementally
    instead, fed by the assigner's raw event stream.
    """

    def __init__(self, pad: Pad, player: int, axes: dict[int, Any]) -> None:
        self.pad = pad
        self.player = player
        self.axes = axes
        self.phase = PHASE_AWAIT_REST
        self.started = time.monotonic()
        self.seen: dict[int, list[int]] = {}
        self.rest: dict[int, Any] | None = None
        # Set when a button goes down during an await phase.
        self.advance_requested = False
        self._reset_samples()

    def _reset_samples(self) -> None:
        # Each phase starts a fresh window: reach must not inherit the rest
        # phase's samples, or an axis that never moved would look like it
        # spanned nothing.
        self.seen = {
            code: [info.value, info.value] for code, info in self.axes.items()
        }

    @property
    def elapsed(self) -> float:
        return time.monotonic() - self.started

    def fraction(self) -> float:
        """Progress 0..1, meaning whatever the current phase measures."""
        if self.phase == PHASE_REST:
            return min(self.elapsed / PHASE_SECONDS[PHASE_REST], 1.0)
        if self.phase == PHASE_REACH:
            return self.coverage()
        return 0.0

    def coverage(self) -> float:
        """How much of each axis's declared travel has been swept so far.

        Better feedback than a countdown: it tells the user whether the circles
        they are making are actually reaching the edges, which is the thing
        that determines whether the calibration is any good.
        """
        if not self.axes:
            return 0.0
        fractions = []
        for code, info in self.axes.items():
            low, high = self.seen.get(code, [info.value, info.value])
            declared = info.max - info.min
            fractions.append((high - low) / declared if declared else 0.0)
        return min(sum(fractions) / len(fractions), 1.0)

    def feed(self, event: Any) -> None:
        if event.type == ecodes.EV_KEY and event.value == 1:
            self.advance_requested = True
            return
        if event.type != ecodes.EV_ABS or event.code not in self.seen:
            return
        low, high = self.seen[event.code]
        self.seen[event.code] = [min(low, event.value), max(high, event.value)]

    def begin_phase(self, phase: str) -> None:
        self.phase = phase
        self.started = time.monotonic()
        self.advance_requested = False
        self._reset_samples()


def _event_nodes() -> frozenset[str]:
    """Names of the evdev nodes that exist right now.

    A directory listing, deliberately: the question is only "has anything
    appeared or gone away", and answering it must not cost what answering
    "what exactly is out there" costs.
    """
    try:
        return frozenset(
            name for name in os.listdir("/dev/input")
            if name.startswith("event")
        )
    except OSError:
        return frozenset()


class Server:
    def __init__(
        self,
        socket_path: Path | None = None,
        state_path: Path | None = None,
        launch_config_path: Path | None = None,
    ) -> None:
        runtime = protocol.runtime_dir()
        self.socket_path = socket_path or protocol.socket_path()
        self.state_path = state_path or (runtime / "assignments.json")
        self.launch_config_path = launch_config_path or (runtime / "launch.cfg")
        # Flags that go with it. Kept beside the config rather than in it,
        # because emptying an unassigned core port is not expressible as a
        # config setting -- see retroarch.launch_args.
        self.launch_args_path = self.launch_config_path.with_suffix(".args")

        self._selector = selectors.DefaultSelector()
        self._listener: socket.socket | None = None
        self._clients: dict[int, Client] = {}

        self._state = STATE_IDLE
        self._assigner: Assigner | None = None
        self._assignments: list[Assignment] = []
        self._republisher: virtual.Republisher | None = None
        self._confirm_started: dict[str, float] = {}
        self._last_progress = 0.0
        self._last_confirm = 0.0
        self._slots = 4
        # Read once and reused: _players_payload runs on every state change,
        # and re-reading the config file each time would be wasteful.
        self._icon_overrides = icons.load_overrides()
        self._calibration: CalibrationRun | None = None
        # Button mapping in flight. Modal in the same way calibration is:
        # the user is pressing buttons to answer prompts, and those must not
        # also claim slots or end the session.
        self._mapping: capture.MappingRun | None = None
        # Which console the user says this pad is. Modal like the two above,
        # and for the same reason: it is answered with the pad itself.
        self._choice: capture.Chooser | None = None
        # Which scope the wizard that a picker is about to start will file
        # its capture under. Set when the scope picker is answered and read
        # when the mapping finishes; "" is this controller's default, which
        # is what every flow that never asks produces.
        self._pending_scope = ""
        # The SDL lines last written, so a front-end connecting later can be
        # handed them without the daemon recomputing an assignment it may no
        # longer have.
        self._sdl_lines: list[str] = []
        self._running = False

        # Controller models already offered a setup screen. Keyed by profile
        # signature, so the four ports of one adapter prompt once, and so
        # declining does not re-prompt a second later.
        #
        # Persisted, because the daemon is now restarted on every front-end
        # launch to pick up code changes -- keeping this in memory would turn
        # "asked once" into "asked every single time you start Pegasus" for
        # any controller the user chose not to configure. It lives in
        # XDG_RUNTIME_DIR, so declining lasts for the login session and a
        # fresh boot offers again.
        self.prompted_path = self.launch_config_path.parent / "prompted"
        self._prompted: set[str] = self._load_prompted()
        # (event nodes, prompted mtime) at the last full scan; see
        # _poll_new_controllers. None until the first one has run.
        self._last_scan_signature: tuple[frozenset[str], int] | None = None
        self._prompted_stamp = self._read_prompted_stamp()
        self._last_pad_scan = 0.0

    # -- lifecycle --------------------------------------------------------

    def start(self) -> None:
        self.socket_path.parent.mkdir(parents=True, exist_ok=True)
        # A socket left by a crashed daemon would make bind() fail with
        # EADDRINUSE even though nothing is listening. Probe before removing,
        # so we refuse to steal the socket from a daemon that *is* alive.
        if self.socket_path.exists():
            if self._daemon_alive():
                raise RuntimeError(
                    f"another padmap daemon is already listening on "
                    f"{self.socket_path}"
                )
            self.socket_path.unlink()

        self._listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._listener.bind(str(self.socket_path))
        self._listener.listen(8)
        self._listener.setblocking(False)
        self._selector.register(
            self._listener, selectors.EVENT_READ, self._on_accept
        )
        log.info("listening on %s", self.socket_path)

    def _daemon_alive(self) -> bool:
        probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            probe.settimeout(0.2)
            probe.connect(str(self.socket_path))
            return True
        except OSError:
            return False
        finally:
            probe.close()

    def run(self) -> None:
        self._running = True
        while self._running:
            for key, _mask in self._selector.select(timeout=TICK_SECONDS):
                handler = key.data
                handler(key.fileobj)
            self._tick()

    def stop(self) -> None:
        self._running = False

    def close(self) -> None:
        self._end_session(release=True)
        self._stop_republisher()
        for client in list(self._clients.values()):
            self._drop_client(client.sock)
        if self._listener is not None:
            self._selector.unregister(self._listener)
            self._listener.close()
            self._listener = None
        try:
            self.socket_path.unlink()
        except OSError:
            pass
        self._selector.close()

    # -- socket -----------------------------------------------------------

    def _on_accept(self, listener: Any) -> None:
        try:
            sock, _ = listener.accept()
        except OSError:
            return
        sock.setblocking(False)
        client = Client(sock)
        self._clients[sock.fileno()] = client
        self._selector.register(sock, selectors.EVENT_READ, self._on_client_read)
        log.info("client connected (%d total)", len(self._clients))
        self._send(client, self._state_event())

    def _on_client_read(self, sock: Any) -> None:
        client = self._clients.get(sock.fileno())
        if client is None:
            return
        try:
            data = sock.recv(65536)
        except OSError:
            data = b""
        if not data:
            self._drop_client(sock)
            return
        for message in client.reader.feed(data):
            self._handle_command(client, message)

    def _drop_client(self, sock: Any) -> None:
        self._clients.pop(sock.fileno(), None)
        try:
            self._selector.unregister(sock)
        except (KeyError, ValueError):
            pass
        sock.close()

        # An assignment session holds EVIOCGRAB on every pad. If the front-end
        # that opened it goes away -- crashed, killed, or simply quit without
        # sending cancel -- nothing would ever release those grabs, and every
        # controller on the machine stays dead until the daemon is restarted.
        # Nobody is left to drive the session anyway, so end it.
        if not self._clients and self._assigner is not None:
            log.info("last client disconnected mid-session; releasing pads")
            self._cancel()

    def _send(self, client: Client, message: dict[str, Any]) -> None:
        try:
            client.sock.sendall(protocol.encode(message))
        except OSError as exc:
            if exc.errno not in (errno.EPIPE, errno.ECONNRESET):
                log.debug("send failed: %s", exc)
            self._drop_client(client.sock)

    def _broadcast(self, message: dict[str, Any]) -> None:
        for client in list(self._clients.values()):
            self._send(client, message)

    # -- commands ---------------------------------------------------------

    def _handle_command(self, client: Client, message: dict[str, Any]) -> None:
        """Dispatch one command, surviving anything the client sends.

        Every argument below arrives over a socket from a separate program,
        and several are coerced with int(). A front-end sending
        {"cmd": "begin", "players": "lots"} raised ValueError straight through
        _on_client_read and the selector loop, out of serve(), and the daemon
        *exited* -- taking every virtual pad with it, so the machine had no
        controllers at all until something restarted it. A daemon must not be
        killable by the thing it exists to serve.

        Broad on purpose. The point is not to enumerate the ways a message can
        be wrong, it is that no message may end the process.
        """
        try:
            self._dispatch(client, message)
        except Exception as error:                      # noqa: BLE001
            log.warning("command %r failed: %s: %s",
                        message.get("cmd"), type(error).__name__, error)
            reply = {"event": "error",
                     "message": f"{message.get('cmd')!r} failed: {error}"}
            if client is not None:
                self._send(client, reply)
            else:
                self._broadcast(reply)

    def _dispatch(self, client: Client, message: dict[str, Any]) -> None:
        command = message.get("cmd")
        if command == "begin":
            self._begin(int(message.get("players", 4)))
        elif command == "reset":
            self._reset()
        elif command == "accept":
            self._accept()
        elif command == "cancel":
            self._cancel()
        elif command == "map":
            self._begin_mapping(
                int(message.get("player", 0)), str(message.get("layout", "")),
                scope=str(message.get("scope", "")))
        elif command == "choose_layout":
            self._begin_layout_choice(int(message.get("player", 0)))
        elif command == "choose_scope":
            self._begin_scope_choice(int(message.get("player", 0)))
        elif command == "map_for_game":
            self._begin_game_scope_choice(
                int(message.get("player", 0)), str(message.get("console", "")),
                str(message.get("key", "")), str(message.get("title", "")))
        elif command == "forget_pad":
            self._forget_pad(int(message.get("player", 0)))
        elif command == "skip_control":
            self._skip_control()
        elif command == "calibrate":
            self._begin_calibration(int(message.get("player", 0)))
        elif command == "configure_end":
            # Escape hatch: leave the modal flow without finishing it, e.g.
            # the user cancelled or the overlay was dismissed.
            if self._calibration is not None:
                self._finish_calibration()
            if self._choice is not None:
                self._end_layout_choice()
            if self._mapping is not None:
                self._finish_mapping(store=False)
        elif command == "set_icon":
            self._set_icon(
                int(message.get("player", 0)), str(message.get("icon", ""))
            )
        elif command == "status":
            self._send(client, self._state_event())
            # Every connect, not only after a capture. Pegasus reads
            # sdl_controllers.txt once at startup, so a front-end that
            # started before the daemon last wrote it -- or reconnected after
            # a daemon restart -- is running on whatever the file said at the
            # time. Re-sending is idempotent: SDL replaces a mapping for a
            # GUID it already has.
            self._send(client, self._sdl_mapping_event())
        else:
            self._send(client, {"event": "error",
                                "message": f"unknown command {command!r}"})

    def _begin(self, players: int) -> None:
        # Republishing grabs the physical pads, so it has to stop before the
        # assigner can open them; otherwise every press would be invisible.
        self._stop_republisher()
        self._end_session(release=True)

        pads = devices.discover()
        if not pads:
            self._broadcast({"event": "error", "message": "no joypads found"})
            return

        # Re-read here so an icon correction takes effect on the next setup
        # rather than requiring the daemon to be restarted.
        self._icon_overrides = icons.load_overrides()

        self._assigner = Assigner(pads, grab=True)
        self._assigner.__enter__()
        self._assigner.on_claimed_event = self._on_claimed_event
        # Calibration needs the EV_ABS stream, which claim detection ignores.
        self._assigner.on_raw_event = self._on_raw_event
        self._calibration = None
        self._choice = None
        # And the wizard. Without this a capture in flight survived into the
        # new session holding a pad from the closed one, and _tick returns
        # early while a mapping is open -- so for the whole of the next
        # session no hold could claim a slot and confirm never fired. The
        # setup screen simply sat there, with nothing logged and no error.
        self._mapping = None
        # `players` is advisory: the session ends on the confirm gesture, not
        # on a count, so a front-end showing 4 slots and a user assigning 2 is
        # a normal outcome rather than an unfinished one.
        self._slots = max(1, players)
        for fd in self._assigner.fds:
            self._selector.register(fd, selectors.EVENT_READ, self._on_pad_read)
        self._confirm_started.clear()
        self._last_progress = 0.0
        self._last_confirm = 0.0
        self._state = STATE_ASSIGNING
        # The session lifecycle was the one thing the log did not record, and
        # it is exactly what a report like "I have to press it twice before
        # anything is assigned" turns on: whether a session opened at all,
        # whether the pads were grabbed, and whether a hold was ever seen.
        # Without these lines the answer to all three is unobtainable after
        # the fact.
        log.info("session open: %d pad(s), %d slot(s), %d already assigned",
                 len(pads), self._slots, len(self._assignments))
        if self._assigner.grab_failures:
            log.warning(
                "session: %d pad(s) not grabbed exclusively (%s) -- presses "
                "also reach the front-end",
                len(self._assigner.grab_failures),
                ", ".join(p.event for p in self._assigner.grab_failures))
        self._broadcast({"event": "pads", "count": len(pads)})
        self._broadcast(self._state_event())

    def _reset(self) -> None:
        if self._assigner is None:
            return
        self._assigner.reset()
        self._confirm_started.clear()
        self._broadcast(self._state_event())

    def _cancel(self) -> None:
        """Abandon an assignment session. Does not stop republishing.

        Only moves to idle if a session was actually open. Cancelling while
        READY used to drop the state to idle while the virtual pads kept
        running, so a front-end would believe nothing was assigned when the
        controllers were in fact live.
        """
        had_session = self._assigner is not None
        if had_session:
            # getattr, because a session object is not always a real Assigner
            # -- the checks stand a stub in its place -- and a log line is
            # never worth raising from a teardown path.
            claims = getattr(self._assigner, "assignments", ())
            log.info("session cancelled after %d claim(s)", len(claims))
        # Close a modal flow properly rather than dropping it. Nothing will
        # read the pad after this, so an overlay left believing it is active
        # would sit waiting for events that cannot arrive -- and the front-end
        # renders it from the daemon's own "still running" flags.
        if self._choice is not None:
            self._end_layout_choice()
        if self._mapping is not None:
            self._finish_mapping(store=False)
        self._end_session(release=True)
        self._calibration = None
        if had_session:
            # Put the previous assignments back on the air. Opening a session
            # stops republishing, so without this, backing out of setup left
            # the machine with no virtual pads at all and nothing but a
            # daemon restart to bring them back. Cheap before the daemon
            # opened setup on its own; now that it does, an unwanted screen
            # would cost the user their controllers for declining it.
            if self._republisher is None and self._assignments:
                try:
                    self._start_republisher()
                except OSError as error:
                    # A pad that went away while the session was open. Idle is
                    # the honest state then, rather than pretending.
                    log.warning("could not resume republishing: %s", error)
            self._state = STATE_READY if self._republisher else STATE_IDLE
        self._broadcast(self._state_event())

    def _accept(self) -> None:
        if self._assigner is None or not self._assigner.assignments:
            self._broadcast({"event": "error",
                             "message": "nothing assigned yet"})
            return

        self._assignments = list(self._assigner.assignments)

        self._end_session(release=True)
        self._save_assignments()
        self._write_controller_configs()
        # Guarded exactly as _cancel guards the same call. A controller
        # unplugged between the last claim and the confirm hold makes
        # virtual.create raise, and the exception escaped _accept *after*
        # _save_assignments had written the state file -- so the front-end
        # never got "accepted" or "error", the setup screen waited forever,
        # and the daemon exited.
        try:
            self._start_republisher()
        except OSError as error:
            log.warning("could not start republishing: %s", error)
            self._broadcast({
                "event": "error",
                "message": f"a controller went away before it could be "
                           f"published: {error}"})
        self._state = STATE_READY if self._republisher else STATE_IDLE
        self._broadcast({
            "event": "accepted",
            "players": self._players_payload(),
            "launch_config": str(self.launch_config_path),
        })
        self._broadcast(self._state_event())

    # -- calibration ------------------------------------------------------

    def _pad_for_player(self, player: int) -> Pad | None:
        """The pad behind a player number, claims first, then what is stored.

        A session's claims win, because during one they are the live answer:
        someone re-assigning slots means player 1 is whatever just pressed a
        button, not whatever held the slot before.

        But a session starts with no claims at all, and the stored assignment
        is still true -- that pad is assigned, republishing, and the thing the
        user means by "player 1". Consulting only the claims made every
        per-player command fail for the first few seconds of a session with
        "no controller assigned to player 1", about a controller that was
        plainly assigned. That is how asking to calibrate came to require
        holding a button first, with nothing saying so.

        Note what this deliberately does *not* change: `_players_payload`
        still reports claims alone, so the setup screen keeps drawing only
        slots claimed in front of the user. Reporting stored assignments there
        is a bug that has already been fixed once -- it offered to configure a
        controller nobody had touched.
        """
        if self._assigner is not None:
            for assignment in self._assigner.assignments:
                if assignment.player == player:
                    return assignment.pad
        for assignment in self._assignments:
            if assignment.player == player:
                return assignment.pad
        return None

    def _begin_calibration(self, player: int) -> None:
        if self._assigner is None:
            self._broadcast({"event": "error",
                             "message": "calibration needs an open session"})
            return
        pad = self._pad_for_player(player)
        if pad is None:
            self._broadcast({"event": "error",
                             "message": f"no controller assigned to player {player}"})
            return

        device = self._assigner.device_for(pad)
        if device is None:
            self._broadcast({"event": "error",
                             "message": "controller is no longer open"})
            return

        axes = calibrate.calibratable_axes(device)
        if not axes:
            # Nothing to centre -- a d-pad-only pad is already correct. Skip
            # straight to the icon step rather than bailing out: the profile
            # still needs writing so this pad counts as configured, and the
            # user should still get to choose how it looks.
            self._store_profile(pad, {})
            self._calibration = CalibrationRun(pad, player, {})
            self._calibration.begin_phase(PHASE_ICON)
            self._confirm_started.clear()
            self._broadcast({
                "event": "calibration", "phase": PHASE_ICON, "frac": 1.0,
                "player": player, "axes": 0, "name": _clean(pad.name),
            })
            return

        self._calibration = CalibrationRun(pad, player, axes)
        # Drop any in-flight confirm hold: the button that opened this is
        # very likely still down, and it must not resume a confirm later.
        self._confirm_started.clear()
        self._emit_calibration(self._calibration)

    def _on_raw_event(self, pad: Pad, event: Any) -> None:
        run = self._calibration
        if run is not None and pad.path == run.pad.path:
            run.feed(event)

        choice = self._choice
        if choice is not None and pad.path == choice.pad.path:
            if choice.feed(event):
                if choice.confirmed:
                    self._choice_confirmed(choice)
                else:
                    self._broadcast(choice.to_event())

        mapping_run = self._mapping
        if mapping_run is not None and pad.path == mapping_run.pad.path:
            if mapping_run.feed(event):
                self._emit_mapping()
                if mapping_run.finished:
                    self._finish_mapping(store=True)

    def _tick_calibration(self) -> None:
        run = self._calibration
        if run is None:
            return

        # Await phases are driven by the user, not the clock.
        if run.phase == PHASE_AWAIT_REST:
            if run.advance_requested:
                run.begin_phase(PHASE_REST)
                self._emit_calibration(run)
            return

        if run.phase == PHASE_AWAIT_REACH:
            if run.advance_requested:
                run.begin_phase(PHASE_REACH)
                self._emit_calibration(run)
            return

        if run.phase == PHASE_ICON:
            # Waiting on set_icon. Still modal, so nothing the user presses
            # while choosing can claim a slot or confirm the session.
            return

        if run.phase == PHASE_REST:
            if run.elapsed < PHASE_SECONDS[PHASE_REST]:
                self._emit_calibration(run)
                return
            run.rest = calibrate.rest_from_samples(run.axes, run.seen)
            run.begin_phase(PHASE_AWAIT_REACH)
            self._emit_calibration(run)
            return

        # PHASE_REACH: ends on a button press, not a timer, so the user
        # decides when the circles are good enough.
        if not (run.advance_requested and run.elapsed >= REACH_MINIMUM_SECONDS):
            self._emit_calibration(run)
            return

        rest = run.rest or calibrate.rest_from_samples(run.axes, run.seen)
        reach = {code: (low, high) for code, (low, high) in run.seen.items()}
        axes = calibrate.merge_reach(rest, reach)
        self._store_profile(run.pad, axes)

        run.begin_phase(PHASE_ICON)
        self._broadcast({
            "event": "calibration", "phase": PHASE_ICON, "frac": 1.0,
            "player": run.player, "axes": len(axes),
        })

    def _emit_calibration(self, run: CalibrationRun) -> None:
        self._broadcast({
            "event": "calibration",
            "phase": run.phase,
            "frac": round(run.fraction(), 3),
            "player": run.player,
            "name": _clean(run.pad.name),
        })

    def _finish_calibration(self) -> None:
        """Leave the modal flow and hand input back to claim detection."""
        run = self._calibration
        self._calibration = None
        # A button held while choosing an icon would otherwise be sitting in
        # the confirm tracker and fire the moment normal handling resumes.
        self._confirm_started.clear()
        self._last_confirm = 0.0
        self._broadcast({
            "event": "calibration", "phase": "done", "frac": 1.0,
            "player": run.player if run else 0,
        })
        self._broadcast(self._state_event())

    def _write_controller_configs(self) -> None:
        """Emit the SDL mappings for whatever now holds each slot.

        Regenerated on every accept rather than migrated. SDL keys its database
        on the device name, and padmap's virtual pads are named after the
        player slot, so a stored line describes "whatever was in slot 1 last
        time". Rewriting from the current assignment means the question of
        keeping it in step never arises.

        RetroArch's side is handled by install_profiles, which prefers a
        capture over libretro's database entry.

        A pad with no capture gets a line too -- see fallback_line_for. The
        wizard lives inside the front-end, so a controller the front-end
        cannot be driven with is a controller that can never be mapped, and
        under padmap's own identity that is exactly what an unmapped pad is:
        SDL has never heard of 1209:0001, so it guesses the whole controller
        and puts the d-pad on buttons a hat-based pad does not have.
        """
        lines = {}
        notes = {}
        for assignment in self._assignments:
            player = assignment.player
            keys, axis_codes = controllercfg.pad_capabilities(assignment.pad)
            # Where each axis rests, so a trigger is not declared a stick --
            # the Mayflash GameCube adapter puts its triggers on ABS_RX/RY,
            # and calling those the right stick left the front-end reading a
            # stick held hard over that nobody was touching.
            axes = controllercfg.pad_axis_spans(assignment.pad)
            bindings = controllercfg.stored_bindings(assignment.pad)
            if bindings:
                # The pad, because the virtual one mirrors its identity by
                # default and so the GUID depends on the hardware. A line
                # written under the wrong GUID is never looked up, and neither
                # SDL nor the front-end says anything about it.
                lines[player] = controllercfg.sdl_line_for(
                    player, bindings, axis_codes=axis_codes,
                    pad=assignment.pad, axes=axes)
                continue
            fallback = controllercfg.fallback_line_for(
                player, assignment.pad, keys, axis_codes, axes)
            if fallback is not None:
                lines[player], notes[player] = fallback
                log.info("player %d: no capture yet, SDL mapping %s",
                         player, notes[player])
        try:
            path = controllercfg.write_sdl_mappings(lines, notes=notes)
        except OSError as error:
            log.warning("could not write SDL mappings: %s", error)
            return
        log.info("wrote %d SDL mapping(s) to %s", len(lines), path)

        # Writing the file is not enough, and this is the whole of the
        # reported bug: Pegasus loads sdl_controllers.txt once, in
        # GamepadManagerSDL2::start, and never looks at it again. A mapping
        # captured mid-session therefore does nothing until the front-end is
        # relaunched -- which is the worst possible moment for it, because it
        # is immediately after finishing the wizard. Handing the lines over
        # lets the client apply them with SDL_GameControllerAddMapping, which
        # replaces a mapping for an already-open controller in place.
        self._sdl_lines = [lines[player] for player in sorted(lines)]
        self._broadcast(self._sdl_mapping_event())

    def _sdl_mapping_event(self) -> dict[str, Any]:
        return {"event": "sdl_mapping", "lines": list(self._sdl_lines)}

    # -- choosing a layout ------------------------------------------------

    def _begin_layout_choice(self, player: int) -> None:
        """Ask which console this controller is, before walking its buttons.

        The layout decides which prompts the wizard shows, and until now
        nothing could say: `_begin_mapping` inferred one from the icon, so an
        adapter whose icon had never been set was walked through the generic
        gamepad and asked to press controls it does not have.
        """
        if self._assigner is None:
            self._broadcast({"event": "error",
                             "message": "choosing a layout needs an open session"})
            return
        pad = self._pad_for_player(player)
        if pad is None:
            self._broadcast({"event": "error",
                             "message": f"no controller assigned to player {player}"})
            return
        device = self._assigner.device_for(pad)
        if device is None:
            self._broadcast({"event": "error",
                             "message": "controller is no longer open"})
            return

        axes = self._absolute_ranges(device)
        # Start on the best guess rather than at the top of the list, so
        # someone whose pad is already recognised only has to hold a button.
        # A stored layout wins: it is what the user chose last time.
        guess = (controllercfg.stored_layout(pad)
                 or icons.for_pad(pad, self._icon_overrides))
        options = capture.layout_options(self._mapped_layouts(pad))
        self._choice = capture.Chooser(
            pad=pad, player=player, options=options,
            kind=capture.KIND_LAYOUT,
            title="Which controller is this?",
            index=layouts.index_of(guess),
            axes=axes, held=self._active_keys(device),
        )
        # The button that opened this is very likely still down; it must not
        # resume a confirm hold when the modal flow ends.
        self._confirm_started.clear()
        self._broadcast(self._choice.to_event())

    def _forget_pad(self, player: int) -> None:
        """Throw away a controller's stored config and map it again, now.

        The recovery path for a mapping that is wrong in a way the wizard
        cannot be talked out of -- a control bound to the wrong axis, a layout
        chosen by mistake -- where the only thing to do is start over.

        Keyboard-driven from the front-end, and it has to be: the daemon holds
        EVIOCGRAB for the whole session, so no controller input reaches the
        front-end while the setup screen is open. A gesture on the pad could
        not reach this, for the same reason "press Select to skip" could never
        have worked.

        Every scope goes, not just the default one. "Reset this controller"
        meaning "reset some of this controller" is the kind of half-measure
        that leaves someone re-running the wizard and still seeing the old
        behaviour from a per-console mapping they had forgotten about.

        The `prompted` record goes too. It exists to stop padmap re-offering
        setup for a model the user has already declined, and leaving it would
        make a freshly forgotten controller one that is never asked about.
        """
        pad = self._pad_for_player(player)
        if pad is None:
            self._broadcast({"event": "error",
                             "message": f"no controller assigned to player {player}"})
            return
        # Check the wizard can actually open *before* throwing anything away.
        # Deleting first and discovering afterwards that there is no session
        # to map in leaves the controller with no configuration and no way to
        # make one -- strictly worse than the wrong mapping it started with.
        if self._assigner is None or self._assigner.device_for(pad) is None:
            self._broadcast({
                "event": "error",
                "message": "resetting a controller needs an open session"})
            return

        removed = profiles.forget(pad)
        signature = profiles.signature(pad)
        if signature in self._prompted:
            self._prompted.discard(signature)
            self._save_prompted()
        log.info("player %d: forgot %s (%s)", player, _clean(pad.name),
                 "profile removed" if removed else "nothing stored")

        # Straight into the wizard rather than back to the setup screen. The
        # request is "this is wrong, fix it", and an extra step between the
        # key and the first prompt is one the user has to discover.
        self._begin_layout_choice(player)

    def _begin_game_scope_choice(
        self, player: int, console: str, key: str, title: str,
    ) -> None:
        """Ask console-or-this-game, for a game the front-end is sitting on.

        The same question `_begin_scope_choice` asks, minus the guessing. That
        one is reached from the controller setup screen, which knows nothing
        about what anyone wants to play, so it has to offer every console and
        a few recently launched games and hope the right one is among them.
        Reached from a game in the library, both facts are already known, and
        the question is genuinely two entries wide.

        The console id and the game key both come from the front-end, which
        got them from the exporter, which computed them with `layouts.for_core`
        and `profiles.game_key` -- the same two functions `padmap.launch` uses
        to decide which scope to look up when the game actually starts. Deriving
        them here instead would be a second implementation with nothing to
        notice when it drifted, and the symptom would be a mapping filed under
        a scope nothing ever reads.
        """
        if self._assigner is None:
            self._broadcast({"event": "error",
                             "message": "mapping needs an open session"})
            return
        pad = self._pad_for_player(player)
        if pad is None:
            self._broadcast({"event": "error",
                             "message": f"no controller assigned to player {player}"})
            return
        device = self._assigner.device_for(pad)
        if device is None:
            self._broadcast({"event": "error",
                             "message": "controller is no longer open"})
            return

        profile = profiles.load(pad)
        options = capture.game_scope_options(
            console=console, key=key, title=title,
            scopes=set(profile.mappings) if profile else set(),
        )
        if not options:
            # No console means no scope worth offering: a mapping filed under
            # a console padmap cannot name is one the launcher will never look
            # for. Better to say so than to record something inert.
            self._broadcast({
                "event": "error",
                "message": "no console known for this game"})
            return

        self._pending_scope = ""
        self._choice = capture.Chooser(
            pad=pad, player=player, options=options,
            kind=capture.KIND_SCOPE,
            title=f"Map for {title}?" if title else "What is this mapping for?",
            axes=self._absolute_ranges(device),
            held=self._active_keys(device),
        )
        self._confirm_started.clear()
        self._broadcast(self._choice.to_event())

    def _begin_scope_choice(self, player: int) -> None:
        """Ask what a mapping is *for* before asking where the buttons are.

        The case this exists for, reported verbatim: a GameCube controller
        used to play N64 games, wanting "the mapping that my gamecube
        controller uses for n64 games, and then a universal configuration in
        general". One mapping per controller cannot express that -- the
        console decides which controls exist and which RetroArch key each is
        emitted under, so there is no single answer to "where is A".

        Not offered on a controller's first run. Someone who has just plugged
        a pad in wants it to work, not to be asked to think about scopes; the
        automatic flow still goes straight to "which controller is this?" and
        files the result as the default. This is the deliberate route, for
        when the default is not enough.
        """
        if self._assigner is None:
            self._broadcast({"event": "error",
                             "message": "choosing a scope needs an open session"})
            return
        pad = self._pad_for_player(player)
        if pad is None:
            self._broadcast({"event": "error",
                             "message": f"no controller assigned to player {player}"})
            return
        device = self._assigner.device_for(pad)
        if device is None:
            self._broadcast({"event": "error",
                             "message": "controller is no longer open"})
            return

        profile = profiles.load(pad)
        guess = (controllercfg.stored_layout(pad)
                 or icons.for_pad(pad, self._icon_overrides))
        options = capture.scope_options(
            scopes=set(profile.mappings) if profile else set(),
            default_layout=guess,
            recent=[
                (game["console"], game["key"], game["title"])
                for game in protocol.read_recent_games()
            ],
        )
        # Asking the question again abandons whatever the last answer was.
        # Otherwise a scope chosen for a run that never reached the wizard --
        # the pad was unplugged, the overlay was dismissed -- would still be
        # sitting here to file the *next* capture under.
        self._pending_scope = ""
        self._choice = capture.Chooser(
            pad=pad, player=player, options=options,
            kind=capture.KIND_SCOPE,
            title="What is this mapping for?",
            axes=self._absolute_ranges(device),
            held=self._active_keys(device),
        )
        self._confirm_started.clear()
        self._broadcast(self._choice.to_event())

    def _mapped_layouts(self, pad: Pad) -> set[str]:
        """Layout ids this controller already has a capture under.

        Shown on the layout strip so re-mapping is not a blind act: the
        picker is the only route to a different layout, and choosing one that
        already has bindings replaces them.
        """
        profile = profiles.load(pad)
        if profile is None:
            return set()
        return {m.layout for m in profile.mappings.values() if m.buttons}

    def _choice_confirmed(self, choice: capture.Chooser) -> None:
        """Act on a picker the user just held a button to accept.

        Straight into the next step with no confirmation between: the hold
        that chose is itself the confirmation, and an extra prompt would be
        answered by the release of the very button that got here.
        """
        player = choice.player
        chosen = choice.chosen
        kind = choice.kind
        layout = choice.chosen_layout
        self._end_layout_choice()

        if kind == capture.KIND_LAYOUT:
            self._begin_mapping(player, chosen, scope=self._pending_scope)
            return

        # A scope answer. Every scope but the default names a console, and
        # the console *is* the control set -- a mapping for N64 games has to
        # be captured against the N64 layout whatever the pad physically is,
        # because those are the controls the core reads. So asking which
        # layout afterwards could only produce a contradiction, and is
        # skipped.
        #
        # The default scope is the exception, and there the layout question
        # is the real one: "any game" says nothing about the shape of the
        # controller, so the picker runs and the user answers it.
        self._pending_scope = chosen
        if chosen == profiles.SCOPE_UNIVERSAL:
            self._begin_layout_choice(player)
        else:
            self._begin_mapping(player, layout, scope=chosen)

    def _end_layout_choice(self) -> None:
        """Leave the picker without starting a wizard."""
        run = self._choice
        self._choice = None
        self._confirm_started.clear()
        self._last_confirm = 0.0
        self._broadcast({
            "event": "layout_choice", "active": False,
            "player": run.player if run else 0,
            "index": run.index if run else 0,
            "kind": run.kind if run else capture.KIND_LAYOUT,
            "title": run.title if run else "",
            "chosen": run.chosen if run else "",
            "choices": [],
        })
        self._broadcast(self._state_event())

    def _active_keys(self, device: Any) -> set[int]:
        """What is held right now, read from the device rather than inferred.

        The press that opened a modal flow is usually still down, and its
        release must not answer the first thing the flow asks.
        """
        try:
            return set(device.active_keys())
        except OSError:
            return set()

    def _absolute_ranges(self, device: Any) -> dict[int, capture.AxisSpan]:
        """Declared travel per axis plus where each one is sitting right now.

        Rest is read from the driver rather than assumed to be the middle of
        the range, because on plenty of hardware it is not. An analogue
        trigger rests at its minimum -- a GameCube pad's L and R were unusable
        in the wizard until this was measured -- and an uncalibrated stick can
        rest well off centre. capture.deflection measures from this value.

        Read once, as a picker or wizard opens, which is the best moment
        available: nothing is being pressed yet. It is not a guarantee, since
        an adapter may report a stale power-on default until the stick is
        physically moved, so capture's release threshold leaves room for rest
        to be somewhat wrong.
        """
        caps: dict[int, Any] = device.capabilities()
        ranges: dict[int, capture.AxisSpan] = {}
        for entry in caps.get(capture.EV_ABS) or []:
            # evdev reports EV_ABS as (code, AbsInfo) pairs; EV_KEY as bare
            # codes. Guard rather than trust, since a stub device can report
            # either shape.
            if not isinstance(entry, tuple) or len(entry) != 2:
                continue
            code, info = entry
            minimum = int(getattr(info, "min", 0))
            maximum = int(getattr(info, "max", 0))
            rest = int(getattr(info, "value", 0))
            if not minimum <= rest <= maximum:
                # Nonsense from the driver, or a stub that reports no current
                # value. The midpoint is the old behaviour: right for a stick,
                # and no worse than before for anything else.
                rest = (minimum + maximum) // 2
            ranges[int(code)] = (minimum, maximum, rest)
        return ranges

    # -- button mapping ---------------------------------------------------

    def _begin_mapping(self, player: int, layout_id: str = "",
                       scope: str = "") -> None:
        if self._assigner is None:
            self._broadcast({"event": "error",
                             "message": "mapping needs an open session"})
            return
        pad = self._pad_for_player(player)
        if pad is None:
            self._broadcast({"event": "error",
                             "message": f"no controller assigned to player {player}"})
            return
        device = self._assigner.device_for(pad)
        if device is None:
            self._broadcast({"event": "error",
                             "message": "controller is no longer open"})
            return

        # Which layout to walk. An explicit request always wins -- the
        # protocol carries one for a picker that does not exist yet, and a
        # guess must never override someone who has said which pad this is.
        #
        # Failing that, ask icons.for_pad rather than reading the stored icon
        # directly. It consults the stored profile first, so this is strictly
        # wider, and it adds the two sources that matter for a pad that has
        # never been through setup: the user's icons.json, and the device
        # name. Without those, an N64 adapter with no icon yet got the
        # generic layout and was asked to press an X and a Y it does not
        # have, with no way to answer.
        #
        # Icons that are not layouts (playstation, xbox, wheel, gamepad) fall
        # back to the generic pad, which is what they are.
        #
        # Deliberately the same answer the UI already draws, rather than a
        # second guess of its own. When the guess is wrong -- and on a resold
        # vendor ID it will be -- the user sees an icon that disagrees with
        # the wizard, and the one documented fix (icons.json, or the icon
        # step) corrects both at once instead of only the picture.
        chosen = layout_id or icons.for_pad(pad, self._icon_overrides)
        layout = layouts.for_icon(chosen)

        caps: dict[int, Any] = device.capabilities()
        keys = sorted(caps.get(capture.EV_KEY) or [])

        self._mapping = capture.MappingRun(
            pad=pad, player=player, layout=layout, keys=keys,
            axes=self._absolute_ranges(device),
            held=self._active_keys(device),
            scope=scope,
        )
        # Consumed: the pending scope belongs to the run now, and leaving it
        # set would file the *next* wizard -- possibly for a different
        # controller -- under a scope nobody chose for it.
        self._pending_scope = ""
        self._confirm_started.clear()
        log.info("mapping %s as %s for scope %r",
                 _clean(pad.name), layout.id, scope)
        self._emit_mapping()

    def _skip_control(self) -> None:
        run = self._mapping
        if run is None:
            return
        run.skip()
        self._emit_mapping()
        if run.finished:
            self._finish_mapping(store=True)

    def _emit_mapping(self) -> None:
        run = self._mapping
        if run is not None:
            self._broadcast(run.to_event())

    def _finish_mapping(self, store: bool) -> None:
        """Leave the modal flow, keeping what was captured if asked.

        Abandoning halfway keeps nothing: a partial mapping is worse than
        none, because the pad then counts as configured and is never offered
        again, leaving half its buttons dead with no indication why.
        """
        run = self._mapping
        self._mapping = None
        self._confirm_started.clear()
        self._last_confirm = 0.0
        self._pending_scope = ""

        if run is not None and store and run.bindings:
            self._store_mapping(run.pad, run.layout.id, run.bindings,
                                run.scope)

        self._broadcast({
            "event": "mapping", "done": True, "stored": bool(store),
            "player": run.player if run else 0,
            "index": run.index if run else 0,
            "total": len(run.layout.controls) if run else 0,
            "layout": run.layout.to_json() if run else {},
            "control": "", "label": "", "captured": {},
        })
        self._broadcast(self._state_event())

    def _store_mapping(self, pad: Pad, layout_id: str, bindings: dict,
                       scope: str = "") -> None:
        """Keep the capture against the *controller*, under one scope.

        Everything else on the profile is carried over rather than rebuilt:
        recording an N64 mapping is not a reason to forget the calibration,
        the icon, or the mapping for every other console.
        """
        existing = profiles.load(pad)
        # Only layout ids that are also icon names, which is all of them bar
        # "generic": the icon is a filename in the theme, and generic.svg
        # does not exist, so storing it would leave the pad with no picture
        # at all rather than the fallback one.
        #
        # And only from a capture with no scope. A GameCube controller mapped
        # *for N64 games* is captured against the N64 layout, and taking the
        # icon from it would relabel the pad as an N64 controller -- which it
        # is not, and which is the picture the user then sees on the setup
        # screen forever after. Only "this is what my controller is", which
        # is what the unscoped flow asks, may say what it looks like.
        icon = existing.icon if existing else ""
        if not icon and not scope and layout_id in icons.ICON_NAMES:
            icon = layout_id
        profile = profiles.Profile(
            signature=profiles.signature(pad),
            name=_clean(pad.name),
            icon=icon,
            axes=existing.axes if existing else {},
            mappings=dict(existing.mappings) if existing else {},
        )
        # The console this capture is for travels with it. Emission needs it
        # to pick the RetroArch keys the core actually reads; without it every
        # pad gets the gamepad table and the console-specific buttons are
        # bound to controls their core never looks at.
        profile.record(scope, profiles.Mapping(
            buttons=dict(bindings), layout=layout_id))
        profiles.save(profile)
        log.info("mapped %s for scope %r: %d control(s)",
                 _clean(pad.name), scope, len(bindings))

    def _store_profile(self, pad: Pad, axes: dict[int, Any]) -> None:
        """Write a profile, preserving any icon already chosen for this pad."""
        existing = profiles.load(pad)
        profile = profiles.Profile(
            # Mappings survive a re-calibration: measuring the sticks again is
            # not a reason to forget where every button is. The layout goes
            # with them -- it is what says which RetroArch keys those buttons
            # are emitted under, so dropping it silently degrades a mapped
            # console pad to the generic key table -- and so does every scope
            # beyond the default.
            mappings=dict(existing.mappings) if existing else {},
            signature=profiles.signature(pad),
            name=_clean(pad.name),
            icon=existing.icon if existing else "",
            axes=axes,
        )
        profiles.save(profile)
        log.info("calibrated %s: %d axis/axes", _clean(pad.name), len(axes))

    def _set_icon(self, player: int, icon: str) -> None:
        """Record the user's choice of icon for a controller.

        This is what retires the built-in vid/pid table: once a pad has been
        through setup, its icon comes from the person who owns it.
        """
        if icon not in icons.ICON_NAMES:
            self._broadcast({"event": "error",
                             "message": f"unknown icon {icon!r}"})
            return
        pad = self._pad_for_player(player)
        if pad is None:
            return

        existing = profiles.load(pad)
        profile = profiles.Profile(
            signature=profiles.signature(pad),
            name=_clean(pad.name),
            icon=icon,
            # Carried over, not rebuilt. Picking a picture is not a reason to
            # forget where every button is, and a profile that keeps its
            # bindings but loses its layout is worse than one that loses
            # both: it still counts as configured, so the pad is never
            # offered for mapping again, and every console-specific button
            # quietly reverts to a key its core does not read.
            mappings=dict(existing.mappings) if existing else {},
            axes=existing.axes if existing else {},
        )
        profiles.save(profile)

        # Choosing an icon is the last step, so this ends the modal flow.
        if self._calibration is not None and self._calibration.player == player:
            self._finish_calibration()
        else:
            self._broadcast(self._state_event())

    # -- assignment -------------------------------------------------------

    def _on_pad_read(self, fd: Any) -> None:
        if self._assigner is not None:
            self._assigner.handle_readable(int(fd))
        elif self._republisher is not None:
            self._republisher.handle_readable(int(fd))

    def _on_claimed_event(self, pad: Pad, _code: int, value: int) -> None:
        if value == 1:
            self._confirm_started.setdefault(pad.path, time.monotonic())
        elif value == 0:
            self._confirm_started.pop(pad.path, None)

    def _read_prompted_stamp(self) -> int:
        try:
            return self.prompted_path.stat().st_mtime_ns
        except OSError:
            return 0

    def _reload_prompted_if_changed(self) -> None:
        """Pick up edits made while the daemon is running.

        `padmap forget` clears entries here so a controller is offered setup
        again. Without this the daemon would keep serving the copy it read at
        startup, and forgetting a controller would appear to do nothing --
        which is exactly how it behaved: the profile was gone, the pad
        reported itself as never configured, and the screen still never
        appeared.
        """
        stamp = self._read_prompted_stamp()
        if stamp != self._prompted_stamp:
            self._prompted_stamp = stamp
            self._prompted = self._load_prompted()

    def _load_prompted(self) -> set[str]:
        try:
            # decode(errors="replace") rather than read_text(): a file that is
            # not valid UTF-8 raises UnicodeDecodeError, which is not an
            # OSError, and this runs from __init__ -- so `padmap serve` could
            # not start at all, and ensure-daemon kept failing forever. The
            # file lives in XDG_RUNTIME_DIR where anything may have written it.
            raw = self.prompted_path.read_bytes().decode("utf-8", "replace")
        except OSError:
            return set()
        return {line.strip() for line in raw.splitlines() if line.strip()}

    def _save_prompted(self) -> None:
        try:
            self.prompted_path.parent.mkdir(parents=True, exist_ok=True)
            self.prompted_path.write_text(
                "".join(f"{sig}\n" for sig in sorted(self._prompted))
            )
            # Our own write is not a change to react to.
            self._prompted_stamp = self._read_prompted_stamp()
        except OSError as error:
            # Not fatal: the worst case is offering setup again next launch.
            log.warning("could not record prompted controllers: %s", error)

    def _autosetup_blocked(self) -> str | None:
        """Why a first-time controller must not open setup now, or None.

        Every one of these is a way the screen could appear at a moment the
        user would experience as the machine breaking rather than helping.
        """
        if os.environ.get(ENV_NO_AUTOSETUP) == "1":
            return f"disabled by {ENV_NO_AUTOSETUP}"
        if self._state == STATE_ASSIGNING:
            return "a session is already open"
        # A *settled* client, not merely a connected socket. `padmap
        # ensure-daemon` and padctl connect for a few milliseconds to read
        # status, and counting those meant the daemon grabbed every pad to
        # display a screen on a front-end that was not running -- observed
        # immediately, as `ensure-daemon` left the machine in `assigning`
        # with no virtual pads at all. A front-end stays connected; a query
        # does not.
        now = time.monotonic()
        if not any(now - client.connected_at >= AUTOSETUP_CLIENT_SECONDS
                   for client in self._clients.values()):
            return "no front-end is connected"
        if protocol.game_is_running():
            return "a game is running"
        return None

    def _poll_new_controllers(self) -> None:
        """Open setup when a controller model is seen for the first time.

        The point is that plugging in a new controller should be enough --
        the alternative is knowing in advance that a settings screen exists
        and that a newly attached pad needs something doing to it.

        Deliberately keyed on `profiles.is_known`, not on arrival: a pad that
        has been set up before is silently republished, and only a model with
        no stored profile is worth interrupting anyone for.

        No front-end change is needed for this. Entering the assigning state
        is what surfaces the screen, and the theme already follows the daemon
        rather than assuming it is the only thing that can start a session.
        """
        now = time.monotonic()
        if now - self._last_pad_scan < PAD_SCAN_SECONDS:
            return
        self._last_pad_scan = now

        # Nothing plugged or unplugged since the last look? Then there is
        # nothing a full scan could discover, and a full scan is expensive in
        # a way that is easy to miss: devices.discover spawns `udevadm info`
        # once per input device to read ID_INPUT_JOYSTICK, and this machine
        # has 32 of them. Measured at 257-294ms, once a second, on the single
        # thread that forwards controller events to the virtual pads.
        #
        # That is the controller lag. A capture of the virtual pad while the
        # stick was moving showed a healthy 7.9ms median gap punctuated by
        # stalls of 80-176ms, roughly one a second.
        #
        # Event nodes are the right signal: a controller that appears adds
        # one, and listing a directory costs a fraction of a millisecond. The
        # prompted stamp is checked too, because `padmap forget` clears that
        # file to have a controller offered again, and it changes nothing
        # about what is plugged in.
        nodes = _event_nodes()
        stamp = self._read_prompted_stamp()
        if (nodes, stamp) == self._last_scan_signature:
            return

        # Ask *first* whether setup could open at all, because everything
        # below is expensive and this is not.
        #
        # discover() globs /sys/class/input and reads several files per pad,
        # and has_mapping parses a profile off disk for each one -- seven pads
        # here. That ran once a second on the same single thread that forwards
        # controller events to the virtual pads, including all the way through
        # a game, only to reach this test and give up. A periodic stall in the
        # input path is exactly the shape of "there's a lag on the controllers"
        # and it gets worse with every pad attached.
        #
        # Nothing is lost by checking early: the answer does not depend on
        # what is plugged in, and the reasons it returns are all states the
        # user leaves -- quitting the game, connecting a front-end -- after
        # which the next scan a second later proceeds normally.
        blocked = self._autosetup_blocked()
        if blocked is not None:
            # Deliberately without recording the signature. Being blocked is
            # temporary -- a game ends, a front-end connects -- and a
            # controller plugged in meanwhile still has to be noticed once the
            # reason goes away. Recording here means the next look sees
            # nothing changed and skips it for good.
            return
        self._last_scan_signature = (nodes, stamp)

        self._reload_prompted_if_changed()

        fresh = {}
        for pad in devices.discover():
            signature = profiles.signature(pad)
            if signature in self._prompted or controllercfg.has_mapping(pad):
                continue
            fresh[signature] = pad
        if not fresh:
            return

        names = ", ".join(sorted(_clean(pad.name) for pad in fresh.values()))

        # Mark before starting: if _begin fails, the user still gets to reach
        # setup by hand rather than being re-prompted every second.
        self._prompted.update(fresh)
        self._save_prompted()
        log.info("first time seeing %s -- opening controller setup", names)
        self._broadcast({
            "event": "newpad",
            "names": sorted(_clean(pad.name) for pad in fresh.values()),
        })
        self._begin(self._slots)

    def _tick(self) -> None:
        self._poll_new_controllers()

        if self._assigner is None:
            return

        # Mapping is modal for the same reason calibration is: every prompt
        # is answered with a button press, and those must not also claim
        # slots or trip the confirm gesture. Choosing a layout comes first and
        # is answered the same way, so it is modal too.
        if self._choice is not None or self._mapping is not None:
            return

        # Calibration runs inside a session and suspends claim detection: a
        # user rotating a stick will press buttons incidentally, and those
        # must not burn a player slot.
        if self._calibration is not None:
            self._tick_calibration()
            return

        before = len(self._assigner.assignments)
        progress = 0.0

        def on_progress(_pad: Pad, fraction: float) -> None:
            nonlocal progress
            progress = max(progress, min(fraction, 1.0))

        def on_claim(assignment: Assignment) -> None:
            log.info("claim: player %d <- %s (%s)", assignment.player,
                     _clean(assignment.pad.name), assignment.pad.event)
            # `configured` lets a front-end offer calibration the first time
            # it sees a controller, and stay quiet on every later run.
            known = profiles.is_known(assignment.pad)
            self._broadcast({
                "event": "claim",
                "player": assignment.player,
                "name": _clean(assignment.pad.name),
                "node": assignment.pad.event,
                "icon": icons.for_pad(assignment.pad, self._icon_overrides),
                "configured": known,
            })

        self._assigner.tick(on_progress=on_progress, on_claim=on_claim)

        if progress != self._last_progress:
            self._last_progress = progress
            self._broadcast({"event": "progress", "frac": round(progress, 3)})

        if len(self._assigner.assignments) != before:
            self._broadcast(self._state_event())

        self._tick_confirm()

    def _tick_confirm(self) -> None:
        if not self._confirm_started:
            if self._last_confirm:
                self._last_confirm = 0.0
                self._broadcast({"event": "confirm", "frac": 0.0})
            return

        now = time.monotonic()
        elapsed = max(now - started for started in self._confirm_started.values())
        fraction = min(elapsed / CONFIRM_HOLD_SECONDS, 1.0)
        if fraction != self._last_confirm:
            self._last_confirm = fraction
            self._broadcast({"event": "confirm", "frac": round(fraction, 3)})
        if fraction >= 1.0:
            self._accept()

    def _end_session(self, release: bool) -> None:
        if self._assigner is None:
            return
        for fd in self._assigner.fds:
            try:
                self._selector.unregister(fd)
            except (KeyError, ValueError):
                pass
        if release:
            self._assigner.close()
        self._assigner = None
        self._confirm_started.clear()

    # -- republishing -----------------------------------------------------

    def _start_republisher(self) -> None:
        self._stop_republisher()
        vpads = [virtual.create(a.pad, a.player) for a in self._assignments]
        self._republisher = virtual.Republisher(vpads)
        for fd in self._republisher.fds:
            self._selector.register(fd, selectors.EVENT_READ, self._on_pad_read)

        paths = {vp.player: vp.ui.device.path for vp in vpads}
        # Regenerate the profiles the *launcher* would, not context-free ones.
        #
        # padmap.launch resolves a scope from the core and ROM at launch and
        # writes it into this same directory. Writing a context-free profile
        # here overwrites that with the default mapping, and republishing
        # restarts for reasons that have nothing to do with the game -- a
        # session being accepted, a controller reconnecting, the daemon being
        # upgraded. Whichever wrote last wins, so a game-specific mapping
        # could be live one launch and silently gone the next.
        #
        # The last game is the right context because it is the only one this
        # side knows, and it is what the next launch will resolve again anyway
        # if it is still the game being played. If nothing has been launched,
        # this is exactly the context-free write it always was.
        last = protocol.read_last_game()
        retroarch.install_profiles(
            self._assignments,
            console=last.get("console", ""), game=last.get("key", ""),
            context=last.get("title", ""))
        retroarch.write_launch_config(
            self._assignments, paths, self.launch_config_path
        )
        retroarch.write_launch_args(
            self._assignments, paths, self.launch_args_path
        )
        # Also here, not only on accept. The pads going live is what makes
        # these files describe reality, and that happens on every restore --
        # otherwise a mapping cleared with `forget`, or one captured under a
        # previous version, stays in Pegasus's database until the next time
        # someone completes an assignment.
        self._write_controller_configs()
        log.info("republishing %d pad(s); launch config at %s",
                 len(vpads), self.launch_config_path)

    def _stop_republisher(self) -> None:
        if self._republisher is None:
            return
        for fd in self._republisher.fds:
            try:
                self._selector.unregister(fd)
            except (KeyError, ValueError):
                pass
        self._republisher.close()
        self._republisher = None

    # -- state ------------------------------------------------------------

    def _players_payload(self) -> list[dict[str, Any]]:
        source = (
            self._assigner.assignments
            if self._assigner is not None
            else self._assignments
        )
        return [
            {
                "player": a.player,
                "name": _clean(a.pad.name),
                "node": a.pad.event,
                "icon": icons.for_pad(a.pad, self._icon_overrides),
                # "Configured" now means *mapped*, not merely known.
                #
                # A profile exists for several reasons -- calibration writes
                # one, and so does finishing a session -- so keying this on
                # the profile's existence meant a controller counted as set
                # up before anyone had told padmap where its buttons were,
                # and the wizard was offered exactly once and never again.
                "configured": controllercfg.has_mapping(a.pad),
                # Which scopes this controller has a capture under, so a
                # front-end can say what already exists rather than making
                # re-mapping a blind, destructive act. Scope strings, not
                # labels: the labels are built where the picker is built, and
                # a second set here would be a second thing to keep in step.
                "mappings": self._mapping_scopes(a.pad),
            }
            for a in source
        ]

    def _mapping_scopes(self, pad: Pad) -> list[str]:
        profile = profiles.load(pad)
        if profile is None:
            return []
        return sorted(
            scope for scope, m in profile.mappings.items() if m.buttons
        )

    def _state_event(self) -> dict[str, Any]:
        return {
            "event": "state",
            "state": self._state,
            "slots": self._slots,
            "players": self._players_payload(),
            # So a client can tell whether this daemon is running the same
            # code it is. Nothing else distinguishes a stale daemon: it keeps
            # answering, and keeps writing files that look right.
            "build": protocol.build_id(),
            # So a client that wants to replace this daemon can signal
            # exactly it. Matching on the command line instead would hit
            # every daemon the user has running, including ones on a
            # different XDG_RUNTIME_DIR that are none of its business.
            "pid": os.getpid(),
            # What the virtual pads claim to be. Reported because it is
            # decided from the daemon's environment and cannot be seen from
            # anywhere else: a daemon started without PADMAP_ONLY_VIRTUAL and
            # a front-end started with it disagree about which pads exist, and
            # the symptom is a machine with no controllers at all.
            "identity": virtual.identity_mode(),
        }

    def restore(self) -> None:
        """Republish the saved assignments, making a restart invisible.

        Without this a restart silently costs the user their controller
        order -- the virtual pads vanish and the assignment has to be redone
        by hand. That also makes restarting too expensive to do automatically,
        which is the whole point of `padmap ensure-daemon`.

        Pads are matched by device path. Nothing has been replugged across a
        restart in the normal case, so that is enough; a pad that really is
        gone is skipped rather than faked, because republishing a pad that no
        longer exists would put a dead virtual pad in the enumeration and
        shift every index after it.
        """
        if not self.state_path.is_file():
            return
        try:
            raw = json.loads(self.state_path.read_text())
        except (OSError, ValueError) as error:
            log.warning("ignoring unreadable %s: %s", self.state_path, error)
            return
        if not isinstance(raw, list):
            return

        by_path = {pad.path: pad for pad in devices.discover()}
        restored: list[Assignment] = []
        for entry in raw:
            if not isinstance(entry, dict):
                continue
            pad = by_path.get(entry.get("path", ""))
            if pad is None:
                log.warning(
                    "player %s: %s (%s) is gone, not restored",
                    entry.get("player"), _clean(str(entry.get("name", "?"))),
                    entry.get("path"),
                )
                continue
            try:
                player = int(entry["player"])
            except (KeyError, TypeError, ValueError):
                continue
            restored.append(Assignment(player=player, pad=pad, button=0))

        if not restored:
            return

        self._assignments = restored
        self._slots = max(a.player for a in restored)
        # serve() calls restore() outside its try/finally, so an exception
        # here ends the process during startup -- after the pads have been
        # discovered, and with nothing republished. Coming up idle is a state
        # the user can fix from the setup screen; not coming up is not.
        try:
            self._start_republisher()
        except OSError as error:
            log.warning("could not republish restored assignments: %s", error)
            self._state = STATE_IDLE
            return
        self._state = STATE_READY
        log.info("restored %d assignment(s) from %s",
                 len(restored), self.state_path)

    def _save_assignments(self) -> None:
        self.state_path.parent.mkdir(parents=True, exist_ok=True)
        self.state_path.write_text(json.dumps([
            {"player": a.player, "path": a.pad.path, "name": a.pad.name,
             "phys": a.pad.phys, "vid": a.pad.vid, "pid": a.pad.pid}
            for a in self._assignments
        ], indent=2))


def _clean(name: str) -> str:
    """Strip control characters some adapters prefix to their device name."""
    return "".join(ch for ch in name if ch.isprintable()).strip()


def serve() -> int:
    server = Server()
    server.start()
    # Before the signal handlers, so a restart that is interrupted mid-restore
    # still tears down cleanly through the same `finally`.
    server.restore()

    # Without this, a SIGTERM (systemd stop, pkill, logout) kills the process
    # outright: `finally` never runs, so the socket is left behind and -- far
    # worse -- an open session keeps EVIOCGRAB on every pad, leaving the
    # machine with no working controllers. Ask the loop to exit instead, so
    # the normal teardown path runs.
    def on_terminate(_signum: int, _frame: object) -> None:
        log.info("terminating")
        server.stop()

    signal.signal(signal.SIGTERM, on_terminate)
    signal.signal(signal.SIGINT, on_terminate)

    try:
        server.run()
    except KeyboardInterrupt:
        pass
    finally:
        server.close()
    return 0
