"""Does the favourite key work in the front-end that will actually run it?

Everything else about this feature is checked against a stub api, and a stub
api answers whatever it was written to answer. Three things can only be
learned from the real binary:

  * that `game.favorite` is writable from a theme at all, and that setting it
    makes Pegasus write ~/.config/pegasus-frontend/favorites.txt itself;
  * that `import QtQml.Models` -- which is how the library reorders 8302 games
    without copying them -- resolves in Pegasus's Qt build, and not only in
    the PySide6 harness the previews run under;
  * that a favourites file written by a previous session comes back as
    starred rows at the top of the collection.

Nothing here touches the running session or the installed theme: the whole
front-end runs against a throwaway XDG_CONFIG_HOME on a virtual display.

    python3 tools/e2e_favorites.py           # all of it
    python3 tools/e2e_favorites.py --keep    # leave the temp tree and shots

Needs Xvfb: Pegasus renders through OpenGL and the offscreen platform gives it
no input at all, so a real (virtual) display is required. Nothing appears on
your screen.
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

GAMES = ["Alpha Quest", "Beta Blaster", "Comet Chaser", "Delta Force",
         "Echo Valley", "Fable Fighter", "Gamma Ray", "Hyper Zone"]

FAILURES: list[str] = []


def check(name: str, got, want) -> None:
    if got == want:
        print(f"  ok    {name}")
    else:
        print(f"  FAIL  {name}: got {got!r}, wanted {want!r}")
        FAILURES.append(name)


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
        cwd=REPO, capture_output=True, text=True, check=True)
    return out.stdout.strip().splitlines()[-1]


def write_library(root: Path) -> Path:
    """A small collection of games that exist on disk.

    Real files, because the favourites provider writes the resolved absolute
    path of every game file and falls back to the raw string only for files it
    cannot stat -- checking the file it writes means giving it something to
    resolve.
    """
    games = root / "games"
    games.mkdir(parents=True, exist_ok=True)
    lines = ["collection: Test Bench", "extensions: rom",
             "launch: /bin/true", ""]
    for title in GAMES:
        rom = games / (title.lower().replace(" ", "_") + ".rom")
        rom.write_bytes(b"\0" * 16)
        lines += [f"game: {title}", f"file: {rom}", ""]
    (games / "metadata.pegasus.txt").write_text("\n".join(lines))
    return games


def write_config(config: Path, theme: str, games: Path) -> None:
    pegasus_dir = config / "pegasus-frontend"
    themes = pegasus_dir / "themes"
    themes.mkdir(parents=True, exist_ok=True)
    # A copy rather than a symlink into the store: this is the theme as the
    # flake ships it, and it must not be confused with the one the live
    # session has installed.
    shutil.copytree(Path(theme) / "share" / "pegasus-frontend" / "themes"
                    / "padmap", themes / "padmap")
    (pegasus_dir / "game_dirs.txt").write_text(f"{games}\n")
    (pegasus_dir / "settings.txt").write_text(
        "general.theme: themes/padmap/\n"
        "general.fullscreen: false\n"
    )


def rom_path(games: Path, title: str) -> str:
    return str(games / (title.lower().replace(" ", "_") + ".rom"))


class xvfb:
    """Xvfb for the duration of the run; nothing lands on the real display."""

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


def keys(*names: str) -> str:
    """Send keys to the Pegasus window.

    Focus and the keys go in one xdotool session, and the focus is set every
    time. There is no window manager on the virtual display, so the input
    focus starts at PointerRoot and a key sent with XTEST alone lands wherever
    the (nonexistent) pointer is -- which looks exactly like a theme that
    ignores the key.
    """
    script = ["W=$(xdotool search --onlyvisible --name Pegasus | tail -1)",
              'test -n "$W" || { echo "no window"; exit 1; }',
              'xdotool windowfocus "$W"', "sleep 0.3"]
    for name in names:
        script += [f"xdotool key --clearmodifiers {name}", "sleep 0.5"]
    script.append('echo "sent to $W"')
    env = dict(os.environ, DISPLAY=f":{DISPLAY_NUM}")
    out = subprocess.run(
        ["nix", "shell", "nixpkgs#xdotool", "--command", "bash", "-c",
         "\n".join(script)],
        cwd=REPO, env=env, capture_output=True, text=True)
    return out.stdout.strip()


def screenshot(path: Path) -> None:
    env = dict(os.environ, DISPLAY=f":{DISPLAY_NUM}")
    subprocess.run(["import", "-window", "root", str(path)],
                   env=env, capture_output=True)


def run(pegasus: str, env: dict, log_path: Path, seconds: float):
    handle = log_path.open("w")
    proc = subprocess.Popen([pegasus], env=env, stdout=handle,
                            stderr=subprocess.STDOUT)
    # The library has to be scanned and the first frame drawn before any key
    # means anything.
    time.sleep(seconds)
    return proc, handle


def stop(proc, handle) -> None:
    proc.terminate()
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
    handle.close()


def qml_complaints(log: str) -> list[str]:
    """Lines that mean the theme did not load cleanly.

    A QML type that the running Qt does not have is not fatal to the process:
    the screen is simply never created, the front-end shows a blank window and
    carries on. Only the log says so.
    """
    bad = []
    for line in log.splitlines():
        lowered = line.lower()
        if any(mark in lowered for mark in (
                "is not installed", "is not a type", "typeerror",
                "referenceerror", "cannot assign", "qml error",
                "unable to assign", "not a function")):
            bad.append(line.strip())
    return bad


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--keep", action="store_true",
                        help="leave the temp tree and the screenshots")
    parser.add_argument("--wait", type=float, default=8.0,
                        help="seconds to let Pegasus start up")
    args = parser.parse_args()

    print("building...")
    pegasus = build("pegasus") + "/bin/pegasus-fe"
    theme = build("pegasus-theme")
    print(f"  {pegasus}")
    print(f"  {theme}")

    root = Path(tempfile.mkdtemp(prefix="padmap-e2e-fav-"))
    config = root / "config"
    favorites = config / "pegasus-frontend" / "favorites.txt"
    shots = Path("/tmp") if not args.keep else root
    try:
        games = write_library(root)
        write_config(config, theme, games)

        env = dict(os.environ)
        env.update({
            "XDG_CONFIG_HOME": str(config),
            "XDG_DATA_HOME": str(root / "data"),
            "XDG_RUNTIME_DIR": str(root / "run"),
            "DISPLAY": f":{DISPLAY_NUM}",
        })
        (root / "run").mkdir(exist_ok=True)
        # The wrapper would otherwise start a daemon against this throwaway
        # runtime dir and leak it; this test is about the theme.
        env["PADMAP_SKIP_DAEMON_CHECK"] = "1"

        with xvfb():
            print("\nfirst run: nothing is a favourite yet")
            proc, handle = run(pegasus, env, root / "run1.log", args.wait)
            screenshot(shots / "e2e-fav-before.png")
            # The cursor starts on the first game. Page Down is PAGE_DOWN,
            # which is R2 on a pad.
            print("  " + keys("Down", "Down", "Page_Down"))
            # Taken at once: the line that explains the deferred reordering is
            # on a four second timer, and a screenshot that waits for the file
            # to be written misses it.
            screenshot(shots / "e2e-fav-marked.png")
            time.sleep(1.5)
            stop(proc, handle)

            log = (root / "run1.log").read_text(errors="replace")
            check("the theme loaded without QML complaints",
                  qml_complaints(log), [])
            check("Pegasus wrote a favourites file", favorites.is_file(), True)
            written = favorites.read_text() if favorites.is_file() else ""
            check("holding the game the cursor was on",
                  rom_path(games, GAMES[2]) in written, True)
            check("and only that one",
                  len([ln for ln in written.splitlines()
                       if ln and not ln.startswith("#")]), 1)

            print("\nsecond run: the file is read back")
            proc, handle = run(pegasus, env, root / "run2.log", args.wait)
            screenshot(shots / "e2e-fav-reloaded.png")
            # Marking a second game and unmarking nothing: the file should
            # grow, which is what proves the write path is not a one-off.
            print("  " + keys("Down", "Page_Down"))
            time.sleep(1.5)
            screenshot(shots / "e2e-fav-second.png")
            stop(proc, handle)

            log2 = (root / "run2.log").read_text(errors="replace")
            check("still no QML complaints", qml_complaints(log2), [])
            written = favorites.read_text()
            kept = [ln for ln in written.splitlines()
                    if ln and not ln.startswith("#")]
            check("the earlier favourite survived the restart",
                  rom_path(games, GAMES[2]) in written, True)
            check("and a second one was added", len(kept), 2)

        print(f"\nscreenshots in {shots}:")
        for name in ("e2e-fav-before.png", "e2e-fav-marked.png",
                     "e2e-fav-reloaded.png", "e2e-fav-second.png"):
            print(f"  {shots / name}")
        print("\nfavorites.txt:")
        print("   " + "\n   ".join(favorites.read_text().splitlines()))
    finally:
        if args.keep:
            print(f"\nkept: {root}")
        else:
            shutil.rmtree(root, ignore_errors=True)

    print()
    if FAILURES:
        print(f"{len(FAILURES)} failure(s): {', '.join(FAILURES)}")
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
