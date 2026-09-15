#!/usr/bin/env python3
"""Does the mapping a user recorded for one game reach RetroArch, and stay?

The chain this exercises is the real one, start to finish, with nothing
stubbed but the hardware:

    padmap-play  ->  python3 -m padmap.launch -- -L <core.so> <rom>
                 ->  layouts.for_core / profiles.game_key / Profile.resolve
                 ->  $XDG_RUNTIME_DIR/padmap/autoconfig/udev/<player>.cfg

That last file is the only observable that matters: it is what RetroArch reads,
and it is the artefact the reports were about. Everything below asserts on its
*contents* -- the scope it names and the button numbers it binds -- rather than
on a return value, because every failure here is silent. A launch that resolves
the wrong scope still starts the game, still writes a profile, and still reports
success; the only symptom is that the buttons are wrong, which is exactly what
was reported and exactly what no test could see.

The scenarios, in the words of the reports that produced them:

  * "the ability to specify the mapping that my gamecube controller uses for
    n64 games, and then a universal configuration in general" -- so one
    synthetic pad carries three captures with deliberately different bindings,
    and each launch must pick exactly one of them.
  * "didn't seem to apply on the second go" -- a game-scoped profile was being
    overwritten with the default one by `Server._start_republisher`, which
    rewrites the same directory for reasons that have nothing to do with the
    game (a session accepted, a pad reconnecting, the daemon restarted).
    Simulated here: the republisher's write must *preserve* the game scope, and
    the old context-free write is shown still replacing it, so the guard is
    demonstrably load-bearing rather than decorative.

Isolation is total. XDG_RUNTIME_DIR, XDG_CONFIG_HOME and XDG_DATA_HOME are all
redirected into a temp tree before padmap is imported, and the pad enumeration
is replaced in both this process and the launcher's. Nothing here talks to the
live daemon, reads the real profile store, or opens a real controller.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \\
        nix develop --command python3 tests/e2e_scoped_launch.py [--keep]
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
KEEP = "--keep" in sys.argv

# The temp tree and the environment pointing at it are built *before* padmap is
# imported, and deliberately at module scope. `retroarch.CONFIG_DIR` is
# evaluated at import time from RETROARCH_CONFIG_DIR/$HOME, so redirecting it
# inside main() would be too late and the run would scan the user's real
# RetroArch autoconfig directory.
ROOT = Path(tempfile.mkdtemp(prefix="padmap-scoped-launch-"))
RUNTIME = ROOT / "run"
CONFIG = ROOT / "config"
DATA = ROOT / "data"
SHIM = ROOT / "shim"
ROMS = ROOT / "roms"
CORES = ROOT / "cores"
for _path in (RUNTIME, CONFIG, DATA, SHIM, ROMS, CORES):
    _path.mkdir(parents=True, exist_ok=True)
RUNTIME.chmod(0o700)          # XDG_RUNTIME_DIR is 0700 by the spec
(RUNTIME / "padmap").mkdir(exist_ok=True)

PADS_JSON = ROOT / "pads.json"

os.environ.update({
    "XDG_RUNTIME_DIR": str(RUNTIME),
    "XDG_CONFIG_HOME": str(CONFIG),
    "XDG_DATA_HOME": str(DATA),
    # autoconfig_dirs() reaches into ~/.config/retroarch otherwise.
    "RETROARCH_CONFIG_DIR": str(CONFIG / "retroarch"),
    "PADMAP_AUTOCONFIG_DIRS": str(ROOT / "autoconfig-db"),
    # The virtual pad's advertised ids. In mirror mode `identity_for` opens the
    # source pad's device node to read them, and these pads have none -- it
    # falls back to padmap's own ids and logs a warning while doing it. Saying
    # so explicitly keeps the launcher's stderr, which several checks below
    # read, free of a warning that is an artefact of the test rather than of
    # the code under test.
    "PADMAP_PAD_IDENTITY": "padmap",
    "PADMAP_E2E_PADS": str(PADS_JSON),
})
(ROOT / "autoconfig-db").mkdir(exist_ok=True)

sys.path.insert(0, str(REPO / "src"))

from padmap import (  # noqa: E402
    controllercfg, devices, layouts, profiles, protocol, retroarch,
)
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


# -- the hardware, and only the hardware --------------------------------------
#
# Two physical pads and the two virtual ones padmap would republish them as.
# Synthetic vid/pid and names nothing in libretro's database can match, so
# `find_profile` finds no upstream profile to copy and the emitted .cfg is
# built purely from what these checks recorded.
#
# The physical pads are `retroarch_visible=False`, which is what the `padmap
# hide` udev rules make them: padmap republishes them and RetroArch is shown
# only the virtual pads. Getting that wrong here would shift every pad index in
# the launch override.
PAD_ONE = Pad(path="/dev/input/event200", name="padmap e2e GameCube pad",
              phys="usb-e2e-1/input0", uniq="", vid=0x1209, pid=0xF00D,
              syspath="/sys/e2e/input200", retroarch_visible=False)
PAD_TWO = Pad(path="/dev/input/event201", name="padmap e2e plain pad",
              phys="usb-e2e-2/input0", uniq="", vid=0x1209, pid=0xF00E,
              syspath="/sys/e2e/input201", retroarch_visible=False)
VIRTUAL_PATHS = {1: "/dev/input/event210", 2: "/dev/input/event211"}
VIRTUAL_PADS = [
    Pad(path=VIRTUAL_PATHS[1], name=retroarch.virtual_name(1),
        phys="padmap/1", uniq="", vid=0x1209, pid=0x0BAD,
        syspath="/sys/e2e/input210"),
    Pad(path=VIRTUAL_PATHS[2], name=retroarch.virtual_name(2),
        phys="padmap/2", uniq="", vid=0x1209, pid=0x0BAD,
        syspath="/sys/e2e/input211"),
]
ASSIGNMENTS = [
    Assignment(player=1, pad=PAD_ONE, button=0),
    Assignment(player=2, pad=PAD_TWO, button=0),
]


def fake_discover(include_virtual: bool = False,
                  retroarch_only: bool = False) -> list[Pad]:
    """The pad enumeration, without a kernel.

    The one thing stubbed. Everything downstream -- the profile store, the key
    derivation, the scope resolution, the .cfg -- is the shipped code working
    on real files.
    """
    pads = [PAD_ONE, PAD_TWO]
    if include_virtual:
        pads = pads + VIRTUAL_PADS
    if retroarch_only:
        pads = [pad for pad in pads if pad.retroarch_visible]
    return pads


devices.discover = fake_discover  # type: ignore[assignment]


# -- three captures of one controller -----------------------------------------
#
# Told apart by RetroArch keys only one of them can produce, and by button
# numbers only one of them holds. Both matter: a check that only looked for
# "some profile was written" would pass on every bug this file exists to catch,
# and a check that only read the scope comment in the header would pass on a
# profile that named the right scope and bound the wrong buttons.
#
#   gamecube overrides a -> input_a_btn (dolphin binds GC A from RetroPad A),
#   and it is the only layout of the three with an X at all.
#   n64 has no control that reaches input_a_btn -- mupen64plus-next never reads
#   RetroPad A -- and overrides b -> input_y_btn.
#
# Start is captured at a different button in each, so a single line of the .cfg
# says which capture won: 7 default, 17 console, 27 game.
DEFAULT_CAPTURE = profiles.Mapping(
    layout="gamecube",
    buttons={
        "a": Binding("button", 0), "b": Binding("button", 1),
        "x": Binding("button", 2), "y": Binding("button", 3),
        "start": Binding("button", 7),
    },
)
CONSOLE_CAPTURE = profiles.Mapping(
    layout="n64",
    buttons={
        "a": Binding("button", 10), "b": Binding("button", 11),
        "start": Binding("button", 17),
        # The N64 Z trigger is an analogue axis on this adapter, which is the
        # one binding that has to land under an _axis key rather than _btn.
        "lefttrigger": Binding("axis", 4, 1),
        "rightstick_up": Binding("button", 13),
    },
)
GAME_CAPTURE = profiles.Mapping(
    layout="n64",
    buttons={
        "a": Binding("button", 20), "b": Binding("button", 21),
        "start": Binding("button", 27),
        "rightstick_right": Binding("button", 23),
    },
)
PAD_TWO_CAPTURE = profiles.Mapping(
    layout="generic",
    buttons={"a": Binding("button", 4), "start": Binding("button", 9)},
)

# Filenames, not contents, are what drives every derivation here: `for_core`
# reads the core's basename and `game_key` the ROM's stem. No real core and no
# real ROM is needed, and deliberately so -- a test that needed a 300MB N64
# image could not be run.
N64_CORE = CORES / "mupen64plus_next_libretro.so"
SNES_CORE = CORES / "snes9x_libretro.so"
UNKNOWN_CORE = CORES / "genesis_plus_gx_libretro.so"
MAPPED_ROM = ROMS / "Super Smash Bros. (U) [!].z64"
OTHER_ROM = ROMS / "GoldenEye 007 (U) [!].z64"
SNES_ROM = ROMS / "Super Metroid (JU).sfc"
MISSING_ROM = ROMS / "Never Dumped (U).z64"

MAPPED_KEY = ""     # filled in by build_world(), from the real game_key()

LAUNCH_CFG = RUNTIME / "padmap" / "launch.cfg"

PLAYER_ONE_CFG = retroarch.runtime_autoconfig_dir() / "udev" / (
    retroarch.virtual_name(1) + ".cfg")
PLAYER_TWO_CFG = retroarch.runtime_autoconfig_dir() / "udev" / (
    retroarch.virtual_name(2) + ".cfg")

# Installed into the launcher subprocess through PYTHONPATH. `python3 -m
# padmap.launch` is spawned verbatim, exactly as the padmap-play wrapper spawns
# it, so there is no argument or import hook to pass a stub through -- and the
# launcher's very first act is to enumerate pads. sitecustomize is the one
# place a stub can be installed without changing the command line.
SHIM_SOURCE = '''\
"""Stand in for the input hardware inside the launcher, and nothing else."""
import json
import os
import sys

# Chain to whatever sitecustomize this one shadowed. In a Nix python
# environment that is what puts the site-packages named by NIX_PYTHONPATH on
# sys.path; skipping it leaves evdev unimportable, and padmap.devices imports
# evdev, so the stub below could not even be installed.
_self = os.path.realpath(__file__)
for _entry in list(sys.path):
    _candidate = os.path.join(_entry, "sitecustomize.py")
    if os.path.isfile(_candidate) and os.path.realpath(_candidate) != _self:
        with open(_candidate) as _handle:
            exec(compile(_handle.read(), _candidate, "exec"),
                 {"__file__": _candidate, "__name__": "sitecustomize"})
        break

from padmap import devices as _devices

with open(os.environ["PADMAP_E2E_PADS"]) as _handle:
    _spec = json.load(_handle)
_PHYSICAL = [_devices.Pad(**entry) for entry in _spec["physical"]]
_VIRTUAL = [_devices.Pad(**entry) for entry in _spec["virtual"]]


def _discover(include_virtual=False, retroarch_only=False):
    pads = list(_PHYSICAL)
    if include_virtual:
        pads = pads + _VIRTUAL
    if retroarch_only:
        pads = [pad for pad in pads if pad.retroarch_visible]
    return pads


_devices.discover = _discover
# Printed so the checks can prove the stub was actually in force. Without it a
# shim that silently failed to load would let the launcher enumerate the real
# machine, and the run would look like a pass while testing nothing.
print("padmap-e2e: discovery stubbed with %d pad(s)" % len(_PHYSICAL),
      file=sys.stderr)
'''

STUB_MARKER = "padmap-e2e: discovery stubbed with 2 pad(s)"


def pad_json(pad: Pad) -> dict:
    return {
        "path": pad.path, "name": pad.name, "phys": pad.phys,
        "uniq": pad.uniq, "vid": pad.vid, "pid": pad.pid,
        "syspath": pad.syspath, "retroarch_visible": pad.retroarch_visible,
    }


def build_world() -> None:
    """Everything a launch would find already on disk: pads, profiles, state."""
    global MAPPED_KEY

    (SHIM / "sitecustomize.py").write_text(SHIM_SOURCE)
    PADS_JSON.write_text(json.dumps({
        "physical": [pad_json(PAD_ONE), pad_json(PAD_TWO)],
        "virtual": [pad_json(pad) for pad in VIRTUAL_PADS],
    }))

    for core in (N64_CORE, SNES_CORE, UNKNOWN_CORE):
        core.write_bytes(b"")
    for rom in (MAPPED_ROM, OTHER_ROM, SNES_ROM):
        rom.write_bytes(b"\x80\x37\x12\x40" + b"\0" * 64)

    # Through the same API the mapping wizard uses, into the same store, so a
    # change to how a profile is written is a change this test sees.
    one = profiles.Profile(signature=profiles.signature(PAD_ONE),
                           name=PAD_ONE.name, icon="gamecube")
    one.record(profiles.SCOPE_UNIVERSAL, DEFAULT_CAPTURE)
    one.record(profiles.console_scope("n64"), CONSOLE_CAPTURE)
    MAPPED_KEY = profiles.game_key("n64", str(MAPPED_ROM))
    one.record(profiles.game_scope(MAPPED_KEY), GAME_CAPTURE)
    profiles.save(one)

    two = profiles.Profile(signature=profiles.signature(PAD_TWO),
                           name=PAD_TWO.name, icon="generic")
    two.record(profiles.SCOPE_UNIVERSAL, PAD_TWO_CAPTURE)
    profiles.save(two)

    # What the daemon persists when an assignment is accepted, in its own
    # format -- the launcher reads this file and nothing else to learn the
    # player order.
    (RUNTIME / "padmap" / "assignments.json").write_text(json.dumps([
        {"player": a.player, "path": a.pad.path, "name": a.pad.name,
         "phys": a.pad.phys, "vid": a.pad.vid, "pid": a.pad.pid}
        for a in ASSIGNMENTS
    ], indent=2))

    # The override the front-end hands RetroArch via --appendconfig, written by
    # the same call the daemon makes. It is what names the autoconfig directory
    # the launcher then writes into, and it is also a real file for the
    # "a flag's value looks like a path" check to point at.
    retroarch.write_launch_config(ASSIGNMENTS, VIRTUAL_PATHS, LAUNCH_CFG)


def run_launcher(argv: list[str]) -> subprocess.CompletedProcess:
    """`python3 -m padmap.launch -- <retroarch command line>`.

    Spawned exactly as the padmap-play wrapper spawns it, including the `--`
    separator, so the argument handling under test is the real one rather than
    a call into `resolve()` that skips argparse.
    """
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join(
        [str(SHIM), str(REPO / "src")]
        + ([env["PYTHONPATH"]] if env.get("PYTHONPATH") else [])
    )
    result = subprocess.run(
        [sys.executable, "-m", "padmap.launch", "--", *argv],
        env=env, capture_output=True, text=True, timeout=120,
    )
    if STUB_MARKER not in result.stderr:
        fail("the launcher enumerated the real machine's controllers instead "
             "of this test's synthetic ones -- the run would have touched "
             f"live hardware. stderr:\n{result.stderr}")
    return result


def launch(core: Path, rom: Path | None) -> subprocess.CompletedProcess:
    """One launch, with the flags padmap-play really prepends around it."""
    argv = ["--appendconfig", str(LAUNCH_CFG), "--nodevice", "3",
            "-L", str(core)]
    if rom is not None:
        argv.append(str(rom))
    result = run_launcher(argv)
    if result.returncode != 0:
        fail(f"padmap-play exited {result.returncode}, so the game would not "
             f"have started at all. stderr:\n{result.stderr}")
    return result


def scope_of(text: str) -> str:
    """The scope a written profile says it came from, from its own header."""
    for line in text.splitlines():
        if line.startswith("# Mapping scope:"):
            said = line[len("# Mapping scope:"):].split(";")[0]
            return said.strip().rstrip(".")
    fail("the profile does not record which mapping scope it came from, so "
         "'why is player 1 bound like this' is unanswerable from the file "
         f"RetroArch reads:\n{text}")
    return ""


def expect_lines(text: str, present: list[str], absent: list[str],
                 what: str) -> None:
    for line in present:
        if line not in text:
            fail(f"{what}: {line!r} is missing from the profile RetroArch "
                 f"reads, so that control is unbound or bound to the wrong "
                 f"button:\n{text}")
    for line in absent:
        if line in text:
            fail(f"{what}: {line!r} is in the profile, which only the wrong "
                 f"capture could have produced:\n{text}")


# -- checks -------------------------------------------------------------------


def check_isolation() -> None:
    """Nothing this run does may reach the live daemon or the real profiles."""
    print("\nwhere this run reads and writes:")
    for label, path in [
        ("profile store", profiles.profile_dir()),
        ("autoconfig", retroarch.runtime_autoconfig_dir()),
        ("daemon socket", protocol.socket_path()),
        ("last game", protocol.last_game_path()),
        ("retroarch config", retroarch.CONFIG_DIR),
    ]:
        if ROOT not in path.parents and path != ROOT:
            fail(f"the {label} is at {path}, outside the temp tree -- this run "
                 "would have written over the user's own state, and a live "
                 "padmap daemon is using it")
        print(f"  ok  {label:<16} -> {path.relative_to(ROOT)}")


def check_stored_profile() -> None:
    """The three captures are on disk, read back through the shipped loader."""
    print("\nthe controller carries three mappings:")
    stored = profiles.load(PAD_ONE)
    if stored is None:
        fail("the profile just written cannot be read back, so no scope could "
             "resolve to anything")
    expected = {profiles.SCOPE_UNIVERSAL, profiles.console_scope("n64"),
                profiles.game_scope(MAPPED_KEY)}
    if set(stored.mappings) != expected:
        fail(f"scopes on disk are {sorted(stored.mappings)}, expected "
             f"{sorted(expected)}")
    if MAPPED_KEY != "n64/super-smash-bros-u":
        fail(f"the game key derived from the ROM filename is {MAPPED_KEY!r}; a "
             "key that is not stable against the filename means a per-game "
             "mapping stops applying for reasons the user cannot see")
    print(f"  ok  {', '.join(repr(s) for s in sorted(stored.mappings))}")


def check_game_scope() -> None:
    print("\nlaunching the game the mapping was made for:")
    result = launch(N64_CORE, MAPPED_ROM)
    if not PLAYER_ONE_CFG.is_file():
        fail(f"no autoconfig profile at {PLAYER_ONE_CFG}, so RetroArch has "
             "nothing of padmap's to read and the pad is unconfigured")
    text = PLAYER_ONE_CFG.read_text()
    if scope_of(text) != f"game:{MAPPED_KEY}":
        fail(f"the launch resolved {scope_of(text)!r}, not the mapping made "
             "for this very game -- the controls the user recorded for it are "
             "not the ones they will play with")
    expect_lines(
        text,
        present=['input_start_btn = "27"', 'input_y_btn = "21"',
                 'input_r_x_plus_btn = "23"'],
        absent=['input_a_btn', 'input_l2_axis', 'input_start_btn = "17"',
                'input_start_btn = "7"'],
        what="game launch",
    )
    if f"player 1 using the game:{MAPPED_KEY} mapping" not in result.stderr:
        fail("the launcher did not report which mapping it chose; that line is "
             f"the only trace a launch leaves. stderr:\n{result.stderr}")
    if "(console n64)" not in result.stderr:
        fail("the launcher did not name the console it resolved for")
    print(f"  ok  game:{MAPPED_KEY}, start on button 27, C-right bound")
    print("  ok  the launcher said so on stderr")

    print("\nand the other player's pad is untouched by it:")
    two = PLAYER_TWO_CFG.read_text()
    if scope_of(two) != "default (any game)":
        fail(f"player 2 resolved {scope_of(two)!r}; a per-game mapping made "
             "for one controller was applied to a different controller")
    expect_lines(two, present=['input_start_btn = "9"'],
                 absent=['input_start_btn = "27"'], what="player 2")
    print("  ok  player 2 still on its own default mapping")


def check_console_scope() -> None:
    print("\nlaunching a different game on the same console:")
    launch(N64_CORE, OTHER_ROM)
    text = PLAYER_ONE_CFG.read_text()
    if scope_of(text) != "console:n64":
        fail(f"resolved {scope_of(text)!r} for an N64 game with no mapping of "
             "its own; the user's N64 mapping is not being used")
    expect_lines(
        text,
        present=['input_start_btn = "17"', 'input_y_btn = "11"',
                 'input_l2_axis = "+4"'],
        absent=['input_a_btn', 'input_start_btn = "27"',
                'input_r_x_plus_btn = "23"'],
        what="console launch",
    )
    print("  ok  console:n64, start on 17, Z under an axis key not a button")


def check_default_scope() -> None:
    print("\nlaunching something that is not an N64 game at all:")
    launch(SNES_CORE, SNES_ROM)
    text = PLAYER_ONE_CFG.read_text()
    if scope_of(text) != "default (any game)":
        fail(f"a SNES launch resolved {scope_of(text)!r}; the N64 mapping is "
             "leaking onto consoles it was never recorded for")
    expect_lines(
        text,
        present=['input_start_btn = "7"', 'input_a_btn = "0"',
                 'input_x_btn = "2"'],
        absent=['input_start_btn = "17"', 'input_start_btn = "27"',
                'input_l2_axis'],
        what="snes launch",
    )
    print("  ok  the default mapping, with the GameCube key table intact")


def check_unknown_core() -> None:
    """An unrecognised core must degrade, not fail."""
    print("\nlaunching through a core padmap knows nothing about:")
    result = launch(UNKNOWN_CORE, MAPPED_ROM)
    text = PLAYER_ONE_CFG.read_text()
    if layouts.for_core(str(UNKNOWN_CORE)) != "":
        fail("this core is meant to be the unrecognised case and is not; the "
             "check below proves nothing")
    if scope_of(text) != "default (any game)":
        fail(f"an unknown core resolved {scope_of(text)!r}; with no console "
             "known the only safe answer is the controller's default")
    expect_lines(text, present=['input_start_btn = "7"'],
                 absent=['input_start_btn = "17"', 'input_start_btn = "27"'],
                 what="unknown core")
    if "(unknown console)" not in result.stderr:
        fail("the launcher did not say the console was unknown, so a core "
             "padmap does not know about looks exactly like one it does")
    if result.returncode != 0:
        fail("an unrecognised core stopped the launch; a game that will not "
             "start is far worse than one played on the default mapping")
    print("  ok  default mapping, exit 0, and it says the console is unknown")


def check_idempotent() -> None:
    """Twice with the same launch must give byte-identical files.

    Verified by hand while chasing "didn't apply on the second go" and pinned
    here: if the launcher itself drifted between runs, every other check in
    this file would be measuring one run of a coin toss.
    """
    print("\nrunning the very same launch twice:")
    launch(N64_CORE, MAPPED_ROM)
    first_one = PLAYER_ONE_CFG.read_bytes()
    first_two = PLAYER_TWO_CFG.read_bytes()
    first_game = protocol.last_game_path().read_bytes()

    launch(N64_CORE, MAPPED_ROM)
    if PLAYER_ONE_CFG.read_bytes() != first_one:
        fail("the second launch of the same game wrote a different profile, "
             "so which mapping a user gets depends on how many times they "
             "have played it")
    if PLAYER_TWO_CFG.read_bytes() != first_two:
        fail("player 2's profile changed on a repeat launch")
    if protocol.last_game_path().read_bytes() != first_game:
        fail("replaying a game changed the recent-games list, so the scope "
             "picker's entries move around under the user between launches")
    print("  ok  both profiles and the recent-games list byte-identical")


def republish_as_daemon_does() -> None:
    """What `Server._start_republisher` now does, with nothing else it does.

    Lifted deliberately rather than by starting a daemon: the daemon would grab
    controllers and open a socket, and the only part of it this is about is the
    two lines that decide what context its profile write carries.
    """
    last = protocol.read_last_game()
    retroarch.install_profiles(
        ASSIGNMENTS,
        console=last.get("console", ""), game=last.get("key", ""),
        context=last.get("title", ""))


def check_simulation_matches_the_daemon() -> None:
    """The republish simulated below is still the one the daemon performs.

    `_start_republisher` cannot be called for real here: it creates uinput
    devices and grabs controllers, and a live daemon is using the ones on this
    machine. So it is reproduced -- and a reproduction is only worth anything
    while it still resembles the original. This reads the real method and
    checks the two things the fix consists of, so that removing the context
    from the daemon fails here rather than passing quietly against a copy.
    """
    import ast
    import inspect
    import textwrap

    from padmap import server

    print("\nthe daemon's own republish still carries the launch context:")
    source = textwrap.dedent(
        inspect.getsource(server.Server._start_republisher))
    tree = ast.parse(source)
    calls = [
        node for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and getattr(node.func, "attr", "") == "install_profiles"
    ]
    if not calls:
        fail("Server._start_republisher no longer writes autoconfig profiles; "
             "the simulation below is testing something the daemon does not do")
    keywords = {kw.arg for call in calls for kw in call.keywords}
    for needed in ("console", "game", "context"):
        if needed not in keywords:
            fail(f"the daemon's republish passes no {needed!r} to "
                 "install_profiles, so it writes the default mapping over "
                 "whatever the launcher just resolved -- the reported "
                 "'didn't apply on the second go'")
    if "read_last_game" not in source:
        fail("the daemon no longer reads the last launched game, so it has no "
             "context to write with and the check below proves nothing")
    print("  ok  it reads the last game and passes console, game and context")


def check_republish_preserves_game_scope() -> None:
    """The regression: a republish after a launch must not undo the launch.

    Reported as a mapping that "didn't seem to apply on the second go". Two
    writers, one file: the launcher writes the scope resolved from the core and
    ROM in hand, and the daemon rewrote the same path with no context at all,
    which resolves to the default. Republishing happens for reasons that have
    nothing to do with the game -- a setup session accepted, a pad reconnecting,
    the daemon restarted for new code -- so whichever wrote last won, and the
    game mapping was live one launch and gone the next.
    """
    print("\na republish landing after a game-scoped launch:")
    launch(N64_CORE, MAPPED_ROM)
    if scope_of(PLAYER_ONE_CFG.read_text()) != f"game:{MAPPED_KEY}":
        fail("the launch under test did not produce a game scope to begin with")

    last = protocol.read_last_game()
    if last.get("key") != MAPPED_KEY:
        fail(f"the launch recorded {last.get('key')!r} as the last game; the "
             "daemon has no other way to know what is being played, so the "
             "republish below could not preserve anything")
    if last.get("console") != "n64" or not last.get("title"):
        fail(f"the last-game record is incomplete ({last!r}); the console and "
             "the title are what the republish passes back in")

    republish_as_daemon_does()
    text = PLAYER_ONE_CFG.read_text()
    if scope_of(text) != f"game:{MAPPED_KEY}":
        fail(f"a republish replaced the game mapping with {scope_of(text)!r}. "
             "This is the reported bug: the mapping applies on the first go "
             "and silently reverts to the default on the next")
    expect_lines(text, present=['input_start_btn = "27"'],
                 absent=['input_start_btn = "7"'], what="after republish")
    print("  ok  the game scope survived, bindings and all")

    print("\nand the write it replaced would not have:")
    # The old behaviour, run for real, so this check is known to be capable of
    # failing. A guard nobody has watched fail is a guard nobody knows works.
    retroarch.install_profiles(ASSIGNMENTS)
    stale = PLAYER_ONE_CFG.read_text()
    if scope_of(stale) != "default (any game)":
        fail("a context-free install_profiles no longer resolves to the "
             "default, so the check above can no longer tell the fix from the "
             "bug and proves nothing")
    if 'input_start_btn = "7"' not in stale:
        fail("the context-free write did not produce the default bindings")
    print("  ok  context-free write gives the default -- so the check above "
          "can fail")

    # Leave the tree as the fixed code would.
    republish_as_daemon_does()
    if scope_of(PLAYER_ONE_CFG.read_text()) != f"game:{MAPPED_KEY}":
        fail("the game scope could not be restored by a second republish")
    print("  ok  and a further republish puts it back")


def check_no_assignments() -> None:
    """Before anyone has been through setup, a launch must change nothing."""
    print("\nlaunching with no controllers assigned:")
    state = RUNTIME / "padmap" / "assignments.json"
    saved = state.read_bytes()
    before_one = PLAYER_ONE_CFG.read_bytes()
    before_two = PLAYER_TWO_CFG.read_bytes()
    state.unlink()
    try:
        result = launch(N64_CORE, OTHER_ROM)
    finally:
        state.write_bytes(saved)

    if result.returncode != 0:
        fail("a launch with no assignment failed; running a game before ever "
             "opening the setup screen is normal and must still start")
    if "no assigned controllers" not in result.stderr:
        fail(f"nothing said why nothing was written. stderr:\n{result.stderr}")
    if PLAYER_ONE_CFG.read_bytes() != before_one or \
            PLAYER_TWO_CFG.read_bytes() != before_two:
        fail("a launch with no assignments rewrote the autoconfig directory; "
             "the profiles already there are the working answer and must be "
             "left alone")
    print("  ok  exit 0, says so, and leaves the existing profiles alone")


def check_missing_rom() -> None:
    """A ROM that is not there is not a game whose mapping can be resolved."""
    print("\nlaunching a ROM path that does not exist:")
    before = protocol.last_game_path().read_bytes()
    result = launch(N64_CORE, MISSING_ROM)
    text = PLAYER_ONE_CFG.read_text()
    if result.returncode != 0:
        fail("a missing ROM took the launch down with it")
    if scope_of(text) != "console:n64":
        fail(f"resolved {scope_of(text)!r}; with no game identified the "
             "console mapping is still the right answer, and is still better "
             "than the default")
    expect_lines(text, present=['input_start_btn = "17"'],
                 absent=['input_start_btn = "27"'], what="missing rom")
    if protocol.last_game_path().read_bytes() != before:
        fail("a game that does not exist was added to the recent-games list, "
             "so the scope picker would offer a mapping for a ROM that is not "
             "there")
    print("  ok  console:n64, exit 0, recent games unchanged")


def check_flag_value_that_looks_like_a_path() -> None:
    """A flag's value must never be mistaken for the game."""
    print("\na flag whose value is an existing file path:")
    config = LAUNCH_CFG
    if not config.is_file():
        fail("the launch override this check feeds in does not exist")

    # No positional ROM at all: everything that looks like a path here belongs
    # to a flag. Taking one as the game would file a mapping under a key
    # derived from `launch.cfg`.
    result = run_launcher(["--appendconfig", str(config), "-L", str(N64_CORE)])
    text = PLAYER_ONE_CFG.read_text()
    if result.returncode != 0:
        fail("a launch with no positional ROM failed")
    if scope_of(text) != "console:n64":
        fail(f"resolved {scope_of(text)!r} from a command line with no game on "
             "it; --appendconfig's value was read as the ROM")
    if "launch" in protocol.read_last_game().get("key", ""):
        fail("the launch override was recorded as a game the user played, and "
             "would be offered in the scope picker")
    print("  ok  console:n64, and nothing was recorded as a game")

    # And with a real ROM beside it, the ROM still wins.
    result = run_launcher([
        "--appendconfig", str(config), "--nodevice", "2",
        "-L", str(N64_CORE), str(MAPPED_ROM)])
    text = PLAYER_ONE_CFG.read_text()
    if scope_of(text) != f"game:{MAPPED_KEY}":
        fail(f"resolved {scope_of(text)!r}; the real ROM was passed over in "
             "favour of a flag's value")
    print("  ok  with a real ROM beside it, the ROM still wins")


def check_lands_where_retroarch_looks() -> None:
    """The directory the profile is written to is the one RetroArch is told.

    Two files that have to agree: this is the launch override the front-end
    hands RetroArch via --appendconfig, and the profile the launcher wrote. A
    correct mapping in a directory RetroArch does not scan is exactly as
    useless as no mapping at all, and that has happened here before -- padmap
    wrote to ~/.config/retroarch/autoconfig while the setting pointed into the
    Nix store.
    """
    print("\nthe profile is where the launch override says to look:")
    override = LAUNCH_CFG.read_text()
    wanted = f'joypad_autoconfig_dir = "{retroarch.runtime_autoconfig_dir()}"'
    if wanted not in override:
        fail(f"the override does not point RetroArch at {wanted}; whatever the "
             "launcher resolved will never be read")
    if PLAYER_ONE_CFG.parent.parent != retroarch.runtime_autoconfig_dir():
        fail(f"the profile landed in {PLAYER_ONE_CFG.parent}, outside the "
             "directory RetroArch is pointed at")
    if PLAYER_ONE_CFG.parent.name != "udev":
        fail("the profile is not in the driver subdirectory; RetroArch looks "
             "in <dir>/<driver> first and only falls back to the base "
             "directory when it is empty")
    print(f"  ok  {retroarch.runtime_autoconfig_dir().relative_to(ROOT)}/udev, "
          "named by the override")


def check_resolution_agrees_with_the_file() -> None:
    """What the library says applies is what the file on disk binds.

    The daemon's scope picker and the front-end both ask `resolved_mapping`
    rather than reading the .cfg, so the two answers being the same is what
    stops a user being shown one mapping and given another.
    """
    print("\nthe library and the artefact agree:")
    for console, game, expected in [
        ("n64", MAPPED_KEY, f"game:{MAPPED_KEY}"),
        ("n64", "n64/goldeneye-007-u", "console:n64"),
        ("snes", "snes/super-metroid-ju", ""),
        ("", "", ""),
    ]:
        scope, resolved = controllercfg.resolved_mapping(PAD_ONE, console, game)
        if scope != expected:
            fail(f"resolved_mapping({console!r}, {game!r}) -> {scope!r}, "
                 f"expected {expected!r}")
        retroarch.install_profiles(
            ASSIGNMENTS, console=console, game=game, context="agreement check")
        written = scope_of(PLAYER_ONE_CFG.read_text())
        if written != (scope or "default (any game)"):
            fail(f"the library resolved {scope!r} and the file RetroArch reads "
                 f"says {written!r}; a user told one thing and given another")
        start = resolved.buttons["start"].retroarch()
        if f'input_start_btn = "{start}"' not in PLAYER_ONE_CFG.read_text():
            fail(f"the resolved capture binds start to {start} and the file "
                 "does not")
        print(f"  ok  console={console or '-'!r:<7} game={game or '-'!r:<26} "
              f"-> {scope or 'default'!r}")


def main() -> int:
    print(f"temp tree: {ROOT}")
    build_world()
    check_isolation()
    check_stored_profile()
    check_game_scope()
    check_console_scope()
    check_default_scope()
    check_unknown_core()
    check_idempotent()
    check_simulation_matches_the_daemon()
    check_republish_preserves_game_scope()
    check_no_assignments()
    check_missing_rom()
    check_flag_value_that_looks_like_a_path()
    check_lands_where_retroarch_looks()
    check_resolution_agrees_with_the_file()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    finally:
        if KEEP:
            print(f"kept: {ROOT}")
        else:
            shutil.rmtree(ROOT, ignore_errors=True)
