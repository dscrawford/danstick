"""The daemon must not be killable by the things it exists to serve.

Every fault here came out of the story sweep, and they share one shape: a read
or a coercion that assumes the world is well formed, on a path where failing
ends the *process*. That is what makes them worse than ordinary bugs. When the
daemon exits its uinput nodes go with it, so the machine does not degrade --
it simply has no controllers, mid-game, with nothing on screen to explain it.

The five guarded here:

  * a malformed command argument. Several commands coerce with int(), and a
    front-end sending {"cmd": "begin", "players": "lots"} raised straight
    through the selector loop and out of serve().
  * a `prompted` file that is not UTF-8. It is read in Server.__init__, and
    UnicodeDecodeError is not an OSError, so `padmap serve` could not start at
    all -- and the file lives in XDG_RUNTIME_DIR, where anything may write it.
  * a wizard outliving its session. _begin cleared the picker and the
    calibration but not the mapping, and _tick returns early while a mapping
    is open -- so for the whole of the next session no hold could claim a slot
    and confirm never fired. Silently: no error, nothing logged.
  * a controller unplugged mid-session. _cancel guarded _start_republisher and
    _accept and restore did not, so a pad vanishing ended the daemon *after*
    the state file was written, leaving the setup screen waiting forever.
  * a player number outside 1..16. launch_config hands out one spare index per
    unmanaged slot but counted an out-of-range player as managed without
    consuming one, so the iterator ran dry. restore() calls this during
    startup, after the pads are grabbed.

Nothing here may touch real hardware. A live daemon owns the controllers on
this machine, and an earlier draft of this very file began a session with the
real ones because {"players": 1.5} is a *valid* command -- so devices.discover
and the Assigner are both replaced before any command is dispatched.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \\
        python3 tools/check_daemon_survival.py
"""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

# Before padmap is imported: several modules read these at import time.
_SANDBOX = tempfile.mkdtemp(prefix="padmap-survival-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import devices, protocol, retroarch, server  # noqa: E402
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402


def pad(name: str = "Test Pad", node: str = "event90") -> Pad:
    return Pad(path=f"/dev/input/{node}", name=name, phys=f"usb-{node}",
               uniq="", vid=0x1234, pid=0x0001, syspath="")


class FakeAssigner:
    """Stands in for the real one, which opens and grabs devices."""

    def __init__(self, pads, grab=True):
        self.pads = list(pads)
        self.assignments: list[Assignment] = []
        self.fds: list[int] = []
        self.grab_failures: list[Pad] = []
        self.on_claimed_event = None
        self.on_raw_event = None

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        return None

    def close(self):
        return None

    def device_for(self, _pad):
        return None

    def reset(self):
        self.assignments = []

    def tick(self, **_kwargs):
        return None


def isolate_hardware() -> None:
    """No test below may reach a real controller."""
    devices.discover = lambda *a, **k: [pad()]      # type: ignore[assignment]
    server.Assigner = FakeAssigner                  # type: ignore[assignment]


def quiet_server() -> tuple[server.Server, list[dict]]:
    """A Server whose replies are captured rather than written to a socket."""
    srv = server.Server()
    seen: list[dict] = []
    srv._broadcast = lambda message: seen.append(message)  # type: ignore[assignment]
    srv._send = lambda _client, message: seen.append(message)  # type: ignore[assignment]
    srv._start_republisher = lambda: None           # type: ignore[assignment]
    srv._save_assignments = lambda: None            # type: ignore[assignment]
    srv._write_controller_configs = lambda: None    # type: ignore[assignment]
    return srv, seen


def check_malformed_arguments_do_not_end_the_process() -> None:
    print("\na command argument of the wrong type is answered, not fatal:")
    srv, seen = quiet_server()

    # Every one of these raised out of _handle_command before the guard, and
    # _handle_command is called straight from the selector dispatch in run().
    hostile = [
        {"cmd": "begin", "players": "lots"},
        {"cmd": "begin", "players": None},
        {"cmd": "begin", "players": [4]},
        {"cmd": "calibrate", "player": "one"},
        {"cmd": "calibrate", "player": {"n": 1}},
        {"cmd": "forget_pad", "player": "x"},
        {"cmd": "map_for_game", "player": "x", "console": "n64"},
        {"cmd": "set_icon", "player": [1], "icon": "n64"},
        {"cmd": "map", "player": None},
        {"cmd": "choose_scope", "player": "2"},
        {"cmd": "choose_layout", "player": 1.5e400},
    ]
    for message in hostile:
        before = len(seen)
        try:
            srv._handle_command(None, message)
        except Exception as error:                      # noqa: BLE001
            raise SystemExit(
                f"FAIL: {message!r} raised {type(error).__name__} out of the "
                f"command handler. That unwinds through serve(), the daemon "
                f"exits, and every virtual pad goes with it -- the machine "
                f"has no controllers until something restarts it")
        if len(seen) == before:
            raise SystemExit(
                f"FAIL: {message!r} produced no reply at all, so a front-end "
                f"waits forever for an answer that is not coming")
    print(f"  ok  {len(hostile)} malformed commands answered, daemon alive")

    print("\n...and a well formed command still works afterwards:")
    seen.clear()
    srv._handle_command(None, {"cmd": "status"})
    if not any(m.get("event") == "state" for m in seen):
        raise SystemExit(
            "FAIL: the daemon stopped answering valid commands after a bad "
            "one -- the guard must not swallow the dispatch itself")
    print("  ok  status still answered")


def check_prompted_file_that_is_not_utf8() -> None:
    print("\na prompted list that is not UTF-8 does not stop the daemon:")
    path = protocol.prompted_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"1234:0001:Working Pad\n\xff\xfe\x00\n")
    try:
        srv = server.Server()
    except Exception as error:                          # noqa: BLE001
        raise SystemExit(
            f"FAIL: Server() raised {type(error).__name__} on a prompted file "
            f"that is not UTF-8. This runs in __init__, so `padmap serve` "
            f"cannot start at all and ensure-daemon keeps failing -- and the "
            f"file lives in XDG_RUNTIME_DIR where anything may write it")
    # The readable line must survive; only the damaged one is lost.
    if "1234:0001:Working Pad" not in srv._prompted:
        raise SystemExit(
            f"FAIL: a damaged byte cost the whole prompted list "
            f"({srv._prompted!r}), so every controller is offered setup again")
    path.unlink(missing_ok=True)
    print(f"  ok  daemon constructs, {len(srv._prompted)} entries kept")


def check_begin_clears_a_live_wizard() -> None:
    print("\nbeginning a session clears a wizard left from the last one:")
    srv, _ = quiet_server()

    # Stand in for the modal flows. The real ones hold a Pad from the session
    # that is being torn down, which is exactly the danger.
    srv._choice = object()          # type: ignore[assignment]
    srv._calibration = object()     # type: ignore[assignment]
    srv._mapping = object()         # type: ignore[assignment]

    srv._begin(4)

    if srv._mapping is not None:
        raise SystemExit(
            "FAIL: a capture from the previous session survived into the new "
            "one. _tick returns early while a mapping is open, so for the "
            "whole of the next session no hold can claim a slot and confirm "
            "never fires -- the setup screen just sits there, with nothing "
            "logged and no error shown")
    if srv._choice is not None or srv._calibration is not None:
        raise SystemExit("FAIL: the picker or calibration was left behind")
    print("  ok  wizard, picker and calibration all cleared")


def check_republisher_failure_is_reported_not_fatal() -> None:
    print("\na controller vanishing before it can be published:")
    srv, _ = quiet_server()

    def explode() -> None:
        raise OSError(19, "No such device")

    srv._start_republisher = explode        # type: ignore[assignment]

    # restore(): serve() calls this outside its try/finally, so raising here
    # ends the process during startup, after the pads have been discovered.
    srv.state_path.parent.mkdir(parents=True, exist_ok=True)
    srv.state_path.write_text(
        '[{"player": 1, "path": "/dev/input/event90", "name": "Test Pad",'
        ' "phys": "usb-event90", "vid": 4660, "pid": 1}]')
    try:
        srv.restore()
    except Exception as error:                       # noqa: BLE001
        raise SystemExit(
            f"FAIL: restore() raised {type(error).__name__} when a pad could "
            f"not be published. serve() calls it outside its try/finally, so "
            f"the daemon dies at startup rather than coming up idle")
    if srv._state == protocol.STATE_READY:
        raise SystemExit(
            "FAIL: restore() reported READY while nothing is republished -- "
            "a front-end would show controllers that do not exist")
    print(f"  ok  restore() came up {srv._state!r} instead of dying")

    print("\n...and the same failure during accept is reported, not fatal:")
    srv2, seen = quiet_server()
    srv2._start_republisher = explode       # type: ignore[assignment]
    srv2._assigner = FakeAssigner([pad()])  # type: ignore[assignment]
    srv2._assigner.assignments = [Assignment(player=1, pad=pad(), button=0)]
    try:
        srv2._accept()
    except Exception as error:                       # noqa: BLE001
        raise SystemExit(
            f"FAIL: _accept() raised {type(error).__name__} after the state "
            f"file was already written -- the front-end never hears "
            f"'accepted' or 'error' and the setup screen waits forever")
    if not any(m.get("event") in ("error", "accepted") for m in seen):
        raise SystemExit(
            "FAIL: _accept() said nothing at all, so the screen hangs")
    print("  ok  accept answered instead of ending the daemon")


def check_out_of_range_player_numbers() -> None:
    print("\na player number outside the slot range does not kill a launch:")
    original = retroarch.visible_order
    retroarch.visible_order = lambda: {0: "/dev/input/event100"}  # type: ignore[assignment]
    try:
        for player in (0, -1, 17, 99):
            assignments = [Assignment(player=player, pad=pad(), button=0)]
            try:
                config = retroarch.launch_config(
                    assignments, {player: "/dev/input/event100"})
            except Exception as error:               # noqa: BLE001
                raise SystemExit(
                    f"FAIL: player {player} raised {type(error).__name__}. "
                    f"A corrupted assignments.json is enough to reach this, "
                    f"and Server.restore calls it during startup after the "
                    f"pads are grabbed -- so the daemon dies with the machine "
                    f"already committed")
            written = config.count("_joypad_index")
            if written != retroarch.MAX_PLAYERS:
                raise SystemExit(
                    f"FAIL: player {player} produced {written} slots, not "
                    f"{retroarch.MAX_PLAYERS} -- RetroArch falls back to "
                    f"whatever retroarch.cfg holds for the rest")
    finally:
        retroarch.visible_order = original           # type: ignore[assignment]
    print("  ok  0, -1, 17 and 99 all produce a complete config")


def main() -> int:
    isolate_hardware()
    check_malformed_arguments_do_not_end_the_process()
    check_prompted_file_that_is_not_utf8()
    check_begin_clears_a_live_wizard()
    check_republisher_failure_is_reported_not_fatal()
    check_out_of_range_player_numbers()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
