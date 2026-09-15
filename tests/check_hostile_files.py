#!/usr/bin/env python3
"""Every file padmap reads, damaged ten ways.

padmap keeps almost nothing in memory. What a controller is, what it was
mapped to, which pads are hidden, which game was played last and which player
slot holds which pad are all files -- and every one of them can be truncated
by a full disk, half-written by a power cut, hand-edited by the user, replaced
by a directory by a rebuild, or left behind unreadable by a `sudo` that should
not have been.

The contract for a damaged file is the same everywhere, and it has two halves:

  * It must not raise out of a read path. `padmap ensure-daemon` (S18) reads
    the udev rules and the prompted list before it has decided anything; the
    daemon reads a profile per pad, once a second,
    on the same thread that forwards controller events. A read path that can
    throw is a startup that can die, and the user's symptom is "the front-end
    came up and no controller does anything".

  * It must not present as VALID. This is the half that is easy to get wrong
    and worse when it happens. A corrupted profile that loads as an
    empty-but-present profile marks that controller as already configured, so
    the wizard is never offered again (S1) and nothing ever says why. Silence
    is not degradation; "nothing stored" is.

So each file below is fed the same ten inputs -- empty, whitespace only,
truncated mid-token, valid JSON of the wrong shape, deeply nested JSON, NUL
bytes, invalid UTF-8, a directory where a file is expected, a dangling
symlink, and no read permission -- through the real entry points rather than
the parsers underneath them, because the guard that matters is the one on the
path `ensure-daemon` actually takes.

Stories: S18 (ensure-daemon), S20 (forget), S21 (clean-config), with
S1/S4/S14 where a damaged file decides whether a controller is offered the
wizard or a game launches with bindings.

Nothing here touches real user state: XDG_RUNTIME_DIR, XDG_CONFIG_HOME,
XDG_DATA_HOME, PADMAP_PROFILE_DIR and PADMAP_SDL_DB are redirected into a temp
tree before padmap is imported, hide's two rules paths are pointed into it, no
device is opened, no uinput node is created and no command is sent to the live
daemon.

Lines marked "gap:" are defects found by this file and reported rather than
fixed; each names its reproduction. Everything else is asserted.
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path

# Before importing padmap: several modules read these at import time, and
# nothing here should be able to reach real user state even by accident.
_SANDBOX = Path(tempfile.mkdtemp(prefix="check-hostile-files-"))
os.environ["XDG_RUNTIME_DIR"] = str(_SANDBOX / "run")
os.environ["XDG_CONFIG_HOME"] = str(_SANDBOX / "config")
os.environ["XDG_DATA_HOME"] = str(_SANDBOX / "data")
os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "devices")
# Named outright rather than left to follow XDG_CONFIG_HOME: this file is read
# by every SDL program on the machine, and damaging the real one would take
# the user's own mappings with it.
os.environ["PADMAP_SDL_DB"] = str(_SANDBOX / "config" / "sdl_controllers.txt")
for _sub in ("run/padmap", "config", "data", "devices"):
    (_SANDBOX / _sub).mkdir(parents=True, exist_ok=True)

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import (  # noqa: E402
    cli, controllercfg, hide, launch, mapping, profiles, protocol,
    retroarch, server,
)
from padmap.devices import Pad  # noqa: E402


# -- reporting ---------------------------------------------------------------

CHECKS = 0


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


# -- the damage vocabulary ---------------------------------------------------

# 200k deep. Not a number anybody would type: it is the shape a runaway
# writer or a fuzzed file has, and json.loads answers it with RecursionError,
# which is neither OSError nor ValueError and so goes through every guard
# padmap has.
DEEP_JSON = ("[" * 200_000 + "]" * 200_000).encode()

# What json.loads raises on the deeply nested case. Named so the gap lines
# below cannot quietly start swallowing a different failure.
NESTING = RecursionError

# Everything SDL will accept on the right of a colon in its database: a
# button, an axis (optionally signed, optionally half), or a hat and its bit.
USABLE_TARGET = re.compile(r"[-+]?(b\d+|a\d+~?|h\d+\.\d+)")


def damaged(truncated: bytes, wrong_shape: bytes = b"[1, 2, 3]") -> dict[str, bytes]:
    """The seven damaged *contents*, keyed by what a reader would call them.

    `truncated` is per file, because "truncated mid-token" only means anything
    if the prefix is what that file really starts with -- a generic fragment
    would be rejected by the first character and prove nothing.
    """
    return {
        "empty": b"",
        "whitespace only": b"  \n\t\r\n   ",
        "truncated mid-token": truncated,
        "valid JSON of the wrong shape": wrong_shape,
        "deeply nested JSON": DEEP_JSON,
        "NUL bytes": b"\x00\x00\x00padmap\x00\x00\n\x00",
        "invalid UTF-8": b"\xff\xfe\x00\x80\x81 not text at all\n\xc3\x28",
    }


def _clear(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        shutil.rmtree(path)


def not_a_readable_file(path: Path, sound: bytes = b""):
    """Yield each of the three "there is no readable file here" shapes.

    Sets `path` up, yields a label, and puts it back. The permission case is
    skipped with a note rather than failed when running as root, where mode
    000 is still readable.
    """
    cases = [
        "a directory where a file is expected",
        "a dangling symlink",
        "no read permission",
    ]
    for label in cases:
        _clear(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        if label.startswith("a directory"):
            path.mkdir()
        elif label.startswith("a dangling"):
            path.symlink_to(path.parent / "gone-away-when-the-usb-stick-did")
        else:
            if os.geteuid() == 0:
                note(f"skipped '{label}' for {path.name}: running as root, "
                     f"where mode 000 is still readable")
                continue
            path.write_bytes(sound)
            path.chmod(0)
        try:
            yield label
        finally:
            if path.is_file() and not path.is_symlink():
                path.chmod(0o644)
    _clear(path)


def degrades(what: str, call, empty, *, gap_on=(), gap_note: str = "") -> None:
    """Assert one read path answers "nothing stored" for a damaged file.

    Two ways to fail, both real: raising at all, and returning something other
    than `empty` -- which is the corrupted-file-presents-as-valid case, the
    one that silently marks a controller configured forever.

    `gap_on` is the narrow set of exception types a known, reported defect
    raises today. Anything else still fails, so a mutation cannot hide behind
    it, and a fix turns the gap line into the assertion below it.
    """
    try:
        result = call()
    except gap_on as error:  # noqa: BLE001 -- deliberately narrow, see above
        gap(f"{what} raised {type(error).__name__}. {gap_note}")
        return
    except Exception as error:  # noqa: BLE001
        fail(f"{what} raised {type(error).__name__}: {error} -- a damaged "
             f"file must read as nothing stored, not take the caller down")
    if result != empty:
        fail(f"{what} answered {result!r} rather than {empty!r}: damaged data "
             f"presented as valid is worse than a crash, because nothing "
             f"reports it")
    ok(what)


def quiet(call):
    """Run a CLI command without its printing all over the check output."""
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        return call()


# -- fixtures ----------------------------------------------------------------

# The pad the reports came from. Never opened: everything below is files.
PAD = Pad(path="/dev/input/event21", name="USB GamePad", phys="usb-1/input0",
          uniq="", vid=0x0079, pid=0x1879, syspath="/sys/class/input/event21")
SIGNATURE = profiles.signature(PAD)

# A capture that is unambiguously good, used as the "way to fail" control
# beside every damaged one.
SOUND_PROFILE = {
    "signature": SIGNATURE,
    "name": "USB GamePad",
    "icon": "n64",
    "axes": {"0": {"center": 128, "min": 0, "max": 255, "flat": 8,
                   "reach_min": 20, "reach_max": 240}},
    "mappings": {
        "": {"layout": "n64", "name": "",
             "buttons": {"a": {"kind": "button", "index": 0, "value": 0,
                               "ra_index": None},
                         "up": {"kind": "hat", "index": 0, "value": 1,
                                "ra_index": None}}},
    },
}


def profile_path() -> Path:
    """Where PAD's profile lives, via save() rather than a private helper."""
    return profiles.save(profiles.Profile(signature=SIGNATURE))


def write_profile(data: bytes) -> Path:
    path = profile_path()
    path.write_bytes(data)
    return path


@contextlib.contextmanager
def stubbed(module, name: str, value):
    original = getattr(module, name)
    setattr(module, name, value)
    try:
        yield
    finally:
        setattr(module, name, original)


# -- S20 / S1: the profile store ---------------------------------------------

def check_damaged_profile_is_never_a_configured_controller() -> None:
    heading("S20/S1: a damaged profile reads as nothing stored, not as a "
            "controller somebody already set up")

    truncated = b'{"signature": "0079:1879:USB GamePad", "mapp'
    for label, data in damaged(truncated).items():
        path = write_profile(data)
        degrades(
            f"profiles.load of a profile that is {label}",
            lambda: profiles.load(PAD), None,
            gap_on=(NESTING,) if label == "deeply nested JSON" else (),
            gap_note=(
                "profiles.load catches (OSError, ValueError); json.loads "
                "answers 200k-deep nesting with RecursionError, which is "
                "neither. is_known() calls load() for every pad in "
                "discovery, so one such file takes the daemon's pad scan "
                "down once a second. Repro: write '['*200000+']'*200000 to "
                f"{path.name} and call profiles.is_known(pad)."),
        )
        degrades(
            f"profiles.is_known of a profile that is {label}",
            lambda: profiles.is_known(PAD), False,
            gap_on=(NESTING,) if label == "deeply nested JSON" else (),
            gap_note="same RecursionError as above, one call deeper.",
        )
        degrades(
            f"controllercfg.has_mapping of a profile that is {label}",
            lambda: controllercfg.has_mapping(PAD), False,
            gap_on=(NESTING,) if label == "deeply nested JSON" else (),
            gap_note="same RecursionError as above.",
        )
    _clear(profile_path())

    # The control: an intact profile is found, is known, and has bindings.
    # Without this every assertion above would pass on a load() that returned
    # None unconditionally.
    write_profile(json.dumps(SOUND_PROFILE).encode())
    loaded = profiles.load(PAD)
    if loaded is None or not loaded.has_bindings():
        fail("an intact profile did not load -- the damaged-file checks above "
             "would then prove nothing")
    if not profiles.is_known(PAD) or not controllercfg.has_mapping(PAD):
        fail("an intact profile did not report the controller as configured")
    ok("an intact profile still loads, so the checks above have a way to fail")
    _clear(profile_path())


def check_profile_that_is_not_a_readable_file() -> None:
    heading("S20/S1: a profile that is a directory, a dangling symlink or "
            "unreadable is nothing stored")

    path = profile_path()
    _clear(path)
    for label in not_a_readable_file(path, json.dumps(SOUND_PROFILE).encode()):
        degrades(f"profiles.load with {label}", lambda: profiles.load(PAD), None)
        degrades(f"profiles.is_known with {label}",
                 lambda: profiles.is_known(PAD), False)
        degrades(f"controllercfg.stored_bindings with {label}",
                 lambda: controllercfg.stored_bindings(PAD), {})


def check_a_profile_that_parses_but_cannot_be_replayed() -> None:
    heading("S1/S4: a profile whose stored bindings are junk must not claim "
            "the controller is mapped")

    # A binding whose index is not a number is dropped, and correctly: a
    # control padmap cannot place is a control it must ask for again.
    write_profile(json.dumps({
        "mappings": {"": {"layout": "n64", "buttons": {
            "a": {"kind": "button", "index": "not a number"}}}},
    }).encode())
    if controllercfg.stored_bindings(PAD) != {}:
        fail("a binding with a non-numeric index was kept: it cannot be "
             "written to either consumer, so the control is silently dead")
    ok("a binding with a non-numeric index is dropped, not stored")

    # ...but the *kind* is not checked at all, and unlike the index it is not
    # dropped: it survives load, counts towards has_mapping, and only fails
    # much later when the pad is republished or a game is launched.
    for label, button in {
        "an unknown binding kind": {"kind": "hyperbutton", "index": 3},
        "a hat direction that is not a direction": {
            "kind": "hat", "index": 0, "value": 99},
    }.items():
        write_profile(json.dumps({
            "mappings": {"": {"layout": "n64", "buttons": {"a": button}}}},
        ).encode())
        bindings = controllercfg.stored_bindings(PAD)
        mapped = controllercfg.has_mapping(PAD)
        repro = json.dumps(
            {"mappings": {"": {"buttons": {"a": button}}}})
        try:
            for binding in bindings.values():
                binding.sdl()
            mapping.retroarch_lines(bindings)
        except (ValueError, KeyError) as error:
            if not mapped:
                ok(f"{label} is not counted as a mapping")
                continue
            gap(f"a profile holding {label} loads as a *valid* capture "
                f"(has_mapping is True) and then raises "
                f"{type(error).__name__} when the bindings are written out "
                f"-- so the pad is never offered the wizard again (S1) and "
                f"every launch after it fails (S14). Repro: put {repro} in "
                f"the profile, call controllercfg.stored_bindings(pad), then "
                f"mapping.retroarch_lines() on the result.")
            continue
        ok(f"{label} either does not count as a mapping or can be written out")
    _clear(profile_path())

    # The control for the paragraph above: a sound capture renders for both
    # consumers, so a mutation to either renderer is caught here.
    write_profile(json.dumps(SOUND_PROFILE).encode())
    bindings = controllercfg.stored_bindings(PAD)
    if sorted(b.sdl() for b in bindings.values()) != ["b0", "h0.1"]:
        fail("an intact capture no longer renders the SDL fields it stored")
    if mapping.retroarch_lines(bindings) == []:
        fail("an intact capture produced no RetroArch lines")
    ok("an intact capture renders for both consumers")
    _clear(profile_path())


# -- S18 / S20: the prompted list --------------------------------------------

class PromptedReader:
    """The daemon's prompted-list reader, without starting a daemon.

    Bound off Server so this exercises the real method rather than a copy of
    it: `_load_prompted` runs in Server.__init__, so anything it raises is a
    daemon that does not start.
    """

    _load_prompted = server.Server._load_prompted
    _read_prompted_stamp = server.Server._read_prompted_stamp

    def __init__(self, path: Path) -> None:
        self.prompted_path = path


def check_damaged_prompted_list() -> None:
    heading("S18: a damaged prompted list must not stop the daemon starting")

    path = protocol.prompted_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    reader = PromptedReader(path)

    truncated = b"0079:1879:USB Gam"
    for label, data in damaged(truncated, wrong_shape=b'{"a": 1}').items():
        path.write_bytes(data)
        try:
            loaded = reader._load_prompted()
        except UnicodeDecodeError as error:
            gap(f"a prompted list that is {label} raised "
                f"{type(error).__name__} out of Server._load_prompted, which "
                f"runs in Server.__init__ -- so the daemon does not start at "
                f"all, and the front-end comes up with no controllers. "
                f"read_text() raises UnicodeDecodeError (a ValueError, not an "
                f"OSError) and only OSError is caught. Repro: write "
                f"b'\\xff\\xfe' to {path} and construct a Server. Same shape "
                f"as the 99-padmap.rules bug already fixed in hide.unhidden, "
                f"which reads bytes and decodes with errors='replace'.")
            continue
        except Exception as error:  # noqa: BLE001
            fail(f"a prompted list that is {label} raised "
                 f"{type(error).__name__}: {error} -- the daemon would not "
                 f"start")
        if not isinstance(loaded, set):
            fail(f"a prompted list that is {label} produced {loaded!r}")
        if any(not entry or entry != entry.strip() for entry in loaded):
            fail(f"a prompted list that is {label} produced blank or unstripped "
                 f"signatures ({loaded!r}), which no pad can ever match -- an "
                 f"entry that matches nothing silently re-offers setup")
        ok(f"a prompted list that is {label} yields only usable signatures")

    for label in not_a_readable_file(path, b"0079:1879:USB GamePad\n"):
        degrades(f"Server._load_prompted with {label}",
                 reader._load_prompted, set())

    # The control: a sound list is read, so "returns an empty set" above is a
    # statement about the damage rather than about the reader.
    path.write_text("0079:1879:USB GamePad\n0079:1843:Adapter\n")
    if reader._load_prompted() != {"0079:1879:USB GamePad", "0079:1843:Adapter"}:
        fail("an intact prompted list did not read back")
    ok("an intact prompted list still reads back")
    path.unlink()


def check_forget_over_a_damaged_prompted_list() -> None:
    heading("S20: padmap forget must survive a damaged prompted list")

    path = protocol.prompted_path()
    path.parent.mkdir(parents=True, exist_ok=True)

    for label, data in damaged(b"0079:1879:USB Gam", b'{"a": 1}').items():
        path.write_bytes(data)
        try:
            cleared = cli._forget_prompted({SIGNATURE})
        except UnicodeDecodeError as error:
            gap(f"padmap forget over a prompted list that is {label} raised "
                f"{type(error).__name__}: cli._forget_prompted catches only "
                f"OSError around read_text(). The user's command tracebacks "
                f"instead of doing the one thing it exists for. Repro: write "
                f"b'\\xff\\xfe' to {path} and run `padmap forget`.")
            continue
        except Exception as error:  # noqa: BLE001
            fail(f"padmap forget over a prompted list that is {label} raised "
                 f"{type(error).__name__}: {error}")
        if cleared != 0:
            fail(f"padmap forget claimed to clear {cleared} record(s) from a "
                 f"prompted list that is {label} -- reporting work it did not "
                 f"do is how 'forget did nothing' goes unnoticed")
        ok(f"padmap forget over a prompted list that is {label} clears nothing "
           f"and says so")

    for label in not_a_readable_file(path, f"{SIGNATURE}\n".encode()):
        degrades(f"cli._forget_prompted with {label}",
                 lambda: cli._forget_prompted({SIGNATURE}), 0)

    # The control: a real entry is really cleared, and an unrelated one kept.
    path.write_text(f"{SIGNATURE}\n0079:1843:Adapter\n")
    if cli._forget_prompted({SIGNATURE}) != 1:
        fail("padmap forget did not clear the prompted record for a connected "
             "controller, so the daemon would stay silent about it forever")
    if path.read_text().strip() != "0079:1843:Adapter":
        fail("padmap forget dropped a prompted record it was not asked about")
    ok("an intact prompted list still has exactly the asked-for record cleared")
    path.unlink(missing_ok=True)


def check_forget_over_a_damaged_profile_store() -> None:
    heading("S20: padmap forget must survive a damaged profile store")

    args = type("Args", (), {"all": False})()
    truncated = b'{"signature": "0079:1879:USB Gam'
    with stubbed(cli.devices, "discover", lambda: [PAD]):
        for label, data in damaged(truncated).items():
            write_profile(data)
            try:
                code = quiet(lambda: cli.cmd_forget(args))
            except AttributeError as error:
                gap(f"padmap forget over a profile that is {label} raised "
                    f"{type(error).__name__}: cmd_forget does its own "
                    f"json.loads and then calls raw.get('signature'), so "
                    f"valid JSON that is not an object (a list, `null`) walks "
                    f"straight past its (OSError, ValueError) guard. "
                    f"profiles.load was already fixed for exactly this; this "
                    f"reader was not. Repro: write '[1, 2, 3]' to a file in "
                    f"the profile dir and run `padmap forget`.")
                continue
            except NESTING as error:
                gap(f"padmap forget over a profile that is {label} raised "
                    f"{type(error).__name__} from json.loads -- same "
                    f"unguarded nesting as profiles.load. Repro: write "
                    f"'['*200000+']'*200000 into the profile dir and run "
                    f"`padmap forget`.")
                continue
            except Exception as error:  # noqa: BLE001
                fail(f"padmap forget over a profile that is {label} raised "
                     f"{type(error).__name__}: {error}")
            if code != 0:
                fail(f"padmap forget returned {code} for a profile that is "
                     f"{label}; a damaged file is not a failed command")
            ok(f"padmap forget over a profile that is {label} completes")
        _clear(profile_path())

        # The control: an intact profile for a connected pad is deleted, which
        # is the whole of S20.
        path = write_profile(json.dumps(SOUND_PROFILE).encode())
        if quiet(lambda: cli.cmd_forget(args)) != 0:
            fail("padmap forget failed on an intact profile")
        if path.exists():
            fail("padmap forget left the profile of a connected controller in "
                 "place, so it would never be offered setup again")
        ok("an intact profile for a connected controller is still forgotten")


# -- S9 / S18: lastgame.json --------------------------------------------------

def check_damaged_last_game() -> None:
    heading("S9/S18: a damaged lastgame.json costs the recent-games entries, "
            "nothing else")

    path = protocol.last_game_path()
    path.parent.mkdir(parents=True, exist_ok=True)

    cases = damaged(b'{"games": [{"key": "n64/goldene')
    cases["a games list of scalars"] = b'{"games": [1, "x", null]}'
    cases["a games key that is not a list"] = b'{"games": {"a": 1}}'
    cases["entries with no key"] = b'{"games": [{"console": "n64"}]}'
    for label, data in cases.items():
        path.write_bytes(data)
        deep = label == "deeply nested JSON"
        degrades(
            f"protocol.read_recent_games over lastgame.json that is {label}",
            protocol.read_recent_games, [],
            gap_on=(NESTING,) if deep else (),
            gap_note=("read_recent_games catches (OSError, ValueError) and "
                      "documents itself as never raising; json.loads answers "
                      "deep nesting with RecursionError. The scope picker "
                      "(S9) is drawn from this, so opening it would take the "
                      "daemon's command handler down. Repro: write "
                      f"'['*200000+']'*200000 to {path.name} and call "
                      "protocol.read_recent_games()."))
        degrades(
            f"protocol.read_last_game over lastgame.json that is {label}",
            protocol.read_last_game, {},
            gap_on=(NESTING,) if deep else (),
            gap_note="same RecursionError, one call deeper.")

    for label in not_a_readable_file(path, b'{"games": []}'):
        degrades(f"protocol.read_recent_games with {label}",
                 protocol.read_recent_games, [])

    path.write_text(json.dumps({"games": [
        {"console": "n64", "key": "n64/goldeneye-007", "title": "GoldenEye"}]}))
    if protocol.read_last_game().get("key") != "n64/goldeneye-007":
        fail("an intact lastgame.json did not read back, so the checks above "
             "would pass on a reader that always returned nothing")
    ok("an intact lastgame.json still reads back")
    path.unlink()


# -- S14: assignments.json ----------------------------------------------------

def check_damaged_assignments() -> None:
    heading("S14/S17: a damaged assignments.json means no assignments, not a "
            "failed launch")

    path = _SANDBOX / "run" / "padmap" / "assignments.json"
    cases = damaged(b'[{"player": 1, "path": "/dev/input/ev',
                    wrong_shape=b'{"player": 1}')
    cases["a list of scalars"] = b'[1, "two", null]'
    cases["entries with no player"] = b'[{"path": "/dev/input/event21"}]'
    cases["a player that is not a number"] = (
        b'[{"path": "/dev/input/event21", "player": "one"}]')

    # discover() is stubbed rather than called: this check is about the file,
    # and enumerating the real /dev/input on a machine running a live daemon
    # is exactly what these tools must not do.
    with stubbed(launch.devices, "discover", lambda: [PAD]):
        for label, data in cases.items():
            path.write_bytes(data)
            degrades(
                f"launch.load_assignments over assignments.json that is {label}",
                lambda: launch.load_assignments(path), [],
                gap_on=(NESTING,) if label == "deeply nested JSON" else (),
                gap_note=("load_assignments catches (OSError, ValueError); "
                          "deep nesting is a RecursionError. padmap-play "
                          "calls this on every launch, so the game does not "
                          "start. Repro: write '['*200000+']'*200000 to "
                          f"{path.name} and call launch.load_assignments()."))

        for label in not_a_readable_file(path, b"[]"):
            degrades(f"launch.load_assignments with {label}",
                     lambda: launch.load_assignments(path), [])

        path.write_text(json.dumps([{"player": 2, "path": PAD.path}]))
        restored = launch.load_assignments(path)
        if [a.player for a in restored] != [2] or restored[0].pad != PAD:
            fail("an intact assignments.json did not restore the player order, "
                 "so the damaged-file checks above would prove nothing")
        ok("an intact assignments.json still restores the player order")
    path.unlink()


# -- S21: retroarch.cfg -------------------------------------------------------

def check_damaged_retroarch_config() -> None:
    heading("S21: clean-config over a damaged retroarch.cfg changes nothing "
            "and takes no backup")

    path = _SANDBOX / "config" / "retroarch.cfg"
    args = type("Args", (), {"config": str(path), "dry_run": False})()

    for label, data in damaged(b'input_player1_joypad_ind',
                               wrong_shape=b'{"input_player1_joypad_index": 3}'
                               ).items():
        path.write_bytes(data)
        before = path.read_bytes()
        changes, backup = retroarch.clean_user_config(path, dry_run=True)
        if changes:
            fail(f"clean-config claimed {changes!r} in a retroarch.cfg that is "
                 f"{label}: rewriting settings read out of junk is how a "
                 f"config the user did write gets damaged")
        if backup is not None:
            fail("clean-config took a backup of a file it did not change")
        if path.read_bytes() != before:
            fail(f"a dry run rewrote a retroarch.cfg that is {label}")
        ok(f"a retroarch.cfg that is {label} has nothing to clean and is left "
           f"byte-for-byte alone")
        if quiet(lambda: cli.cmd_clean_config(args)) not in (0, 1):
            fail(f"padmap clean-config returned neither success nor a plain "
                 f"failure for a retroarch.cfg that is {label}")
        ok(f"padmap clean-config over a retroarch.cfg that is {label} exits "
           f"cleanly")

    for label in not_a_readable_file(path, b'video_driver = "gl"\n'):
        try:
            code = quiet(lambda: cli.cmd_clean_config(args))
        except Exception as error:  # noqa: BLE001
            fail(f"padmap clean-config with {label} raised "
                 f"{type(error).__name__}: {error} rather than reporting it")
        if code != 1:
            fail(f"padmap clean-config with {label} returned {code}: a config "
                 f"it cannot read is a failure to report, not a success")
        ok(f"padmap clean-config with {label} reports a failure instead of "
           f"raising")

    # The control, and the file-fidelity question the backup exists to answer.
    sound = ('video_driver = "gl"\n'
             'input_player1_joypad_index = "3"\n'
             '# a comment padmap must not touch\n')
    path.write_text(sound)
    changes, backup = retroarch.clean_user_config(path)
    if changes != ['input_player1_joypad_index: "3" -> "0"']:
        fail(f"clean-config no longer strips a leaked joypad index: {changes!r}")
    if backup is None or backup.read_text() != sound:
        fail("clean-config rewrote retroarch.cfg without a faithful backup")
    if path.read_text() != sound.replace('"3"', '"0"'):
        fail("clean-config changed a line other than the leaked one")
    ok("a real leftover is stripped, every other line is preserved, and the "
       "backup is the original")

    non_utf8 = b'# rom dir = "/roms/caf\xe9"\ninput_player1_joypad_index = "3"\n'
    path.write_bytes(non_utf8)
    changes, backup = retroarch.clean_user_config(path)
    if backup is not None and backup.read_bytes() != non_utf8:
        gap("clean-config read retroarch.cfg with errors='replace' and then "
            "wrote both the new file and the backup from the replaced text, "
            "so a byte that is not UTF-8 -- a latin-1 ROM path in a comment, "
            "say -- becomes U+FFFD in the rewritten config AND in the backup "
            "taken to make the rewrite undoable. The one file padmap admits "
            "is the user's is damaged in a way its own backup cannot restore. "
            "Repro: put b'caf\\xe9' in a retroarch.cfg beside an "
            "input_playerN_joypad_index line and run `padmap clean-config`.")
    else:
        ok("a retroarch.cfg containing non-UTF-8 bytes is backed up verbatim")
    path.unlink()


# -- S19: the udev rules ------------------------------------------------------

def check_damaged_udev_rules() -> None:
    heading("S19/S18: a damaged 99-padmap.rules reads as covering nothing")

    runtime = _SANDBOX / "run" / "99-padmap.rules"
    etc = _SANDBOX / "etc" / "99-padmap.rules"
    etc.parent.mkdir(parents=True, exist_ok=True)

    with stubbed(hide, "RUNTIME_RULES_PATH", runtime), \
            stubbed(hide, "RULES_PATH", etc):
        for label, data in damaged(b'SUBSYSTEM=="input", ATTRS{idVend',
                                   wrong_shape=b'{"pads": []}').items():
            runtime.write_bytes(data)
            try:
                missing = hide.unhidden([PAD])
            except Exception as error:  # noqa: BLE001
                fail(f"a rules file that is {label} raised "
                     f"{type(error).__name__}: {error} -- ensure-daemon calls "
                     f"unhidden() on every start, so padmap would not start")
            if [p.pid for p in missing] != [PAD.pid]:
                fail(f"a rules file that is {label} was read as covering "
                     f"{PAD.name}: a rule that is not there cannot hide "
                     f"anything, and RetroArch would see the physical pad as "
                     f"well as the virtual one with nothing reporting it")
            ok(f"a rules file that is {label} is reported as covering nothing")

        for label in not_a_readable_file(runtime, b""):
            # Deliberately different from the damaged-content answer above:
            # with no readable file at either location padmap cannot tell
            # "rules were never installed" from "rules are unreadable", and
            # warning on a machine where `sudo padmap hide` was simply never
            # run would be noise on every single start.
            degrades(f"hide.unhidden with {label}",
                     lambda: hide.unhidden([PAD]), [])

        runtime.write_text(
            'SUBSYSTEM=="input", ATTRS{idVendor}=="0079", '
            'ATTRS{idProduct}=="1879", ENV{ID_INPUT_JOYSTICK}=""\n')
        if hide.unhidden([PAD]) != []:
            fail("an intact rules file no longer covers the pad it names, so "
                 "the checks above would pass on a reader that found nothing")
        ok("an intact rules file still covers the pad it names")
        runtime.unlink()


# -- S4 / S18: sdl_controllers.txt --------------------------------------------

def check_damaged_sdl_database() -> None:
    heading("S4/S18: a damaged sdl_controllers.txt carries nothing over "
            "instead of raising")

    path = controllercfg.sdl_config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    guid = "03000000790000001879000010010000"
    # Seed the built-in cache so nothing here spawns an SDL probe subprocess:
    # this check is about the file, and the answer must come from it.
    controllercfg._builtin_cache[guid] = {}

    cases = damaged(b"030000007900000018790000100",
                    wrong_shape=b'{"03000000790000001879000010010000": "a:b0"}')
    cases["a line with one field"] = b"03000000790000001879000010010000\n"
    for label, data in cases.items():
        path.write_bytes(data)
        degrades(f"controllercfg.carried_fields over a database that is "
                 f"{label}", lambda: controllercfg.carried_fields(guid), None)

    # A line for this GUID whose every target was lost -- half-written, or
    # hand-edited down to nothing. It matches, so carried_fields reports a
    # find with no fields in it, and fallback_line_for takes "not None" as
    # "there is a mapping to carry" and never reaches guessed_fields. The pad
    # is then published with an SDL line that binds nothing at all, which is a
    # front-end nobody can navigate -- strictly worse than the guess padmap
    # would have made from the pad's own capabilities.
    path.write_bytes(b"03000000790000001879000010010000,USB GamePad,a:,b:,\n")
    carried = controllercfg.carried_fields(guid)
    if carried is None:
        ok("a database line whose targets are all empty carries nothing over")
    elif carried[0] == {}:
        gap("a database line for this GUID with no usable targets is reported "
            "as a find with zero fields. fallback_line_for tests `carried is "
            "not None`, so an empty carry-over beats guessed_fields and the "
            "virtual pad is published with an SDL line that binds nothing -- "
            "a front-end that cannot be navigated at all, from a file the "
            "user may only have half-saved. Repro: put "
            "'03000000790000001879000010010000,USB GamePad,a:,b:,' in "
            "sdl_controllers.txt and call carried_fields(that guid).")
    else:
        fail(f"a database line with no usable targets produced "
             f"{carried[0]!r}")

    # A line truncated *after* the GUID is the interesting one: the GUID still
    # matches, so the half-written tail is carried over verbatim onto the
    # virtual pad. Every target that survives has to be something SDL can
    # parse, or the front-end is left with a button bound to nothing and no
    # sign of why.
    path.write_bytes(b"03000000790000001879000010010000,USB GamePad,a:b\n")
    carried = controllercfg.carried_fields(guid)
    unusable = [] if carried is None else [
        f"{field}:{target}" for field, target in carried[0].items()
        if not USABLE_TARGET.fullmatch(target)]
    if carried is None:
        ok("a database line truncated after the GUID carries nothing over")
    elif unusable:
        gap(f"a database line truncated mid-target is carried over verbatim "
            f"({', '.join(unusable)}): parse_sdl_line accepts any non-empty "
            f"text after the colon, so half a target ends up in the line "
            f"padmap writes for the virtual pad and SDL drops the binding. "
            f"Repro: put "
            f"'03000000790000001879000010010000,USB GamePad,a:b' in "
            f"sdl_controllers.txt and call "
            f"controllercfg.carried_fields(that guid).")
    else:
        ok("a database line truncated mid-target carries over only usable "
           "targets")

    for label in not_a_readable_file(path, b""):
        degrades(f"controllercfg.carried_fields with {label}",
                 lambda: controllercfg.carried_fields(guid), None)

    # The same damage reached through SDL_GAMECONTROLLERCONFIG_FILE, which is
    # the other path _database_paths() offers and is a string the user sets.
    extra = _SANDBOX / "config" / "extra_controllers.txt"
    os.environ["SDL_GAMECONTROLLERCONFIG_FILE"] = (
        f"{extra}{os.pathsep}{_SANDBOX / 'config' / 'nothing-here.txt'}")
    try:
        for label, data in damaged(b"030000007900000018790000100").items():
            extra.write_bytes(data)
            degrades(f"controllercfg.carried_fields over an "
                     f"SDL_GAMECONTROLLERCONFIG_FILE that is {label}",
                     lambda: controllercfg.carried_fields(guid), None)
        for label in not_a_readable_file(extra, b""):
            degrades(f"controllercfg.carried_fields with an "
                     f"SDL_GAMECONTROLLERCONFIG_FILE that is {label}",
                     lambda: controllercfg.carried_fields(guid), None)

        # The control: a real line for this GUID is still found, and padmap's
        # own output for it is still ignored.
        extra.write_text(
            "03000000790000001879000010010000,USB GamePad,a:b0,b:b1,"
            "platform:Linux,\n")
        found = controllercfg.carried_fields(guid)
        if found is None or found[0].get("a") != "b0":
            fail("an intact database line was not carried over, so every "
                 "check above would pass on a reader that always failed")
        if any(not USABLE_TARGET.fullmatch(t) for t in found[0].values()):
            fail(f"an intact database line carried over a target SDL cannot "
                 f"parse: {found[0]!r}")
        ok("an intact database line is still carried over, every target usable")
    finally:
        os.environ.pop("SDL_GAMECONTROLLERCONFIG_FILE", None)
    _clear(extra)
    _clear(path)


def check_rewriting_a_damaged_sdl_database() -> None:
    heading("S4: rewriting a damaged sdl_controllers.txt keeps the lines that "
            "are not padmap's")

    path = _SANDBOX / "config" / "sdl_controllers.txt"
    ours = "0600c9a7091200000100000001000000,padmap Player 1,a:b0,"
    foreign = ("03000000790000001830000010010000,MAYFLASH Fightstick,a:b1,"
               "platform:Linux,")

    for label, data in damaged(b"03000000790000001830000010010000,MAYFL").items():
        path.write_bytes(data + b"\n" + foreign.encode() + b"\n")
        try:
            written = controllercfg.write_sdl_mappings({1: ours}, path)
        except Exception as error:  # noqa: BLE001
            fail(f"rewriting a database that is {label} raised "
                 f"{type(error).__name__}: {error} -- accepting a session "
                 f"(S4) writes this file, so the assignment would be lost")
        text = written.read_text(errors="replace")
        if ours not in text:
            fail(f"padmap's own line was not written into a database that is "
                 f"{label}, so no SDL program has a mapping for the pad")
        if foreign not in text:
            fail(f"a user's own mapping was dropped while rewriting a database "
                 f"that is {label}: lines for devices padmap does not manage "
                 f"are theirs")
        ok(f"a database that is {label} keeps the user's line and gains "
           f"padmap's")
    _clear(path)


def main() -> int:
    print("hostile files: every file padmap reads, damaged ten ways")
    print(f"sandbox: {_SANDBOX}")

    check_damaged_profile_is_never_a_configured_controller()
    check_profile_that_is_not_a_readable_file()
    check_a_profile_that_parses_but_cannot_be_replayed()
    check_damaged_prompted_list()
    check_forget_over_a_damaged_prompted_list()
    check_forget_over_a_damaged_profile_store()
    check_damaged_last_game()
    check_damaged_assignments()
    check_damaged_retroarch_config()
    check_damaged_udev_rules()
    check_damaged_sdl_database()
    check_rewriting_a_damaged_sdl_database()

    # Nothing above may have left a real path in play.
    for name, value in (
            ("XDG_RUNTIME_DIR", os.environ["XDG_RUNTIME_DIR"]),
            ("XDG_CONFIG_HOME", os.environ["XDG_CONFIG_HOME"]),
            ("PADMAP_PROFILE_DIR", os.environ["PADMAP_PROFILE_DIR"]),
            ("PADMAP_SDL_DB", os.environ["PADMAP_SDL_DB"])):
        if not value.startswith(str(_SANDBOX)):
            fail(f"{name} was left pointing at {value}")
    if not str(hide.RULES_PATH).startswith("/etc/udev"):
        fail(f"hide.RULES_PATH was left as {hide.RULES_PATH}")
    if not str(hide.RUNTIME_RULES_PATH).startswith("/run/udev"):
        fail(f"hide.RUNTIME_RULES_PATH was left as {hide.RUNTIME_RULES_PATH}")

    shutil.rmtree(_SANDBOX, ignore_errors=True)
    print(f"\n{CHECKS} assertions held")
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
