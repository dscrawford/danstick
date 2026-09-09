"""Every padmap command line, given arguments nobody would type on purpose.

Maintenance is done from a shell, usually from a script, usually in a hurry:
`padmap clean-config --config "$CFG"` with CFG unset, `padmap export-pegasus
--out ~/collections` where that name is already a file, `padmap forget --all`
against a profile directory something else has been rummaging in. The promise
these stories make is not that any of that works -- it is that padmap says
what is wrong and stops, rather than printing a Python traceback at a user
who is looking at an arcade cabinet.

    S18  padmap ensure-daemon restarts a stale daemon
    S19  sudo padmap hide installs udev rules
    S20  padmap forget offers a controller setup again
    S21  padmap clean-config removes padmap leftovers from retroarch.cfg
    S22  padmap export-pegasus regenerates collections

Plus the two argv parsers behind the launch path, which nobody types directly
but everything depends on:

    launch.split_args   pulls the core and the ROM out of RetroArch's own
                        command line; a flag's *value* must never be mistaken
                        for the ROM
    protocol.daemon_pids  finds `padmap serve` by matching argv structurally,
                        because `pgrep -f` also matches the shell running a
                        diagnostic that merely mentions the string -- and
                        signalling that kills the user's shell

Nothing here may reach the live daemon, the real controllers or the real
RetroArch config: every XDG directory and RETROARCH_CONFIG_DIR are redirected
to a temporary tree before padmap is imported, subprocesses inherit that
redirection, discovery is pinned to a device name that cannot exist, and no
scenario runs a subcommand that would grab a pad or spawn a daemon.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_hostile_cli.py
"""

import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

SRC = pathlib.Path(__file__).resolve().parent.parent / "src"
sys.path.insert(0, str(SRC))

# ---------------------------------------------------------------------------
# Redirect every path padmap reads from the environment, before importing it.
# ---------------------------------------------------------------------------

SANDBOX = pathlib.Path(tempfile.mkdtemp(prefix="padmap-hostile-cli-"))

for _name in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
              "XDG_CACHE_HOME", "XDG_STATE_HOME"):
    _dir = SANDBOX / _name.lower()
    _dir.mkdir(parents=True, exist_ok=True)
    os.environ[_name] = str(_dir)

# retroarch.CONFIG_DIR is read from this at import and does *not* follow
# XDG_CONFIG_HOME, so without it `clean-config` with no --config would open
# the real user's retroarch.cfg.
RA_CONFIG_DIR = SANDBOX / "retroarch"
RA_CONFIG_DIR.mkdir(parents=True, exist_ok=True)
os.environ["RETROARCH_CONFIG_DIR"] = str(RA_CONFIG_DIR)

# devices.discover's documented escape hatch: no real controller can match, so
# nothing this test starts can grab one away from the live daemon.
os.environ["PADMAP_ONLY_DEVICE"] = "padmap-hostile-cli-no-such-device"
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from padmap import launch, protocol  # noqa: E402

# Subcommands that talk to hardware or start a daemon. They may only be run
# when the invocation is certain to die in argparse first; the runner enforces
# that, so a later edit cannot quietly turn one of these into a real daemon
# start or an EVIOCGRAB on somebody's controller.
NEVER_REACH_MAIN = {"serve", "ui", "run", "calibrate"}

CHECKS = 0
GAPS = 0


def heading(text):
    print(f"\n{text}")


def ok(text):
    global CHECKS
    CHECKS += 1
    print(f"  ok  {text}")


def check(condition, text, failure):
    if not condition:
        raise SystemExit(f"FAIL: {failure}")
    ok(text)


def gap(text):
    global GAPS
    GAPS += 1
    print(f"  gap: {text}")


# ---------------------------------------------------------------------------
# Running the CLI
# ---------------------------------------------------------------------------

def child_env():
    env = dict(os.environ)
    env["PYTHONPATH"] = str(SRC)
    return env


def padmap(*args, cwd=None):
    """`padmap <args>` in a subprocess, with everything redirected."""
    proc = subprocess.run(
        [sys.executable, "-m", "padmap.cli", *args],
        env=child_env(), cwd=str(cwd or SANDBOX),
        capture_output=True, text=True, timeout=120,
    )
    if args and args[0] in NEVER_REACH_MAIN and proc.returncode != 2:
        raise SystemExit(
            f"FAIL: `padmap {' '.join(args)}` was expected to be rejected by "
            f"the argument parser (exit 2) and got as far as running the "
            f"command (exit {proc.returncode}); this test must never start a "
            f"daemon or grab a controller"
        )
    return proc


def traceback_in(proc):
    return "Traceback (most recent call last)" in proc.stderr


def said_something(proc):
    return bool((proc.stdout + proc.stderr).strip())


def refused(proc, label, *, expect_message=None):
    """Non-zero exit, something said, and (usually) no traceback."""
    check(proc.returncode != 0,
          f"{label}: exits non-zero ({proc.returncode})",
          f"{label}: exited 0, so a script would carry on as if it worked")
    check(said_something(proc),
          f"{label}: says something rather than failing in silence",
          f"{label}: exited {proc.returncode} with no output at all")
    if expect_message is not None:
        combined = proc.stdout + proc.stderr
        check(expect_message in combined,
              f"{label}: explains itself ({expect_message!r})",
              f"{label}: message did not mention {expect_message!r}; got "
              f"{combined.strip()[:200]!r}")


def no_traceback(proc, label):
    check(not traceback_in(proc),
          f"{label}: no traceback",
          f"{label}: printed a Python traceback at the user:\n"
          + "\n".join(proc.stderr.strip().splitlines()[-3:]))


# ---------------------------------------------------------------------------
# launch.split_args -- the core and the ROM out of a RetroArch command line
# ---------------------------------------------------------------------------

def touch(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("rom")
    return str(path)


def scenario_split_args_value_flags():
    heading("S14/S22: a flag's value is never mistaken for the ROM")

    roms = SANDBOX / "roms"
    decoy = touch(roms / "looks-like-a-rom.n64")
    real = touch(roms / "Real Game.n64")

    # Against the real list, not a copy of it: a flag added there without a
    # test is exactly how the value of the next flag becomes the "game".
    check(len(launch.VALUE_FLAGS) > 10,
          f"VALUE_FLAGS is the module's own list ({len(launch.VALUE_FLAGS)} "
          f"entries)",
          "launch.VALUE_FLAGS is suspiciously short; this test is asserting "
          "against the wrong thing")

    for flag in launch.VALUE_FLAGS:
        # The flag *after* the ROM is the telling order: "last existing path
        # wins" would otherwise hide the value being read as a game.
        core, rom = launch.split_args(["retroarch", real, flag, decoy])
        if rom != real:
            raise SystemExit(
                f"FAIL: `{flag} {decoy}` made padmap resolve the mapping for "
                f"{rom or 'no game at all'} instead of the game being "
                f"launched -- a flag's value was taken for the ROM"
            )
        core, rom = launch.split_args(["retroarch", flag, decoy, real])
        if rom != real:
            raise SystemExit(
                f"FAIL: `{flag} {decoy} {real}` resolved "
                f"{rom or 'no game at all'} as the game -- a flag's value was "
                f"taken for the ROM"
            )
    ok(f"all {len(launch.VALUE_FLAGS)} VALUE_FLAGS swallow their value, "
       f"before or after the ROM, and the real ROM is still found")

    # The single most likely confusion: a flag whose value really is a file
    # under /roms, sitting after the game on the command line.
    core, rom = launch.split_args(
        ["retroarch", "-L", "core.so", real, "--savestate", decoy])
    check(rom == real,
          "--savestate pointed at an existing file in the ROM directory is "
          "not the game",
          f"the value of --savestate was resolved as the game: {rom!r}")

    for flag in launch.CORE_FLAGS:
        core, rom = launch.split_args(["retroarch", flag, decoy, real])
        check(core == decoy and rom == real,
              f"{flag} takes the following word as the core, not as the game",
              f"{flag} {decoy} gave core={core!r} rom={rom!r}")

    # The one padmap itself emits, pointed at a file that really is there.
    cfg = touch(SANDBOX / "launch.cfg")
    core, rom = launch.split_args(
        ["retroarch", "-L", "core.so", real, "--appendconfig", cfg])
    check(rom == real,
          "--appendconfig <an existing file> is not the game",
          f"--appendconfig value was taken as the ROM: {rom!r}")
    core, rom = launch.split_args(["retroarch", "--appendconfig", cfg])
    check(rom == "",
          "a command line that is only flags and their values names no game",
          f"a launch with no ROM at all resolved {rom!r} as the game")

    # --eof-exit is in VALUE_FLAGS but RetroArch takes no argument for it.
    core, rom = launch.split_args(["retroarch", "-L", "core.so",
                                   "--eof-exit", real])
    if rom != real:
        gap("`--eof-exit <rom>` resolves no game: --eof-exit is listed in "
            "VALUE_FLAGS but RetroArch takes no value for it, so the ROM "
            "after it is swallowed and the launch falls back to the console "
            "mapping")


def scenario_split_args_missing_and_repeated():
    heading("S14: a core flag that is missing, empty or repeated")

    real = touch(SANDBOX / "roms" / "Real Game.n64")

    core, rom = launch.split_args(["retroarch", real])
    check(core == "" and rom == real,
          "no -L at all: no core, and the ROM is still identified",
          f"argv with no core flag gave core={core!r} rom={rom!r}")

    core, rom = launch.split_args(["retroarch", "-L"])
    check((core, rom) == ("", ""),
          "-L as the last word: no core, no crash",
          "a trailing -L with no value did not resolve to an empty core")

    core, rom = launch.split_args(["retroarch", real, "-L"])
    check(core == "" and rom == real,
          "-L with no value still leaves the ROM found",
          f"trailing -L lost the ROM: core={core!r} rom={rom!r}")

    core, rom = launch.split_args(
        ["retroarch", "-L", "first.so", "-L", "second.so", real])
    check(core == "second.so",
          "several -L flags: the last one wins, as RetroArch's own parser does",
          f"repeated -L resolved to {core!r} rather than the last one")

    core, rom = launch.split_args(
        ["-L", "a.so", "--libretro", "b.so", real])
    check(core == "b.so",
          "-L and --libretro together: the last one wins",
          f"mixed core flags resolved to {core!r}")

    core, rom = launch.split_args([])
    check((core, rom) == ("", ""),
          "an empty command line resolves to nothing rather than raising",
          "split_args([]) did not return two empty strings")

    core, rom = launch.split_args(["retroarch", "--libretro=" + "core.so", real])
    if core == "":
        gap("`--libretro=<core>` (the equals form getopt accepts) resolves no "
            "core, so a hand-written launch line gets the controller default "
            "instead of the console mapping")


def scenario_split_args_order_and_separator():
    heading("S14: the ROM found by existing, not by position")

    roms = SANDBOX / "roms"
    real = touch(roms / "Real Game.n64")
    other = touch(roms / "Other Game.z64")

    core, rom = launch.split_args([real, "-L", "core.so"])
    check(core == "core.so" and rom == real,
          "a ROM before the core is still both",
          f"ROM-first command line gave core={core!r} rom={rom!r}")

    core, rom = launch.split_args(["retroarch", "-L", "core.so", "--", real])
    check(rom == real,
          "arguments after a bare -- are still searched for the ROM",
          f"a ROM after `--` was lost: rom={rom!r}")

    core, rom = launch.split_args(["--", "--", "-L", "core.so", real])
    check(core == "core.so" and rom == real,
          "repeated -- separators do not shift the parse",
          f"two `--` separators gave core={core!r} rom={rom!r}")

    core, rom = launch.split_args(
        ["retroarch", "-L", "core.so", other, real])
    check(rom == real,
          "several existing files: the last is taken (a subsystem load)",
          f"a multi-file launch resolved {rom!r} rather than the last file")

    core, rom = launch.split_args(
        ["retroarch", "-L", "core.so", "/no/such/rom.n64"])
    check(rom == "",
          "a ROM path that does not exist resolves no game",
          "a non-existent path was accepted as the game to map for")

    core, rom = launch.split_args(["retroarch", "-L", "core.so", "--fullscreen",
                                   "--menu", "-v", real])
    check(rom == real,
          "unknown valueless flags do not hide the ROM behind them",
          f"flags padmap does not know about lost the ROM: {rom!r}")

    weird = touch(roms / 'My Game (USA) "quoted" & odd.n64')
    core, rom = launch.split_args(["retroarch", "-L", "core.so", weird])
    check(rom == weird,
          "a ROM path with spaces, quotes and an ampersand is found verbatim",
          f"an awkwardly named ROM was not identified: {rom!r}")
    check(launch.title_for(weird) == 'My Game (USA) "quoted" & odd',
          "its title strips only the extension, quotes and all",
          f"title_for mangled the name: {launch.title_for(weird)!r}")

    newline = touch(roms / "line\nbreak.n64")
    core, rom = launch.split_args(["retroarch", "-L", "core.so", newline])
    check(rom == newline,
          "a ROM path containing a newline does not break the split",
          "a newline in the ROM path lost the game")

    empty = launch.split_args(["retroarch", "", "-L", "core.so"])
    check(empty[1] == "",
          "an empty argument is not treated as a path that exists",
          "an empty string argument was resolved as the ROM")

    core, rom = launch.split_args(["retroarch", "-L", "core.so", str(SANDBOX)])
    if rom == str(SANDBOX):
        gap("a directory given where the ROM goes is accepted as the game, so "
            "a mapping can be filed under a scope key for something that is "
            "not a game")


def scenario_launch_main_never_fails_a_game():
    heading("S14: `padmap-play` never takes a launch down")

    real = touch(SANDBOX / "roms" / "Real Game.n64")

    rc = launch.main(["--", "-L", "/x/mupen64plus_next_libretro.so", real])
    check(rc == 0,
          "a launch with no assignments returns 0 (the daemon's default "
          "mapping is already on disk)",
          f"launch.main returned {rc}; RetroArch would not have started")

    rc = launch.main([])
    check(rc == 0,
          "an empty RetroArch command line still returns 0",
          f"launch.main([]) returned {rc}")

    rc = launch.main(["--", "--", "-L"])
    check(rc == 0,
          "a truncated command line still returns 0",
          f"launch.main returned {rc} for a truncated command line")

    broken = launch.split_args

    def explode(_argv):
        raise RuntimeError("synthetic failure inside resolve")

    launch.split_args = explode
    try:
        rc = launch.main(["--", "-L", "core.so", real])
    except Exception as error:  # noqa: BLE001
        raise SystemExit(
            f"FAIL: a failure inside padmap-play escaped launch.main "
            f"({error!r}); the game would refuse to start because a mapping "
            f"could not be narrowed"
        ) from None
    finally:
        launch.split_args = broken
    check(rc == 0,
          "a failure while resolving the mapping is reported and swallowed, "
          "not raised at the launcher",
          f"launch.main propagated an internal failure (returned {rc}); a "
          f"game would refuse to start because a mapping could not be "
          f"narrowed")


# ---------------------------------------------------------------------------
# protocol.daemon_pids -- structural argv matching, on synthetic /proc data
# ---------------------------------------------------------------------------

def write_proc(root, pid, argv, environ):
    entry = root / str(pid)
    entry.mkdir(parents=True, exist_ok=True)
    if argv is not None:
        entry.joinpath("cmdline").write_bytes(
            b"".join(part.encode() + b"\0" for part in argv))
    if environ is not None:
        entry.joinpath("environ").write_bytes(
            b"".join(f"{k}={v}".encode() + b"\0" for k, v in environ.items()))
    return entry


def with_fake_proc(root, runtime):
    """daemon_pids(runtime) reading `root` in place of /proc."""
    real = protocol.Path

    def fake(arg, *rest):
        text = str(arg)
        if rest:
            return real(arg, *rest)
        if text == "/proc":
            return real(root)
        if text.startswith("/proc/"):
            return real(root) / text[len("/proc/"):]
        return real(text)

    protocol.Path = fake
    try:
        return sorted(protocol.daemon_pids(runtime))
    finally:
        protocol.Path = real


def scenario_daemon_pids():
    heading("S18: ensure-daemon finds the daemon by argv shape, not by text")

    root = SANDBOX / "fakeproc"
    ours = os.environ["XDG_RUNTIME_DIR"]
    theirs = "/run/user/65534"
    python = "/nix/store/whatever/bin/python3"

    write_proc(root, 1001, [python, "-m", "padmap.cli", "serve"],
               {"XDG_RUNTIME_DIR": ours, "HOME": "/home/x"})
    write_proc(root, 1002, [python, "-m", "padmap.cli", "serve"],
               {"XDG_RUNTIME_DIR": theirs})
    write_proc(root, 1003,
               ["/bin/bash", "-c", "pgrep -f 'padmap.cli serve' | xargs kill"],
               {"XDG_RUNTIME_DIR": ours})
    write_proc(root, 1004, ["/bin/sh", "-c", "padmap.cli", "serve"],
               {"XDG_RUNTIME_DIR": ours})
    write_proc(root, 1005, [], {"XDG_RUNTIME_DIR": ours})
    write_proc(root, 1006, [python, "-m", "padmap.cli", "serve", "--verbose"],
               {"XDG_RUNTIME_DIR": ours})
    write_proc(root, 1007, [python, "-m", "padmap.cli", "serve"], None)
    write_proc(root, 1008, [python, "-m", "padmap.cli"],
               {"XDG_RUNTIME_DIR": ours})
    write_proc(root, 1009, ["/usr/bin/grep", "-m", "1", "padmap.cli", "serve"],
               {"XDG_RUNTIME_DIR": ours})
    # Not a pid at all: /proc is full of these.
    (root / "self").mkdir(exist_ok=True)
    (root / "cpuinfo").write_text("processor: 0\n")

    found = with_fake_proc(root, ours)

    check(1001 in found,
          "a real `python -m padmap.cli serve` on our runtime dir is found",
          f"the daemon was not found among {found}; ensure-daemon would start "
          f"a second one beside it")
    check(1002 not in found,
          "a daemon on another XDG_RUNTIME_DIR is left alone",
          f"a daemon serving a different runtime dir was matched ({found}); "
          f"stopping it would take down a session that is none of our "
          f"business")
    check(1003 not in found,
          "a shell whose command line merely mentions 'padmap.cli serve' is "
          "not matched",
          f"a diagnostic shell was matched as a daemon ({found}); signalling "
          f"it would kill the user's shell")
    check(1004 not in found,
          "a shell whose argv *ends* with padmap.cli serve is still not "
          "matched: an interpreter running the module is required",
          f"`sh -c padmap.cli serve` was matched as a daemon ({found}); "
          f"signalling it would kill the user's shell")
    check(1005 not in found,
          "a process with an empty cmdline (a kernel thread) is skipped, not "
          "an IndexError",
          f"an empty argv was matched ({found})")
    check(1006 not in found,
          "argv with a flag appended is not the daemon padmap spawns",
          f"a differently-spelled argv was matched ({found}); see the note in "
          f"cli._spawn_daemon")
    check(1008 not in found,
          "`python -m padmap.cli` with no subcommand is not a daemon",
          f"a bare padmap.cli invocation was matched ({found})")
    check("self" not in str(found) and all(isinstance(p, int) for p in found),
          "non-numeric /proc entries are skipped and every result is an int",
          f"daemon_pids returned something that is not a pid: {found}")

    if 1009 in found:
        gap("`grep -m 1 padmap.cli serve` is matched as a daemon: the "
            "structural test looks for '-m' anywhere in argv rather than "
            "immediately before the module, so a diagnostic that greps for "
            "the daemon can be signalled instead of it")

    # A process whose environ cannot be read (or has none) is treated as
    # XDG_RUNTIME_DIR=/tmp, which is the documented default.
    default = with_fake_proc(root, "/tmp")
    check(1007 in default,
          "a process with no readable environ falls back to the /tmp default "
          "rather than being matched everywhere",
          f"the environ default is not /tmp: {default}")
    check(1007 not in found,
          "and it is therefore not matched for a real runtime dir",
          f"a process with no readable environ was matched for {ours}")

    elsewhere = with_fake_proc(root, "/run/user/12345")
    check(elsewhere == [],
          "asking about a runtime dir nothing serves finds nothing",
          f"an unrelated runtime dir matched {elsewhere}")

    # The same question against the real /proc, where a live daemon may well
    # be running: our sandbox runtime dir must match none of it.
    real_hits = protocol.daemon_pids(runtime=str(SANDBOX / "no-such-runtime"))
    check(real_hits == [],
          "against the real /proc, a runtime dir we invented matches no "
          "process at all",
          f"daemon_pids matched live processes {real_hits} for a runtime dir "
          f"that has never existed; ensure-daemon would signal them")


# ---------------------------------------------------------------------------
# The command line itself
# ---------------------------------------------------------------------------

def scenario_unknown_commands_and_flags():
    heading("every command: unknown subcommands and flags are refused")

    proc = padmap()
    refused(proc, "no subcommand", expect_message="required")
    no_traceback(proc, "no subcommand")

    proc = padmap("bogus-command")
    refused(proc, "an unknown subcommand", expect_message="invalid choice")
    no_traceback(proc, "an unknown subcommand")
    check(proc.returncode == 2,
          "an unknown subcommand exits 2, the usage-error code",
          f"exit {proc.returncode} for an unknown subcommand")

    for command, flag in (
        ("forget", "--bogus"),
        ("clean-config", "--bogus"),
        ("export-pegasus", "--bogus"),
        ("fetch-art", "--bogus"),
        ("ensure-daemon", "--bogus"),
        ("hide", "--bogus"),
        ("serve", "--bogus"),
        ("list", "--bogus"),
    ):
        proc = padmap(command, flag)
        refused(proc, f"`{command} {flag}`",
                expect_message="unrecognized arguments")
        no_traceback(proc, f"`{command} {flag}`")

    proc = padmap("hide", "extra-positional")
    refused(proc, "`hide` with a stray positional",
            expect_message="unrecognized arguments")

    proc = padmap("forget", "--all", "extra-positional")
    refused(proc, "`forget --all` with a stray positional",
            expect_message="unrecognized arguments")

    # `--` passthrough exists for `launch` only; anywhere else it has to be an
    # error rather than silently ignored.
    proc = padmap("clean-config", "--", "--dry-run")
    refused(proc, "`clean-config -- --dry-run`",
            expect_message="unrecognized arguments")
    no_traceback(proc, "`clean-config -- --dry-run`")

    proc = padmap("--", "--nonsense")
    refused(proc, "`padmap -- --nonsense` with no subcommand at all",
            expect_message="required")

    proc = padmap("--help")
    check(proc.returncode == 0 and "clean-config" in proc.stdout,
          "`--help` still lists the maintenance commands",
          "padmap --help did not exit 0 listing the subcommands")


def scenario_flag_values_that_look_like_flags():
    heading("every command: a flag value that looks like another flag")

    for args, label in (
        (("clean-config", "--config"), "clean-config --config <nothing>"),
        (("clean-config", "--config", "--dry-run"),
         "clean-config --config --dry-run"),
        (("export-pegasus", "--playlists"),
         "export-pegasus --playlists <nothing>"),
        (("export-pegasus", "--playlists", "--out"),
         "export-pegasus --playlists --out"),
        (("export-pegasus", "--out", "--no-game-dirs"),
         "export-pegasus --out --no-game-dirs"),
        (("fetch-art", "--playlists", "--dry-run"),
         "fetch-art --playlists --dry-run"),
        (("fetch-art", "--dest", "--kind"), "fetch-art --dest --kind"),
        (("ensure-daemon", "--timeout"), "ensure-daemon --timeout <nothing>"),
        (("ensure-daemon", "--timeout", "--check"),
         "ensure-daemon --timeout --check"),
    ):
        proc = padmap(*args)
        refused(proc, f"`{label}`", expect_message="expected one argument")
        no_traceback(proc, f"`{label}`")


def scenario_numeric_flags():
    heading("S18/S22: numbers that are not numbers, or are absurd")

    for args, label in (
        (("ensure-daemon", "--timeout", "abc"), "ensure-daemon --timeout abc"),
        (("ensure-daemon", "--timeout", "10s"), "ensure-daemon --timeout 10s"),
        (("fetch-art", "--jobs", "abc"), "fetch-art --jobs abc"),
        (("fetch-art", "--jobs", "1e9"), "fetch-art --jobs 1e9"),
        (("setup", "-n", "abc"), "setup -n abc"),
        (("setup", "-n", "3.5"), "setup -n 3.5"),
        (("ui", "-n", "abc"), "ui -n abc"),
        (("ui", "-n", ""), "ui -n <empty string>"),
    ):
        proc = padmap(*args)
        refused(proc, f"`{label}`", expect_message="invalid")
        no_traceback(proc, f"`{label}`")

    proc = padmap("fetch-art", "--kind", "Named_Bogus")
    refused(proc, "`fetch-art --kind Named_Bogus`",
            expect_message="invalid choice")

    # ensure-daemon with a nonsense timeout must still not start anything:
    # --check is the read-only mode.
    proc = padmap("ensure-daemon", "--check", "--timeout", "-1")
    refused(proc, "`ensure-daemon --check --timeout -1` (no daemon here)",
            expect_message="no daemon running")
    no_traceback(proc, "`ensure-daemon --check --timeout -1`")

    proc = padmap("ensure-daemon", "--check", "--timeout", "1e400")
    refused(proc, "`ensure-daemon --check --timeout 1e400` (infinite)",
            expect_message="no daemon running")
    no_traceback(proc, "`ensure-daemon --check --timeout 1e400`")

    # -n 0 / -n -5 are accepted by the parser; only an empty controller list
    # stops them here, which is what the sandbox guarantees.
    for value in ("0", "-5", "99999999"):
        proc = padmap("setup", "-n", value)
        no_traceback(proc, f"`setup -n {value}`")
        check(proc.returncode != 0,
              f"`setup -n {value}` exits non-zero with no controllers",
              f"`setup -n {value}` exited 0 having assigned nothing")
    gap("`setup -n 0`, `-n -5` and `-n 99999999` are accepted by the parser "
        "and only stop because no controller is visible; with pads plugged in "
        "a zero or negative count opens a session that can never finish")


def scenario_ensure_daemon_check():
    heading("S18: ensure-daemon --check on a runtime dir with no daemon")

    proc = padmap("ensure-daemon", "--check")
    no_traceback(proc, "`ensure-daemon --check`")
    refused(proc, "`ensure-daemon --check`",
            expect_message="no daemon running")

    proc = padmap("ensure-daemon", "--check", "extra")
    refused(proc, "`ensure-daemon --check extra`",
            expect_message="unrecognized arguments")

    # It must not have spawned one either -- the whole point of --check.
    socket = pathlib.Path(os.environ["XDG_RUNTIME_DIR"]) / "padmap"
    check(not (socket / "padmap.sock").exists(),
          "`--check` started no daemon on our runtime dir",
          "ensure-daemon --check created a socket; it changed state in the "
          "mode that promises not to")


def scenario_clean_config_paths():
    heading("S21: clean-config given a path that is not a config")

    proc = padmap("clean-config", "--config", "/nonexistent/retroarch.cfg")
    refused(proc, "a --config path that does not exist",
            expect_message="No RetroArch config at")
    no_traceback(proc, "a --config path that does not exist")

    proc = padmap("clean-config", "--config", str(SANDBOX))
    refused(proc, "a --config path that is a directory",
            expect_message="No RetroArch config at")
    no_traceback(proc, "a --config path that is a directory")

    proc = padmap("clean-config", "--config", "/dev/null")
    refused(proc, "a --config path that is a device node",
            expect_message="No RetroArch config at")

    dangling = SANDBOX / "dangling.cfg"
    if not dangling.is_symlink():
        dangling.symlink_to(SANDBOX / "nothing-here.cfg")
    proc = padmap("clean-config", "--config", str(dangling))
    refused(proc, "a --config symlink pointing nowhere",
            expect_message="No RetroArch config at")
    no_traceback(proc, "a --config symlink pointing nowhere")

    # Unreadable file: is_file() succeeds, the read does not.
    locked = SANDBOX / "locked.cfg"
    locked.write_text('input_player3_joypad_index = "0"\n')
    os.chmod(locked, 0o000)
    try:
        proc = padmap("clean-config", "--config", str(locked))
        no_traceback(proc, "a --config file with no read permission")
        refused(proc, "a --config file with no read permission",
                expect_message="Could not rewrite")
    finally:
        os.chmod(locked, 0o600)

    # Writable file in an unwritable directory: the backup cannot be taken.
    ro_dir = SANDBOX / "readonly-dir"
    ro_dir.mkdir(exist_ok=True)
    cfg = ro_dir / "retroarch.cfg"
    original = 'input_player3_joypad_index = "0"\n'
    cfg.write_text(original)
    os.chmod(ro_dir, 0o500)
    try:
        proc = padmap("clean-config", "--config", str(cfg))
        no_traceback(proc, "a config whose directory cannot be written")
        refused(proc, "a config whose directory cannot be written",
                expect_message="Could not rewrite")
        check(cfg.read_text() == original,
              "and the config is left exactly as it was",
              "clean-config half-rewrote a file it could not back up")
    finally:
        os.chmod(ro_dir, 0o700)

    # An empty --config is falsy, so it silently means "the default".
    default_cfg = RA_CONFIG_DIR / "retroarch.cfg"
    default_cfg.write_text('input_player3_joypad_index = "0"\n')
    proc = padmap("clean-config", "--config", "", "--dry-run")
    no_traceback(proc, "`clean-config --config ''`")
    check(proc.returncode == 0,
          "`clean-config --config ''` does not crash",
          f"an empty --config exited {proc.returncode} with a hard failure")
    if str(default_cfg) in proc.stdout:
        gap("`clean-config --config \"\"` (an unset shell variable) silently "
            "falls back to the *default* retroarch.cfg instead of refusing an "
            "empty path -- and without --dry-run that rewrites a file the "
            "caller never named")


def scenario_clean_config_hostile_content():
    heading("S21: clean-config on a retroarch.cfg that is not what it claims")

    empty = SANDBOX / "empty.cfg"
    empty.write_text("")
    proc = padmap("clean-config", "--config", str(empty))
    no_traceback(proc, "an empty config file")
    check(proc.returncode == 0 and "no padmap leftovers" in proc.stdout,
          "an empty config is reported as having no leftovers, exit 0",
          f"an empty config exited {proc.returncode}: {proc.stdout[:120]!r}")

    binary = SANDBOX / "binary.cfg"
    binary.write_bytes(bytes(range(256)) * 8)
    proc = padmap("clean-config", "--config", str(binary))
    no_traceback(proc, "a config that is arbitrary binary")
    check(proc.returncode == 0,
          "arbitrary binary in place of a config is not an error, just no "
          "leftovers",
          f"a binary config exited {proc.returncode}")
    check(binary.read_bytes() == bytes(range(256)) * 8,
          "and it is not rewritten",
          "clean-config rewrote a file it found no leftovers in")

    huge_key = SANDBOX / "hugekey.cfg"
    huge_key.write_text("x" * 500000 + ' = "1"\n')
    proc = padmap("clean-config", "--config", str(huge_key))
    no_traceback(proc, "a config with a 500k-character key")
    check(proc.returncode == 0,
          "a half-megabyte setting name is survivable",
          f"an absurd key exited {proc.returncode}")

    # Non-UTF-8 *and* a real padmap leftover: the leftover must be fixed.
    latin = SANDBOX / "latin1.cfg"
    original = (b'input_player3_joypad_index = "0"\n'
                b'rgui_browser_directory = "/roms/Pok\xe9mon"\n')
    latin.write_bytes(original)
    proc = padmap("clean-config", "--config", str(latin))
    no_traceback(proc, "a config with a latin-1 path in it")
    check(proc.returncode == 0 and "input_player3_joypad_index" in proc.stdout,
          "a padmap leftover beside a latin-1 path is still corrected",
          f"clean-config did not fix the leftover: {proc.stdout[:200]!r}")
    after = latin.read_bytes()
    backup = latin.with_suffix(".cfg.padmap-backup")
    if b"\xe9" not in after or (backup.is_file()
                                and b"\xe9" not in backup.read_bytes()):
        gap("a retroarch.cfg containing non-UTF-8 bytes (a latin-1 ROM path) "
            "is rewritten with every such byte replaced by U+FFFD, and the "
            "'Original saved to ...' backup is mangled the same way, so the "
            "user's own settings are corrupted and unrecoverable")


def scenario_export_pegasus_paths():
    heading("S22: export-pegasus given directories that are not directories")

    proc = padmap("export-pegasus", "--playlists", "/nonexistent/playlists",
                  "--out", str(SANDBOX / "out-a"))
    refused(proc, "a --playlists directory that does not exist",
            expect_message="No playlist directory at")
    no_traceback(proc, "a --playlists directory that does not exist")

    plain = SANDBOX / "just-a-file"
    plain.write_text("not a playlist directory\n")
    proc = padmap("export-pegasus", "--playlists", str(plain),
                  "--out", str(SANDBOX / "out-b"))
    refused(proc, "a --playlists path that is a regular file",
            expect_message="No playlist directory at")
    no_traceback(proc, "a --playlists path that is a regular file")

    unreadable = SANDBOX / "unreadable-playlists"
    unreadable.mkdir(exist_ok=True)
    (unreadable / "N64.lpl").write_text("{}")
    os.chmod(unreadable, 0o000)
    try:
        proc = padmap("export-pegasus", "--playlists", str(unreadable),
                      "--out", str(SANDBOX / "out-c"))
        no_traceback(proc, "a --playlists directory with no read permission")
        refused(proc, "a --playlists directory with no read permission",
                expect_message="No usable playlists")
    finally:
        os.chmod(unreadable, 0o700)

    empty_dir = SANDBOX / "empty-playlists"
    empty_dir.mkdir(exist_ok=True)
    proc = padmap("export-pegasus", "--playlists", str(empty_dir),
                  "--out", str(SANDBOX / "out-d"))
    refused(proc, "a --playlists directory with no .lpl files in it",
            expect_message="No usable playlists")
    no_traceback(proc, "a --playlists directory with no .lpl files")


def good_playlists():
    """A directory holding one playlist that really does export."""
    import json

    directory = SANDBOX / "playlists"
    directory.mkdir(exist_ok=True)
    (directory / "Nintendo - Nintendo 64.lpl").write_text(json.dumps({
        "default_core_path": "/cores/mupen64plus_next_libretro.so",
        "items": [{"path": "/roms/n64/GoldenEye 007 (USA).n64",
                   "label": "GoldenEye 007 (USA)"}],
    }))
    return directory


def scenario_export_pegasus_output():
    heading("S22: export-pegasus given an --out it cannot write")

    playlists = good_playlists()

    # Control: the same playlists really do export, so the failures below are
    # about the output path and nothing else.
    good_out = SANDBOX / "out-good"
    proc = padmap("export-pegasus", "--playlists", str(playlists),
                  "--out", str(good_out), "--no-game-dirs")
    check(proc.returncode == 0 and (
        good_out / "Nintendo - Nintendo 64" / "metadata.pegasus.txt").is_file(),
        "control: a valid export writes its collection and exits 0",
        f"the control export failed ({proc.returncode}), so nothing below "
        f"proves anything: {(proc.stdout + proc.stderr)[:300]!r}")

    occupied = SANDBOX / "out-is-a-file"
    occupied.write_text("something else already lives here\n")
    proc = padmap("export-pegasus", "--playlists", str(playlists),
                  "--out", str(occupied))
    refused(proc, "an --out path that is an existing file")
    if traceback_in(proc):
        gap("`export-pegasus --out <an existing file>` dies with a raw "
            "NotADirectoryError traceback instead of saying the output path "
            "is not a directory")
    else:
        no_traceback(proc, "an --out path that is an existing file")

    ro_parent = SANDBOX / "readonly-parent"
    ro_parent.mkdir(exist_ok=True)
    os.chmod(ro_parent, 0o500)
    try:
        proc = padmap("export-pegasus", "--playlists", str(playlists),
                      "--out", str(ro_parent / "collections"))
        refused(proc, "an --out directory that cannot be created")
        if traceback_in(proc):
            gap("`export-pegasus --out <under an unwritable directory>` dies "
                "with a raw PermissionError traceback instead of saying it "
                "cannot write there")
        else:
            no_traceback(proc, "an --out directory that cannot be created")
    finally:
        os.chmod(ro_parent, 0o700)

    # An empty --out is Path("") -- the current directory.
    cwd = SANDBOX / "cwd-probe"
    cwd.mkdir(exist_ok=True)
    proc = padmap("export-pegasus", "--playlists", str(playlists),
                  "--out", "", "--no-game-dirs", cwd=cwd)
    no_traceback(proc, "an empty --out")
    if proc.returncode == 0 and (cwd / "Nintendo - Nintendo 64").is_dir():
        gap("`export-pegasus --out \"\"` (an unset shell variable) writes the "
            "collections into the current working directory instead of "
            "refusing an empty path")


def scenario_export_pegasus_hostile_playlists():
    heading("S22: export-pegasus on .lpl files that are not playlists")

    import json

    directory = SANDBOX / "hostile-playlists"
    shutil.rmtree(directory, ignore_errors=True)
    directory.mkdir()
    (directory / "truncated.lpl").write_text('{"items": [')
    (directory / "notjson.lpl").write_text("\x00\x01\x02 not json at all")
    (directory / "toplevel-list.lpl").write_text("[1, 2, 3]")
    (directory / "no-items.lpl").write_text('{"default_core_path": "x"}')
    (directory / "empty.lpl").write_text("")
    subdir = directory / "adirectory.lpl"
    subdir.mkdir(exist_ok=True)

    proc = padmap("export-pegasus", "--playlists", str(directory),
                  "--out", str(SANDBOX / "out-hostile"))
    no_traceback(proc, "playlists that are truncated, binary, empty or a "
                       "directory")
    refused(proc, "playlists that are truncated, binary, empty or a directory",
            expect_message="No usable playlists")

    # A well-formed playlist whose items are not objects.
    bad_items = SANDBOX / "bad-items"
    shutil.rmtree(bad_items, ignore_errors=True)
    bad_items.mkdir()
    (bad_items / "N64.lpl").write_text(json.dumps(
        {"default_core_path": "/cores/mupen64plus_next_libretro.so",
         "items": ["GoldenEye 007 (USA).n64"]}))
    proc = padmap("export-pegasus", "--playlists", str(bad_items),
                  "--out", str(SANDBOX / "out-bad-items"))
    check(proc.returncode != 0,
          "a playlist whose items are strings does not exit 0",
          "export-pegasus reported success on a playlist it could not read")
    if traceback_in(proc):
        gap("a .lpl whose \"items\" holds strings rather than objects kills "
            "`export-pegasus` with AttributeError: 'str' object has no "
            "attribute 'get' -- one malformed playlist takes the whole export "
            "down instead of being skipped like every other unreadable one")
    else:
        no_traceback(proc, "a playlist whose items are strings")


def scenario_forget():
    heading("S20: forget with a profile store somebody has been rummaging in")

    from padmap import profiles

    directory = profiles.profile_dir()
    shutil.rmtree(directory, ignore_errors=True)
    directory.mkdir(parents=True, exist_ok=True)

    proc = padmap("forget")
    no_traceback(proc, "`forget` with an empty profile directory")
    check(proc.returncode == 0 and "Nothing to forget" in proc.stdout,
          "`forget` with nothing stored says so and exits 0",
          f"forget on an empty store exited {proc.returncode}: "
          f"{proc.stdout[:150]!r}")

    (directory / "notjson.json").write_text("\x00\x01 not json")
    (directory / "empty.json").write_text("")
    (directory / "truncated.json").write_text('{"signature": ')
    proc = padmap("forget")
    no_traceback(proc, "`forget` over profiles that are not JSON at all")
    check(proc.returncode == 0,
          "`forget` skips profiles that will not parse",
          f"forget exited {proc.returncode} over unparseable profiles")

    # Valid JSON that is not an object. The same shape crashed controller
    # discovery once and was fixed there; `forget` reads the store itself.
    (directory / "toplevel-list.json").write_text("[1,2,3]")
    (directory / "null.json").write_text("null")
    (directory / "number.json").write_text("42")
    proc = padmap("forget")
    check(proc.returncode != 0 or not traceback_in(proc),
          "`forget` over a profile that is valid JSON but not an object does "
          "not claim success it did not have",
          "forget crashed and still exited 0")
    if traceback_in(proc):
        gap("`padmap forget` dies with AttributeError: 'NoneType' object has "
            "no attribute 'get' when the profile store holds a file of valid "
            "JSON that is not an object (`null`, `42`, `[1,2,3]`) -- the "
            "except clause catches OSError and ValueError but json.loads "
            "returns a non-dict without raising either, so the command that "
            "exists to recover from a bad profile store is stopped by one")
        for name in ("toplevel-list.json", "null.json", "number.json"):
            (directory / name).unlink()
    else:
        no_traceback(proc, "`forget` over a profile that is not a JSON object")

    unreadable = directory / "locked.json"
    unreadable.write_text('{"signature": "x"}')
    os.chmod(unreadable, 0o000)
    try:
        proc = padmap("forget")
        no_traceback(proc, "`forget` over a profile it cannot read")
        check(proc.returncode == 0,
              "`forget` skips a profile with no read permission",
              f"forget exited {proc.returncode} over an unreadable profile")
    finally:
        os.chmod(unreadable, 0o600)

    # prompted is a plain text file; make it a directory instead.
    runtime = pathlib.Path(os.environ["XDG_RUNTIME_DIR"]) / "padmap"
    runtime.mkdir(parents=True, exist_ok=True)
    prompted = runtime / "prompted"
    if prompted.is_file():
        prompted.unlink()
    prompted.mkdir(exist_ok=True)
    try:
        proc = padmap("forget")
        no_traceback(proc, "`forget` when the 'already asked' record is a "
                           "directory")
        check(proc.returncode == 0,
              "`forget` survives an unreadable 'already asked' record",
              f"forget exited {proc.returncode} over a directory named "
              f"'prompted'")
    finally:
        prompted.rmdir()

    # A directory named like a profile. `forget` without --all skips it (the
    # read fails); `forget --all` deletes without looking.
    trap = directory / "trap.json"
    trap.mkdir(exist_ok=True)
    proc = padmap("forget")
    no_traceback(proc, "`forget` with a directory named *.json in the store")
    check(proc.returncode == 0,
          "`forget` ignores a directory named like a profile",
          f"forget exited {proc.returncode} over a directory named trap.json")

    proc = padmap("forget", "--all")
    check(proc.returncode != 0 or not traceback_in(proc),
          "`forget --all` over a directory named *.json does not report "
          "success it did not have",
          "forget --all crashed but still exited 0")
    if traceback_in(proc):
        gap("`forget --all` dies with IsADirectoryError when the profile "
            "store contains a directory whose name ends in .json: --all "
            "unlinks every glob hit without checking, while the default path "
            "skips the same entry safely -- so the recovery command is the "
            "one that crashes")
    else:
        no_traceback(proc, "`forget --all` with a directory named *.json")
    if trap.is_dir():
        trap.rmdir()


def scenario_hide():
    heading("S19: hide, which is the one command that is run as root")

    proc = padmap("hide", "--print")
    no_traceback(proc, "`hide --print`")
    check(proc.returncode == 0,
          "`hide --print` exits 0 with no controllers to hide",
          f"hide --print exited {proc.returncode}: {proc.stderr[:200]!r}")

    proc = padmap("hide", "--install")
    no_traceback(proc, "`hide --install` without root")
    check(proc.returncode == 0 and "needs root" in proc.stdout,
          "`hide --install` without root says so instead of half-installing",
          f"hide --install exited {proc.returncode}: {proc.stdout[:200]!r}")

    proc = padmap("hide", "--install", "--print")
    no_traceback(proc, "`hide --install --print` (contradictory flags)")
    check(proc.returncode == 0,
          "contradictory --install --print prints rather than failing",
          f"hide --install --print exited {proc.returncode}")

    # sudo runs this with root's XDG_RUNTIME_DIR, where the assignment file is
    # unreadable. Simulate the file being there and unreadable.
    runtime = pathlib.Path(os.environ["XDG_RUNTIME_DIR"]) / "padmap"
    runtime.mkdir(parents=True, exist_ok=True)
    state = runtime / "assignments.json"
    state.write_text("[]")
    os.chmod(state, 0o000)
    try:
        proc = padmap("hide", "--print")
        no_traceback(proc, "`hide` with an unreadable assignments.json")
        check(proc.returncode == 0,
              "`hide` reads the pads from /sys when the assignment file "
              "cannot be read (which is what sudo does)",
              f"hide exited {proc.returncode} because it could not read the "
              f"assignment file")
    finally:
        os.chmod(state, 0o600)
        state.write_text("[]")

    state.write_text("not json at all")
    proc = padmap("hide", "--print")
    no_traceback(proc, "`hide` with a corrupt assignments.json")
    check(proc.returncode == 0,
          "`hide` survives a corrupt assignment file",
          f"hide exited {proc.returncode} over a corrupt assignments.json")
    state.unlink()


def scenario_launch_passthrough():
    heading("S14: `padmap launch` forwards nonsense instead of choking on it")

    # With no assignments this stops before RetroArch is started, which is
    # what makes it safe to run at all here.
    for args, label in (
        (("launch", "--log"), "launch --log with no path"),
        (("launch", "--bogus"), "launch --bogus (forwarded, not rejected)"),
        (("launch", "--", "-L", "core.so"), "launch -- -L core.so"),
        (("launch", "-L"), "launch -L with no value"),
    ):
        proc = padmap(*args)
        no_traceback(proc, f"`{label}`")
        refused(proc, f"`{label}`", expect_message="No assignments")


def main():
    print(f"sandbox: {SANDBOX}")
    print(f"XDG_RUNTIME_DIR={os.environ['XDG_RUNTIME_DIR']}")

    # Precondition: discovery must be empty, or the device-touching scenarios
    # below would reach real controllers.
    probe = padmap("list")
    if "No joypads found." not in probe.stdout:
        raise SystemExit(
            "FAIL: PADMAP_ONLY_DEVICE did not hide the real controllers "
            "(`padmap list` said: "
            f"{probe.stdout.strip()[:120]!r}); refusing to run, because these "
            "scenarios must never touch a pad the live daemon owns"
        )
    ok("guard: discovery is empty, so nothing here can reach a real pad")

    scenario_split_args_value_flags()
    scenario_split_args_missing_and_repeated()
    scenario_split_args_order_and_separator()
    scenario_launch_main_never_fails_a_game()
    scenario_daemon_pids()
    scenario_unknown_commands_and_flags()
    scenario_flag_values_that_look_like_flags()
    scenario_numeric_flags()
    scenario_ensure_daemon_check()
    scenario_clean_config_paths()
    scenario_clean_config_hostile_content()
    scenario_export_pegasus_paths()
    scenario_export_pegasus_output()
    scenario_export_pegasus_hostile_playlists()
    scenario_forget()
    scenario_hide()
    scenario_launch_passthrough()

    print(f"\n{CHECKS} checks, {GAPS} gap(s) left for the maintainer")
    shutil.rmtree(SANDBOX, ignore_errors=True)
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
