#!/usr/bin/env python3
"""What RetroArch is actually handed for one launch: profiles, override, flags.

Three artefacts, all generated fresh for every launch, and every one of them
fails silently when it is wrong -- a pad that reports itself as configured
while the buttons do nothing looks exactly like a pad that works until someone
presses a button.

  * `install_profiles` -- one autoconfig .cfg per managed player, holding the
    mapping resolved from the scopes of the controller behind that player.
  * `launch_config` -- the --appendconfig override: all sixteen player slots,
    every managed bind cleared to `nul`, RetroArch pointed at padmap's own
    autoconfig directory, save-on-exit off.
  * `launch_args` -- `--nodevice` for the core ports nobody was assigned to.

The scenario this file exists for is the one where two writers targeted the
same file. `padmap.launch` writes the profile resolved from the core and ROM
of the launch in progress; `Server._start_republisher` used to write a
context-free one, which resolves to the controller's *default* mapping.
Whichever ran last won -- and republishing restarts for reasons that have
nothing to do with the game (a session accepted, a pad reconnecting, the
daemon upgraded), so a game-specific mapping could be live for one launch and
silently gone for the next. So the check below runs the real launcher, then
the daemon's regeneration, and demands the bytes be identical.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_launch_profiles.py
"""

from __future__ import annotations

import ast
import atexit
import inspect
import io
import os
import shutil
import sys
import tempfile
import textwrap
from contextlib import redirect_stderr
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

# Every path padmap writes to is redirected before the package is imported.
# `retroarch.CONFIG_DIR` in particular is read at import time, and the runtime
# directory is where a LIVE daemon on this machine keeps the very autoconfig
# profiles this file rewrites -- clobbering those would change the mapping of
# a controller in use.
_SANDBOX = Path(tempfile.mkdtemp(prefix="padmap-check-launch-profiles-"))
os.environ["XDG_RUNTIME_DIR"] = str(_SANDBOX / "run")
os.environ["XDG_CONFIG_HOME"] = str(_SANDBOX / "config")
os.environ["XDG_DATA_HOME"] = str(_SANDBOX / "data")
os.environ["RETROARCH_CONFIG_DIR"] = str(_SANDBOX / "retroarch")
os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "devices")
# The virtual pads' advertised identity, pinned so nothing here has to open a
# device to find it out. `mirror` (the default) reads the ids off the physical
# controller, and the pads below are fictional -- it would fall back to
# exactly these ids anyway, after a warning about a node that does not exist.
os.environ["PADMAP_PAD_IDENTITY"] = "padmap"
for _name in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    Path(os.environ[_name]).mkdir(parents=True, exist_ok=True)
# Everything this file writes lives under _SANDBOX, so one removal is the
# whole cleanup. Registered rather than done at the end of main() so a failing
# check leaves nothing behind either.
atexit.register(shutil.rmtree, _SANDBOX, ignore_errors=True)

from padmap import (launch, profiles, protocol,  # noqa: E402
                    retroarch, server, virtual)
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

# A database of one entry, so `find_profile` never reads the machine's real
# autoconfig directories: what libretro happens to ship must not decide
# whether these checks pass.
FAKE_DB = _SANDBOX / "autoconfig-db"
FAKE_DB.mkdir(parents=True, exist_ok=True)
retroarch.autoconfig_dirs = lambda: [FAKE_DB]  # type: ignore[assignment]


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


# -- fixtures ----------------------------------------------------------------

def pad(name: str, vid: int, pid: int, node: int) -> Pad:
    """A fictional controller. Node numbers are far above anything real."""
    return Pad(path=f"/dev/input/event{node}", name=name, phys=f"check/{node}",
               uniq="", vid=vid, pid=pid, syspath="", retroarch_visible=True)


GC = pad("padmap-check GameCube Adapter", 0x057E, 0x0337, 901)
PLAIN = pad("padmap-check Nameless Pad", 0x0079, 0x1879, 902)
CAL = pad("padmap-check Calibrated Pad", 0x1111, 0x2222, 903)
UNCAL = pad("padmap-check Uncalibrated Pad", 0x3333, 0x4444, 904)

# One physical GameCube pad with three captures, the case that forced scopes
# to exist: "the mapping my gamecube controller uses for n64 games, and then a
# universal configuration in general".
#
# They are told apart by RetroArch keys only one of them can produce, never by
# "something was written". The GameCube layout binds its A to input_a_btn; the
# N64 layout has no A override at all, so its 'a' goes to the canonical
# input_b_btn and input_a_btn cannot appear -- mupen64plus-next never reads
# RetroPad A. Conversely the N64 layout puts *b* on input_y_btn.
GC_DEFAULT = profiles.Mapping(
    layout="gamecube",
    buttons={
        "a": Binding("button", 0), "b": Binding("button", 1),
        "x": Binding("button", 2), "y": Binding("button", 3),
        "start": Binding("button", 7),
    },
)
GC_FOR_N64 = profiles.Mapping(
    layout="n64",
    buttons={
        "a": Binding("button", 0), "b": Binding("button", 1),
        "start": Binding("button", 7),
        "lefttrigger": Binding("axis", 4, 1),
        "rightstick_up": Binding("button", 3),
    },
)
GC_FOR_THIS_GAME = profiles.Mapping(
    layout="n64",
    buttons={"a": Binding("button", 5), "start": Binding("button", 7)},
)

CORE = "/nix/store/zzz/lib/retroarch/cores/mupen64plus_next_libretro.so"
ROM_DIR = _SANDBOX / "roms" / "n64"
ROM_DIR.mkdir(parents=True, exist_ok=True)
ROM = ROM_DIR / "Super Smash Bros (U).z64"
ROM.write_text("not really a rom")
GAME_KEY = profiles.game_key("n64", str(ROM))
GAME_TITLE = launch.title_for(str(ROM))


def store(pad_: Pad, mappings: dict[str, profiles.Mapping] | None = None,
          axes: dict[int, profiles.AxisCalibration] | None = None) -> None:
    """Put a profile on disk for a pad, in the temp store."""
    profile = profiles.Profile(signature=profiles.signature(pad_),
                               name=pad_.name, icon="gamecube")
    profile.axes = dict(axes or {})
    profile.mappings = dict(mappings or {})
    profiles.save(profile)


def forget(pad_: Pad) -> None:
    profiles.forget(pad_)


def calibration() -> profiles.AxisCalibration:
    return profiles.AxisCalibration(center=128, minimum=0, maximum=255,
                                    flat=4, reach_min=10, reach_max=250)


def assign(pad_: Pad, player: int) -> Assignment:
    return Assignment(player=player, pad=pad_, button=0)


def virtual_path(player: int) -> str:
    """The node the virtual pad for a player is pretending to live on."""
    return f"/dev/input/event{950 + player}"


def set_order(paths: list[str]) -> dict[int, str]:
    """Pin what RetroArch's udev driver would enumerate, in order.

    The real one reads /sys, which on this machine is a live arcade box with
    controllers plugged in; the pad indices in the override must be decided by
    the fixture, not by what happens to be attached.
    """
    order = {index: path for index, path in enumerate(paths)}
    retroarch.visible_order = lambda: order  # type: ignore[assignment]
    return order


def settings(text: str) -> dict[str, str]:
    """key -> value for every `key = "value"` line, rejecting duplicates."""
    out: dict[str, str] = {}
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or " = " not in stripped:
            continue
        key, _, value = stripped.partition(" = ")
        key = key.strip()
        if key in out:
            fail(f"{key} is written twice in one config; RetroArch takes the "
                 f"last one, so which binding wins is not decided here")
        out[key] = value.strip().strip('"')
    return out


def profile_dir() -> Path:
    """Where install_profiles puts things when nobody names a directory."""
    return retroarch.runtime_autoconfig_dir() / "udev"


def profile_text(player: int, directory: Path | None = None) -> str:
    path = (directory or profile_dir()) / f"{virtual.virtual_name(player)}.cfg"
    if not path.exists():
        fail(f"no autoconfig profile for player {player}; RetroArch would fall "
             f"back to libretro's database or to nothing at all")
    return path.read_text()


# -- install_profiles: the files themselves ----------------------------------

def check_files_written() -> None:
    print("\none profile per managed player, and nothing else:")
    store(GC, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})
    store(PLAIN, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})
    dest = Path(tempfile.mkdtemp(dir=_SANDBOX))

    # Left behind by a previous session with more players, plus a file that is
    # not ours at all.
    (dest / f"{virtual.virtual_name(9)}.cfg").write_text("stale\n")
    keep = dest / "some-real-controller.cfg"
    keep.write_text('input_device = "Someone Else"\n')

    written = retroarch.install_profiles(
        [assign(GC, 1), assign(PLAIN, 2)], dest=dest)
    names = sorted(p.name for p in dest.glob("*.cfg"))
    if names != ["padmap Player 1.cfg", "padmap Player 2.cfg",
                 "some-real-controller.cfg"]:
        fail(f"the autoconfig directory holds {names}; a profile for a player "
             f"who no longer exists is still scanned by RetroArch and can "
             f"match a pad it was never meant for")
    if [p.name for p in written] != ["padmap Player 1.cfg",
                                     "padmap Player 2.cfg"]:
        fail(f"install_profiles reported {[p.name for p in written]}")
    if keep.read_text() != 'input_device = "Someone Else"\n':
        fail("a profile padmap did not write was deleted; the user's own "
             "autoconfig entries have to survive a launch")
    print("  ok  players 1-2 written, player 9's stale profile removed, "
          "a foreign profile untouched")

    print("\nand writing them twice does not change a byte:")
    first = profile_text(1, dest)
    retroarch.install_profiles([assign(GC, 1), assign(PLAIN, 2)], dest=dest)
    if profile_text(1, dest) != first:
        fail("two identical writes produced different profiles, so what "
             "RetroArch reads depends on how many times the daemon happened "
             "to republish")
    print("  ok  idempotent")

    print("\nand a session with no players leaves nothing behind:")
    retroarch.install_profiles([], dest=dest)
    left = sorted(p.name for p in dest.glob("*.cfg"))
    if left != ["some-real-controller.cfg"]:
        fail(f"{left} survived an empty assignment list; those profiles name "
             f"pads that are no longer republished")
    print("  ok  padmap's profiles cleared, the foreign one kept")


def check_default_destination() -> None:
    """The directory the override names has to be the one profiles land in."""
    print("\nwith no directory named, profiles land where the override "
          "points RetroArch:")
    store(GC, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})
    order = set_order([virtual_path(1)])
    written = retroarch.install_profiles([assign(GC, 1)])
    cfg = settings(retroarch.launch_config(
        [assign(GC, 1)], {1: virtual_path(1)}))

    named = Path(cfg.get("joypad_autoconfig_dir", ""))
    if str(named) != str(retroarch.runtime_autoconfig_dir()):
        fail(f"the override points RetroArch at {named}, not at "
             f"{retroarch.runtime_autoconfig_dir()}; RetroArch scans exactly "
             f"one autoconfig directory and would read the wrong one")
    if written[0].parent != named / "udev":
        fail(f"profiles were written to {written[0].parent}, but RetroArch "
             f"looks in <dir>/<driver> first -- i.e. {named / 'udev'}")
    if str(named).startswith("/run/user") and "padmap-check" not in str(named):
        fail("this check is writing into the real runtime directory, where a "
             "live daemon keeps the profiles a running game is using")
    if len(order) != 1:
        fail("fixture order was not applied")
    print(f"  ok  {written[0].parent} under joypad_autoconfig_dir")


# -- install_profiles: which mapping goes in --------------------------------

def check_scope_header() -> None:
    """The profile says which mapping it came from and what for."""
    print("\nevery profile records the scope it was resolved from:")
    store(GC, {
        profiles.SCOPE_UNIVERSAL: GC_DEFAULT,
        profiles.console_scope("n64"): GC_FOR_N64,
        profiles.game_scope(GAME_KEY): GC_FOR_THIS_GAME,
    })
    dest = Path(tempfile.mkdtemp(dir=_SANDBOX))

    def emit(console: str, game: str, context: str) -> str:
        retroarch.install_profiles([assign(GC, 1)], dest=dest,
                                   console=console, game=game, context=context)
        return profile_text(1, dest)

    cases = [
        (("", "", ""), "# Mapping scope: default (any game)."),
        (("n64", "", "GoldenEye 007"),
         "# Mapping scope: console:n64; resolved for GoldenEye 007."),
        (("n64", GAME_KEY, GAME_TITLE),
         f"# Mapping scope: game:{GAME_KEY}; resolved for {GAME_TITLE}."),
    ]
    for (console, game, context), expected in cases:
        text = emit(console, game, context)
        if expected not in text:
            got = [l for l in text.splitlines() if "Mapping scope" in l]
            fail(f"a profile resolved for console={console!r} game={game!r} "
                 f"says {got}, not {expected!r}. That header is the only way "
                 f"to answer 'why is player 1 bound like this' from the file "
                 f"RetroArch actually read")
        print(f"  ok  console={console or '-'!r:<7} game={game or '-'!r:<26} "
              f"-> {expected.split(': ', 1)[1]}")


def check_scope_resolution() -> None:
    """The bindings themselves change with the console and the game."""
    print("\nand carries that scope's bindings, not another scope's:")
    store(GC, {
        profiles.SCOPE_UNIVERSAL: GC_DEFAULT,
        profiles.console_scope("n64"): GC_FOR_N64,
        profiles.game_scope(GAME_KEY): GC_FOR_THIS_GAME,
    })
    dest = Path(tempfile.mkdtemp(dir=_SANDBOX))

    def emit(console: str, game: str) -> dict[str, str]:
        retroarch.install_profiles([assign(GC, 1)], dest=dest,
                                   console=console, game=game)
        return settings(profile_text(1, dest))

    default = emit("", "")
    if default.get("input_a_btn") != "0" or default.get("input_x_btn") != "2":
        fail(f"a launch with no context did not get the controller's default "
             f"mapping (input_a_btn={default.get('input_a_btn')!r}, "
             f"input_x_btn={default.get('input_x_btn')!r}); every console "
             f"other than the ones mapped by hand depends on it")
    print("  ok  no context      -> the GameCube default (input_a_btn = 0)")

    n64 = emit("n64", "n64/some-other-game")
    if "input_a_btn" in n64:
        fail("an N64 launch was given the GameCube capture: nothing on that "
             "console reads RetroPad A, so input_a_btn is a button that does "
             "nothing at all")
    if n64.get("input_y_btn") != "1":
        fail(f"the N64 mapping's B (which mupen64plus-next reads from RetroPad "
             f"Y) is {n64.get('input_y_btn')!r}, so B is unbound in every N64 "
             f"game")
    if n64.get("input_l2_axis") != "+4":
        fail(f"the N64 capture's Z trigger is an axis and came out as "
             f"{n64.get('input_l2_axis')!r}; under a _btn key RetroArch's "
             f"strtoull reads '+4' as button 4 and both directions collapse")
    print("  ok  console n64     -> the N64 capture (input_y_btn = 1, Z on an "
          "axis)")

    game = emit("n64", GAME_KEY)
    if "input_y_btn" in game:
        fail("the per-game capture lost to the console one, so 'map this "
             "differently for this game' did nothing")
    if game.get("input_b_btn") != "5":
        fail(f"the per-game capture's own A binding is {game.get('input_b_btn')!r}")
    print("  ok  that one game   -> the per-game capture (input_b_btn = 5)")

    snes = emit("snes", "snes/super-metroid")
    if snes.get("input_a_btn") != "0":
        fail("a console with no mapping of its own did not fall back to the "
             "controller's default, leaving the pad on nothing")
    print("  ok  console snes    -> back to the default")


def check_unmapped_pad() -> None:
    """A pad nobody has mapped still gets a profile, under padmap's identity."""
    print("\na controller that has never been through the wizard:")
    forget(PLAIN)
    source = FAKE_DB / "Nameless.cfg"
    source.write_text(
        'input_driver = "udev"\n'
        f'input_device = "{PLAIN.name}"\n'
        'input_device_display_name = "Nameless Pad"\n'
        f'input_vendor_id = "{PLAIN.vid}"\n'
        f'input_product_id = "{PLAIN.pid}"\n'
        'input_phys = "usb-0000:00:14.0-3/input0"\n'
        'input_b_btn = "1"\n'
        'input_start_btn = "9"\n'
    )
    dest = Path(tempfile.mkdtemp(dir=_SANDBOX))
    retroarch.install_profiles([assign(PLAIN, 2)], dest=dest)
    text = profile_text(2, dest)
    if "Nameless Pad" in text:
        fail("the source's own display name is still in the copy, so "
             '"padmap Player 2" announces itself as the pad it wraps -- '
             "RetroArch shows that name in its input menus and compares it "
             "against the reservation, which names the virtual pad")
    values = settings(text)
    source.unlink()

    if values.get("input_b_btn") != "1" or values.get("input_start_btn") != "9":
        fail("the source profile's button mapping was not carried over, so a "
             "pad libretro already knows about arrives at RetroArch unmapped")
    if values.get("input_device") != "padmap Player 2" or \
            values.get("input_device_display_name") != "padmap Player 2":
        fail(f"the profile identifies itself as "
             f"{values.get('input_device_display_name')!r}: RetroArch matches "
             f"autoconfig by device name, so a profile named after the "
             f"physical pad is never applied to the virtual one")
    if "input_phys" in values:
        fail("the source's input_phys was copied; it is scored +/-10 during "
             "profile matching and can only ever mismatch our synthetic phys")
    identity = virtual.identity_for(PLAIN)
    if values.get("input_vendor_id") != str(identity.vendor) or \
            values.get("input_product_id") != str(identity.product):
        fail(f"the profile claims {values.get('input_vendor_id')}:"
             f"{values.get('input_product_id')} while the virtual pad "
             f"advertises {identity.vendor}:{identity.product}; a disagreeing "
             f"vid/pid scores the profile down in RetroArch's matching")
    print(f"  ok  mapping copied, identity rewritten to "
          f"{values['input_device']} ({identity.vendor}:{identity.product})")


# -- the two writers of one file --------------------------------------------

def check_launcher_and_daemon_agree() -> None:
    """The bug: republishing overwrote the launcher's game-specific profile.

    `padmap.launch` resolves the scope from the core and the ROM of the launch
    in progress. `Server._start_republisher` wrote a context-free profile --
    the controller's default mapping -- into the same file, and republishing
    restarts for reasons that have nothing to do with the game. Whichever ran
    last won, so a game-specific mapping was live for one launch and silently
    gone for the next.
    """
    print("\nthe launcher writes a profile for the game that is starting:")
    store(GC, {
        profiles.SCOPE_UNIVERSAL: GC_DEFAULT,
        profiles.console_scope("n64"): GC_FOR_N64,
        profiles.game_scope(GAME_KEY): GC_FOR_THIS_GAME,
    })
    assignments = [assign(GC, 1)]
    # The launcher reads assignments.json and re-enumerates the real devices
    # to match it. Stood in for here: this machine's controllers are not the
    # fixture, and discovery must not reach them.
    original = launch.load_assignments
    launch.load_assignments = lambda state=None: assignments  # type: ignore
    try:
        argv = ["--appendconfig", "/nonexistent/launch.cfg",
                "--nodevice", "2", "-L", CORE, str(ROM)]
        noise = io.StringIO()
        with redirect_stderr(noise):
            code = launch.resolve(argv)
    finally:
        launch.load_assignments = original  # type: ignore[assignment]

    if code != 0:
        fail(f"padmap.launch returned {code}; a launch must never fail over a "
             f"mapping it could not narrow")
    from_launcher = profile_text(1)
    if f"# Mapping scope: game:{GAME_KEY}" not in from_launcher:
        fail(f"the launcher did not resolve the per-game scope for {ROM.name} "
             f"(core {Path(CORE).name}); its header says "
             f"{[l for l in from_launcher.splitlines() if 'scope' in l]}")
    if 'input_b_btn = "5"' not in from_launcher:
        fail("the per-game bindings never reached the file RetroArch reads")
    print(f"  ok  game:{GAME_KEY}, resolved for {GAME_TITLE}")

    print("\nand a republish for some unrelated reason writes the same one:")
    # Exactly what Server._start_republisher does after a session is accepted,
    # a pad reconnects, or the daemon is restarted to pick up new code.
    last = protocol.read_last_game()
    if last.get("key") != GAME_KEY:
        fail(f"the daemon has no record of the launch ({last!r}), so it has "
             f"no context to regenerate with and would fall back to the "
             f"default mapping")
    retroarch.install_profiles(
        assignments, console=last.get("console", ""), game=last.get("key", ""),
        context=last.get("title", ""))
    from_daemon = profile_text(1)
    if from_daemon != from_launcher:
        fail("republishing rewrote the profile RetroArch is using into "
             "something else. The game keeps running and the buttons change "
             "under the player's hands -- this is the mapping that was live "
             "one launch and gone the next")
    print("  ok  byte-identical to the launcher's profile")

    print("\nwhereas the context-free write is a different profile entirely:")
    retroarch.install_profiles(assignments)
    context_free = profile_text(1)
    if context_free == from_launcher:
        fail("a write with no console and no game produced the game-specific "
             "profile, so this check cannot tell the two writers apart and "
             "proves nothing about either")
    if 'input_a_btn = "0"' not in context_free:
        fail("the no-context write did not fall back to the controller's "
             "default mapping")
    print("  ok  falls back to the default (input_a_btn = 0), which is what "
          "used to clobber the launch")

    print("\nand with nothing ever launched, the daemon writes that default:")
    fresh = _SANDBOX / "never-launched.json"
    original_path = protocol.last_game_path
    protocol.last_game_path = lambda: fresh  # type: ignore[assignment]
    try:
        empty = protocol.read_last_game()
        retroarch.install_profiles(
            assignments, console=empty.get("console", ""),
            game=empty.get("key", ""), context=empty.get("title", ""))
    finally:
        protocol.last_game_path = original_path  # type: ignore[assignment]
    if profile_text(1) != context_free:
        fail("with no launch on record the daemon must write exactly the "
             "context-free profile it always wrote; anything else is a "
             "mapping nobody asked for")
    print("  ok  same as the context-free write")


def check_daemon_still_passes_context() -> None:
    """The daemon's own call site, read rather than run.

    `_start_republisher` creates real uinput devices, so it cannot be called
    here -- but the whole fix is one argument at one call site, and dropping it
    restores the bug the check above describes with nothing to notice.
    """
    print("\nServer._start_republisher's call to install_profiles:")
    source = inspect.getsource(server.Server._start_republisher)
    tree = ast.parse(textwrap.dedent(source))
    calls = [
        node for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and getattr(node.func, "attr", "") == "install_profiles"
    ]
    if len(calls) != 1:
        fail(f"expected exactly one install_profiles call in "
             f"_start_republisher, found {len(calls)}")
    keywords = {kw.arg: kw.value for kw in calls[0].keywords}
    for name in ("console", "game"):
        if name not in keywords:
            fail(f"the daemon calls install_profiles without {name}=, so every "
                 f"republish overwrites the launcher's game-specific profile "
                 f"with the controller's default mapping")
        value = keywords[name]
        if isinstance(value, ast.Constant) and not value.value:
            fail(f"the daemon passes {name}={value.value!r}, which resolves to "
                 f"the default mapping and contradicts the launcher on every "
                 f"republish")
    if "read_last_game" not in source:
        fail("the daemon no longer takes its context from the last launched "
             "game, which is the only context that side knows")
    print("  ok  console= and game= come from protocol.read_last_game()")


# -- launch_config -----------------------------------------------------------

def check_all_sixteen_slots() -> None:
    print("\nlaunch.cfg speaks for all sixteen player slots:")
    store(GC, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})
    paths = {1: virtual_path(1), 3: virtual_path(3)}
    set_order([virtual_path(1), virtual_path(3)])
    assignments = [assign(GC, 1), assign(GC, 3)]
    cfg = settings(retroarch.launch_config(assignments, paths))

    for key in ("joypad_index", "reserved_device", "device_reservation_type"):
        missing = [p for p in range(1, retroarch.MAX_PLAYERS + 1)
                   if f"input_player{p}_{key}" not in cfg]
        if missing:
            fail(f"players {missing} have no input_playerN_{key}. A slot left "
                 f"unwritten keeps whatever retroarch.cfg holds -- RetroArch's "
                 f"own default is joypad_index = N-1, which is how one "
                 f"controller ended up driving players 1 and 3")
    print(f"  ok  {3 * retroarch.MAX_PLAYERS} lines: index, reservation and "
          f"reservation type for every slot")

    reserved = {p: cfg[f"input_player{p}_reserved_device"]
                for p in range(1, retroarch.MAX_PLAYERS + 1)}
    kinds = {p: cfg[f"input_player{p}_device_reservation_type"]
             for p in range(1, retroarch.MAX_PLAYERS + 1)}
    for player in (1, 3):
        if reserved[player] != virtual.virtual_name(player):
            fail(f"player {player} is reserved for {reserved[player]!r} "
                 f"rather than its own virtual pad")
        if kinds[player] != str(retroarch.RESERVATION_RESERVED):
            fail(f"player {player}'s reservation type is {kinds[player]!r}; "
                 f"only RESERVED holds the slot for that pad")
    for player in (2, 4, 16):
        if reserved[player] or kinds[player] != str(retroarch.RESERVATION_NONE):
            fail(f"player {player} keeps a reservation "
                 f"({reserved[player]!r}, type {kinds[player]!r}); a "
                 f"reservation naming a pad that is no longer republished "
                 f"holds the slot open for nothing")
    print("  ok  players 1 and 3 reserved by name, every other slot cleared")

    indices = {p: int(cfg[f"input_player{p}_joypad_index"])
               for p in range(1, retroarch.MAX_PLAYERS + 1)}
    if indices[1] != 0 or indices[3] != 1:
        fail(f"the assigned players got pad indices {indices[1]} and "
             f"{indices[3]}, not the enumeration order 0 and 1")
    live = [indices[p] for p in indices if indices[p] < 2]
    if sorted(live) != [0, 1]:
        fail(f"a live pad index is shared by more than the assigned players "
             f"({live}); the same controller then drives two slots")
    print("  ok  assigned slots get the enumerated indices, the rest vacant ones")

    if cfg.get("config_save_on_exit") != "false":
        fail("config_save_on_exit is not disabled, so RetroArch writes this "
             "whole per-launch override back over the user's retroarch.cfg on "
             "quit and the values outlive the assignments that made them")
    print("  ok  config_save_on_exit disabled, so the override stays per-launch")

    if any(key.startswith("input_libretro_device") for key in cfg):
        fail("input_libretro_device_pN is in the override; RetroArch ignores "
             "it outside .rmp remap files, so a core port emptied this way is "
             "not emptied at all")
    print("  ok  no input_libretro_device_pN (it is ignored from a config)")


def check_binds_are_nul() -> None:
    """Every per-player bind of a managed slot, cleared.

    input_driver.c: `joykey = (bind_joykey != NO_BTN) ? bind_joykey :
    autobind_joykey`. A value left in retroarch.cfg therefore beats the
    autoconfig profile padmap just wrote, and the pad reports itself as
    configured while the buttons do nothing.
    """
    print("\nevery bind of a managed player is handed back to autoconfig:")
    store(GC, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})
    paths = {1: virtual_path(1)}
    set_order([virtual_path(1)])
    cfg = settings(retroarch.launch_config([assign(GC, 1)], paths))

    # Spelled out rather than read from retroarch.PLAYER_BINDS: the list being
    # complete is the thing under test. Face buttons, d-pad, shoulders,
    # triggers, stick clicks, and both sticks -- a stale
    # input_player1_l_x_plus_axis silently outranks the profile's stick just
    # as a stale start_btn outranks its Start.
    required = (
        "b", "y", "select", "start", "up", "down", "left", "right",
        "a", "x", "l", "r", "l2", "r2", "l3", "r3",
        "l_x_plus", "l_x_minus", "l_y_plus", "l_y_minus",
        "r_x_plus", "r_x_minus", "r_y_plus", "r_y_minus",
    )
    for bind in required:
        for form in ("btn", "axis"):
            key = f"input_player1_{bind}_{form}"
            if cfg.get(key) != "nul":
                fail(f"{key} is {cfg.get(key)!r}, not 'nul'. RetroArch only "
                     f"falls back to the autoconfig profile when the explicit "
                     f"bind is nul, so this control keeps whatever "
                     f"retroarch.cfg says and the captured mapping is ignored")
    print(f"  ok  {2 * len(required)} binds cleared for player 1, sticks "
          f"included")

    stray = [key for key in cfg
             if key.startswith("input_player2_") and key.endswith(("_btn",
                                                                   "_axis"))]
    if stray:
        fail(f"unmanaged player 2 has binds written ({stray[:3]}...); that "
             f"slot has no pad, so padmap has no business rewriting the "
             f"user's binds for it")
    print("  ok  unmanaged slots' binds left alone")


def check_analog_gain() -> None:
    """input_analog_sensitivity, written only once padmap sets the range.

    A GameCube stick under-reaches the range its adapter declares, so the stick
    feels weak and the usual workaround is to wind the gain up -- libretro's
    own GameCube profile ships a commented-out 1.4. It costs the top of the
    travel: at 1.6 the stick saturates around 62% deflection. Calibration fixes
    the cause instead, so gain on top of a calibrated pad double-compensates --
    but the setting is global, so one uncalibrated pad means the boost is still
    doing useful work and taking it away makes that stick worse.
    """
    print("\nanalog gain is reset only when every managed pad is calibrated:")
    store(CAL, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT},
          axes={0: calibration(), 1: calibration()})
    store(UNCAL, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})
    both = [virtual_path(1), virtual_path(2)]
    paths = {1: virtual_path(1), 2: virtual_path(2)}

    set_order(both)
    cfg = settings(retroarch.launch_config(
        [assign(CAL, 1), assign(CAL, 2)], paths))
    if cfg.get("input_analog_sensitivity") != "1.000000":
        fail(f"two calibrated pads still carry a gain "
             f"({cfg.get('input_analog_sensitivity')!r}); padmap already "
             f"rescales the measured reach onto the full declared range, so "
             f"anything above 1.0 saturates the stick early")
    print("  ok  both calibrated              -> 1.000000")

    cfg = settings(retroarch.launch_config(
        [assign(CAL, 1), assign(UNCAL, 2)], paths))
    if "input_analog_sensitivity" in cfg:
        fail("the gain was reset while a managed pad had no calibration. "
             "Sensitivity is global, so that pad loses the compensation it "
             "still needs and its stick gets weaker")
    print("  ok  one uncalibrated             -> left at the user's value")

    forget(UNCAL)
    cfg = settings(retroarch.launch_config(
        [assign(CAL, 1), assign(UNCAL, 2)], paths))
    if "input_analog_sensitivity" in cfg:
        fail("a pad with no profile at all was treated as calibrated")
    print("  ok  one pad with no profile      -> left at the user's value")
    store(UNCAL, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})

    # An assignment whose virtual pad is not enumerated is not managed:
    # RetroArch cannot see that pad, so its calibration cannot be an argument
    # about the sticks of the pads it can see.
    set_order([virtual_path(1)])
    cfg = settings(retroarch.launch_config(
        [assign(CAL, 1), assign(UNCAL, 2)], paths))
    if cfg.get("input_analog_sensitivity") != "1.000000":
        fail("an assignment RetroArch cannot see decided the gain for the pad "
             "it can; the calibrated pad that is actually playing keeps a "
             "boost it does not need")
    print("  ok  uncalibrated pad not enumerated -> the calibrated one decides")

    set_order([])
    cfg = settings(retroarch.launch_config([assign(CAL, 1)], paths))
    if "input_analog_sensitivity" in cfg:
        fail("a launch with no managed pads at all overrode the user's gain; "
             "padmap is setting nobody's range here, so it has not earned the "
             "right to touch it")
    print("  ok  no managed pads              -> left at the user's value")


# -- launch_args -------------------------------------------------------------

def check_launch_args() -> None:
    print("\n--nodevice for exactly the unassigned core ports:")
    store(GC, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})

    def ports(assignments: list[Assignment], paths: dict[int, str]) -> list[int]:
        flags = retroarch.launch_args(assignments, paths)
        if len(flags) % 2:
            fail(f"launch_args emitted an odd number of tokens ({flags}); "
                 f"RetroArch would read the next flag as a port number")
        if any(flags[i] != "--nodevice" for i in range(0, len(flags), 2)):
            fail(f"launch_args emitted something other than --nodevice: "
                 f"{flags}. It is the only spelling RetroArch honours -- "
                 f"input_libretro_device_pN in a config file does nothing")
        return [int(flags[i + 1]) for i in range(0, len(flags), 2)]

    paths = {1: virtual_path(1)}
    set_order([virtual_path(1)])
    got = ports([assign(GC, 1)], paths)
    if got != list(range(2, 17)):
        fail(f"one assigned player left ports {got} emptied, not 2..16. A "
             f"core that declares four ports hands the other three a "
             f"controller nobody assigned")
    print("  ok  1 player  -> ports 2..16 emptied")

    paths = {1: virtual_path(1), 3: virtual_path(3)}
    set_order([virtual_path(1), virtual_path(3)])
    got = ports([assign(GC, 1), assign(GC, 3)], paths)
    if got != [2] + list(range(4, 17)):
        fail(f"players 1 and 3 assigned left ports {got} emptied; the flags "
             f"have to follow which slots are taken, not how many")
    print("  ok  players 1 and 3 -> port 2 emptied, ports 1 and 3 kept")

    # An assignment whose virtual pad is missing from the enumeration cannot be
    # bound at all, so its port must be emptied like any other.
    set_order([])
    got = ports([assign(GC, 1)], {1: virtual_path(1)})
    if got != list(range(1, 17)):
        fail(f"a player whose virtual pad is not enumerated kept its core "
             f"port ({got}); the slot then falls back to whatever "
             f"retroarch.cfg holds")
    print("  ok  pad not enumerated -> that port emptied too")

    print("\nand the flags agree with the override about who is managed:")
    paths = {1: virtual_path(1), 2: virtual_path(2)}
    set_order([virtual_path(1), virtual_path(2)])
    assignments = [assign(GC, 1), assign(GC, 2)]
    cfg = settings(retroarch.launch_config(assignments, paths))
    emptied = set(ports(assignments, paths))
    reserved = {p for p in range(1, retroarch.MAX_PLAYERS + 1)
                if cfg[f"input_player{p}_reserved_device"]}
    if emptied & reserved:
        fail(f"players {sorted(emptied & reserved)} have a reserved pad and an "
             f"emptied core port; the pad is bound to a player the core will "
             f"never ask about")
    if emptied | reserved != set(range(1, retroarch.MAX_PLAYERS + 1)):
        fail("some slot is neither reserved nor emptied, so it keeps whatever "
             "retroarch.cfg happens to hold")
    print(f"  ok  reserved {sorted(reserved)}, emptied {len(emptied)} ports, "
          f"no overlap and no gaps")

    print("\nand the wrapper reads them back one token per line:")
    target = _SANDBOX / "launch.args"
    retroarch.write_launch_args(assignments, paths, target)
    tokens = target.read_text().split("\n")
    if tokens[-1] != "":
        fail("the args file does not end in a newline, so the wrapper's last "
             "token is joined to whatever it reads next")
    if tokens[:-1] != retroarch.launch_args(assignments, paths):
        fail(f"the file holds {tokens[:-1]}, not the flags launch_args "
             f"produced; padmap-play passes on what is in the file")
    print(f"  ok  {len(tokens) - 1} tokens on {len(tokens) - 1} lines")


def main() -> int:
    check_files_written()
    check_default_destination()
    check_scope_header()
    check_scope_resolution()
    check_unmapped_pad()
    check_launcher_and_daemon_agree()
    check_daemon_still_passes_context()
    check_all_sixteen_slots()
    check_binds_are_nul()
    check_analog_gain()
    check_launch_args()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
