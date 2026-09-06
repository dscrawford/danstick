#!/usr/bin/env python3
"""Does a mapping sent mid-session reach the *running* front-end?

The reported bug: "the new controller config isn't immediately loaded into
Pegasus, and it just uses the old SDL default." Pegasus reads
sdl_controllers.txt exactly once, in GamepadManagerSDL2::start; a mapping
padmap writes afterwards does nothing until the frontend is relaunched, which
is precisely the moment after finishing the wizard.

`check_sdl_live.py` measures the mechanism -- that SDL_GameControllerAddMapping
re-binds a controller that is already open, proved with a real press. What it
cannot show is that the patched Pegasus is wired to it. That needs the real
binary, so this runs it:

  * a uinput pad exists *before* Pegasus starts, so Pegasus opens it and gives
    it SDL's blind default -- the exact state the bug report describes;
  * a stub daemon then sends one `sdl_mapping` event carrying a line for that
    pad's GUID;
  * the frontend's own log is read back for what SDL then holds.

What is asserted is the *readback*: after applying, the patched client asks
SDL_GameControllerMappingForGUID what it would now use for that device, and
that has to be padmap's line. Deliberately not the return code. Measured
here, SDL reports 1 ("added a new entry") rather than 0 ("replaced") in this
situation, because Pegasus's blind default carries no `crc:` field while the
GUID padmap writes under does -- so both lines coexist and SDL picks the
checksummed one. That is the same asymmetry that lets padmap's virtual-pad
line win over a database entry for the physical controller, and it means the
return code says nothing useful about whether the live pad changed.

That the live pad *does* change is measured in check_sdl_live.py, with a real
press through a real uinput device.

Nothing here touches the running session: a throwaway XDG_CONFIG_HOME and
XDG_RUNTIME_DIR, on a virtual display.

Needs Xvfb and /dev/uinput.
"""

from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

import evdev  # noqa: E402
from evdev import ecodes  # noqa: E402

from padmap import mapping, protocol  # noqa: E402

PAD_NAME = "PADMAP RELOADTEST"
PAD_VID, PAD_PID, PAD_VERSION = 0x1209, 0x0004, 1
SIGNATURE = f"{PAD_VID:04x}:{PAD_PID:04x}:{PAD_NAME}"

FIRST_KEY = 0x130
CAPABILITIES = {
    ecodes.EV_KEY: list(range(FIRST_KEY, FIRST_KEY + 12)),
    ecodes.EV_ABS: [
        (0x00, evdev.AbsInfo(128, 0, 255, 0, 0, 0)),
        (0x01, evdev.AbsInfo(128, 0, 255, 0, 0, 0)),
    ],
}

# Deliberately not what Pegasus's own default assumes (a:b0). If the frontend
# ignored the event, the readback would still say b0 and look plausible.
A_BUTTON = 7


def pick_display() -> int:
    for number in range(90, 130):
        if not Path(f"/tmp/.X{number}-lock").exists():
            return number
    raise SystemExit("no free X display between :90 and :129")


DISPLAY_NUM = pick_display()


def build(attr: str) -> str:
    out = subprocess.run(
        ["nix", "build", "--no-link", "--print-out-paths", f".#{attr}"],
        cwd=REPO, capture_output=True, text=True, check=True)
    return out.stdout.strip().splitlines()[-1]


def guard_live_daemon(add: bool) -> None:
    """Creating a joystick node is not neutral while a daemon is watching."""
    path = protocol.prompted_path()
    try:
        lines = [line for line in path.read_text().splitlines() if line.strip()]
    except OSError:
        lines = []
    lines = [line for line in lines if line.strip() != SIGNATURE]
    if add:
        lines.append(SIGNATURE)
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("".join(f"{line}\n" for line in lines))
    except OSError:
        pass


class StubDaemon(threading.Thread):
    """Just enough of the protocol to answer `status` and push a mapping.

    A stub rather than the real daemon on purpose: the real one would have to
    be talked into a whole assignment session to reach the moment a mapping is
    written, and none of that is what is under test here. What is under test
    is the socket client inside Pegasus.
    """

    def __init__(self, runtime: Path, line: str) -> None:
        super().__init__(daemon=True)
        self.path = runtime / "padmap" / "padmap.sock"
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.line = line
        self.sent = threading.Event()
        self.connected = threading.Event()
        self._stop = False
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(str(self.path))
        self.server.listen(4)
        self.server.settimeout(0.5)

    def run(self) -> None:
        while not self._stop:
            try:
                client, _ = self.server.accept()
            except (TimeoutError, socket.timeout):
                continue
            except OSError:
                return
            self.connected.set()
            threading.Thread(target=self._serve, args=(client,),
                             daemon=True).start()

    def _serve(self, client: socket.socket) -> None:
        client.sendall(json.dumps({
            "event": "state", "state": "ready", "slots": 4, "players": [],
            "build": "stub", "pid": os.getpid(), "identity": "mirror",
        }).encode() + b"\n")
        # Let Pegasus finish enumerating and opening the pad first: the whole
        # question is whether a mapping arriving *after* that takes effect.
        time.sleep(4.0)
        client.sendall(json.dumps({
            "event": "sdl_mapping", "lines": [self.line],
        }).encode() + b"\n")
        self.sent.set()
        while not self._stop:
            time.sleep(0.2)

    def close(self) -> None:
        self._stop = True
        try:
            self.server.close()
        except OSError:
            pass
        self.path.unlink(missing_ok=True)


class xvfb:
    def __enter__(self):
        self.proc = subprocess.Popen(
            ["nix", "shell", "nixpkgs#xvfb", "--command",
             "Xvfb", f":{DISPLAY_NUM}", "-screen", "0", "1280x720x24"],
            cwd=REPO, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(2)
        return self

    def __exit__(self, *_):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def main() -> int:
    config = Path(tempfile.mkdtemp())
    runtime = Path(tempfile.mkdtemp())
    log_path = config / "pegasus.log"

    guid = mapping.sdl_guid(bus=ecodes.BUS_USB, vendor=PAD_VID,
                            product=PAD_PID, version=PAD_VERSION,
                            name=PAD_NAME)
    line = mapping.sdl_line(guid, PAD_NAME, {
        "a": f"b{A_BUTTON}", "b": "b1", "x": "b2", "y": "b3",
        "leftx": "a0", "lefty": "a1",
    })
    print(f"pad GUID {guid}\nline     {line}")

    print("\nbuilding the patched frontend...")
    pegasus = build("pegasus")

    (config / "pegasus-frontend").mkdir(parents=True, exist_ok=True)
    (config / "pegasus-frontend" / "settings.txt").write_text(
        "general.fullscreen: false\n")

    guard_live_daemon(add=True)
    ui = evdev.UInput(events=CAPABILITIES, name=PAD_NAME, phys="reload/0",
                      vendor=PAD_VID, product=PAD_PID, version=PAD_VERSION,
                      bustype=ecodes.BUS_USB, max_effects=0)
    time.sleep(0.6)

    daemon = StubDaemon(runtime, line)
    daemon.start()
    proc = None
    handle = None
    try:
        with xvfb():
            env = dict(
                os.environ,
                DISPLAY=f":{DISPLAY_NUM}",
                XDG_CONFIG_HOME=str(config),
                XDG_RUNTIME_DIR=str(runtime),
                # The wrapper otherwise restarts the user's live daemon.
                PADMAP_SKIP_DAEMON_CHECK="1",
            )
            handle = log_path.open("w")
            proc = subprocess.Popen(
                [f"{pegasus}/bin/pegasus-fe"], env=env, stdout=handle,
                stderr=subprocess.STDOUT)
            print("\nfrontend started; waiting for the mapping to be sent...")
            if not daemon.connected.wait(30):
                raise SystemExit(
                    "FAIL: the frontend never connected to the padmap socket")
            if not daemon.sent.wait(30):
                raise SystemExit("FAIL: the stub never sent the mapping")
            time.sleep(3)
    finally:
        if proc is not None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
        if handle is not None:
            handle.close()
        daemon.close()
        ui.close()
        guard_live_daemon(add=False)

    log = log_path.read_text(errors="replace")
    padmap_lines = [ln for ln in log.splitlines()
                    if "padmap" in ln.lower() or "gamepad" in ln.lower()
                    or "SDL2:" in ln]
    print("\nwhat the frontend logged:")
    for ln in padmap_lines:
        print("  " + ln.strip())
    if PAD_NAME not in log:
        raise SystemExit(
            "FAIL: the frontend never saw the test pad, so nothing here is "
            "about a live controller")

    applied = [ln for ln in padmap_lines if guid in ln]
    if not applied:
        raise SystemExit(
            f"FAIL: the frontend never applied a mapping for {guid}. The "
            f"event arrived on the socket and nothing acted on it.\n"
            f"--- log tail ---\n{log[-3000:]}")

    entry = applied[-1]
    if f"a:b{A_BUTTON}" not in entry:
        raise SystemExit(
            f"FAIL: SDL did not read the new binding back. A mapping stored "
            f"under a GUID nothing looks up is silent in exactly this way:\n"
            f"  {entry}")
    if "not readable back" in entry:
        raise SystemExit(f"FAIL: SDL could not return the mapping:\n  {entry}")

    print(f"\n  ok  the pad was open, and SDL now resolves its GUID to "
          f"padmap's line (a:b{A_BUTTON})")
    print("\nall checks passed")
    if "--keep" in sys.argv:
        print(f"kept: {config}  {runtime}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
