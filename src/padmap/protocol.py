"""Wire protocol between the padmap daemon and a front-end.

Newline-delimited JSON over a unix socket. Chosen over D-Bus or a custom
binary framing because the client is a small C++ patch inside Pegasus: a
QLocalSocket plus QJsonDocument is a few dozen lines, with no extra
dependency and nothing to generate.

The daemon is authoritative. Clients send commands and render whatever
events come back; they hold no assignment state of their own, so a client
that reconnects mid-session picks up correctly from the next `state` event.

Commands (client -> daemon)
---------------------------
{"cmd": "begin",  "players": 4}   start an assignment session; grabs the pads
{"cmd": "reset"}                  drop all claims, keep the session open
{"cmd": "accept"}                 finalise: save, republish, write launch cfg
{"cmd": "cancel"}                 abandon the session, release the grabs
{"cmd": "status"}                 ask for a `state` event now
{"cmd": "calibrate", "player": 1} measure that player's pad: centre, then reach
{"cmd": "set_icon", "player": 1, "icon": "n64"}   remember the chosen icon
{"cmd": "choose_layout", "player": 1}   pick a console, then map its buttons
{"cmd": "choose_scope", "player": 1}    pick what a mapping is *for*, then map
{"cmd": "map_for_game", "player": 1, "console": "n64",
 "key": "n64/goldeneye-007-usa", "title": "GoldenEye 007 (USA)"}
                                  map this player's pad for that console or
                                  that one game -- asked from the library,
                                  where both are already known, so the scope
                                  question is two entries wide. console/key
                                  come from the exporter's x-console and
                                  x-gamekey, computed with the same functions
                                  the launcher uses to resolve a scope
{"cmd": "forget_pad", "player": 1}     throw away everything stored for that
                                  player's controller and open the wizard
                                  again; keyboard-driven, because the daemon
                                  holds EVIOCGRAB and no pad input reaches
                                  the front-end while a session is open
{"cmd": "map", "player": 1, "layout": "n64", "scope": "console:n64"}
                                  map buttons under a layout, filed under a
                                  scope ("" = this controller's default)
{"cmd": "skip_control"}           move past a control this pad does not have
{"cmd": "configure_end"}          leave a modal flow without finishing it

Events (daemon -> client)
-------------------------
{"event": "state", "state": "idle"|"assigning"|"ready", "slots": 4,
 "players": [...], "build": "...", "pid": 123,
 "identity": "mirror"|"padmap"}  slots is how many the front-end asked for;
                                 identity is what the virtual pads advertise
                                 (see virtual.identity_for), which a front-end
                                 hiding pads by vid/pid has to agree with
{"event": "pads", "count": 3}                  pads visible at session start
{"event": "progress", "frac": 0.62}            hold in flight, next free slot
{"event": "claim", "player": 1, "name": "...", "node": "event24",
 "icon": "n64", "configured": false}   configured=false -> offer calibration
{"event": "calibration", "phase": "rest"|"reach"|"done", "frac": 0.0..1.0,
 "player": 1}                          progress of a calibration in flight
{"event": "confirm", "frac": 0.4}              confirm-hold in flight
{"event": "layout_choice", "active": true, "player": 1, "index": 2,
 "kind": "layout"|"scope", "title": "Which controller is this?",
 "chosen": "n64",
 "choices": [{"id", "label", "mapped", "layout": <layout>}, ...]}
                                 picker in flight -- console, or what a
                                 mapping is for; one mechanism, two questions
{"event": "sdl_mapping", "lines": ["03000000...,padmap Player 1,a:b1,...",
 ...]}                           SDL database lines padmap has just written.
                                 Pegasus reads sdl_controllers.txt once at
                                 startup, so a mapping written mid-session
                                 does nothing until it is relaunched; the
                                 client feeds these to
                                 SDL_GameControllerAddMapping instead.
{"event": "mapping", "player": 1, "layout": <layout>, "index": 3, "total": 14,
 "control": "y", "label": "Y (top face)", "done": false, "captured": {...}}
{"event": "accepted", "players": [...], "launch_config": "/run/..."}
{"event": "error", "message": "..."}

The layout picker and the mapping wizard are both driven from the *pad*, not
from the front-end: the daemon holds EVIOCGRAB for the whole session, so a
front-end sees no controller input at all while either is open. `choices`
carries whole layouts (see layouts.catalogue) so a theme can draw the pad it
is offering without holding a copy of the console list.

`players` entries are {"player": int, "name": str, "node": str}.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

STATE_IDLE = "idle"
STATE_ASSIGNING = "assigning"
STATE_READY = "ready"


# Set by the flake wrapper to the store path of the source it runs.
ENV_BUILD_ID = "PADMAP_BUILD_ID"


def build_id() -> str:
    """Identity of the code this process is running.

    A daemon keeps the modules it started with, so changing `retroarch.py`
    and rebuilding does nothing until it is restarted -- and a stale daemon
    is indistinguishable from a fresh one by anything on disk, since it goes
    on writing plausible-looking files. This is what lets a client notice.

    Under Nix the source is a store path that changes with every edit, which
    is exactly the property wanted. Outside it (a dev shell running from
    ./src) there is no such path, so fall back to the newest mtime in the
    package directory -- coarser, but it still changes when you edit.
    """
    store = os.environ.get(ENV_BUILD_ID)
    if store:
        return store

    here = Path(__file__).resolve().parent
    try:
        newest = max(p.stat().st_mtime_ns for p in here.glob("*.py"))
    except (OSError, ValueError):
        return "unknown"
    return f"mtime:{here}:{newest}"


def runtime_dir() -> Path:
    return Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "padmap"


def _proc_field(pid: int, name: str) -> list[str]:
    try:
        raw = Path(f"/proc/{pid}/{name}").read_bytes()
    except OSError:
        return []
    return [
        part.decode("utf-8", "replace")
        for part in raw.split(b"\0")
        if part
    ]


def daemon_pids(runtime: str | None = None) -> list[int]:
    """Pids of `padmap serve` processes on a given XDG_RUNTIME_DIR.

    Two things this does that the obvious `pgrep -f "padmap.cli serve"` does
    not, both of which have already caused trouble:

    * Matches argv **structurally**, not as a substring. `pgrep -f` also
      matches any shell whose command line happens to mention the string --
      including the terminal running a diagnostic about daemons. Signalling
      that would kill the shell instead of a daemon.
    * Filters by runtime dir, since that is what decides which socket a
      daemon serves. Without it, a caller managing its own daemon reaches
      into every other one the user is running.
    """
    wanted = runtime if runtime is not None else os.environ.get(
        "XDG_RUNTIME_DIR", "/tmp")
    uid = os.getuid()
    pids: list[int] = []

    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        try:
            if entry.stat().st_uid != uid:
                continue
        except OSError:
            continue

        argv = _proc_field(pid, "cmdline")
        # [python, -m, padmap.cli, serve] -- an interpreter running the
        # module, not merely something that mentions it.
        if argv[-2:] != ["padmap.cli", "serve"] or "-m" not in argv:
            continue

        env: dict[str, str] = {}
        for item in _proc_field(pid, "environ"):
            key, sep, value = item.partition("=")
            if sep:
                env[key] = value
        if env.get("XDG_RUNTIME_DIR", "/tmp") == wanted:
            pids.append(pid)
    return pids


def daemon_log_path() -> Path:
    """Where a daemon started by `ensure-daemon` writes its log.

    Beside the socket, so it shares the socket's lifetime and a fresh login
    starts a fresh log. The daemon is the only process that watches a
    controller being claimed, a mapping being captured, or a launch config
    being written; without this its output went to /dev/null and none of that
    was recoverable after the fact.
    """
    return runtime_dir() / "padmap.log"


def prompted_path() -> Path:
    """Controller models the daemon has already offered a setup screen for.

    Distinct from the profile store, and the distinction matters: a profile
    means "this controller has been configured", while this means "we have
    already asked about it, do not ask again". Declining has to be
    rememberable, or the offer reappears a second later and on every
    front-end launch.

    In XDG_RUNTIME_DIR, so it lasts the login session and a fresh boot asks
    again.
    """
    return runtime_dir() / "prompted"


def playing_marker() -> Path:
    """File padmap-play holds while a game is running, containing its pid.

    The daemon needs this before it may grab the pads for anything. Opening
    an assignment session stops republishing and takes EVIOCGRAB on every
    physical pad, so doing it during a game would leave the player holding a
    controller that has silently stopped working. Nothing else tells the
    daemon a game is in progress -- from its side a game is just RetroArch
    reading the virtual pads it already published.

    Holds a pid rather than existing/not existing so a padmap-play that was
    killed outright cannot disable the feature forever.
    """
    return runtime_dir() / "playing"


def game_is_running() -> bool:
    marker = playing_marker()
    try:
        pid = int(marker.read_text().strip())
    except (OSError, ValueError):
        return False
    if Path(f"/proc/{pid}").exists():
        return True
    # Stale: the launcher died without running its trap.
    marker.unlink(missing_ok=True)
    return False


def last_game_path() -> Path:
    """The game most recently launched through padmap-play.

    Written by `padmap.launch`, read by the daemon so the scope picker can
    offer "...for this game". It is the only way a per-game scope can be
    offered at all: the controller setup screen is reached from the
    front-end, never from inside a game, so nothing on that screen otherwise
    knows which game the user means -- and "the controls were wrong in the
    game I just played" is exactly when someone wants a per-game mapping.

    In XDG_RUNTIME_DIR beside the other launch state, so it cannot outlive
    the session that produced it and offer a scope for a game whose ROM has
    since been removed.
    """
    return runtime_dir() / "lastgame.json"


# How many recently played games the scope picker may offer.
#
# Small on purpose. The strip is worked from the pad, one step at a time, and
# every entry here sits after the console entries -- a long tail of games
# turns "map this for N64" into a scrolling exercise. Five covers an evening's
# play, which is the span in which someone notices a control was wrong.
RECENT_GAMES = 5


def _game_entry(raw: object) -> dict[str, str] | None:
    if not isinstance(raw, dict) or not raw.get("key"):
        return None
    return {
        "console": str(raw.get("console", "")),
        "key": str(raw.get("key", "")),
        "title": str(raw.get("title", "")),
    }


def read_recent_games() -> list[dict[str, str]]:
    """Recently launched games, newest first.

    Never raises: this decorates a picker, and a missing or malformed file
    means fewer options rather than a failure.

    Reads the pre-list format too -- a bare {"console","key","title"} object,
    which is what earlier versions wrote. A user upgrading mid-session would
    otherwise silently lose the per-game scope for the game they are playing
    right now, which is the exact moment they are most likely to want it.
    """
    try:
        raw = json.loads(last_game_path().read_text())
    except (OSError, ValueError):
        return []
    if isinstance(raw, dict) and isinstance(raw.get("games"), list):
        entries = [_game_entry(item) for item in raw["games"]]
        return [entry for entry in entries if entry][:RECENT_GAMES]
    single = _game_entry(raw)
    return [single] if single else []


def read_last_game() -> dict[str, str]:
    """{"console", "key", "title"} for the most recent launch, or {}."""
    recent = read_recent_games()
    return recent[0] if recent else {}


def write_last_game(console: str, key: str, title: str) -> Path:
    """Record a launch, keeping the previous few.

    Most recent first, deduplicated by key: replaying a game should move it to
    the front, not fill the list with copies of itself and push out everything
    else someone might want to correct.
    """
    entry = {"console": console, "key": key, "title": title}
    kept = [game for game in read_recent_games() if game["key"] != key]
    path = last_game_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(
        {"games": [entry, *kept][:RECENT_GAMES]}, indent=2))
    return path


def socket_path() -> Path:
    """Where the daemon listens.

    Under XDG_RUNTIME_DIR so it is per-user, mode 0700 by the spec, and
    cleaned up on logout without us having to manage stale socket files.
    """
    return runtime_dir() / "padmap.sock"


def encode(message: dict[str, Any]) -> bytes:
    """One message, newline-terminated.

    separators avoids spaces so a message never contains a stray newline from
    pretty-printing, which would desynchronise the framing.
    """
    return (json.dumps(message, separators=(",", ":")) + "\n").encode("utf-8")


class LineReader:
    """Accumulates socket reads and yields whole JSON messages.

    A stream socket splits messages anywhere, so a client that assumes one
    recv() is one message works until it doesn't. This buffers instead.
    """

    def __init__(self) -> None:
        self._buffer = b""

    def feed(self, data: bytes) -> list[dict[str, Any]]:
        self._buffer += data
        messages: list[dict[str, Any]] = []
        while b"\n" in self._buffer:
            line, self._buffer = self._buffer.split(b"\n", 1)
            line = line.strip()
            if not line:
                continue
            try:
                decoded = json.loads(line)
            except ValueError:
                # A malformed line is worth skipping rather than killing the
                # connection: the framing is still intact after the newline.
                continue
            if isinstance(decoded, dict):
                messages.append(decoded)
        return messages
