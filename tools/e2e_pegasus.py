"""End-to-end test of the padmap theme running inside real Pegasus.

Everything up to now has been testable except the part that keeps breaking:
the theme's own logic -- when it offers calibration, whether it drives the
daemon correctly, whether a press leaks into the wrong handler. Those bugs
only showed up by asking a human to press buttons and describe what happened.

This closes that gap. It runs the actual patched Pegasus binary headless
against an isolated daemon and a synthetic controller, then asserts on what
the daemon *observed* -- which is a faithful proxy for what the theme did,
because the theme's only way to affect anything is the socket.

Isolation is total: XDG_RUNTIME_DIR, XDG_CONFIG_HOME and XDG_DATA_HOME are
all redirected to a temp tree, so this never touches the real daemon, the
real profiles, or the user's Pegasus settings.

    python3 tools/e2e_pegasus.py [--keep]
"""

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import evdev
from evdev import ecodes

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))
from padmap import protocol  # noqa: E402

PAD_NAME = "padmap e2e pad"
PAD_VID, PAD_PID = 0x1209, 0x0009
REST = 128
AXIS_MIN, AXIS_MAX = 0, 255
REACH_LOW, REACH_HIGH = 40, 210


class Harness:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.runtime = root / "run"
        self.config = root / "config"
        self.data = root / "data"
        for path in (self.runtime, self.config, self.data):
            path.mkdir(parents=True, exist_ok=True)
        # Qt refuses a runtime dir that is not 0700 and falls back elsewhere.
        self.runtime.chmod(0o700)
        (self.runtime / "padmap").mkdir(exist_ok=True)

        self.env = dict(os.environ)
        self.env.update({
            "XDG_RUNTIME_DIR": str(self.runtime),
            "XDG_CONFIG_HOME": str(self.config),
            "XDG_DATA_HOME": str(self.data),
            "QT_QPA_PLATFORM": "offscreen",
            # Keep the test daemon off the machine's real controllers, which
            # a live daemon may already hold an exclusive grab on.
            "PADMAP_ONLY_DEVICE": PAD_NAME,
        })
        self.daemon: subprocess.Popen | None = None
        self.pegasus: subprocess.Popen | None = None
        self.pad: evdev.UInput | None = None
        self.client: socket.socket | None = None
        self.reader = protocol.LineReader()
        self.events: list[dict] = []

    # -- setup ------------------------------------------------------------

    def write_pegasus_config(self, theme_dir: Path) -> None:
        cfg = self.config / "pegasus-frontend"
        (cfg / "themes").mkdir(parents=True, exist_ok=True)
        link = cfg / "themes" / "padmap"
        if link.exists() or link.is_symlink():
            link.unlink()
        link.symlink_to(theme_dir)
        (cfg / "settings.txt").write_text(
            "general.theme: themes/padmap/\n"
            "general.fullscreen: false\n"
        )

        # A library is mandatory, not decoration. main.qml swaps the theme for
        # messages/NoGamesError.qml when api.collections.count is zero, so a
        # gameless Pegasus logs "Theme set to `padmap`" and then never runs a
        # line of it -- which looked exactly like the theme being broken.
        games = self.root / "games"
        games.mkdir(exist_ok=True)
        rom = games / "placeholder.bin"
        rom.write_bytes(b"")
        (games / "metadata.pegasus.txt").write_text(
            "collection: Test\n"
            "extensions: bin\n"
            "launch: /bin/true {file.path}\n"
            "\n"
            "game: Placeholder\n"
            f"file: {rom}\n"
        )
        (cfg / "game_dirs.txt").write_text(f"{games}\n")

    def create_pad(self) -> None:
        def absinfo(value):
            return evdev.AbsInfo(value, AXIS_MIN, AXIS_MAX, 0, 0, 0)

        self.pad = evdev.UInput(
            events={
                ecodes.EV_KEY: [ecodes.BTN_SOUTH, ecodes.BTN_EAST],
                ecodes.EV_ABS: [
                    (ecodes.ABS_X, absinfo(REST)),
                    (ecodes.ABS_Y, absinfo(REST)),
                ],
            },
            name=PAD_NAME, vendor=PAD_VID, product=PAD_PID,
        )
        time.sleep(0.8)  # let udev classify it

    def start_daemon(self, padmap_bin: str) -> None:
        self.daemon = subprocess.Popen(
            [padmap_bin, "serve"], env=self.env,
            stdout=open(self.root / "daemon.log", "w"),
            stderr=subprocess.STDOUT,
        )
        sock = self.runtime / "padmap" / "padmap.sock"
        for _ in range(80):
            if sock.exists():
                return
            time.sleep(0.1)
        raise RuntimeError("daemon never created its socket")

    def start_pegasus(self, pegasus_bin: str) -> None:
        self.pegasus = subprocess.Popen(
            [pegasus_bin], env=self.env,
            stdout=open(self.root / "pegasus.log", "w"),
            stderr=subprocess.STDOUT,
        )

    def connect(self) -> None:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(str(self.runtime / "padmap" / "padmap.sock"))
        sock.settimeout(0.1)
        self.client = sock

    # -- interaction ------------------------------------------------------

    def drain(self, seconds: float) -> None:
        assert self.client is not None
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            try:
                data = self.client.recv(65536)
            except socket.timeout:
                continue
            except OSError:
                break
            if not data:
                break
            self.events.extend(self.reader.feed(data))

    def send(self, message: dict) -> None:
        assert self.client is not None
        self.client.sendall(protocol.encode(message))

    def hold(self, seconds: float = 0.45, code: int = ecodes.BTN_SOUTH) -> None:
        assert self.pad is not None
        self.pad.write(ecodes.EV_KEY, code, 1)
        self.pad.syn()
        self.drain(seconds)
        self.pad.write(ecodes.EV_KEY, code, 0)
        self.pad.syn()
        self.drain(0.2)

    def tap(self, code: int = ecodes.BTN_EAST) -> None:
        self.hold(seconds=0.12, code=code)

    def sweep(self, seconds: float = 2.0) -> None:
        assert self.pad is not None
        end = time.monotonic() + seconds
        low = True
        while time.monotonic() < end:
            value = REACH_LOW if low else REACH_HIGH
            low = not low
            self.pad.write(ecodes.EV_ABS, ecodes.ABS_X, value)
            self.pad.write(ecodes.EV_ABS, ecodes.ABS_Y, value)
            self.pad.syn()
            self.drain(0.2)

    def phases(self) -> list[str]:
        return [e["phase"] for e in self.events
                if e.get("event") == "calibration"]

    def profile(self) -> dict | None:
        directory = self.data / "padmap" / "devices"
        for path in directory.glob("*.json"):
            try:
                return json.loads(path.read_text())
            except (OSError, ValueError):
                continue
        return None

    def close(self) -> None:
        for proc in (self.pegasus, self.daemon):
            if proc is not None and proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
        if self.client is not None:
            self.client.close()
        if self.pad is not None:
            self.pad.close()


def main() -> int:
    keep = "--keep" in sys.argv
    repo = Path(__file__).resolve().parent.parent

    def build(attr: str) -> str:
        out = subprocess.run(
            ["nix", "build", "--no-link", "--print-out-paths", f".#{attr}"],
            cwd=repo, capture_output=True, text=True,
        )
        if out.returncode != 0:
            raise RuntimeError(f"nix build .#{attr} failed:\n{out.stderr}")
        return out.stdout.strip().splitlines()[-1]

    print("building...")
    padmap_bin = build("padmap") + "/bin/padmap"
    pegasus_bin = build("pegasus") + "/bin/pegasus-fe"
    theme_dir = Path(build("pegasus-theme")) / "share/pegasus-frontend/themes/padmap"

    root = Path(tempfile.mkdtemp(prefix="padmap-e2e-"))
    harness = Harness(root)
    failures: list[str] = []

    try:
        harness.write_pegasus_config(theme_dir)
        harness.create_pad()
        harness.start_daemon(padmap_bin)
        harness.connect()
        harness.start_pegasus(pegasus_bin)

        # Pegasus needs a moment to load the theme and connect.
        harness.drain(4.0)

        pegasus_log = (root / "pegasus.log").read_text()
        if "Theme set to `padmap`" not in pegasus_log:
            failures.append("Pegasus did not load the padmap theme")

        # The theme opens the controller screen on the Details key. There is
        # no way to inject a keypress into an offscreen window, so drive the
        # daemon side directly -- what matters is what the theme does *after*
        # a session is open, which is where its logic lives.
        harness.send({"cmd": "begin", "players": 4})
        harness.drain(0.6)

        harness.hold()  # claim player 1
        harness.drain(0.6)

        claims = [e for e in harness.events if e.get("event") == "claim"]
        if not claims:
            failures.append("synthetic pad never claimed a slot")
        elif claims[0].get("configured") is not False:
            failures.append("a never-seen pad should report configured=false")

        # The theme should now have sent `calibrate` on its own. If it has,
        # the daemon is in await_rest.
        if "await_rest" not in harness.phases():
            failures.append(
                "theme did not start calibration for an unconfigured pad "
                f"(phases seen: {harness.phases() or 'none'})")
        else:
            harness.tap()          # -> rest
            harness.drain(1.3)
            harness.tap()          # -> reach
            harness.drain(0.4)
            harness.sweep()
            harness.tap()          # -> icon
            harness.drain(0.6)

            if "icon" not in harness.phases():
                failures.append("did not reach the icon step")

            harness.send({"cmd": "set_icon", "player": 1, "icon": "n64"})
            harness.drain(0.6)

            stored = harness.profile()
            if stored is None:
                failures.append("no profile written")
            else:
                axes = stored.get("axes", {})
                print(f"  profile: icon={stored.get('icon')!r} axes={len(axes)}")
                for code, axis in sorted(axes.items()):
                    print(f"    axis {code}: centre={axis['center']} "
                          f"reach={axis['reach_min']}..{axis['reach_max']}")
                if stored.get("icon") != "n64":
                    failures.append("icon not stored")
                if not axes:
                    failures.append("no axes calibrated")

        # Second pass: a configured pad must NOT be offered setup again.
        before = len(harness.phases())
        harness.send({"cmd": "cancel"})
        harness.drain(0.4)
        harness.send({"cmd": "begin", "players": 4})
        harness.drain(0.6)
        harness.hold()
        harness.drain(0.8)
        if len(harness.phases()) != before:
            failures.append(
                "already-configured pad was offered calibration again")

        crashed = [n for n, p in (("daemon", harness.daemon),
                                  ("pegasus", harness.pegasus))
                   if p is not None and p.poll() is not None]
        if crashed:
            failures.append(f"process exited early: {', '.join(crashed)}")
    finally:
        harness.close()
        if keep:
            print(f"\nlogs kept in {root}")
        else:
            shutil.rmtree(root, ignore_errors=True)

    if failures:
        print("\nFAIL:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print("\nPASS: theme drove calibration on first sight and stayed quiet after")
    return 0


if __name__ == "__main__":
    sys.exit(main())
