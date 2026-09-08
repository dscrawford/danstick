#!/usr/bin/env python3
"""Scoped mappings: storage, migration, resolution, and what reaches RetroArch.

The reported need, in the user's words: "the ability to specify the mapping
that my gamecube controller uses for n64 games, and then a universal
configuration in general". So the case driving every check below is one
physical GameCube pad with two captures -- its default, and one for N64 games
-- and the question is which set of bindings ends up in the .cfg RetroArch
reads, for a launch that is an N64 game and for one that is not.

The failure this guards against is silent in both directions. A resolution
that always picks the default looks exactly like one that works, until the
buttons are wrong in one console; and a resolution that always picks the
console mapping looks fine until the pad is used somewhere else. Neither
reports anything.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import controllercfg, launch, layouts, profiles, retroarch  # noqa: E402
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

PAD = Pad(path="/dev/input/event9", name="Mayflash GameCube Adapter",
          phys="usb-0000:00:14.0-1/input0", uniq="", vid=0x057E, pid=0x0337,
          syspath="")

# Two captures of the *same* physical pad. They differ in which control each
# physical button answers, because the layouts ask different questions -- and
# that is the whole point: RetroArch's key for "the button I press to jump"
# is not the same key on both consoles.
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
GC_FOR_MARIO = profiles.Mapping(
    layout="n64",
    buttons={"a": Binding("button", 5), "start": Binding("button", 7)},
)

MARIO = "/roms/n64/Super Mario 64 (USA).z64"
MARIO_KEY = profiles.game_key("n64", MARIO)


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def check_migration() -> None:
    """A profile written before scopes must come back as the default one.

    Not a nicety. Every profile on the machine right now is that shape, and a
    migration that dropped them would present every already-configured
    controller as new -- re-offering the wizard, and in the meantime driving
    the pad from a guess.
    """
    print("\na profile written before scopes existed:")
    legacy = {
        "signature": "057e:0337:Mayflash GameCube Adapter",
        "name": "Mayflash GameCube Adapter",
        "icon": "gamecube",
        "layout": "gamecube",
        "buttons": {"a": {"kind": "button", "index": 0, "value": 0,
                          "ra_index": None}},
        "axes": {"0": {"center": 128, "min": 0, "max": 255, "flat": 4,
                       "reach_min": 10, "reach_max": 250}},
    }
    profile = profiles.Profile.from_json(legacy)
    if profile.layout != "gamecube" or "a" not in profile.buttons:
        fail(f"the old flat capture was lost: {profile.mappings}")
    if profiles.SCOPE_UNIVERSAL not in profile.mappings:
        fail("the old capture did not become the default scope")
    if not profile.has_bindings():
        fail("a migrated profile reports itself unconfigured, so the wizard "
             "would be offered again for every controller on the machine")
    if 0 not in profile.axes or profile.axes[0].reach_max != 250:
        fail("calibration was lost in migration")
    print(f"  ok  -> scope {profiles.SCOPE_UNIVERSAL!r}, layout "
          f"{profile.layout!r}, calibration intact")

    print("\nand a profile older still, with no layout at all:")
    older = profiles.Profile.from_json(
        {"signature": "x", "buttons": {"a": {"kind": "button", "index": 1}}})
    if older.layout != "":
        fail("an absent layout should read as empty, which resolves to the "
             "generic pad and the canonical keys")
    if not older.has_bindings():
        fail("a pre-layout profile stopped counting as configured")
    print("  ok  empty layout, still configured")


def check_round_trip() -> None:
    print("\nevery scope survives a write and a read:")
    profile = profiles.Profile(signature="sig", name="Pad", icon="gamecube")
    profile.record(profiles.SCOPE_UNIVERSAL, GC_DEFAULT)
    profile.record(profiles.console_scope("n64"), GC_FOR_N64)
    profile.record(profiles.game_scope(MARIO_KEY), GC_FOR_MARIO)

    back = profiles.Profile.from_json(json.loads(json.dumps(profile.to_json())))
    if sorted(back.mappings) != sorted(profile.mappings):
        fail(f"scopes changed across a round trip: {sorted(back.mappings)}")
    for scope, original in profile.mappings.items():
        if back.mappings[scope].buttons != original.buttons:
            fail(f"bindings for {scope!r} did not survive")
        if back.mappings[scope].layout != original.layout:
            fail(f"the layout for {scope!r} did not survive, so its "
                 f"console-specific keys would revert to the gamepad table")
    print(f"  ok  {', '.join(repr(s) for s in sorted(back.mappings))}")

    print("\nthe default is still written where it always was:")
    raw = profile.to_json()
    if raw["layout"] != "gamecube" or "a" not in raw["buttons"]:
        fail("the flat mirror is gone, so rolling back to a build predating "
             "scopes would find every controller unmapped")
    print("  ok  flat buttons/layout mirror the default scope")


def check_seeding() -> None:
    """The first capture becomes the default, later ones do not disturb it."""
    print("\nmapping only for N64, having never mapped at all:")
    fresh = profiles.Profile(signature="sig")
    fresh.record(profiles.console_scope("n64"), GC_FOR_N64)
    if not fresh.buttons:
        fail("no default mapping was seeded, so Pegasus would have no SDL "
             "line and every other console would run on a guess")
    if fresh.layout != "n64":
        fail("the seeded default did not carry the capture's layout")
    print("  ok  the N64 capture is also the default")

    print("\nand a later N64 capture does not rewrite the default:")
    settled = profiles.Profile(signature="sig")
    settled.record(profiles.SCOPE_UNIVERSAL, GC_DEFAULT)
    settled.record(profiles.console_scope("n64"), GC_FOR_N64)
    if settled.layout != "gamecube":
        fail("saying 'and for N64, this instead' silently changed what every "
             "other console does")
    print("  ok  default still gamecube, N64 scope separate")


def check_resolution() -> None:
    print("\nresolution, most specific first:")
    profile = profiles.Profile(signature="sig")
    profile.record(profiles.SCOPE_UNIVERSAL, GC_DEFAULT)
    profile.record(profiles.console_scope("n64"), GC_FOR_N64)
    profile.record(profiles.game_scope(MARIO_KEY), GC_FOR_MARIO)

    cases = [
        (("n64", MARIO_KEY), profiles.game_scope(MARIO_KEY)),
        (("n64", "n64/goldeneye-007"), profiles.console_scope("n64")),
        (("n64", ""), profiles.console_scope("n64")),
        (("snes", "snes/super-metroid"), profiles.SCOPE_UNIVERSAL),
        (("", ""), profiles.SCOPE_UNIVERSAL),
    ]
    for (console, game), expected in cases:
        scope, _ = profile.resolve(console, game)
        if scope != expected:
            fail(f"console={console!r} game={game!r} resolved to {scope!r}, "
                 f"expected {expected!r}")
        print(f"  ok  console={console or '-'!r:<8} game={game or '-'!r:<24} "
              f"-> {scope or 'default'!r}")

    print("\nan empty scope does not shadow a populated one:")
    hollow = profiles.Profile(signature="sig")
    hollow.record(profiles.SCOPE_UNIVERSAL, GC_DEFAULT)
    # Written directly rather than through record(), which is how an abandoned
    # or cleared capture could leave one behind.
    hollow.mappings[profiles.console_scope("n64")] = profiles.Mapping(
        layout="n64", buttons={})
    scope, resolved = hollow.resolve("n64", "")
    if scope != profiles.SCOPE_UNIVERSAL or not resolved.buttons:
        fail("an empty console mapping shadowed the default, leaving the pad "
             "with no bindings at all on that console")
    print("  ok  falls through to the default")


def check_cores_and_keys() -> None:
    print("\ncore name -> console:")
    for core, expected in [
        ("/nix/store/abc-mupen64plus-next/lib/mupen64plus_next_libretro.so", "n64"),
        ("snes9x_libretro.so", "snes"),
        ("/x/mame2010_libretro.so", "arcade"),
        ("/x/dolphin_libretro.so", "gamecube"),
        ("/x/genesis_plus_gx_libretro.so", ""),
        ("", ""),
    ]:
        got = layouts.for_core(core)
        if got != expected:
            fail(f"{core!r} -> {got!r}, expected {expected!r}")
        print(f"  ok  {Path(core).name or '<none>':<32} -> {got or '<unknown>'}")

    print("\ngame keys are stable against the things that move:")
    same = [
        "/roms/n64/Super Mario 64 (USA).z64",
        "/mnt/usb/games/n64/Super Mario 64 (USA).z64",
        "/home/someone/Super Mario 64 (USA).n64",
    ]
    keys = {profiles.game_key("n64", path) for path in same}
    if len(keys) != 1:
        fail(f"the same game keyed differently from different paths: {keys}")
    if profiles.game_key("n64", "/a/Sonic.zip") == \
            profiles.game_key("arcade", "/b/Sonic.zip"):
        fail("the same filename on two consoles shares one mapping")
    print(f"  ok  {keys.pop()!r}, and console-qualified")


def check_launch_argv() -> None:
    """The command line padmap-play actually receives."""
    print("\npulling the core and the ROM out of a launch:")
    argv = [
        "--appendconfig", "/run/user/1000/padmap/launch.cfg",
        "--nodevice", "2", "--nodevice", "3",
        "-L", "/nix/store/abc/lib/retroarch/cores/mupen64plus_next_libretro.so",
        MARIO,
    ]
    directory = tempfile.mkdtemp()
    rom = Path(directory) / Path(MARIO).name
    rom.write_text("")
    argv[-1] = str(rom)

    core, found = launch.split_args(argv)
    if "mupen64plus_next" not in core:
        fail(f"the core was not found: {core!r}")
    if found != str(rom):
        fail(f"the ROM was not found: {found!r}")
    if layouts.for_core(core) != "n64":
        fail("the console did not come out of the core")
    print(f"  ok  core={Path(core).name} rom={Path(found).name}")

    print("\nand a flag's value is never mistaken for the ROM:")
    config = Path(directory) / "launch.cfg"
    config.write_text("")
    core2, rom2 = launch.split_args(["--appendconfig", str(config), "-L", "x.so"])
    if rom2:
        fail(f"--appendconfig's value was taken as a game ({rom2!r})")
    print("  ok  no ROM identified, which falls back to the console mapping")


def check_emitted_profiles() -> None:
    """The whole point, measured at the artefact RetroArch reads."""
    print("\nwhat RetroArch is actually given, per launch:")
    store = tempfile.mkdtemp()
    dest = Path(tempfile.mkdtemp())
    os.environ[profiles.ENV_DIR] = store
    # Keep the probe away from the machine's real autoconfig database: this
    # check is about which of *our* mappings is chosen, not about what
    # libretro happens to ship.
    os.environ["PADMAP_AUTOCONFIG_DIRS"] = store
    try:
        profile = profiles.Profile(
            signature=profiles.signature(PAD), name=PAD.name, icon="gamecube")
        profile.record(profiles.SCOPE_UNIVERSAL, GC_DEFAULT)
        profile.record(profiles.console_scope("n64"), GC_FOR_N64)
        profile.record(profiles.game_scope(MARIO_KEY), GC_FOR_MARIO)
        profiles.save(profile)

        assignments = [Assignment(player=1, pad=PAD, button=0)]

        def emit(console: str, game: str) -> str:
            retroarch.install_profiles(
                assignments, dest=dest, console=console, game=game,
                context=game or "no game")
            return (dest / "padmap Player 1.cfg").read_text()

        # The two captures are told apart by keys only one of them can
        # produce, not by "something was written":
        #
        #   dolphin binds GC A from RetroPad A, so the GameCube layout
        #   overrides a -> input_a_btn. The N64 layout has no such override,
        #   so its 'a' goes to the canonical input_b_btn and input_a_btn
        #   cannot appear at all -- mupen64plus-next never reads RetroPad A.
        #
        #   mupen64plus-next reads N64 'B' from RetroPad Y, so the N64 layout
        #   overrides b -> input_y_btn with the *b* binding. The GameCube
        #   layout also emits input_y_btn, but from its own 'y' control,
        #   which the N64 capture does not have.
        default = emit("", "")
        if 'input_a_btn = "0"' not in default:
            fail("the GameCube default did not emit its own key table:\n"
                 + default)
        if 'input_x_btn = "2"' not in default:
            fail("the GameCube capture's X is missing; the N64 layout has no "
                 "X at all, so this is what distinguishes the two")

        n64 = emit("n64", "n64/goldeneye-007")
        if 'input_a_btn' in n64:
            fail("input_a_btn is in an N64 profile, so the GameCube capture "
                 "was used for an N64 launch. Nothing on this console reads "
                 "RetroPad A:\n" + n64)
        if 'input_y_btn = "1"' not in n64:
            fail("an N64 launch did not use the N64 mapping:\n" + n64)
        if 'input_l2_axis' not in n64:
            fail("the N64 capture's Z trigger (an axis) is missing")
        if "console:n64" not in n64:
            fail("the profile does not record which scope it came from")

        game = emit("n64", MARIO_KEY)
        if 'input_y_btn' in game:
            fail("the per-game capture did not win over the console one")
        if 'input_b_btn = "5"' not in game:
            fail("the per-game capture's own binding is missing:\n" + game)

        snes = emit("snes", "snes/super-metroid")
        if 'input_a_btn = "0"' not in snes or 'input_x_btn = "2"' not in snes:
            fail("a console with no mapping of its own did not fall back to "
                 "the default:\n" + snes)
        print("  ok  no context      -> gamecube default (input_a_btn, input_x_btn)")
        print("  ok  n64, other game -> n64 mapping (input_y_btn, no input_a_btn)")
        print("  ok  n64, Mario 64   -> the per-game mapping")
        print("  ok  snes            -> back to the default")

        print("\nand the same answers through controllercfg:")
        if controllercfg.stored_layout(PAD, "n64") != "n64":
            fail("stored_layout ignored the console")
        if controllercfg.stored_layout(PAD) != "gamecube":
            fail("stored_layout with no context did not give the default")
        if not controllercfg.has_mapping(PAD):
            fail("a mapped pad reported itself unconfigured")
        print("  ok  stored_layout and has_mapping agree")
    finally:
        os.environ.pop(profiles.ENV_DIR, None)
        os.environ.pop("PADMAP_AUTOCONFIG_DIRS", None)


def check_only_console_mapped() -> None:
    """A pad mapped *only* for one console must still count as configured."""
    print("\na pad mapped only for N64:")
    store = tempfile.mkdtemp()
    os.environ[profiles.ENV_DIR] = store
    try:
        profile = profiles.Profile(
            signature=profiles.signature(PAD), name=PAD.name)
        profile.record(profiles.console_scope("n64"), GC_FOR_N64)
        profiles.save(profile)
        if not controllercfg.has_mapping(PAD):
            fail("reported unconfigured, so the wizard would be offered again "
                 "every session")
        if not controllercfg.stored_bindings(PAD):
            fail("no default bindings, so the SDL line Pegasus navigates with "
                 "would fall back to a guess")
    finally:
        os.environ.pop(profiles.ENV_DIR, None)
    print("  ok  configured, and the capture doubles as the default")


def main() -> int:
    check_migration()
    check_round_trip()
    check_seeding()
    check_resolution()
    check_cores_and_keys()
    check_launch_argv()
    check_emitted_profiles()
    check_only_console_mapped()
    print("\nwhich games a per-game mapping may be made for:")
    # A per-game scope can only be offered for a game the daemon has seen
    # launched -- the setup screen is reached from the frontend, never from
    # inside a game. Offering only the *newest* meant someone who had since
    # started something else could no longer reach the game they wanted to
    # fix, with no route to it but to launch it again.
    from padmap import protocol

    with tempfile.TemporaryDirectory() as tmp:
        original = protocol.last_game_path
        store = Path(tmp) / "lastgame.json"
        protocol.last_game_path = lambda: store   # type: ignore[assignment]
        try:
            protocol.write_last_game("n64", "n64/goldeneye", "GoldenEye")
            protocol.write_last_game("n64", "n64/smash", "Smash")
            keys = [g["key"] for g in protocol.read_recent_games()]
            if keys != ["n64/smash", "n64/goldeneye"]:
                raise SystemExit(f"FAIL: newest-first order lost ({keys})")
            if protocol.read_last_game()["key"] != "n64/smash":
                raise SystemExit("FAIL: read_last_game is no longer the newest")

            # Replaying moves a game to the front rather than duplicating it.
            protocol.write_last_game("n64", "n64/goldeneye", "GoldenEye")
            keys = [g["key"] for g in protocol.read_recent_games()]
            if keys != ["n64/goldeneye", "n64/smash"]:
                raise SystemExit(f"FAIL: replay did not move to front ({keys})")

            # And the list stays short, or the picker becomes a scroll.
            for n in range(protocol.RECENT_GAMES + 3):
                protocol.write_last_game("n64", f"n64/game{n}", f"Game {n}")
            if len(protocol.read_recent_games()) != protocol.RECENT_GAMES:
                raise SystemExit(
                    f"FAIL: {len(protocol.read_recent_games())} kept, cap is "
                    f"{protocol.RECENT_GAMES}")

            # The pre-list format still reads, or upgrading mid-session drops
            # the scope for the game being played right now.
            store.write_text(json.dumps(
                {"console": "n64", "key": "n64/old", "title": "Old Format"}))
            if [g["key"] for g in protocol.read_recent_games()] != ["n64/old"]:
                raise SystemExit("FAIL: the old lastgame.json format was lost")

            store.write_text("{ not json")
            if protocol.read_recent_games() != []:
                raise SystemExit("FAIL: a corrupt file should mean no options")
        finally:
            protocol.last_game_path = original   # type: ignore[assignment]
    print(f"  ok  newest first, replays move up, capped at "
          f"{protocol.RECENT_GAMES}, old format still read")

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
