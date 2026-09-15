#!/usr/bin/env python3
"""sdl_controllers.txt is rewritten, so what padmap cannot read it must not eat.

padmap generates one SDL controller database, at
`controllercfg.sdl_config_path()`, and is not the only writer of it: users type
lines into it by hand for controllers padmap never sees, and a mapping carried
over from another database lands there too. That is why `controllercfg.write_sdl_mappings` reads the
file, keeps every line that is not one of padmap's own, and writes the whole
thing back -- appending would leave two lines for one GUID and let SDL choose.

The rewrite therefore rests entirely on the read. A read that failed and a
read that returned nothing look identical to the caller, and treating them the
same is how a database full of the user's own mappings became a database with
one padmap line in it: mode 0222 on that file (a hand-edit, a restore with the
wrong mode, a root-owned file in a user's config) is readable to nobody and
writable to everybody, so the rewrite "succeeded" and the mappings were gone,
with nothing said. Losing them is the single outcome the read-and-rewrite
design exists to prevent, so a read that fails aborts the write.

What is asserted here:

  A FAILED READ ABORTS. Unreadable file, unreadable parent directory, target
    that is a directory: OSError out, and the bytes on disk unchanged. OSError
    specifically, because every guard padmap has around a write -- including
    the daemon's command guard, which is what keeps a wizard-accept from
    killing a daemon that holds EVIOCGRAB on every pad -- catches OSError.

  A MISSING FILE IS STILL EMPTY. First run, and a dangling symlink. If
    "cannot read" swallowed these too, no machine could ever write its first
    database and no controller would work at all.

  WHAT SURVIVES A READ THAT WORKED. Foreign lines kept verbatim, padmap's own
    previous generations replaced, and the file recoverable: fix the mode and
    the next rewrite keeps the user's lines and adds padmap's.

  READS THAT DO NOT REWRITE MAY DEGRADE. carried_fields looks a mapping up
    rather than rewriting it, so an unreadable database there costs a carried
    mapping, not the file. It must not raise into the wizard.

Stories: S4 (a finished wizard writes what the front-end reads), S12.

Nothing here touches real user state or hardware: XDG_CONFIG_HOME,
XDG_RUNTIME_DIR, XDG_DATA_HOME and PADMAP_PROFILE_DIR are redirected into a
temp tree before padmap is imported, every path passed to a writer is inside
it, no device is opened or grabbed, and no daemon command is dispatched.
"""

from __future__ import annotations

import logging
import os
import shutil
import sys
import tempfile
from pathlib import Path

# Before importing padmap: its state paths are read from the environment, and
# a live daemon owns the controllers on this machine.
_SANDBOX = Path(tempfile.mkdtemp(prefix="check-sdl-rewrite-"))
os.environ["XDG_RUNTIME_DIR"] = str(_SANDBOX / "run")
os.environ["XDG_CONFIG_HOME"] = str(_SANDBOX / "config")
os.environ["XDG_DATA_HOME"] = str(_SANDBOX / "data")
os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "devices")
os.environ.pop("SDL_GAMECONTROLLERCONFIG_FILE", None)
for _sub in ("run/padmap", "config", "data", "devices"):
    (_SANDBOX / _sub).mkdir(parents=True, exist_ok=True)

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import controllercfg, devices  # noqa: E402

# Belt and braces: nothing below enumerates pads, but a stray call must not
# reach the ones the running daemon has grabbed.
devices.discover = lambda *a, **k: []           # type: ignore[assignment]

# The refusal logs a warning, which is the point, but the assertions are on
# behaviour rather than log text.
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


def note(text: str) -> None:
    print(f"  --  {text}")


def skip_as_root(what: str) -> bool:
    """Permission scenarios prove nothing as root, where mode 000 still opens."""
    if ROOT:
        note(f"skipped '{what}': running as root, where an unreadable file is "
             f"still readable")
        return True
    return False


# -- fixtures ----------------------------------------------------------------

# A line for a controller padmap does not manage: the user's own, typed by
# hand or written by Pegasus's Gamepad Editor. This is the line the whole
# read-and-rewrite exists to keep.
HAND_WRITTEN = "030000005e0400008e02000014010000,Xbox 360,a:b0,b:b1,platform:Linux,"
# ...and a second one, so "kept the first line it saw" is not enough to pass.
SECOND_HAND_WRITTEN = (
    "03000000790000001830000010010000,Mayflash N64 Adapter,a:b1,x:b0,"
    "platform:Linux,"
)
NEW_LINE = {1: "0600c9a7091200000100000001000000,padmap Player 1,a:b0,"}


def fresh(name: str) -> Path:
    path = _SANDBOX / "cases" / name
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    return path


def database(name: str, contents: str) -> Path:
    """A sdl_controllers.txt with something in it worth losing."""
    target = fresh(name) / "sdl_controllers.txt"
    target.write_text(contents)
    return target


def must_raise(what: str, call, path: Path, before: bytes, restore=None) -> None:
    """A rewrite that must refuse, and leave the file exactly as it was.

    `restore` puts the permissions back before the file is read for the
    comparison -- the scenario that made it unreadable in the first place also
    makes checking it impossible.
    """
    try:
        call()
    except OSError as error:
        ok(f"{what} -> {type(error).__name__}")
    except Exception as error:  # noqa: BLE001
        fail(f"{what} raised {type(error).__name__}: {error}, which is not an "
             f"OSError -- the daemon guards this call with `except OSError`, "
             f"so anything else takes the daemon down mid-wizard and every "
             f"physical pad stays grabbed with no virtual pad to drive it")
    else:
        fail(f"{what} reported success. padmap could not read the database it "
             f"was about to rewrite, so it cannot know whether the user had "
             f"mappings in there -- and it has just published its answer over "
             f"them")
    if restore is not None:
        restore()
    path.chmod(0o644)
    after = path.read_bytes() if path.is_file() else b"<not a file>"
    if after != before:
        fail(f"{what}: the database was modified by a rewrite that failed. "
             f"Before: {before!r}; after: {after!r}. A half-rewritten "
             f"sdl_controllers.txt is worse than an unwritten one, because "
             f"Pegasus reads it and nobody is told")
    ok("...and every mapping already in the file is still there")


# -- a read that fails must abort the rewrite --------------------------------


def check_unreadable_database_is_not_emptied() -> None:
    heading("S4 a database padmap cannot read is not rewritten")

    if skip_as_root("an unreadable SDL database"):
        return

    # The reported case: mode 0222, writable and not readable. Reproduce by
    # hand with chmod 0222 on the database and
    # then finishing the mapping wizard.
    target = database("write-only",
                      HAND_WRITTEN + "\n" + SECOND_HAND_WRITTEN + "\n")
    before = target.read_bytes()
    target.chmod(0o222)
    must_raise("a write-only database (chmod 0222) is not rewritten",
               lambda: controllercfg.write_sdl_mappings(NEW_LINE, target),
               target, before)

    target = database("unreadable", HAND_WRITTEN + "\n")
    before = target.read_bytes()
    target.chmod(0o000)
    must_raise("a database with no permissions at all is not rewritten",
               lambda: controllercfg.write_sdl_mappings(NEW_LINE, target),
               target, before)


def check_unreadable_directory_is_not_rewritten_through() -> None:
    heading("S4 a database in a directory padmap cannot search")

    if skip_as_root("an unreadable config directory"):
        return

    target = database("dir-unreadable", HAND_WRITTEN + "\n")
    before = target.read_bytes()
    target.parent.chmod(0o000)
    try:
        must_raise("a database inside an unsearchable directory is not "
                   "rewritten",
                   lambda: controllercfg.write_sdl_mappings(NEW_LINE, target),
                   target, before,
                   restore=lambda: target.parent.chmod(0o755))
    finally:
        target.parent.chmod(0o755)


def check_a_directory_where_the_database_should_be() -> None:
    heading("S4 a directory standing where sdl_controllers.txt should be")

    base = fresh("target-is-a-dir")
    target = base / "sdl_controllers.txt"
    target.mkdir()
    (target / "someones-file").write_text("PRECIOUS\n")
    try:
        controllercfg.write_sdl_mappings(NEW_LINE, target)
    except OSError as error:
        ok(f"a database that is a directory -> {type(error).__name__}")
    except Exception as error:  # noqa: BLE001
        fail(f"a database that is a directory raised {type(error).__name__}: "
             f"{error}, which is not an OSError, so every guard padmap has "
             f"around this write goes straight through")
    else:
        fail("rewriting a directory reported success, so padmap believes "
             "Pegasus has a mapping it will never read")
    if (target / "someones-file").read_text() != "PRECIOUS\n":
        fail("the directory's contents were disturbed by a failed rewrite")
    ok("...and what was in that directory is untouched")


# -- a missing file is still an empty one ------------------------------------


def check_first_run_still_writes() -> None:
    heading("S4 the first run has no database, and must still get one")

    base = fresh("first-run") / "padmap" / "nested"
    target = base / "sdl_controllers.txt"
    try:
        written = controllercfg.write_sdl_mappings(NEW_LINE, target)
    except Exception as error:  # noqa: BLE001
        fail(f"writing a database that does not exist yet raised "
             f"{type(error).__name__}: {error} -- a missing file is an empty "
             f"one, not an unreadable one, and this is every fresh install: "
             f"nobody would have a working controller in Pegasus at all")
    if "padmap Player 1" not in written.read_text():
        fail("the first database written on a fresh install has no padmap "
             "line in it, so the front-end has nothing to navigate with")
    ok("a database three levels deep that does not exist is created")


def check_dangling_symlink_still_writes() -> None:
    heading("S4 a database symlinked to a file that is not there yet")

    base = fresh("dangling")
    target = base / "sdl_controllers.txt"
    target.symlink_to(base / "real-database.txt")
    try:
        controllercfg.write_sdl_mappings(NEW_LINE, target)
    except Exception as error:  # noqa: BLE001
        fail(f"a dangling sdl_controllers.txt symlink raised "
             f"{type(error).__name__}: {error} -- the link's target does not "
             f"exist, which is the same 'nothing to preserve' case as a fresh "
             f"install, and Pegasus follows the link")
    if "padmap Player 1" not in (base / "real-database.txt").read_text():
        fail("a dangling symlink swallowed the mapping: Pegasus reads the "
             "link's target and would find nothing there")
    ok("the link is followed and the mapping lands at its target")


# -- what survives a read that worked ----------------------------------------


def check_a_readable_database_keeps_what_is_not_ours() -> None:
    heading("S4 a database padmap can read keeps every line it does not own")

    stale = "0600c9a7091200000100000001000000,padmap Player 1,a:b9,"
    mirrored = "03000000790000001830000010010000,padmap Player 2,a:b3,"
    target = database("readable", "\n".join(
        [HAND_WRITTEN, stale, mirrored, SECOND_HAND_WRITTEN]) + "\n")
    text = controllercfg.write_sdl_mappings(NEW_LINE, target).read_text()

    for line, whose in ((HAND_WRITTEN, "an Xbox 360"),
                        (SECOND_HAND_WRITTEN, "an N64 adapter")):
        if line not in text:
            fail(f"the user's hand-written mapping for {whose} was dropped by "
                 f"a rewrite that could read the file perfectly well")
    ok("both hand-written mappings are kept verbatim")
    if stale in text:
        fail("a previous generation of padmap's own line survived, so SDL has "
             "two lines for one GUID and picks whichever it likes")
    if mirrored in text:
        fail("a padmap line under a mirrored GUID survived: it is findable by "
             "name only, and it would match again the moment that controller "
             "came back")
    ok("padmap's own previous generations are replaced, by GUID and by name")
    if NEW_LINE[1] not in text:
        fail("the new mapping was not written, so finishing the wizard "
             "changed nothing the front-end can see")
    ok("the new mapping is written")


def check_a_fixed_mode_recovers() -> None:
    heading("S4 a refused rewrite is recoverable, not a dead end")

    if skip_as_root("recovering from an unreadable database"):
        return

    target = database("recover", HAND_WRITTEN + "\n")
    target.chmod(0o222)
    try:
        controllercfg.write_sdl_mappings(NEW_LINE, target)
    except OSError:
        pass
    # What the user would do after reading the warning in the log.
    target.chmod(0o644)
    text = controllercfg.write_sdl_mappings(NEW_LINE, target).read_text()
    if HAND_WRITTEN not in text or NEW_LINE[1] not in text:
        fail("after the mode was fixed, the next rewrite did not produce a "
             "database with both the user's mapping and padmap's -- the "
             "refusal has to be a delay, not a loss")
    ok("once the file can be read, the next rewrite keeps both")


# -- reads that do not rewrite may degrade -----------------------------------


def check_carried_lookup_degrades_without_raising() -> None:
    heading("S4 an unreadable database costs a carried mapping, not the file")

    if skip_as_root("an unreadable database under a lookup"):
        return

    base = fresh("carried")
    # Where padmap itself would look, derived rather than spelled out: the
    # database moved out of a particular front-end's config directory once
    # already, and a literal here would have gone on testing the old place.
    original = os.environ["XDG_CONFIG_HOME"]
    os.environ["XDG_CONFIG_HOME"] = str(base)
    target = controllercfg.sdl_config_path()
    os.environ["XDG_CONFIG_HOME"] = original
    target.parent.mkdir(parents=True)
    # A GUID no real product has, so the fallback below cannot quietly be
    # answered out of SDL's own built-in database and pass for a file read.
    guid = "03000000ffff0000fffe000000010000"
    invented = f"{guid},Nobody's Controller,a:b0,b:b1,platform:Linux,"
    target.write_text(invented + "\n")

    original = os.environ["XDG_CONFIG_HOME"]
    os.environ["XDG_CONFIG_HOME"] = str(base)
    try:
        found = controllercfg.carried_fields(guid)
        if found is None or found[0].get("a") != "b0":
            fail("a mapping the user wrote by hand was not carried over to "
                 "their virtual pad, so a controller SDL already knew about "
                 "comes up unmapped")
        ok("a readable database supplies the mapping to carry over")

        target.chmod(0o000)
        try:
            degraded = controllercfg.carried_fields(guid)
        except Exception as error:  # noqa: BLE001
            fail(f"looking a mapping up in an unreadable database raised "
                 f"{type(error).__name__}: {error} -- this read rewrites "
                 f"nothing, so it must fall back to a guess rather than stop "
                 f"a pad being republished at all")
        if degraded is not None:
            fail("an unreadable database was read anyway; this scenario is "
                 "not testing what it claims")
        ok("an unreadable database falls back rather than raising")
    finally:
        target.chmod(0o644)
        os.environ["XDG_CONFIG_HOME"] = original


def main() -> int:
    print("sdl database: what a rewrite may and may not lose")
    print(f"sandbox: {_SANDBOX}")

    check_unreadable_database_is_not_emptied()
    check_unreadable_directory_is_not_rewritten_through()
    check_a_directory_where_the_database_should_be()
    check_first_run_still_writes()
    check_dangling_symlink_still_writes()
    check_a_readable_database_keeps_what_is_not_ours()
    check_a_fixed_mode_recovers()
    check_carried_lookup_degrades_without_raising()

    heading("the sandbox")
    for name in ("XDG_CONFIG_HOME", "XDG_RUNTIME_DIR", "XDG_DATA_HOME",
                 "PADMAP_PROFILE_DIR"):
        if not os.environ[name].startswith(str(_SANDBOX)):
            fail(f"{name} escaped the sandbox and now points at "
                 f"{os.environ[name]}, which may be real user state")
    ok("every redirected path still points into the temp sandbox")
    if not str(controllercfg.sdl_config_path()).startswith(str(_SANDBOX)):
        fail("the default sdl_controllers.txt path is outside the sandbox: a "
             "scenario could have rewritten the real one")
    ok("the default database path stayed inside the sandbox")

    print(f"\n{CHECKS} assertions held")
    print("all checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
