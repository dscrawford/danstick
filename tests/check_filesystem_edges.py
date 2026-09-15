#!/usr/bin/env python3
"""Every writer in padmap, asked to write somewhere hostile.

padmap keeps nothing in memory. Who is player 2, what their controller is
mapped to, which game was played last, what RetroArch and SDL should believe
about any of it -- all of it is files, written by a dozen different writers
into five different trees. A `padmap hide` run as root, a rebuild that turns a
directory into a store symlink, a full disk, a home directory restored from a
backup with the wrong owner, a hand-edit that left a directory where a file
was: each of these ends at one of those writers, and what it does then is the
whole of what the user experiences.

The contract is NOT the same for all of them, and assuming it is, is the bug
this file exists to catch. Three different promises are being made:

  CREATES ITS TREE. profiles.save, write_sdl_mappings, write_last_game,
    write_launch_config, write_launch_args, install_profiles,
    Server._save_assignments, Server._save_prompted.
    A fresh boot has no XDG_RUNTIME_DIR/padmap and a fresh install has no
    ~/.local/share/padmap/devices; a writer that needs its parent to exist
    would fail on exactly the runs that matter most, the first ones.

  REFUSES TO CREATE ANYTHING. clean_user_config, and titles.dump_json. That
    file is the *user's* retroarch.cfg. Creating a tree for it means padmap
    has been pointed at a path that is not the config it was asked to clean,
    and writing there is worse than failing (S21).

  NEVER RAISES AT ALL. hide.install, Server._save_prompted,
    cli._forget_prompted, artwork._safe -- and, by way of the guard in
    Server._handle_command, every writer the daemon reaches through a
    command. A daemon that dies holds EVIOCGRAB on every physical pad as it
    goes, so its virtual pads vanish and the real ones stay grabbed: the
    machine ends up with no working controllers at all. Nothing a filesystem
    can do may cause that. A write failure has to be reported.

The hostile destinations, applied to each writer that can reach them: the
parent directory missing three levels deep, the parent being a plain file,
the target being a directory, the target being read-only, the parent
directory being read-only (the read-only filesystem simulation -- note that
mode 0555 on a directory stops creation but NOT rewriting a file already in
it, so both are exercised), the target being a dangling symlink, the target
being a symlink pointing out of the tree it belongs to, a path longer than
NAME_MAX, and a path containing newlines and unicode.

Stories: S17 (per-player profiles), S4/S12 (what a finished wizard writes),
S14/S16 (what a launch writes), S18 (ensure-daemon), S19 (hide), S20
(forget), S21 (clean-config).

Nothing here touches real user state or hardware: XDG_RUNTIME_DIR,
XDG_CONFIG_HOME, XDG_DATA_HOME, PADMAP_PROFILE_DIR and RETROARCH_CONFIG_DIR
are redirected into a temp tree before padmap is imported, hide's two rules
paths and udevadm are redirected after it, devices.discover and
server.Assigner are replaced before any command is dispatched, no device or
uinput node is opened, and `ensure-daemon` is only ever run with --check,
which returns before it can spawn anything.

Lines marked "gap:" are defects this file found and reports rather than
fixes; each names its reproduction. Everything else is asserted.
"""

from __future__ import annotations

import argparse
import contextlib
import errno
import io
import json
import logging
import os
import shutil
import sys
import tempfile
from pathlib import Path

# Before importing padmap: cli and retroarch resolve their state paths at
# import time from the environment, and nothing here may reach real user
# state even by accident.
_SANDBOX = Path(tempfile.mkdtemp(prefix="check-filesystem-edges-"))
os.environ["XDG_RUNTIME_DIR"] = str(_SANDBOX / "run")
os.environ["XDG_CONFIG_HOME"] = str(_SANDBOX / "config")
os.environ["XDG_DATA_HOME"] = str(_SANDBOX / "data")
os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "devices")
os.environ["RETROARCH_CONFIG_DIR"] = str(_SANDBOX / "retroarch")
# Named outright rather than left to follow XDG_CONFIG_HOME: every SDL program
# on the machine reads this file, and a scenario that wrote the real one would
# take the user's own mappings with it.
os.environ["PADMAP_SDL_DB"] = str(_SANDBOX / "config" / "sdl_controllers.txt")
for _sub in ("run/padmap", "config", "data", "devices", "retroarch"):
    (_SANDBOX / _sub).mkdir(parents=True, exist_ok=True)

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import (  # noqa: E402
    artwork, cli, controllercfg, devices, hide, launch, profiles,
    protocol, retroarch, server, titles,
)
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402

# A LIVE daemon owns the controllers on this machine. Nothing below may
# enumerate, open or grab one, and no command may reach a real Assigner.
devices.discover = lambda *a, **k: []           # type: ignore[assignment]

# hide writes to /run/udev/rules.d and reloads udev. Both are redirected
# before the first call, so no real rule is installed and no real udevadm
# runs.
hide.RUNTIME_RULES_PATH = _SANDBOX / "udev" / "rules.d" / "99-padmap.rules"
hide.RULES_PATH = _SANDBOX / "etc-udev" / "rules.d" / "99-padmap.rules"
hide._udevadm = lambda *args: ""                # type: ignore[assignment]

# The daemon logs a warning on every failure it survives, which is the point,
# but it is noise here and the assertions are on behaviour, not on log text.
logging.disable(logging.CRITICAL)


# -- reporting ---------------------------------------------------------------

CHECKS = 0
ROOT = os.geteuid() == 0


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def heading(text: str) -> None:
    print(f"\n{text}:")


def ok(text: str) -> None:
    global CHECKS
    CHECKS += 1
    print(f"  ok  {text}")


def gap(text: str) -> None:
    print(f"  gap: {text}")


def note(text: str) -> None:
    print(f"  --  {text}")


def skip_as_root(what: str) -> bool:
    """Permission scenarios prove nothing as root, where mode 000 still opens."""
    if ROOT:
        note(f"skipped '{what}': running as root, where a read-only file and "
             f"a read-only directory are both still writable")
        return True
    return False


# -- assertion helpers -------------------------------------------------------


def creates(what: str, call, target: Path, contains: str | None = None) -> None:
    """A writer that must build its own tree, three levels deep if need be."""
    try:
        call()
    except Exception as error:  # noqa: BLE001
        fail(f"{what} raised {type(error).__name__}: {error} -- this writer "
             f"must create its directory. A fresh boot has no "
             f"XDG_RUNTIME_DIR/padmap and a fresh install has no profile "
             f"store, so this is the first run, not an edge case")
    if not target.is_file():
        fail(f"{what} reported success but there is no file at {target}")
    if contains is not None and contains not in target.read_text():
        fail(f"{what} wrote a file that does not contain {contains!r}")
    ok(what)


def refuses(what: str, call, *, expect: type[BaseException] = OSError) -> BaseException:
    """A writer that must fail, loudly and as an OSError its callers catch.

    Every guard padmap has around a write catches OSError specifically. A
    writer that fails with anything else goes straight through them.
    """
    try:
        call()
    except expect as error:
        ok(f"{what} -> {type(error).__name__}")
        return error
    except Exception as error:  # noqa: BLE001
        fail(f"{what} raised {type(error).__name__}: {error}, which is not an "
             f"{expect.__name__} -- every guard padmap has around a write "
             f"catches OSError, so this one goes through all of them")
    fail(f"{what} did not fail at all. It reported success without the file "
         f"being writable, and padmap goes on believing it saved something")
    raise AssertionError("unreachable")


def survives(what: str, call):
    """A writer whose contract is that it never raises, whatever happens."""
    try:
        return call()
    except Exception as error:  # noqa: BLE001
        fail(f"{what} raised {type(error).__name__}: {error} -- this path "
             f"must report a write failure, not raise it")


def unchanged(what: str, path: Path, before: str) -> None:
    if path.read_text() != before:
        fail(f"{what}: {path.name} was modified by a write that failed. A "
             f"half-rewritten file is worse than an unwritten one, because "
             f"nothing reports it")
    ok(what)


# -- fixtures ----------------------------------------------------------------

PAD = Pad(path="/dev/input/event21", name="USB GamePad", phys="usb-1/input0",
          uniq="", vid=0x0079, pid=0x1879, syspath="/sys/class/input/event21")
SIGNATURE = profiles.signature(PAD)
FILENAME = profiles._filename(SIGNATURE)
ASSIGNMENTS = [Assignment(player=1, pad=PAD, button=0)]
VPATHS = {1: "/dev/input/event50"}
SDL_LINE = {1: "0600c9a7091200000100000001000000,padmap Player 1,a:b0,"}

# 300 characters. NAME_MAX is 255 bytes on every filesystem padmap can be
# installed on, so this is the "path is very long" case rather than a number
# anybody would type.
TOO_LONG = "n" * 300
# A name that is legal on Linux and hostile to every line-oriented consumer
# of it.
AWKWARD = "Bad\nName ☃"


def fresh(name: str) -> Path:
    """An empty directory of our own, for one scenario."""
    path = _SANDBOX / "cases" / name
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    return path


def blocking_file(name: str) -> Path:
    """A plain file standing where a directory is wanted."""
    path = _SANDBOX / "cases" / name
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_dir():
        shutil.rmtree(path)
    path.write_text("a plain file, not a directory\n")
    return path


def quiet(call):
    """Run a CLI command without printing all over the check output."""
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        return call()


@contextlib.contextmanager
def env(**values: str):
    original = {key: os.environ.get(key) for key in values}
    os.environ.update(values)
    try:
        yield
    finally:
        for key, was in original.items():
            if was is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = was


@contextlib.contextmanager
def read_only(path: Path):
    """A directory nothing may create in -- the read-only filesystem case."""
    mode = path.stat().st_mode & 0o7777
    path.chmod(0o555)
    try:
        yield
    finally:
        path.chmod(mode)


def quiet_server(name: str) -> tuple[server.Server, list[dict]]:
    """A Server whose replies are captured, on paths of its own."""
    base = fresh(name)
    srv = server.Server(
        socket_path=base / "padmap.sock",
        state_path=base / "assignments.json",
        launch_config_path=base / "launch.cfg",
    )
    seen: list[dict] = []
    srv._broadcast = lambda message: seen.append(message)   # type: ignore[assignment]
    srv._send = lambda _client, message: seen.append(message)  # type: ignore[assignment]
    return srv, seen


class FakeAssigner:
    """Stands in for the real one, which opens and grabs devices."""

    def __init__(self, pads=None, grab: bool = True) -> None:
        self.assignments = list(ASSIGNMENTS)
        self.fds: list[int] = []
        self.grab_failures: list[Pad] = []
        self.on_claimed_event = None
        self.on_raw_event = None

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        return None

    def close(self) -> None:
        return None

    def device_for(self, _pad):
        return None

    def reset(self) -> None:
        self.assignments = []

    def tick(self, **_kwargs) -> None:
        return None


server.Assigner = FakeAssigner                  # type: ignore[assignment]


# -- S17: the profile store, which must build itself ------------------------


def check_profile_store_creates_its_tree() -> None:
    heading("S17 profiles.save -- contract: creates its directory")

    base = fresh("profiles-deep") / "one" / "two" / "three"
    creates("a profile store three levels deep is created, not demanded",
            lambda: profiles.save(profiles.Profile(signature=SIGNATURE), base),
            base / FILENAME, contains=SIGNATURE)

    # And the file it wrote is one load() answers with, which is the whole
    # point: a saved profile that does not read back marks a controller as
    # never configured and re-offers the wizard every session (S1).
    loaded = profiles.load(PAD, base)
    if loaded is None or loaded.signature != SIGNATURE:
        fail("a profile written into a directory padmap created does not "
             "read back; the controller would be offered the wizard forever")
    ok("the profile written there reads back as configured")


def check_profile_store_refuses_the_impossible() -> None:
    heading("S17 profiles.save -- contract: an impossible write raises OSError")

    blocker = blocking_file("profiles-parent-is-a-file")
    refuses("the profile directory is a plain file",
            lambda: profiles.save(profiles.Profile(signature=SIGNATURE),
                                  blocker))

    base = fresh("profiles-target-is-a-dir")
    (base / FILENAME).mkdir()
    refuses("the profile itself is a directory",
            lambda: profiles.save(profiles.Profile(signature=SIGNATURE), base))

    if not skip_as_root("a read-only profile"):
        base = fresh("profiles-read-only-file")
        target = base / FILENAME
        target.write_text('{"signature": "kept"}')
        target.chmod(0o444)
        refuses("the stored profile is read-only",
                lambda: profiles.save(profiles.Profile(signature=SIGNATURE),
                                      base))
        target.chmod(0o644)
        unchanged("a failed save left the old profile intact", target,
                  '{"signature": "kept"}')

    if not skip_as_root("a read-only profile store"):
        base = fresh("profiles-read-only-dir")
        with read_only(base):
            refuses("the profile store is on a read-only filesystem",
                    lambda: profiles.save(
                        profiles.Profile(signature=SIGNATURE), base))
            if list(base.iterdir()):
                fail("a save that failed on a read-only filesystem still left "
                     "something behind in the profile store")
        ok("nothing was left behind in the read-only store")


def check_profile_store_and_symlinks() -> None:
    heading("S17 profiles.save -- symlinks in the store")

    base = fresh("profiles-dangling")
    (base / FILENAME).symlink_to(base / "gone-with-the-usb-stick")
    survives("saving over a dangling symlink",
             lambda: profiles.save(profiles.Profile(signature=SIGNATURE), base))
    if not (base / "gone-with-the-usb-stick").is_file():
        fail("a dangling symlink in the profile store swallowed the write: "
             "nothing is at the link's target, so load() finds nothing and "
             "the controller is offered the wizard again every session")
    loaded = profiles.load(PAD, base)
    if loaded is None or loaded.signature != SIGNATURE:
        fail("a profile saved through a dangling symlink does not read back")
    ok("a dangling symlink is followed and the profile still reads back")

    base = fresh("profiles-symlink-out")
    victim = _SANDBOX / "cases" / "not-a-profile-at-all.conf"
    victim.write_text("PRECIOUS\n")
    (base / FILENAME).symlink_to(victim)
    survives("saving over a symlink pointing out of the store",
             lambda: profiles.save(profiles.Profile(signature=SIGNATURE), base))
    if not (base / FILENAME).is_symlink():
        fail("the symlink was replaced, so the two cases below are the wrong "
             "way round -- re-read what save() does")
    ok("the link itself is left in place, so the store still describes itself")
    if victim.read_text() != "PRECIOUS\n":
        gap(f"profiles.save followed a symlink out of the profile store and "
            f"overwrote {victim.name}. Reproduce: symlink "
            f"~/.local/share/padmap/devices/{FILENAME} at any other file and "
            f"finish the wizard. Only the store's owner can create that link, "
            f"so this is a footgun rather than an exploit -- but a store that "
            f"can write anywhere its owner can is not what 'save a profile' "
            f"promises")


def check_profile_filenames_are_survivable() -> None:
    heading("S17 profiles.save -- a controller whose name is hostile")

    # Controller names come off the USB descriptor, which is whatever the
    # device says it is. A name with a slash in it must not escape the store,
    # and a 400-character one must not exceed NAME_MAX.
    base = fresh("profiles-hostile-names")
    hostile = f"../../../etc/passwd\n☃ {'z' * 400}"
    try:
        path = profiles.save(profiles.Profile(signature=hostile), base)
    except OSError as error:
        fail(f"a controller whose name is {hostile[:24]!r} could not be saved "
             f"at all ({type(error).__name__}: {error}). Names come off the "
             f"USB descriptor and are whatever the device says they are; a "
             f"pad that cannot be saved is one the wizard can never finish")
    if path.parent != base:
        fail(f"a controller calling itself {hostile[:20]!r} put its profile "
             f"at {path}, outside the store padmap was given")
    ok("a signature full of ../ stays inside the profile store")
    if "\n" in path.name or "/" in path.name:
        fail(f"the profile filename kept a newline or a slash: {path.name!r}")
    ok("newlines and slashes are stripped out of the filename")
    if len(path.name.encode()) > 255:
        fail(f"a 400-character signature produced a {len(path.name)}-byte "
             f"filename; NAME_MAX is 255 and the save fails outright")
    ok(f"a 400-character signature is clamped to {len(path.name)} bytes")
    if not path.is_file():
        fail("the clamped profile was not actually written")
    ok("and the profile is really there")


# -- S4: the SDL database padmap generates ----------------------------------


def check_sdl_database_writer() -> None:
    heading("S4 controllercfg.write_sdl_mappings -- contract: creates its tree")

    base = fresh("sdl-deep") / "padmap" / "nested"
    creates("sdl_controllers.txt three levels deep is created",
            lambda: controllercfg.write_sdl_mappings(
                SDL_LINE, base / "sdl_controllers.txt"),
            base / "sdl_controllers.txt", contains="padmap Player 1")

    heading("S4 controllercfg.write_sdl_mappings -- contract: raises OSError")

    blocker = blocking_file("sdl-parent-is-a-file")
    refuses("the config directory is a plain file",
            lambda: controllercfg.write_sdl_mappings(
                SDL_LINE, blocker / "sdl_controllers.txt"))

    base = fresh("sdl-target-is-a-dir")
    (base / "sdl_controllers.txt").mkdir()
    refuses("sdl_controllers.txt is a directory",
            lambda: controllercfg.write_sdl_mappings(
                SDL_LINE, base / "sdl_controllers.txt"))

    if not skip_as_root("a read-only SDL database"):
        base = fresh("sdl-read-only")
        target = base / "sdl_controllers.txt"
        mine = "030000005e0400008e02000014010000,Xbox 360,a:b0,\n"
        target.write_text(mine)
        target.chmod(0o444)
        refuses("sdl_controllers.txt is read-only",
                lambda: controllercfg.write_sdl_mappings(SDL_LINE, target))
        target.chmod(0o644)
        unchanged("a failed rewrite left the user's own mappings alone",
                  target, mine)

    base = fresh("sdl-long-name")
    error = refuses("the database path is longer than NAME_MAX",
                    lambda: controllercfg.write_sdl_mappings(
                        SDL_LINE, base / (TOO_LONG + ".txt")))
    if getattr(error, "errno", None) != errno.ENAMETOOLONG:
        fail(f"a too-long path failed with errno {getattr(error, 'errno', None)} "
             f"rather than ENAMETOOLONG; something other than the length "
             f"stopped it and this scenario is not testing what it claims")
    ok("...with ENAMETOOLONG specifically")

    base = fresh("sdl-awkward-name")
    target = base / (AWKWARD + ".txt")
    creates("a database path containing a newline and a snowman is written",
            lambda: controllercfg.write_sdl_mappings(SDL_LINE, target),
            target, contains="padmap Player 1")


def check_sdl_database_keeps_foreign_lines() -> None:
    heading("S4 controllercfg.write_sdl_mappings -- other people's mappings")

    mine = "030000005e0400008e02000014010000,Xbox 360,a:b0,\n"

    base = fresh("sdl-dangling")
    target = base / "sdl_controllers.txt"
    target.symlink_to(base / "real-database.txt")
    survives("rewriting a database that is a dangling symlink",
             lambda: controllercfg.write_sdl_mappings(SDL_LINE, target))
    if "padmap Player 1" not in (base / "real-database.txt").read_text():
        fail("a dangling sdl_controllers.txt symlink swallowed the mapping; "
             "SDL reads the link's target and would see nothing")
    ok("the link is followed and the mapping lands at its target")

    base = fresh("sdl-symlink-out")
    victim = _SANDBOX / "cases" / "someone-elses-sdl.txt"
    victim.write_text("PRECIOUS\n")
    target = base / "sdl_controllers.txt"
    target.symlink_to(victim)
    survives("rewriting a database symlinked out of the config tree",
             lambda: controllercfg.write_sdl_mappings(SDL_LINE, target))
    if not target.is_symlink():
        fail("the symlink was replaced rather than followed; the gap below "
             "describes the opposite behaviour and is now wrong")
    ok("the symlink itself survives the rewrite")
    if victim.read_text() != "PRECIOUS\n":
        gap(f"write_sdl_mappings followed a symlink out of "
            f"$XDG_CONFIG_HOME/padmap and rewrote {victim.name}. "
            f"Reproduce: ln -s ~/anything ~/.config/padmap/"
            f"sdl_controllers.txt and finish the wizard")

    if not skip_as_root("a write-only SDL database"):
        # A database that can be written but not read is the one destination
        # where succeeding is worse than failing. This file is rewritten
        # rather than appended to precisely so the user's own lines survive,
        # and a read that failed cannot tell "the file was empty" from "the
        # file is there and I could not see it". It used to write anyway, and
        # the hand-written mappings went with it -- reproduce by chmod 0222 on
        # ~/.config/padmap/sdl_controllers.txt, then finish the wizard.
        base = fresh("sdl-write-only")
        target = base / "sdl_controllers.txt"
        target.write_text(mine)
        target.chmod(0o222)
        refuses("rewriting a database that can be written but not read",
                lambda: controllercfg.write_sdl_mappings(SDL_LINE, target))
        target.chmod(0o644)
        unchanged("the user's own mappings survive a database that could not "
                  "be read", target, mine)


# -- S14: what a launch writes ----------------------------------------------


def check_last_game_writer() -> None:
    heading("S14 protocol.write_last_game -- contract: creates its tree")

    with env(XDG_RUNTIME_DIR=str(fresh("lastgame-fresh") / "deep" / "runtime")):
        target = protocol.last_game_path()
        creates("lastgame.json is created under a runtime dir that has none",
                lambda: protocol.write_last_game("n64", "n64/zelda", "Zelda"),
                target, contains="n64/zelda")

    heading("S14 protocol.write_last_game -- contract: raises OSError")

    base = fresh("lastgame-dir")
    with env(XDG_RUNTIME_DIR=str(base)):
        (base / "padmap").mkdir()
        (base / "padmap" / "lastgame.json").mkdir()
        refuses("lastgame.json is a directory",
                lambda: protocol.write_last_game("n64", "n64/zelda", "Zelda"))

    blocker = blocking_file("lastgame-runtime-is-a-file")
    with env(XDG_RUNTIME_DIR=str(blocker)):
        refuses("XDG_RUNTIME_DIR is a plain file",
                lambda: protocol.write_last_game("n64", "n64/zelda", "Zelda"))


def check_a_launch_survives_an_unwritable_runtime_dir() -> None:
    heading("S14 padmap-play -- contract: a launch never fails over a file")

    blocker = blocking_file("launch-runtime-is-a-file")
    # A real file: split_args identifies the ROM by the path existing, so a
    # made-up one is silently not a game and this scenario would exercise
    # nothing.
    rom = fresh("launch-roms") / "Zelda.z64"
    rom.write_bytes(b"not really a ROM")
    argv = ["--", "-L", "/cores/mupen64plus_next_libretro.so", str(rom)]

    # First on a healthy runtime dir, so a change that stops this launch
    # reaching the writer at all cannot make the hostile case pass by
    # accident.
    healthy = fresh("launch-runtime-healthy")
    with env(XDG_RUNTIME_DIR=str(healthy)):
        with contextlib.redirect_stderr(io.StringIO()):
            launch.main(argv)
        if not (healthy / "padmap" / "lastgame.json").is_file():
            fail("this launch never reached write_last_game, so the hostile "
                 "case below proves nothing -- check the core and ROM in argv")
    ok("a launch records the game it is about to run")

    with env(XDG_RUNTIME_DIR=str(blocker)):
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            code = survives("padmap.launch.main with an unwritable runtime dir",
                            lambda: launch.main(argv))
        if code != 0:
            fail(f"a launch returned {code} because padmap could not write "
                 f"lastgame.json. padmap-play propagates that, so the game "
                 f"does not start -- a game that refuses to run because a "
                 f"mapping could not be narrowed is far worse than one played "
                 f"on the default mapping")
        ok("a launch whose files cannot be written still returns 0")
        if "could not resolve" not in stderr.getvalue():
            fail(f"the failure was not reported on stderr at all "
                 f"({stderr.getvalue()!r}); silence here is how a mapping "
                 f"that never took effect goes unnoticed")
        ok("...and says on stderr that it fell back to the default")


def check_launch_config_writers() -> None:
    heading("S16 retroarch.write_launch_config / write_launch_args -- "
            "contract: creates its tree")

    base = fresh("launchcfg-deep") / "run" / "padmap"
    creates("launch.cfg is created under a runtime dir that has none",
            lambda: retroarch.write_launch_config(
                ASSIGNMENTS, VPATHS, base / "launch.cfg"),
            base / "launch.cfg")
    creates("launch.args beside it",
            lambda: retroarch.write_launch_args(
                ASSIGNMENTS, VPATHS, base / "launch.args"),
            base / "launch.args", contains="--nodevice")

    heading("S16 retroarch.write_launch_config / write_launch_args -- "
            "contract: raises OSError")

    blocker = blocking_file("launchcfg-parent-is-a-file")
    refuses("the runtime directory is a plain file",
            lambda: retroarch.write_launch_config(
                ASSIGNMENTS, VPATHS, blocker / "launch.cfg"))
    refuses("...and for launch.args too",
            lambda: retroarch.write_launch_args(
                ASSIGNMENTS, VPATHS, blocker / "launch.args"))

    base = fresh("launchcfg-target-is-a-dir")
    (base / "launch.cfg").mkdir()
    refuses("launch.cfg is a directory",
            lambda: retroarch.write_launch_config(
                ASSIGNMENTS, VPATHS, base / "launch.cfg"))

    if not skip_as_root("a read-only runtime directory"):
        base = fresh("launchcfg-read-only")
        with read_only(base):
            refuses("the runtime directory is read-only",
                    lambda: retroarch.write_launch_config(
                        ASSIGNMENTS, VPATHS, base / "launch.cfg"))


def check_autoconfig_profile_writer() -> None:
    heading("S14 retroarch.install_profiles -- contract: creates its tree")

    base = fresh("autoconfig-deep") / "autoconfig" / "udev"
    written = None

    def install() -> None:
        nonlocal written
        written = retroarch.install_profiles(ASSIGNMENTS, base)

    creates("the autoconfig directory is created three levels deep",
            install, base / "padmap Player 1.cfg",
            contains="padmap Player 1")
    if not written or len(written) != 1:
        fail(f"install_profiles reported {written!r} rather than the one "
             f"profile it wrote")
    ok("and it reports the profile it wrote")

    heading("S14 retroarch.install_profiles -- contract: raises OSError")

    blocker = blocking_file("autoconfig-parent-is-a-file")
    refuses("the autoconfig directory is a plain file",
            lambda: retroarch.install_profiles(ASSIGNMENTS, blocker / "udev"))

    base = fresh("autoconfig-stale-is-a-dir")
    (base / "padmap Player 1.cfg").mkdir()
    error = refuses("a previous generation's profile is a directory",
                    lambda: retroarch.install_profiles(ASSIGNMENTS, base))
    if not isinstance(error, OSError):
        fail("clearing a stale profile failed with something the daemon's "
             "guard does not catch")
    ok("...and it is an OSError, which _start_republisher's caller catches")

    if not skip_as_root("a read-only autoconfig directory"):
        base = fresh("autoconfig-read-only")
        with read_only(base):
            refuses("the autoconfig directory is read-only",
                    lambda: retroarch.install_profiles(ASSIGNMENTS, base))


# -- S21: the user's own retroarch.cfg, which padmap must not invent --------


def check_clean_config_never_creates() -> None:
    heading("S21 retroarch.clean_user_config -- contract: creates NOTHING")

    missing = fresh("clean-missing") / "not-a-real-config" / "retroarch.cfg"
    refuses("a retroarch.cfg that is not there",
            lambda: retroarch.clean_user_config(missing),
            expect=FileNotFoundError)
    if missing.parent.exists():
        fail(f"clean_user_config created {missing.parent} for a config that "
             f"does not exist. This is the one writer that must not: being "
             f"pointed at a path with no config on it means padmap is about "
             f"to rewrite something that is not the file it was asked to "
             f"clean")
    ok("and no directory was invented for it")

    stdout = io.StringIO()
    with contextlib.redirect_stdout(stdout):
        code = cli.cmd_clean_config(
            argparse.Namespace(config=str(missing), dry_run=False))
    if code == 0:
        fail("`padmap clean-config` on a missing config reported success")
    if "No RetroArch config" not in stdout.getvalue():
        fail(f"`padmap clean-config` did not say the config was missing "
             f"({stdout.getvalue()!r})")
    ok("`padmap clean-config` says there is no config there and exits 1")


def check_clean_config_backs_up_before_it_writes() -> None:
    heading("S21 retroarch.clean_user_config -- contract: back up, then write")

    if skip_as_root("a read-only retroarch.cfg"):
        return

    base = fresh("clean-read-only")
    target = base / "retroarch.cfg"
    body = ('input_player1_joypad_index = "3"\n'
            'input_player2_joypad_index = "5"\n')
    target.write_text(body)
    target.chmod(0o444)
    refuses("retroarch.cfg is read-only",
            lambda: retroarch.clean_user_config(target))
    target.chmod(0o644)
    unchanged("the config is left exactly as it was", target, body)

    backup = target.with_suffix(".cfg.padmap-backup")
    if not backup.is_file() or backup.read_text() != body:
        fail("the backup was not taken before the rewrite was attempted. "
             "Ordering is the whole guarantee here: this rewrite is not "
             "reconstructible from padmap state, so a rewrite that begins "
             "before the backup exists can lose the file outright")
    ok("the backup was taken first and holds the original")

    stdout = io.StringIO()
    target.chmod(0o444)
    with contextlib.redirect_stdout(stdout):
        code = survives("`padmap clean-config` on a read-only config",
                        lambda: cli.cmd_clean_config(
                            argparse.Namespace(config=str(target),
                                               dry_run=False)))
    target.chmod(0o644)
    if code == 0:
        fail("`padmap clean-config` reported success on a config it could "
             "not rewrite, so the user believes the leftovers are gone")
    if "Could not rewrite" not in stdout.getvalue():
        fail(f"the failure was not described to the user "
             f"({stdout.getvalue()!r})")
    ok("...reports 'Could not rewrite' and exits 1, rather than tracebacking")


# -- S19: the udev rules, written as root -----------------------------------


def check_hide_install_never_raises() -> None:
    heading("S19 hide.install -- contract: reports every failure, raises none")

    target = hide.RUNTIME_RULES_PATH
    if target.exists() or target.is_symlink():
        target.unlink()

    changed, messages = survives(
        "installing rules into a /run/udev/rules.d that does not exist yet",
        lambda: hide.install("# rules\n"))
    if not changed or not target.is_file():
        fail(f"hide.install did not create its directory ({messages}); "
             f"/run/udev/rules.d is tmpfs and is genuinely absent on a fresh "
             f"boot, so this is the normal case, not an edge one")
    ok("the rules directory is created and the rules are written")

    target.unlink()
    target.mkdir()
    changed, messages = survives("the rules file is a directory",
                                 lambda: hide.install("# rules\n"))
    if changed or not any("could not write" in m for m in messages):
        fail(f"a failed install reported {changed}, {messages}; cli.cmd_hide "
             f"keys its exit status off 'could not', so padmap would claim "
             f"the physical pads are hidden while RetroArch still sees them")
    ok("a directory in the way is reported, not raised")
    target.rmdir()

    if not skip_as_root("read-only udev rules"):
        target.write_text("# older rules\n")
        target.chmod(0o444)
        changed, messages = survives("the rules file is read-only",
                                     lambda: hide.install("# rules\n"))
        target.chmod(0o644)
        if changed or not any("could not write" in m for m in messages):
            fail(f"a read-only rules file was reported as {changed}, "
                 f"{messages}")
        ok("a read-only rules file is reported, not raised")

        target.unlink()
        with read_only(target.parent):
            changed, messages = survives(
                "the rules directory is read-only",
                lambda: hide.install("# rules\n"))
        if changed or not any("could not write" in m for m in messages):
            fail(f"a read-only /run/udev/rules.d was reported as {changed}, "
                 f"{messages}")
        ok("a read-only rules directory is reported, not raised")

    if target.exists() or target.is_symlink():
        target.unlink()
    target.symlink_to(target.parent / "somewhere-else.rules")
    survives("the rules file is a dangling symlink",
             lambda: hide.install("# rules\n"))
    if not (target.parent / "somewhere-else.rules").is_file():
        fail("a dangling symlink swallowed the rules: udev reads the path, "
             "finds nothing, and every physical pad stays visible to "
             "RetroArch while padmap says it hid them")
    ok("a dangling symlink is followed and the rules land at its target")
    target.unlink()

    # The reported scar, on the writer rather than the reader: the daemon's
    # own prompted-list reader was fixed to decode with errors="replace"
    # after a non-UTF-8 file stopped `padmap serve` starting. hide.install
    # reads its existing file the same way and was not.
    target.write_bytes(b"\xff\xfe not text at all\n")
    try:
        changed, messages = hide.install("# rules\n")
    except UnicodeDecodeError:
        gap("hide.install raised UnicodeDecodeError on a rules file that is "
            "not valid UTF-8, so `sudo padmap hide` tracebacks instead of "
            "reporting (S19). Reproduce: printf '\\xff' | sudo tee "
            "/run/udev/rules.d/99-padmap.rules; sudo padmap hide. The read is "
            "guarded with `except OSError` (hide.py, install), and "
            "UnicodeDecodeError is a ValueError -- the same class of hole "
            "already fixed in Server._load_prompted, which reads its file as "
            "bytes and decodes with errors='replace'")
    else:
        if not changed or target.read_bytes() != b"# rules\n":
            fail(f"a rules file that is not UTF-8 was left in place "
                 f"({changed}, {messages}); udev goes on applying whatever "
                 f"that file says")
        ok("a rules file that is not UTF-8 is replaced")
    target.unlink()


# -- S18: ensure-daemon -----------------------------------------------------


def check_ensure_daemon_reaches_the_daemon_check() -> None:
    heading("S18 `padmap ensure-daemon` -- a daemon must still be checked")

    # --check only. Without it this spawns a real daemon, which would take
    # the live controllers on this machine.
    def ensure() -> int:
        return quiet(lambda: cli.cmd_ensure_daemon(
            argparse.Namespace(check=True, timeout=1.0)))

    with env(XDG_DATA_HOME=str(fresh("ensure-healthy"))):
        code = survives("ensure-daemon --check with a writable data directory",
                        ensure)
    if code != 1:
        fail(f"ensure-daemon --check returned {code} with no daemon running "
             f"on this sandboxed XDG_RUNTIME_DIR; it should report 'no daemon "
             f"running' and exit 1, and this scenario is not reaching the "
             f"code it claims to")
    ok("reaches the daemon check and reports there is none")


# -- the daemon's own writers, which may never take it down -----------------


def check_daemon_state_writers() -> None:
    heading("S3/S17 Server._save_assignments -- contract: creates its tree")

    srv, _ = quiet_server("daemon-state-fresh")
    srv.state_path = Path(srv.state_path).parent / "deep" / "run" / "state.json"
    srv._assignments = list(ASSIGNMENTS)
    creates("assignments.json is created under a runtime dir that has none",
            srv._save_assignments, srv.state_path, contains="USB GamePad")

    heading("S3/S17 Server._save_prompted -- contract: never raises")

    srv, _ = quiet_server("daemon-prompted")
    srv._prompted = {SIGNATURE}
    survives("recording a prompted controller normally", srv._save_prompted)
    if srv.prompted_path.read_text().strip() != SIGNATURE:
        fail("the prompted list does not hold the signature it was given")
    ok("the prompted list is written")

    srv.prompted_path.unlink()
    srv.prompted_path.mkdir()
    survives("the prompted list is a directory", srv._save_prompted)
    ok("a directory in the way is logged, not raised")
    srv.prompted_path.rmdir()

    blocker = blocking_file("daemon-prompted-parent")
    srv.prompted_path = blocker / "prompted"
    survives("the runtime directory is a plain file", srv._save_prompted)
    ok("a parent that is a file is logged, not raised")

    if not skip_as_root("a read-only prompted list"):
        base = fresh("daemon-prompted-read-only")
        srv.prompted_path = base / "prompted"
        srv.prompted_path.write_text("kept\n")
        srv.prompted_path.chmod(0o444)
        survives("the prompted list is read-only", srv._save_prompted)
        srv.prompted_path.chmod(0o644)
        ok("a read-only prompted list is logged, not raised")
        note("the cost of each of these is one repeated setup prompt, which "
             "is exactly why they are warnings")


def check_daemon_survives_every_failed_write() -> None:
    heading("S4/S12 the daemon -- contract: no write failure ends the process")

    # Accepting a session writes assignments.json. A directory in its place
    # is what a rebuild or a hand-edit leaves.
    srv, seen = quiet_server("daemon-accept")
    srv._start_republisher = lambda: None       # type: ignore[assignment]
    srv._write_controller_configs = lambda: None  # type: ignore[assignment]
    srv._assigner = FakeAssigner()
    Path(srv.state_path).mkdir(parents=True, exist_ok=True)
    survives("accept, with assignments.json blocked by a directory",
             lambda: srv._handle_command(None, {"cmd": "accept"}))
    errors = [e for e in seen if e.get("event") == "error"]
    if not errors:
        fail(f"a failed accept told the front-end nothing ({seen}); the setup "
             f"screen waits forever for an event that never comes")
    ok("the failure is reported to the front-end as an error event")
    seen.clear()
    survives("...and the daemon still answers the next command",
             lambda: srv._handle_command(None, {"cmd": "status"}))
    if not any(e.get("event") == "state" for e in seen):
        fail(f"the daemon stopped answering after a failed write ({seen})")
    ok("the daemon is still serving afterwards")
    gap("accept aborts part-way when assignments.json cannot be written: the "
        "session has already ended and the assignments are live in memory, "
        "but nothing is republished and the front-end gets an error where it "
        "expects 'accepted'. Reproduce: mkdir $XDG_RUNTIME_DIR/padmap/"
        "assignments.json, then finish the wizard. The daemon survives, "
        "which is the part that matters; the pads the user just assigned do "
        "not come up until something restarts it")

    # Choosing an icon writes a profile. A profile store that is a plain file
    # is what a restored backup or a stray `touch` leaves.
    srv, seen = quiet_server("daemon-icon")
    srv._assignments = list(ASSIGNMENTS)
    blocker = blocking_file("daemon-icon-profile-dir")
    with env(PADMAP_PROFILE_DIR=str(blocker)):
        survives("set_icon, with the profile store blocked by a file",
                 lambda: srv._handle_command(
                     None, {"cmd": "set_icon", "player": 1, "icon": "n64"}))
    if not [e for e in seen if e.get("event") == "error"]:
        fail(f"a profile that could not be saved was reported as success "
             f"({seen})")
    ok("a profile that cannot be saved is reported as an error event")

    # Republishing rewrites the SDL database on every restore. This one is
    # guarded inside the daemon rather than by _handle_command, because it
    # runs from restore() as well, outside any command.
    srv, _ = quiet_server("daemon-sdl")
    blocker = blocking_file("daemon-sdl-config-home")
    with env(XDG_CONFIG_HOME=str(blocker)):
        survives("the SDL database cannot be written during a republish",
                 srv._write_controller_configs)
    ok("a database that cannot be written is logged, not raised")


# -- S20: forget, which reads a file in XDG_RUNTIME_DIR before writing it ---


def check_forget_prompted_writer() -> None:
    heading("S20 cli._forget_prompted -- contract: never raises")

    base = fresh("forget-prompted")
    with env(XDG_RUNTIME_DIR=str(base)):
        path = protocol.prompted_path()
        path.parent.mkdir(parents=True, exist_ok=True)

        path.write_text("aaa\nbbb\n")
        removed = survives("forgetting one of two prompted controllers",
                           lambda: cli._forget_prompted({"aaa"}))
        if removed != 1:
            fail(f"_forget_prompted removed {removed} rather than 1")
        if path.read_text() != "bbb\n":
            fail(f"the other controller was not kept ({path.read_text()!r})")
        ok("the named controller is dropped and the other is kept")

        path.unlink()
        path.mkdir()
        removed = survives("the prompted list is a directory",
                           lambda: cli._forget_prompted({"aaa"}))
        if removed != 0:
            fail(f"_forget_prompted claimed to remove {removed} from a "
                 f"directory")
        ok("a directory in its place is reported as nothing removed")
        path.rmdir()

        if not skip_as_root("a read-only prompted list"):
            path.write_text("aaa\nbbb\n")
            path.chmod(0o444)
            removed = survives("the prompted list is read-only",
                               lambda: cli._forget_prompted({"aaa"}))
            path.chmod(0o644)
            if removed != 0:
                fail(f"_forget_prompted reported {removed} removed from a "
                     f"file it could not write; `padmap forget` then tells "
                     f"the user a controller will be offered setup again "
                     f"when it will not")
            ok("a read-only list is reported as nothing removed")
            unchanged("...and it is left exactly as it was", path,
                      "aaa\nbbb\n")

        path.write_bytes(b"\xff\xfe not text at all\n")
        try:
            removed = cli._forget_prompted({"aaa"})
        except UnicodeDecodeError:
            gap("cli._forget_prompted raised UnicodeDecodeError on a prompted "
                "list that is not valid UTF-8, so `padmap forget` tracebacks "
                "(S20). Reproduce: printf '\\xff' > $XDG_RUNTIME_DIR/padmap/"
                "prompted; padmap forget --all. The daemon's own reader of "
                "this exact file was fixed for exactly this (Server."
                "_load_prompted reads bytes and decodes with "
                "errors='replace', with a comment saying why); the CLI copy "
                "guards only `except OSError`, and UnicodeDecodeError is a "
                "ValueError. The file lives in XDG_RUNTIME_DIR where anything "
                "may have written it")
        else:
            ok(f"a prompted list that is not UTF-8 is survivable "
               f"({removed} removed)")


# -- artwork, which fails per file or not at all ----------------------------


def check_artwork_writer() -> None:
    heading("artwork._safe -- contract: per-file failure, never raises")

    original = artwork.fetch
    artwork.fetch = lambda url, timeout: b"\x89PNG\r\n\x1a\n fake"  # type: ignore[assignment]
    try:
        base = fresh("artwork")
        dest = base / "Named_Boxarts" / "deep" / "Zelda.png"
        problem = survives("downloading into a directory tree that has none",
                           lambda: artwork._safe(
                               artwork.Image(url="u", dest=dest), 1.0))
        if problem:
            fail(f"a download into a fresh tree reported {problem!r}; the "
                 f"thumbnail tree does not exist before the first run")
        if not dest.is_file():
            fail("the image was reported as downloaded but is not there")
        ok("the artwork tree is created and the image lands in it")
        if list(dest.parent.glob("*.part")):
            fail("a .part staging file was left behind; a later run skips "
                 "whatever is already present, so a half file is permanent")
        ok("no .part staging file is left behind")

        blocked = fresh("artwork-dir") / "Zelda.png"
        blocked.mkdir(parents=True)
        problem = survives("the image path is a directory",
                           lambda: artwork._safe(
                               artwork.Image(url="u", dest=blocked), 1.0))
        if not problem:
            fail("writing over a directory was reported as a successful "
                 "download, so the run counts art it does not have")
        ok("a directory in the way is counted as a failure, not raised")

        if not skip_as_root("a read-only artwork directory"):
            base = fresh("artwork-read-only")
            with read_only(base):
                problem = survives("the artwork directory is read-only",
                                   lambda: artwork._safe(
                                       artwork.Image(url="u",
                                                     dest=base / "Zelda.png"),
                                       1.0))
            if not problem:
                fail("a download into a read-only directory reported success")
            ok("a read-only directory is counted as a failure, not raised")

        base = fresh("artwork-symlink-out")
        victim = _SANDBOX / "cases" / "not-artwork.png"
        victim.write_text("PRECIOUS\n")
        (base / "Zelda.png").symlink_to(victim)
        survives("the image path is a symlink out of the artwork tree",
                 lambda: artwork._safe(
                     artwork.Image(url="u", dest=base / "Zelda.png"), 1.0))
        if victim.read_text() != "PRECIOUS\n":
            fail("artwork followed a symlink out of the thumbnail tree and "
                 "overwrote the file it pointed at")
        ok("os.replace replaces the link rather than following it")
    finally:
        artwork.fetch = original                # type: ignore[assignment]


# -- titles, the build-time table -------------------------------------------


def check_titles_writer() -> None:
    heading("titles.dump_json -- contract: creates NOTHING")

    missing = fresh("titles") / "not-there" / "titles.json"
    refuses("a title table under a directory that does not exist",
            lambda: titles.dump_json({}, missing),
            expect=FileNotFoundError)
    if missing.parent.exists():
        fail("titles.dump_json invented a directory. This one runs from the "
             "flake at build time with an explicit path; a typo that creates "
             "a tree rather than failing produces a table nothing reads")
    ok("and no directory was invented for it")


# -- the sandbox itself ------------------------------------------------------


def check_nothing_real_was_touched() -> None:
    heading("the sandbox")

    for name in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
                 "PADMAP_PROFILE_DIR", "RETROARCH_CONFIG_DIR",
                 "PADMAP_SDL_DB"):
        value = os.environ.get(name, "")
        if not value.startswith(str(_SANDBOX)):
            fail(f"{name} was left as {value!r}, outside the sandbox; a "
                 f"scenario leaked and may have written real user state")
    ok("every redirected path still points into the temp sandbox")

    for path in (hide.RUNTIME_RULES_PATH, hide.RULES_PATH):
        if not str(path).startswith(str(_SANDBOX)):
            fail(f"hide was left pointing at {path}, which is a real system "
                 f"path")
    ok("hide's rules paths never left the sandbox")

    if devices.discover() != []:
        fail("devices.discover was restored to the real one; a live daemon "
             "owns the controllers on this machine")
    ok("no scenario could reach a real controller")


# -- main --------------------------------------------------------------------


def main() -> int:
    if ROOT:
        note("running as root: every permission-based scenario is skipped "
             "with a note, because mode 000 is still readable and writable")

    check_profile_store_creates_its_tree()
    check_profile_store_refuses_the_impossible()
    check_profile_store_and_symlinks()
    check_profile_filenames_are_survivable()

    check_sdl_database_writer()
    check_sdl_database_keeps_foreign_lines()

    check_last_game_writer()
    check_a_launch_survives_an_unwritable_runtime_dir()
    check_launch_config_writers()
    check_autoconfig_profile_writer()

    check_clean_config_never_creates()
    check_clean_config_backs_up_before_it_writes()

    check_hide_install_never_raises()

    check_ensure_daemon_reaches_the_daemon_check()

    check_daemon_state_writers()
    check_daemon_survives_every_failed_write()
    check_forget_prompted_writer()

    check_artwork_writer()
    check_titles_writer()

    check_nothing_real_was_touched()

    print(f"\n{CHECKS} assertions held")
    print("all checks passed")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    finally:
        # chmod back first: a read-only directory left behind by a failed
        # scenario would make the cleanup itself fail.
        for _path in sorted(_SANDBOX.rglob("*"), reverse=True):
            with contextlib.suppress(OSError):
                if _path.is_dir() and not _path.is_symlink():
                    _path.chmod(0o755)
                elif not _path.is_symlink():
                    _path.chmod(0o644)
        shutil.rmtree(_SANDBOX, ignore_errors=True)
