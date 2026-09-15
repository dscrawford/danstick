"""Choose a console, then map it -- through a real daemon, with real presses.

The picker and the wizard are both answered by pressing buttons on a pad the
daemon has grabbed, and that is precisely the part no stub can check: a socket
harness can call `choose_layout`, but nothing about it proves that pushing the
stick moves the selection or that holding a button starts the wizard on the
console that was showing. This drives the real `padmap serve`.

Safe to run on a live machine, by construction:

  * its own XDG_RUNTIME_DIR, XDG_CONFIG_HOME, profile directory and
    PADMAP_SDL_DB, so it cannot write to the daemon the user is running, nor
    to the SDL database every other SDL program on the machine reads
  * PADMAP_ONLY_DEVICE restricts discovery to one uinput pad this script
    creates and owns, so no real controller is ever grabbed
  * the pad's signature is written into the *live* daemon's `prompted` file
    first and removed afterwards. Creating a joystick node is not a neutral
    act while a daemon is watching for unfamiliar controllers -- without this,
    a setup screen opens on the user's television and every pad is grabbed.

    python3 tools/e2e_picker.py
    python3 tools/e2e_picker.py --keep    # leave the temp dirs for inspection
"""

import json
import os
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import evdev
from evdev import ecodes

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import controllercfg, protocol  # noqa: E402

# Deliberately not a name any real controller has, and padmap's own vid with a
# product id the virtual pads never use.
PAD_NAME = "PADMAP TESTPAD"
PAD_VID, PAD_PID = 0x1209, 0x0002
SIGNATURE = f"{PAD_VID:04x}:{PAD_PID:04x}:{PAD_NAME}"

# Buttons and axes enough to answer every prompt in the longest layout, and
# no further: 0x140 is BTN_TOOL_PEN, and a device carrying one is classified
# as a tablet rather than a joystick -- padmap then does not discover it at
# all, which looks exactly like the daemon being broken.
FIRST_KEY = 0x130
KEY_COUNT = 16
CAPABILITIES = {
    ecodes.EV_KEY: list(range(FIRST_KEY, FIRST_KEY + KEY_COUNT)),
    ecodes.EV_ABS: [
        (0x00, evdev.AbsInfo(128, 0, 255, 0, 0, 0)),
        (0x01, evdev.AbsInfo(128, 0, 255, 0, 0, 0)),
        (0x10, evdev.AbsInfo(0, -1, 1, 0, 0, 0)),
        (0x11, evdev.AbsInfo(0, -1, 1, 0, 0, 0)),
    ],
}

# Longer than capture.SKIP_HOLD_SECONDS, which is what separates "this is the
# button" from "skip" and, in the picker, a tap from a choice.
HOLD_SECONDS = 1.0
# Longer than capture.CAPTURE_GAP_SECONDS, the deliberate deadness after a
# control is recorded.
GAP_SECONDS = 0.45


class Daemon:
    """A padmap daemon of our own, and a socket client for it."""

    def __init__(self, runtime: str, config: str, profiles: str) -> None:
        self.runtime = runtime
        self.environment = dict(
            os.environ,
            XDG_RUNTIME_DIR=runtime, XDG_CONFIG_HOME=config,
            PADMAP_PROFILE_DIR=profiles,
            PADMAP_ONLY_DEVICE="TESTPAD",
            # This daemon is driven by hand; it must not decide to open a
            # session of its own halfway through one.
            PADMAP_NO_AUTOSETUP="1",
            PYTHONPATH=str(REPO / "src"),
        )
        self.process: subprocess.Popen | None = None
        self.sock: socket.socket | None = None
        self.events: list[dict] = []

    def start(self) -> None:
        self.process = subprocess.Popen(
            [sys.executable, "-m", "padmap.cli", "serve"],
            env=self.environment, stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
        for _ in range(50):
            try:
                self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                self.sock.connect(f"{self.runtime}/padmap/padmap.sock")
                break
            except OSError:
                self.sock = None
                time.sleep(0.2)
        if self.sock is None:
            errors = (self.process.stderr.read().decode()
                      if self.process.stderr else "")
            raise SystemExit(f"FAIL: daemon never came up\n{errors[-800:]}")
        self.pump(0.5)

    def pump(self, seconds: float = 0.4) -> None:
        assert self.sock is not None
        deadline = time.monotonic() + seconds
        buffer = b""
        self.sock.settimeout(0.1)
        while time.monotonic() < deadline:
            try:
                data = self.sock.recv(65536)
            except (TimeoutError, socket.timeout):
                continue
            if not data:
                break
            buffer += data
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                if line.strip():
                    self.events.append(json.loads(line))

    def send(self, **message: object) -> None:
        assert self.sock is not None
        self.sock.sendall((json.dumps(message) + "\n").encode())
        self.pump(0.4)

    def last(self, name: str) -> dict | None:
        for event in reversed(self.events):
            if event.get("event") == name:
                return event
        return None

    def close(self) -> None:
        if self.sock is not None:
            self.sock.close()
        if self.process is not None:
            self.process.terminate()
            self.process.wait(timeout=10)


class Pad:
    """A uinput controller this script can press."""

    def __init__(self) -> None:
        self.ui = evdev.UInput(
            events=CAPABILITIES, name=PAD_NAME, phys="testpad/0",
            vendor=PAD_VID, product=PAD_PID, version=1,
            bustype=ecodes.BUS_USB, max_effects=0,
        )
        time.sleep(0.5)

    def hold(self, code: int, seconds: float = HOLD_SECONDS) -> None:
        self.ui.write(ecodes.EV_KEY, code, 1)
        self.ui.syn()
        time.sleep(seconds)
        self.ui.write(ecodes.EV_KEY, code, 0)
        self.ui.syn()

    def tap(self, code: int) -> None:
        self.hold(code, 0.1)

    def push(self, code: int, value: int) -> None:
        """Deflect an axis and let it go."""
        self.ui.write(ecodes.EV_ABS, code, value)
        self.ui.syn()
        time.sleep(0.15)
        self.ui.write(ecodes.EV_ABS, code, 0 if code == 0x10 else 128)
        self.ui.syn()
        time.sleep(0.15)

    def close(self) -> None:
        self.ui.close()


def guard_live_daemon() -> None:
    """Tell the live daemon we have already been asked about this model."""
    path = protocol.prompted_path()
    try:
        existing = path.read_text()
    except OSError:
        existing = ""
    if SIGNATURE not in existing:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(existing + SIGNATURE + "\n")


def release_live_daemon() -> None:
    path = protocol.prompted_path()
    try:
        kept = [line for line in path.read_text().splitlines()
                if line.strip() and line.strip() != SIGNATURE]
    except OSError:
        return
    path.write_text("".join(f"{line}\n" for line in kept))


def check_scoped_mapping(daemon, pad, profile_dir: str, runtime: str) -> None:
    """A second mapping for one console only, and the launch that picks it.

    The reported case: one controller, a default mapping, and a different one
    for N64 games. Everything below goes through the real daemon and the real
    launch path, because the two halves are written by different processes at
    different times and a check that exercised either alone would pass while
    they disagreed -- which is invisible, since both produce a plausible file.
    """
    before = json.loads(
        next(Path(profile_dir).glob("*.json")).read_text())["mappings"][""]

    # Back to the setup screen, exactly as a user reaches it: accepting ended
    # the session, and the daemon refuses a picker without one because a
    # picker is driven by a pad it does not currently hold.
    print("\nre-opening setup and claiming again:")
    daemon.send(cmd="begin", players=4)
    pad.hold(FIRST_KEY)
    daemon.pump(0.6)
    claim = daemon.last("claim")
    if not claim or not claim["configured"]:
        raise SystemExit(
            f"FAIL: the pad should now report as configured ({claim})")
    print(f"  ok  player {claim['player']}, configured")

    print("\nasking what a second mapping is FOR:")
    daemon.send(cmd="choose_scope", player=1)
    choice = daemon.last("layout_choice")
    if not choice or not choice["active"] or choice.get("kind") != "scope":
        raise SystemExit(f"FAIL: the scope picker did not open ({choice})")
    scopes = [entry["id"] for entry in choice["choices"]]
    if scopes[0] != "" or "console:n64" not in scopes:
        raise SystemExit(f"FAIL: unexpected scopes offered: {scopes}")
    if not choice["choices"][0]["mapped"]:
        raise SystemExit(
            "FAIL: the default scope is not marked as already captured, so "
            "re-mapping it would look like a fresh choice")
    print(f"  ok  {choice['title']!r}: {', '.join(s or 'default' for s in scopes)}")

    target = scopes.index("console:n64")
    print(f"\nmoving to {choice['choices'][target]['label']!r}:")
    for _ in range(target):
        pad.push(0x10, 1)
        daemon.pump(0.3)
    choice = daemon.last("layout_choice")
    if choice["chosen"] != "console:n64":
        raise SystemExit(f"FAIL: landed on {choice['chosen']!r}")
    print(f"  ok  {choice['chosen']}")

    print("\nholding a button goes straight to the wizard, on the N64 pad:")
    # No second question: a console scope *is* the control set. Asking which
    # layout afterwards could only produce a contradiction.
    pad.hold(FIRST_KEY + 1)
    daemon.pump(0.8)
    if daemon.last("layout_choice")["active"]:
        raise SystemExit("FAIL: a second picker opened")
    walking = daemon.last("mapping")
    if walking["layout"]["id"] != "n64":
        raise SystemExit(
            f"FAIL: chose the N64 scope, wizard walks "
            f"{walking['layout']['id']!r}")
    print(f"  ok  {walking['total']} N64 controls to press")

    # A different physical button per control from the first pass, so the two
    # captures cannot be confused for one another downstream.
    for step in range(walking["total"]):
        time.sleep(GAP_SECONDS)
        pad.tap(FIRST_KEY + (KEY_COUNT - 1 - step) % KEY_COUNT)
        daemon.pump(0.3)
    daemon.pump(1.0)
    finished = daemon.last("mapping")
    if not finished.get("done") or not finished.get("stored"):
        raise SystemExit(
            f"FAIL: the second wizard stopped at {finished.get('index')}")

    stored = json.loads(next(Path(profile_dir).glob("*.json")).read_text())
    if "console:n64" not in stored["mappings"]:
        raise SystemExit(
            f"FAIL: nothing filed under console:n64 "
            f"({sorted(stored['mappings'])})")
    if stored["mappings"][""]["buttons"] != before["buttons"]:
        raise SystemExit(
            "FAIL: the N64 capture overwrote the controller's default. "
            "Saying 'and for N64, this instead' must not change what every "
            "other console does.")
    if stored["mappings"]["console:n64"]["buttons"] == before["buttons"]:
        raise SystemExit(
            "FAIL: the two captures came out identical, so nothing here "
            "actually distinguishes them")
    print(f"  ok  profile now holds {sorted(stored['mappings'])}")

    print("\nand the launch picks between them:")
    daemon.send(cmd="accept")
    daemon.pump(1.5)
    autoconfig = Path(runtime) / "padmap" / "autoconfig" / "udev"
    written = next(autoconfig.glob("*.cfg"))
    default_text = written.read_text()
    if "Mapping scope: default" not in default_text:
        raise SystemExit(
            f"FAIL: accept did not write the default mapping:\n{default_text}")

    # The real launch path, as padmap-play runs it: the core decides the
    # console, the ROM decides the game.
    roms = Path(tempfile.mkdtemp())
    rom = roms / "GoldenEye 007 (USA).z64"
    rom.write_text("")
    core = roms / "mupen64plus_next_libretro.so"
    core.write_text("")
    result = subprocess.run(
        [sys.executable, "-m", "padmap.launch", "--",
         "-L", str(core), str(rom)],
        env=daemon.environment, capture_output=True, text=True, timeout=60,
    )
    n64_text = written.read_text()
    if "Mapping scope: console:n64" not in n64_text:
        raise SystemExit(
            f"FAIL: an N64 launch left the default mapping in place.\n"
            f"stderr: {result.stderr}\n{n64_text}")
    if n64_text == default_text:
        raise SystemExit(
            "FAIL: the file did not change, so both consoles get the same "
            "bindings and the whole feature does nothing")
    if "Layout: Nintendo 64" not in n64_text:
        raise SystemExit(f"FAIL: wrong layout emitted:\n{n64_text}")
    print(f"  ok  {result.stderr.strip().splitlines()[-2]}")

    # A list of the last few launches, newest first: the scope picker offers
    # "...for this game" from the front of it.
    recorded = json.loads(
        (Path(runtime) / "padmap" / "lastgame.json").read_text())
    last = recorded["games"][0]
    if last["console"] != "n64" or "goldeneye" not in last["key"]:
        raise SystemExit(f"FAIL: the game was not recorded ({last})")
    print(f"  ok  recorded {last['title']!r} for the scope picker")

    print("\nand a launch on another console goes back to the default:")
    snes_core = roms / "snes9x_libretro.so"
    snes_core.write_text("")
    snes_rom = roms / "Super Metroid (USA).sfc"
    snes_rom.write_text("")
    subprocess.run(
        [sys.executable, "-m", "padmap.launch", "--",
         "-L", str(snes_core), str(snes_rom)],
        env=daemon.environment, capture_output=True, text=True, timeout=60,
    )
    if "Mapping scope: default" not in written.read_text():
        raise SystemExit(
            "FAIL: a SNES launch used the N64 mapping, so a per-console "
            "capture leaks into every other console")
    print("  ok  default mapping restored")


def main() -> int:
    config = tempfile.mkdtemp()
    profiles = tempfile.mkdtemp()
    runtime = tempfile.mkdtemp()
    # The generated SDL database is read by every SDL program on the machine,
    # so a run that rewrote the real one would hand all of them a mapping for
    # a pad that existed for eight seconds. Set before the daemon is built:
    # it inherits this environment.
    os.environ[controllercfg.ENV_SDL_DB] = str(
        Path(config) / "sdl_controllers.txt")

    guard_live_daemon()
    pad = Pad()
    daemon = Daemon(runtime, config, profiles)
    try:
        daemon.start()
        daemon.send(cmd="begin", players=4)
        state = daemon.last("state")
        if not state or state["state"] != "assigning":
            error = daemon.last("error")
            raise SystemExit(
                f"FAIL: no session opened ({state}); last error: {error}")
        print(f"session open, {daemon.last('pads')['count']} pad(s) visible")

        print("\nholding a button claims a slot:")
        pad.hold(FIRST_KEY)
        daemon.pump(0.5)
        claim = daemon.last("claim")
        if not claim or claim["player"] != 1:
            raise SystemExit(f"FAIL: no claim ({claim})")
        if claim["configured"]:
            raise SystemExit("FAIL: a brand new pad reported as configured")
        print(f"  ok  player {claim['player']} = {claim['name']}, unconfigured")

        print("\nthe picker opens on a guess, offering every layout:")
        daemon.send(cmd="choose_layout", player=1)
        choice = daemon.last("layout_choice")
        if not choice or not choice["active"]:
            raise SystemExit(f"FAIL: the picker did not open ({choice})")
        offered = [entry["id"] for entry in choice["choices"]]
        if not choice["choices"][0].get("layout", {}).get("controls"):
            raise SystemExit("FAIL: no controls sent, nothing to draw")
        print(f"  ok  on {choice['chosen']}, offering {', '.join(offered)}")

        print("\nthe d-pad moves the selection, one step per push:")
        for _ in range(2):
            pad.push(0x10, 1)
            daemon.pump(0.3)
        choice = daemon.last("layout_choice")
        if choice["index"] != 2:
            raise SystemExit(
                f"FAIL: two pushes moved to index {choice['index']}")
        print(f"  ok  {choice['chosen']}")

        print("\nand so does the analogue stick:")
        pad.push(0x00, 0)                       # fully left
        daemon.pump(0.3)
        if daemon.last("layout_choice")["index"] != 1:
            raise SystemExit("FAIL: the stick did not move the selection")
        pad.push(0x10, 1)
        daemon.pump(0.3)
        chosen = daemon.last("layout_choice")["chosen"]
        print(f"  ok  back to {chosen}")

        print("\nholding a button starts the wizard on what was showing:")
        pad.hold(FIRST_KEY + 1)
        daemon.pump(0.8)
        if daemon.last("layout_choice")["active"]:
            raise SystemExit("FAIL: the picker is still open")
        walking = daemon.last("mapping")
        if walking["layout"]["id"] != chosen:
            raise SystemExit(
                f"FAIL: chose {chosen}, wizard walks "
                f"{walking['layout']['id']} -- the picture the user chose "
                f"from is not the pad being asked about")
        print(f"  ok  {walking['layout']['id']}, asking for "
              f"{walking['label']!r} ({walking['total']} controls)")

        print("\nwalking it, one button per prompt:")
        for step in range(walking["total"]):
            time.sleep(GAP_SECONDS)
            pad.tap(FIRST_KEY + step)
            daemon.pump(0.3)
        daemon.pump(1.0)
        finished = daemon.last("mapping")
        if not finished.get("done") or not finished.get("stored"):
            raise SystemExit(
                f"FAIL: the wizard stopped at {finished.get('index')} of "
                f"{finished.get('total')}")
        print(f"  ok  {finished['total']} controls recorded and stored")

        print("\nthe capture is stored against the console that was chosen:")
        stored = [json.loads(path.read_text())
                  for path in Path(profiles).glob("*.json")]
        if not stored:
            raise SystemExit("FAIL: no profile written")
        profile = stored[0]
        if profile["layout"] != chosen:
            raise SystemExit(
                f"FAIL: stored under {profile['layout']!r}, not {chosen!r} -- "
                f"every console-specific key would be emitted for the wrong "
                f"core")
        print(f"  ok  {profile['signature']}: layout {profile['layout']}, "
              f"{len(profile['buttons'])} bindings")

        print("\naccepting writes both files:")
        daemon.send(cmd="accept")
        daemon.pump(1.5)
        sdl = controllercfg.sdl_config_path()
        lines = [line for line in sdl.read_text().splitlines()
                 if line and not line.startswith("#")]
        if len(lines) != 1 or "padmap Player 1" not in lines[0]:
            raise SystemExit(f"FAIL: SDL mapping not written ({lines})")
        # Mirrored identity by default, so the GUID is the source pad's -- a
        # line under any other one is never looked up, silently.
        from padmap import devices
        source = [p for p in devices.discover() if PAD_NAME in p.name]
        expected = controllercfg.virtual_guid(1, source[0]) if source else ""
        if not lines[0].startswith(expected):
            raise SystemExit(
                f"FAIL: written under {lines[0][:32]}, expected {expected}")
        print(f"  ok  SDL line under {expected}")

        profiles_written = list(
            (Path(runtime) / "padmap" / "autoconfig" / "udev").glob("*.cfg"))
        if not profiles_written:
            raise SystemExit("FAIL: no RetroArch profile written")
        text = profiles_written[0].read_text()
        if f'input_vendor_id = "{PAD_VID}"' not in text:
            raise SystemExit(
                "FAIL: the RetroArch profile claims ids the pad does not "
                "advertise")
        print(f"  ok  {profiles_written[0].name}, ids matching the pad")

        check_scoped_mapping(daemon, pad, profiles, runtime)

        print("\nall checks passed")
        return 0
    finally:
        daemon.close()
        pad.close()
        release_live_daemon()
        if "--keep" in sys.argv:
            print(f"kept: {runtime}  {config}  {profiles}")


if __name__ == "__main__":
    sys.exit(main())
