"""Does a game launched from Pegasus reach RetroArch with the right ports?

This is the test that should have existed two fixes ago. Both earlier attempts
at "one assigned controller must not produce four players" were reasoned from
source and shipped without ever running the real chain, and both were wrong in
a way a single run would have caught:

  * `input_libretro_device_pN = "0"` in the launch override does nothing --
    RetroArch only reads that key from `.rmp` remap files.
  * The fix that replaced it was correct but never executed, because the
    running daemon kept regenerating launch.cfg with the old code and Pegasus
    still pointed at the previously built padmap-play.
  * And once that was sorted, the exported collections still named a
    padmap-play store path from months earlier, so none of it reached the
    launcher that actually ran. Hence --installed.

So the whole chain is exercised here, end to end, with nothing stubbed:

    Pegasus  ->  padmap-play  ->  retroarch  ->  core

The core is `tools/probe_libretro.c`, which declares four controller ports --
exactly like every N64 core -- and prints the device type RetroArch assigns to
each. That is the only honest observable: mupen64plus's "Game controller N"
lines are printed during retro_load_game, before RetroArch has decided
anything, and do not change even when a port really is disconnected.

Pegasus drives itself. The generated theme launches the first game as soon as
the library finishes scanning, so the handoff is Pegasus's own, not a
subprocess call faked by this script.

    python3 tools/e2e_ports.py                 # 1 player, expects 1 populated port
    python3 tools/e2e_ports.py --players 2     # 2 players, expects 2
    python3 tools/e2e_ports.py --live          # use the running daemon's state
    python3 tools/e2e_ports.py --installed     # use the installed collections
    python3 tools/e2e_ports.py --keep          # leave the temp tree for poking

`--live --installed` together are the "is my actual machine fixed?" run: the
daemon's real launch.cfg/launch.args, and the launcher the real collections
name. That combination is what finally reproduced a report of four
controllers in Smash, long after every other angle said the bug was fixed --
the collections still invoked a padmap-play from before `--nodevice`
existed.

Needs Xvfb: RetroArch's `null` video driver aborts with "Cannot initialize
input driver", so a real (virtual) display is required. Nothing appears on
your screen.
"""

import argparse
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

SUMMARY = re.compile(r"PROBE: SUMMARY ports0-3=([-\d,]+)")


def pick_display() -> int:
    """First display number with no X lock file.

    Not a fixed number: Xvfb's lock outlives the process by a moment, so two
    of these tools run back to back would collide and the second would fail
    for a reason that has nothing to do with what it tests.
    """
    for number in range(90, 130):
        if not Path(f"/tmp/.X{number}-lock").exists():
            return number
    raise SystemExit("no free X display between :90 and :129")


DISPLAY_NUM = pick_display()


def build(attr: str) -> str:
    out = subprocess.run(
        ["nix", "build", "--no-link", "--print-out-paths", f".#{attr}"],
        cwd=REPO, capture_output=True, text=True,
    )
    if out.returncode != 0:
        raise SystemExit(f"nix build .#{attr} failed:\n{out.stderr}")
    return out.stdout.strip().splitlines()[-1]


def build_flake_input(flake_attr: str) -> str:
    out = subprocess.run(
        ["nix", "build", "--no-link", "--print-out-paths", flake_attr],
        cwd=REPO, capture_output=True, text=True,
    )
    if out.returncode != 0:
        raise SystemExit(f"nix build {flake_attr} failed:\n{out.stderr}")
    return out.stdout.strip().splitlines()[-1]


def compile_probe(dest: Path) -> Path:
    """Build the probe core against RetroArch's own libretro.h."""
    headers = Path(build_flake_input("nixpkgs#retroarch-bare.src"))
    so = dest / "probe_libretro.so"
    cmd = [
        "nix", "shell", "nixpkgs#gcc", "--command",
        "gcc", "-shared", "-fPIC", "-O1", "-o", str(so),
        str(REPO / "tools" / "probe_libretro.c"),
        "-I", str(headers / "libretro-common" / "include"),
    ]
    out = subprocess.run(cmd, cwd=REPO, capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit(f"probe core failed to build:\n{out.stderr}")
    return so


def write_padmap_state(runtime: Path, players: int) -> None:
    """Generate launch.cfg and launch.args exactly as padmap would.

    The pad enumeration is stubbed rather than taken from the machine, so the
    test says the same thing whether or not the daemon happens to be running
    and whatever is plugged in. `--live` skips this.
    """
    from padmap import retroarch
    from padmap.assign import Assignment
    from padmap.devices import Pad

    paths = {p: f"/dev/input/event{100 + p}" for p in range(1, players + 1)}
    order = {i: paths[i + 1] for i in range(players)}
    retroarch.visible_order = lambda: order  # type: ignore[assignment]

    assignments = [
        Assignment(
            player=p,
            pad=Pad(path=paths[p], name=f"pad{p}", phys="", uniq="",
                    vid=0x1209, pid=1, syspath=""),
            button=0,
        )
        for p in range(1, players + 1)
    ]

    state = runtime / "padmap"
    retroarch.write_launch_config(assignments, paths, state / "launch.cfg")
    retroarch.write_launch_args(assignments, paths, state / "launch.args")


def write_library(root: Path, play: Path, core: Path) -> Path:
    """A one-game Pegasus collection whose launch line goes through padmap."""
    games = root / "games"
    games.mkdir(parents=True, exist_ok=True)
    rom = games / "dummy.z64"
    rom.write_bytes(b"\x80\x37\x12\x40" + b"\0" * 1024)

    (games / "metadata.pegasus.txt").write_text(
        "collection: Probe\n"
        "extensions: z64\n"
        f"launch: {play} -L {core} " '"{file.path}"\n'
        "\n"
        "game: Probe Game\n"
        f"file: {rom}\n"
    )
    return games


def write_theme(config: Path) -> None:
    """A theme whose only job is to launch the first game and stop.

    Waiting on `api.allGames.count` rather than a fixed delay: the library
    scan is asynchronous and a timer long enough to be safe on a cold cache
    would make every run slow.
    """
    theme = config / "pegasus-frontend" / "themes" / "e2e"
    theme.mkdir(parents=True, exist_ok=True)
    (theme / "theme.cfg").write_text(
        "name: e2e\nauthor: padmap\nqmlpath: theme.qml\n"
    )
    (theme / "theme.qml").write_text("""
import QtQuick 2.0

FocusScope {
    focus: true

    Text {
        anchors.centerIn: parent
        color: "white"
        text: "padmap e2e"
    }

    Timer {
        id: waitForLibrary
        interval: 250
        running: true
        repeat: true
        property int ticks: 0
        onTriggered: {
            ticks += 1;
            if (api.allGames.count > 0) {
                running = false;
                console.log("E2E: launching " + api.allGames.get(0).title);
                api.allGames.get(0).launch();
            } else if (ticks > 80) {
                running = false;
                console.log("E2E: FAILED no games found after 20s");
                Qt.quit();
            }
        }
    }
}
""")
    (config / "pegasus-frontend" / "settings.txt").write_text(
        "general.theme: themes/e2e/\n"
        "general.fullscreen: false\n"
    )


def write_retroarch_config(config: Path, pristine: bool) -> None:
    """RetroArch's config for the run.

    Defaults to a copy of the real one, stale player bindings and all: the
    whole point of writing every player slot at launch is to override those,
    so a test against a pristine config would not exercise it.
    """
    dest = config / "retroarch"
    dest.mkdir(parents=True, exist_ok=True)
    real = Path.home() / ".config" / "retroarch" / "retroarch.cfg"
    if not pristine and real.is_file():
        shutil.copy(real, dest / "retroarch.cfg")
        note = f"copied from {real}"
    else:
        (dest / "retroarch.cfg").write_text(
            'menu_driver = "rgui"\n'
            'audio_driver = "null"\n'
            'video_fullscreen = "false"\n'
            'input_max_users = "8"\n'
        )
        note = "minimal, generated"
    # Keep the run quiet and non-destructive whatever the source config said.
    with (dest / "retroarch.cfg").open("a") as handle:
        handle.write(
            '\naudio_driver = "null"\n'
            'video_fullscreen = "false"\n'
            'config_save_on_exit = "false"\n'
        )
    print(f"  retroarch.cfg: {note}")


def run_pegasus(pegasus: str, env: dict, log_path: Path, timeout: int) -> None:
    with log_path.open("w") as handle:
        proc = subprocess.Popen(
            [pegasus], env=env, stdout=handle, stderr=subprocess.STDOUT)
        deadline = time.time() + timeout
        while time.time() < deadline:
            if proc.poll() is not None:
                return
            # The probe core shuts RetroArch down by itself; once the summary
            # has been printed there is nothing left to wait for.
            if SUMMARY.search(log_path.read_text(errors="replace")):
                break
            time.sleep(0.5)
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--players", type=int, default=1,
                        help="how many controllers padmap has assigned")
    parser.add_argument("--live", action="store_true",
                        help="use the running daemon's launch.cfg/launch.args "
                             "instead of generating them")
    parser.add_argument("--pristine", action="store_true",
                        help="start from a minimal retroarch.cfg rather than "
                             "a copy of the real one")
    parser.add_argument(
        "--installed", action="store_true",
        help="use the launcher from the *installed* Pegasus collections "
             "rather than a freshly built one. This is what actually runs "
             "when you pick a game, and it can be stale in ways a build "
             "cannot show.")
    parser.add_argument("--drop-args", action="store_true",
                        help="delete launch.args before running, reproducing "
                             "the pre-fix behaviour. The test must FAIL with "
                             "this; that is what proves it can catch the "
                             "regression at all.")
    parser.add_argument("--keep", action="store_true",
                        help="keep the temporary tree")
    parser.add_argument("--timeout", type=int, default=90)
    args = parser.parse_args()

    print("building...")
    pegasus = build("pegasus") + "/bin/pegasus-fe"

    # --live means "is the setup I actually have working?", so it uses the
    # installed wrapper a front-end would really spawn. Building a fresh one
    # would test the source and quietly pass while the deployed chain is
    # still broken -- which is exactly how this bug survived two fixes.
    play = Path(build("padmap-play")) / "bin" / "padmap-play"
    if args.installed:
        found = installed_player()
        if found is None:
            print("  no installed collections found; run padmap export-pegasus")
            return 1
        play = found
        print("  padmap-play: from the installed collections")
    elif args.live and os.environ.get("PADMAP_PLAY"):
        play = Path(os.environ["PADMAP_PLAY"])
        print("  padmap-play: from $PADMAP_PLAY (installed)")
    print(f"  pegasus:     {pegasus}")
    print(f"  padmap-play: {play}")
    if args.live:
        reads_args = "launch.args" in play.read_text(errors="replace")
        print(f"  wrapper reads launch.args: {reads_args}")

    root = Path(tempfile.mkdtemp(prefix="padmap-ports-"))
    try:
        core = compile_probe(root)
        print(f"  probe core:  {core}")

        config = root / "config"
        config.mkdir(parents=True, exist_ok=True)

        if args.live:
            runtime = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp"))
            state = runtime / "padmap"
            if not (state / "launch.cfg").is_file():
                print(f"\nNo launch config at {state / 'launch.cfg'}; "
                      "is the daemon running?")
                return 1
            print(f"\nusing LIVE padmap state in {state}")
        else:
            runtime = root / "run"
            (runtime / "padmap").mkdir(parents=True, exist_ok=True)
            runtime.chmod(0o700)
            write_padmap_state(runtime, args.players)
            print(f"\ngenerated padmap state for {args.players} player(s)")

        state = runtime / "padmap"
        if args.drop_args:
            (state / "launch.args").unlink(missing_ok=True)
            print("  --drop-args: launch.args removed, expecting FAIL")

        flags = (state / "launch.args").read_text().split() \
            if (state / "launch.args").is_file() else []
        print(f"  launch.args: {' '.join(flags) if flags else '(none)'}")

        write_library(root, play, core)
        write_theme(config)
        write_retroarch_config(config, args.pristine)
        (config / "pegasus-frontend" / "game_dirs.txt").write_text(
            f"{root / 'games'}\n")

        env = dict(os.environ)
        env.update({
            "XDG_RUNTIME_DIR": str(runtime),
            "XDG_CONFIG_HOME": str(config),
            "XDG_DATA_HOME": str(root / "data"),
            "DISPLAY": f":{DISPLAY_NUM}",
        })
        # PADMAP_ONLY_VIRTUAL would hide every physical pad from Pegasus and
        # leave the test with no controller at all; the theme drives itself,
        # so it is deliberately not set.
        env.pop("PADMAP_ONLY_VIRTUAL", None)

        # This test is about ports, not daemon freshness -- that is
        # e2e_daemon.py's job. Leaving the wrapper's check on would have it
        # start a daemon on whichever runtime dir is in play: a throwaway one
        # per run in the default mode (seven leaked daemons before this was
        # noticed), or, under --live, meddling with the real daemon the run
        # is supposed to be observing rather than managing.
        env["PADMAP_SKIP_DAEMON_CHECK"] = "1"

        log_path = root / "pegasus.log"
        print("\nrunning Pegasus (headless, on a virtual display)...")
        with xvfb():
            run_pegasus(pegasus, env, log_path, args.timeout)

        log = log_path.read_text(errors="replace")
        return report(log, args.players, log_path, args.keep)
    finally:
        reap_daemons(root)
        if args.live:
            release_live_session()
        if not args.keep:
            shutil.rmtree(root, ignore_errors=True)


class xvfb:
    """Xvfb for the duration of the run; nothing lands on the real display."""

    def __enter__(self):
        self.proc = subprocess.Popen(
            ["nix", "shell", "nixpkgs#xvfb", "--command",
             "Xvfb", f":{DISPLAY_NUM}", "-screen", "0", "800x600x24"],
            cwd=REPO,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        time.sleep(4)
        return self

    def __exit__(self, *_exc):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def release_live_session() -> None:
    """Put the real daemon back if this run made it open a session.

    `--live` starts a real Pegasus against the real runtime dir, so it is a
    settled client of the user's own daemon. If an unconfigured controller is
    attached, the daemon quite correctly opens setup for it -- and this test
    then kills Pegasus, leaving a session open with every pad grabbed and no
    front-end to finish it. Observed exactly that: the daemon left `idle`
    with nothing republished.
    """
    import socket

    from padmap import protocol

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.settimeout(2.0)
        sock.connect(str(protocol.socket_path()))
        sock.sendall(protocol.encode({"cmd": "status"}))
        reader = protocol.LineReader()
        state = None
        for _ in range(5):
            for message in reader.feed(sock.recv(65536)):
                if message.get("event") == "state":
                    state = message.get("state")
            if state is not None:
                break
        if state == protocol.STATE_ASSIGNING:
            sock.sendall(protocol.encode({"cmd": "cancel"}))
            print("  released the session this run opened on the live daemon")
    except OSError:
        pass
    finally:
        sock.close()


def installed_player() -> Path | None:
    """The launcher the installed Pegasus collections actually invoke.

    `export-pegasus` writes an absolute path into every `launch:` line, so
    what runs when a game is picked is whatever that file says -- not
    whatever the flake would build today. Those two drifted apart once and
    the difference was invisible from every other angle: the daemon was
    current, launch.args was correct, and the collections still pointed at a
    padmap-play from before `--nodevice` existed.
    """
    config = Path(
        os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")
    ) / "pegasus-frontend" / "game_dirs.txt"
    if not config.is_file():
        return None

    for line in config.read_text().splitlines():
        metadata = Path(line.strip()) / "metadata.pegasus.txt"
        if not line.strip() or not metadata.is_file():
            continue
        for entry in metadata.read_text(errors="replace").splitlines():
            if entry.startswith("launch:"):
                parts = shlex.split(entry[len("launch:"):].strip())
                if parts:
                    return Path(parts[0])
    return None


def reap_daemons(root: Path) -> None:
    """Stop any daemon this run left on its own temporary runtime dir.

    Belt and braces alongside PADMAP_SKIP_DAEMON_CHECK. `daemon_pids` scopes
    by runtime dir and matches argv structurally, so this can only ever reach
    a daemon this run created -- never the user's real one, and never a shell
    whose command line happens to mention padmap.
    """
    from padmap import protocol

    for pid in protocol.daemon_pids(str(root / "run")):
        try:
            os.kill(pid, 15)
            print(f"  stopped leftover test daemon pid {pid}")
        except OSError:
            pass


def report(log: str, players: int, log_path: Path, keep: bool) -> int:
    print()
    for line in log.splitlines():
        if "E2E:" in line or line.startswith("PROBE:"):
            print("  " + line.strip())

    if "E2E: launching" not in log:
        print("\nFAIL: Pegasus never launched the game.")
        print(f"  log: {log_path if keep else '(discarded; re-run --keep)'}")
        return 1

    match = SUMMARY.search(log)
    if not match:
        print("\nFAIL: the core never reported its ports -- did padmap-play "
              "reach RetroArch?")
        print(f"  log: {log_path if keep else '(discarded; re-run --keep)'}")
        return 1

    devices = [int(v) for v in match.group(1).split(",")]
    expected = [1 if i < players else 0 for i in range(4)]
    print(f"\n  core ports 0-3: {devices}")
    print(f"  expected:       {expected}   "
          f"({players} populated, rest RETRO_DEVICE_NONE)")

    if devices != expected:
        populated = sum(1 for d in devices if d == 1)
        print(f"\nFAIL: {populated} populated port(s), wanted {players}.")
        if all(d == 1 for d in devices):
            print("  Every port got a controller -- the --nodevice flags did "
                  "not reach RetroArch.")
            print("  Check that padmap-play is the freshly built one and that "
                  "launch.args exists.")
        print(f"  log: {log_path if keep else '(discarded; re-run --keep)'}")
        return 1

    print(f"\nPASS: {players} populated port(s), "
          f"{4 - players} emptied, through the real Pegasus handoff.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
