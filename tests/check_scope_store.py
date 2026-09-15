#!/usr/bin/env python3
"""The scope store: one controller, several mappings, kept on disk.

The case that forced all of this is a single GameCube pad that has to answer
three different questions -- "what are your buttons for this one game", "for
this console", "for everything else" -- and `src/padmap/profiles.py` is where
those three answers are keyed, resolved, written and thrown away.

Every failure in here is silent to the person holding the controller. A game
key that quietly changes when a library is moved, a scope that resolves to the
wrong mapping, a migration that drops the capture someone made ten minutes
ago, a `forget` that clears some of a controller: none of them report
anything. They present as "the buttons are wrong again" or "it keeps asking me
to set up the same pad", weeks later and with no clue as to why. So the checks
below assert the store's contract directly, and each failure message says what
the user would experience.

Nothing here opens a device or touches the real profile store: profiles are
files and strings, every check passes an explicit `directory=`, and
PADMAP_PROFILE_DIR is pointed at a trap directory that is asserted empty at
the end so a check that forgot the parameter cannot go unnoticed.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import profiles  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

# A plain dataclass; constructing one neither opens nor claims anything.
GC_PAD = Pad(path="/dev/input/event9", name="Mayflash GameCube Adapter",
             phys="usb-0000:00:14.0-1/input0", uniq="", vid=0x057E,
             pid=0x0337, syspath="")
# The same *model* on a different port. Signature deliberately ignores the
# event node and phys, both of which change between plugs.
GC_PAD_REPLUGGED = Pad(path="/dev/input/event14", name="Mayflash GameCube Adapter",
                       phys="usb-0000:00:14.0-4/input0", uniq="", vid=0x057E,
                       pid=0x0337, syspath="")
# Same vid:pid, different product. DragonRise 0079:1879 is resold in a great
# many unrelated adapters, which is why the name is part of the signature.
OTHER_PAD = Pad(path="/dev/input/event10", name="USB GamePad", phys="p2",
                uniq="", vid=0x057E, pid=0x0337, syspath="")

DEFAULT = profiles.Mapping(
    layout="gamecube",
    buttons={"a": Binding("button", 0), "b": Binding("button", 1),
             "x": Binding("button", 2), "y": Binding("button", 3),
             "start": Binding("button", 7)},
)
FOR_N64 = profiles.Mapping(
    layout="n64",
    name="GameCube pad, N64 games",
    buttons={"a": Binding("button", 0), "b": Binding("button", 1),
             "start": Binding("button", 7),
             "lefttrigger": Binding("axis", 4, 1),
             "rightstick_up": Binding("button", 3, ra_index=11)},
)
FOR_MARIO = profiles.Mapping(
    layout="n64",
    buttons={"a": Binding("button", 5), "start": Binding("button", 7)},
)

MARIO = "/roms/n64/Super Mario 64 (USA).z64"


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def store() -> Path:
    return Path(tempfile.mkdtemp(prefix="padmap-scope-store-"))


# -- game_key ----------------------------------------------------------------


def check_game_key_is_not_the_path() -> None:
    """The key must survive the library moving.

    A per-game mapping that stops applying because a drive was remounted
    elsewhere is worse than one that was never made: the buttons revert to the
    console mapping and nothing anywhere says why.
    """
    print("\nthe same game, reached by different paths:")
    moved = [
        "/roms/n64/Super Mario 64 (USA).z64",
        "/mnt/usb/games/n64/Super Mario 64 (USA).z64",
        "../relative/Super Mario 64 (USA).z64",
        "Super Mario 64 (USA).z64",
    ]
    keys = {profiles.game_key("n64", path) for path in moved}
    if len(keys) != 1:
        fail(f"one game keyed {len(keys)} different ways depending on where "
             f"the library sits, so moving a drive silently loses the mapping "
             f"made for it: {sorted(keys)}")
    key = keys.pop()
    if "/" not in key or key.startswith("/"):
        fail(f"the key is not console-qualified ({key!r})")
    if "roms" in key or "mnt" in key:
        fail(f"a directory name leaked into the key ({key!r}), so the mapping "
             f"is tied to today's layout of the disk")
    print(f"  ok  four paths, one key {key!r}")

    print("\nand the extension is not part of the identity:")
    # A re-dump is the same game to the player. Content hashing would make it
    # a different one to padmap, and would mean reading hundreds of megabytes
    # at launch; the stem does neither.
    if profiles.game_key("n64", "/a/Super Mario 64 (USA).z64") != \
            profiles.game_key("n64", "/b/Super Mario 64 (USA).n64"):
        fail("the same game re-dumped under another extension is a different "
             "game to padmap, so its mapping stops applying")
    # A key is computed from the name alone: the file need not exist, which is
    # what lets a mapping be made for a game that is not plugged in.
    if profiles.game_key("n64", "/nowhere/at/all/Super Mario 64 (USA).z64") != \
            profiles.game_key("n64", MARIO):
        fail("keying depends on the file being readable, so a mapping could "
             "not be made or found for a game on a disconnected drive")
    print("  ok  .z64 and .n64 agree, and the file is never read")


def check_game_key_odd_filenames() -> None:
    """Real ROM filenames, which are not tidy."""
    print("\nodd filenames, one key each:")
    cases = [
        # (what it is, console, filename, expected key)
        ("spaces", "n64", "Super Mario 64.z64", "n64/super-mario-64"),
        ("region and dump flags", "n64", "Mario Kart 64 (U) [!].z64",
         "n64/mario-kart-64-u"),
        # Only the *last* suffix goes. Splitting on the first dot would leave
        # "legend-of-zelda-the-v1", so two revisions of one game would collide
        # -- and worse, "The (v1.2)" would lose its version silently.
        ("dots in the name", "n64", "Legend of Zelda, The (v1.2).z64",
         "n64/legend-of-zelda-the-v1-2"),
        ("double extension", "arcade", "sfiii3.tar.gz", "arcade/sfiii3-tar"),
        ("mixed case", "snes", "SUPER Metroid.SFC", "snes/super-metroid"),
        # A directory-shaped "ROM" is normal for some cores.
        ("no extension at all", "arcade", "/roms/arcade/mslug", "arcade/mslug"),
        ("underscores and digits", "snes", "final_fantasy_3.smc",
         "snes/final-fantasy-3"),
        ("trailing punctuation", "snes", "Yoshi's Island!.sfc",
         "snes/yoshi-s-island"),
        # Non-ASCII is transliterated to nothing, not dropped silently mid-key:
        # the accented letter becomes a separator like any other noise.
        ("an accented title", "gba", "Pokémon Ruby.gba", "gba/pok-mon-ruby"),
    ]
    for label, console, name, expected in cases:
        got = profiles.game_key(console, name)
        if got != expected:
            fail(f"{label}: {name!r} keyed as {got!r}, expected {expected!r}. "
                 f"A key that drifts means the per-game mapping made for this "
                 f"file is never found again")
        print(f"  ok  {label:<22} {name!r} -> {got!r}")

    print("\nand a key is the same every time it is computed:")
    if profiles.game_key("n64", MARIO) != profiles.game_key("n64", MARIO):
        fail("keying is not deterministic, so a mapping is filed under one "
             "key and looked up under another")
    print("  ok  stable across calls")


def check_game_key_unusable_names() -> None:
    """A name with nothing to key on must produce no scope at all.

    The dangerous alternative is a key like ``n64/`` -- every unkeyable title
    would then share one per-game mapping, so setting buttons for one Japanese
    title would change them for every other. Returning "" instead means
    `scope_order` simply omits the game scope and the console mapping applies,
    which is the honest answer.
    """
    print("\nfilenames there is nothing to key on:")
    for name in ["", "   ", "...", "/roms/n64/....z64", "---.z64",
                 "/roms/n64/日本語.z64", "!!!.zip"]:
        got = profiles.game_key("n64", name)
        if got != "":
            fail(f"{name!r} keyed as {got!r}; a name with no usable characters "
                 f"must key as nothing, or every such title shares one mapping")
    print("  ok  seven unkeyable names -> '' (no per-game scope)")

    print("\nand an empty key degrades to the console mapping:")
    order = profiles.scope_order("n64", profiles.game_key("n64", "日本語.z64"))
    if any(scope.startswith("game:") for scope in order):
        fail(f"an unkeyable game still produced a game scope ({order}), which "
             f"every other unkeyable game would then share")
    if order != [profiles.console_scope("n64"), profiles.SCOPE_UNIVERSAL]:
        fail(f"an unkeyable game did not fall back to console then default: "
             f"{order}")
    print(f"  ok  {order}")


def check_game_key_console_qualified() -> None:
    print("\nthe console is part of the key:")
    if profiles.game_key("arcade", "/a/Sonic.zip") == \
            profiles.game_key("genesis", "/b/Sonic.zip"):
        fail("the same filename on two consoles shares one mapping, so "
             "mapping the arcade board changes the Mega Drive game")
    if not profiles.game_key("n64", MARIO).startswith("n64/"):
        fail("the key is not prefixed with the console it was launched under")
    print("  ok  arcade/sonic and genesis/sonic are different games")

    print("\nand a launch whose console could not be identified:")
    # layouts.for_core returns "" for a core padmap has no layout for. The key
    # must still be usable, and must not collide with a known console.
    unknown = profiles.game_key("", "/roms/misc/Sonic.zip")
    if not unknown:
        fail("a game launched under an unrecognised core got no key at all, "
             "so no per-game mapping could ever be made for it")
    if unknown == profiles.game_key("genesis", "/b/Sonic.zip"):
        fail("a game under an unknown console shares a key with the same "
             "filename under a known one")
    if unknown != "unknown/sonic":
        fail(f"an unknown console keyed as {unknown!r}; the prefix must be a "
             f"real word, or the key starts with '/' and reads as a path")
    print(f"  ok  {unknown!r}, distinct from genesis/sonic")


# -- scope strings -----------------------------------------------------------


def check_scope_round_trip() -> None:
    print("\nscope strings round-trip:")
    console = profiles.console_scope("n64")
    game = profiles.game_scope("n64/super-mario-64-usa")
    if profiles.scope_console(console) != "n64":
        fail(f"a console scope did not read back as its layout id ({console!r})")
    if profiles.scope_game(game) != "n64/super-mario-64-usa":
        fail(f"a game scope did not read back as its game key ({game!r})")
    print(f"  ok  {console!r} -> 'n64', {game!r} -> 'n64/super-mario-64-usa'")

    print("\nand the two kinds do not answer for each other:")
    if profiles.scope_console(game) != "":
        fail("a game scope reported itself as a console scope, so the setup "
             "screen would list a game where a console belongs")
    if profiles.scope_game(console) != "":
        fail("a console scope reported itself as a game scope")
    if profiles.scope_console(profiles.SCOPE_UNIVERSAL) != "" or \
            profiles.scope_game(profiles.SCOPE_UNIVERSAL) != "":
        fail("the default scope claimed to name a console or a game")
    print("  ok  each accessor answers '' for anything but its own kind")

    print("\na game key containing a colon still round-trips:")
    # The prefixes are stripped by length, not split on ':', so a key that
    # looks like another scope cannot be mangled.
    odd = "n64/console-n64-thing"
    if profiles.scope_game(profiles.game_scope(odd)) != odd:
        fail("a game key was mangled by scope encoding, so its mapping is "
             "filed under a key nothing looks up")
    print(f"  ok  {profiles.game_scope(odd)!r}")

    print("\nno scope kind can be mistaken for the default:")
    # If console_scope("") were "", a launch whose console was not identified
    # would record over the universal mapping -- changing the pad everywhere.
    if profiles.console_scope("") == profiles.SCOPE_UNIVERSAL:
        fail("an unidentified console produces the default scope, so mapping "
             "one unknown game would overwrite the pad's mapping everywhere")
    if profiles.game_scope("") == profiles.SCOPE_UNIVERSAL:
        fail("an unkeyable game produces the default scope, so a per-game "
             "capture would overwrite the pad's mapping everywhere")
    if profiles.console_scope("n64") == profiles.game_scope("n64"):
        fail("a console and a game of the same name share one scope")
    print("  ok  console:'' and game:'' are both distinct from ''")


def check_scope_order() -> None:
    print("\nscope order, most specific first:")
    full = profiles.scope_order("n64", "n64/super-mario-64-usa")
    if full != [profiles.game_scope("n64/super-mario-64-usa"),
                profiles.console_scope("n64"), profiles.SCOPE_UNIVERSAL]:
        fail(f"the game does not outrank the console, or the default is not "
             f"last: {full}")
    print(f"  ok  {full}")

    print("\ndegrading, one piece at a time:")
    cases = [
        ("console but no game", ("n64", ""),
         [profiles.console_scope("n64"), profiles.SCOPE_UNIVERSAL]),
        # Nothing identified: the daemon writes profiles long before anything
        # knows what will be played, and that call must still get an answer.
        ("neither", ("", ""), [profiles.SCOPE_UNIVERSAL]),
        ("no arguments at all", (), [profiles.SCOPE_UNIVERSAL]),
        # A game with no console is odd but must not lose the game scope.
        ("game but no console", ("", "n64/super-mario-64-usa"),
         [profiles.game_scope("n64/super-mario-64-usa"),
          profiles.SCOPE_UNIVERSAL]),
    ]
    for label, args, expected in cases:
        got = profiles.scope_order(*args)
        if got != expected:
            fail(f"{label}: scope_order{args} gave {got}, expected {expected}. "
                 f"Precedence lives here alone, so everything that resolves a "
                 f"mapping is wrong in the same way")
        print(f"  ok  {label:<20} -> {got}")

    print("\nthe default is always reachable, and never listed twice:")
    for args in [(), ("n64",), ("n64", "n64/x"), ("", "n64/x")]:
        order = profiles.scope_order(*args)
        if profiles.SCOPE_UNIVERSAL not in order:
            fail(f"scope_order{args} never reaches the default mapping, so a "
                 f"pad mapped once and used elsewhere has no bindings at all")
        if order[-1] != profiles.SCOPE_UNIVERSAL:
            fail(f"scope_order{args} does not end at the default, so a general "
                 f"mapping can win over a specific one")
        if len(set(order)) != len(order):
            fail(f"scope_order{args} repeats a scope: {order}")
    print("  ok  present, last, and unique in all four shapes")


# -- resolution and recording ------------------------------------------------


def populated() -> profiles.Profile:
    profile = profiles.Profile(signature="057e:0337:Mayflash GameCube Adapter",
                               name="Mayflash GameCube Adapter", icon="gamecube")
    profile.record(profiles.SCOPE_UNIVERSAL, DEFAULT)
    profile.record(profiles.console_scope("n64"), FOR_N64)
    profile.record(profiles.game_scope(profiles.game_key("n64", MARIO)), FOR_MARIO)
    return profile


def check_resolve() -> None:
    print("\nresolve returns the first scope that actually holds bindings:")
    profile = populated()
    mario = profiles.game_key("n64", MARIO)
    cases = [
        ("Mario 64", ("n64", mario), profiles.game_scope(mario), FOR_MARIO),
        ("another N64 game", ("n64", "n64/goldeneye-007"),
         profiles.console_scope("n64"), FOR_N64),
        ("N64, game unknown", ("n64", ""),
         profiles.console_scope("n64"), FOR_N64),
        ("a console never mapped", ("snes", "snes/super-metroid"),
         profiles.SCOPE_UNIVERSAL, DEFAULT),
        ("no context at all", ("", ""), profiles.SCOPE_UNIVERSAL, DEFAULT),
    ]
    for label, (console, game), expected_scope, expected in cases:
        scope, resolved = profile.resolve(console, game)
        if scope != expected_scope:
            fail(f"{label}: resolved to {scope!r}, expected {expected_scope!r} "
                 f"-- the buttons would be wrong for this launch and nothing "
                 f"would report it")
        if resolved.buttons != expected.buttons:
            fail(f"{label}: resolved to the right scope but the wrong bindings")
        if resolved.layout != expected.layout:
            fail(f"{label}: the layout came back as {resolved.layout!r}, so "
                 f"the console's key overrides would not be applied")
        print(f"  ok  {label:<22} -> {scope or 'default'!r} "
              f"({resolved.layout})")

    print("\nan empty capture does not shadow a populated one:")
    hollow = profiles.Profile(signature="sig")
    hollow.record(profiles.SCOPE_UNIVERSAL, DEFAULT)
    # Written past record(), which is how an abandoned or cleared capture
    # could leave an empty scope behind.
    hollow.mappings[profiles.console_scope("n64")] = profiles.Mapping(layout="n64")
    scope, resolved = hollow.resolve("n64", "n64/goldeneye-007")
    if scope != profiles.SCOPE_UNIVERSAL or resolved.buttons != DEFAULT.buttons:
        fail("an empty N64 capture shadowed the default, so on that one "
             "console the pad has no bindings at all")
    print("  ok  falls through to the default")

    print("\nnothing captured yet:")
    blank = profiles.Profile(signature="sig")
    scope, resolved = blank.resolve("n64", "n64/goldeneye-007")
    if scope != profiles.SCOPE_UNIVERSAL:
        fail(f"an unmapped pad resolved to {scope!r} rather than the default")
    if resolved is None or resolved.buttons:
        fail("an unmapped pad resolved to something other than an empty "
             "mapping, so callers would have to handle a missing case that "
             "the contract says never happens")
    if blank.has_bindings():
        fail("a pad with no capture at all reports itself configured, so the "
             "setup wizard would never be offered for it")
    print("  ok  ('' , empty mapping), and reported unconfigured")

    print("\na pad mapped only for one console is still configured:")
    only_n64 = profiles.Profile(signature="sig")
    only_n64.record(profiles.console_scope("n64"), FOR_N64)
    if not only_n64.has_bindings():
        fail("a pad mapped for N64 alone reports itself unconfigured, so the "
             "wizard would be offered again every session")
    print("  ok  has_bindings() is true for any scope, not just the default")

    print("\nthe universal view is a copy, not the stored dict:")
    profile = populated()
    handed_out = profile.buttons
    handed_out["a"] = Binding("button", 99)
    if profile.buttons["a"] == Binding("button", 99):
        fail("a caller reading .buttons can edit the stored mapping by "
             "accident, so a pad's bindings change without a capture")
    print("  ok  editing the returned dict leaves the profile alone")


def check_record_replaces() -> None:
    print("\nre-mapping a scope replaces it rather than merging into it:")
    profile = populated()
    replacement = profiles.Mapping(layout="n64",
                                   buttons={"a": Binding("button", 4)})
    profile.record(profiles.console_scope("n64"), replacement)
    _, resolved = profile.resolve("n64", "")
    if resolved.buttons != replacement.buttons:
        fail(f"re-mapping merged into the old capture instead of replacing "
             f"it: {sorted(resolved.buttons)}. Buttons the user deliberately "
             f"left out would keep working, and the leftovers are invisible")
    if "start" in resolved.buttons or "lefttrigger" in resolved.buttons:
        fail("bindings from the previous N64 capture survived the re-map, so "
             "re-doing a mapping cannot remove a binding")
    if resolved.name != "":
        fail("the replaced capture kept the old one's name")
    print("  ok  one binding in, one binding out, old ones gone")

    print("\nand re-mapping one scope leaves the others alone:")
    if profile.mappings[profiles.SCOPE_UNIVERSAL].buttons != DEFAULT.buttons:
        fail("re-mapping the N64 scope changed the default, so the pad's "
             "behaviour everywhere else moved without being asked")
    mario_scope = profiles.game_scope(profiles.game_key("n64", MARIO))
    if profile.mappings[mario_scope].buttons != FOR_MARIO.buttons:
        fail("re-mapping the console scope disturbed the per-game one")
    print("  ok  default and per-game captures untouched")

    print("\nthe first capture also becomes the default:")
    fresh = profiles.Profile(signature="sig")
    fresh.record(profiles.console_scope("n64"), FOR_N64)
    if fresh.buttons != FOR_N64.buttons or fresh.layout != "n64":
        fail("mapping a pad for N64 first left it with no default mapping, so "
             "Pegasus has no SDL line and every other console runs on a guess")
    print("  ok  N64-only capture seeds the default")

    print("\nbut a later one does not:")
    settled = profiles.Profile(signature="sig")
    settled.record(profiles.SCOPE_UNIVERSAL, DEFAULT)
    settled.record(profiles.console_scope("n64"), FOR_N64)
    if settled.layout != "gamecube" or settled.buttons != DEFAULT.buttons:
        fail("saying 'and for N64, this instead' silently changed what the "
             "pad does on every other console")
    print("  ok  default still the gamecube capture")


# -- storage -----------------------------------------------------------------


def check_json_round_trip() -> None:
    print("\nevery scope survives to_json/from_json:")
    profile = populated()
    profile.axes[0] = profiles.AxisCalibration(center=174, minimum=0,
                                               maximum=255, flat=12,
                                               reach_min=40, reach_max=250)
    raw = json.loads(json.dumps(profile.to_json()))
    back = profiles.Profile.from_json(raw)

    if sorted(back.mappings) != sorted(profile.mappings):
        fail(f"scopes changed across a round trip: {sorted(back.mappings)} "
             f"-- a mapping the user made is simply gone after a restart")
    for scope, original in profile.mappings.items():
        if back.mappings[scope].buttons != original.buttons:
            fail(f"bindings under {scope!r} did not survive a save and load")
        if back.mappings[scope].layout != original.layout:
            fail(f"the layout under {scope!r} did not survive, so that "
                 f"console's key overrides revert to the generic table")
        if back.mappings[scope].name != original.name:
            fail(f"the label on {scope!r} did not survive, so the setup "
                 f"screen stops naming the mapping the user named")
    n64 = back.mappings[profiles.console_scope("n64")]
    if n64.buttons["rightstick_up"].ra_index != 11:
        fail("a binding's separate RetroArch index was lost, so the button "
             "works in the menu and does nothing in the game")
    if n64.buttons["lefttrigger"] != Binding("axis", 4, 1):
        fail("an axis binding came back as something else, so an analogue "
             "trigger stops working")
    if back.axes[0].reach_max != 250 or back.axes[0].flat != 12:
        fail("stick calibration was lost, so a worn stick drifts again")
    if (back.signature, back.name, back.icon) != \
            (profile.signature, profile.name, profile.icon):
        fail("the profile's identity or icon did not survive")
    print(f"  ok  {len(back.mappings)} scopes, bindings, layouts, names, "
          f"ra_index, axes")

    print("\nthe default is mirrored flat, where it has always been:")
    written = profile.to_json()
    if written.get("layout") != "gamecube":
        fail("the flat layout mirror is gone, so rolling back to a build that "
             "predates scopes finds the controller unmapped")
    if set(written.get("buttons") or {}) != set(DEFAULT.buttons):
        fail("the flat buttons mirror does not match the default scope, so a "
             "rollback would drive the pad from stale bindings")
    print(f"  ok  layout={written['layout']!r}, "
          f"{len(written['buttons'])} buttons")

    print("\nand the mirror follows the default when it is re-recorded:")
    profile.record(profiles.SCOPE_UNIVERSAL,
                   profiles.Mapping(layout="snes",
                                    buttons={"a": Binding("button", 6)}))
    written = profile.to_json()
    if written["layout"] != "snes" or list(written["buttons"]) != ["a"]:
        fail("the flat mirror kept the previous default, so a rollback would "
             "find a mapping the user has already replaced")
    print("  ok  mirror rewritten with the new default")

    print("\nan empty profile round-trips without inventing anything:")
    empty = profiles.Profile.from_json(profiles.Profile(signature="s").to_json())
    if empty.mappings:
        fail(f"a never-captured profile came back holding scopes "
             f"({sorted(empty.mappings)}), which would report the pad as "
             f"configured and skip the wizard")
    if empty.has_bindings():
        fail("a never-captured profile reports itself configured")
    print("  ok  no scopes, still unconfigured")


def check_migration_from_flat() -> None:
    """The shape every profile on disk had before scopes existed.

    Someone upgrading mid-session must not lose the mapping they are using.
    Migration happens on read rather than as an upgrade pass, so a profile
    restored from a backup years later is migrated too.
    """
    print("\na profile written before scopes existed:")
    legacy = {
        "signature": "0079:1879:USB GamePad",
        "name": "USB GamePad",
        "icon": "n64",
        "layout": "n64",
        "buttons": {
            "a": {"kind": "button", "index": 0, "value": 0, "ra_index": None},
            "dpup": {"kind": "hat", "index": 0, "value": 1, "ra_index": None},
        },
        "axes": {"0": {"center": 174, "min": 0, "max": 255, "flat": 10,
                       "reach_min": 20, "reach_max": 240}},
    }
    profile = profiles.Profile.from_json(legacy)
    if profiles.SCOPE_UNIVERSAL not in profile.mappings:
        fail(f"the flat capture did not become the default scope "
             f"({sorted(profile.mappings)}), so an already-configured "
             f"controller presents as new and is driven by a guess")
    if profile.layout != "n64":
        fail("the flat layout was dropped, so the console's key overrides "
             "stop being applied to a mapping that already worked")
    if set(profile.buttons) != {"a", "dpup"}:
        fail(f"bindings were lost in migration: {sorted(profile.buttons)}")
    if profile.buttons["dpup"] != Binding("hat", 0, 1):
        fail("a hat binding was mangled in migration, so the d-pad stops "
             "working on an upgrade")
    if not profile.has_bindings():
        fail("a migrated profile reports itself unconfigured, so the wizard "
             "would be offered again for every controller on the machine")
    if profile.axes[0].reach_max != 240:
        fail("calibration was lost in migration, so a worn stick drifts again")
    print("  ok  flat capture -> default scope, hat and calibration intact")

    print("\nand it rewrites in the new shape, mirror included:")
    rewritten = profile.to_json()
    if profiles.SCOPE_UNIVERSAL not in (rewritten.get("mappings") or {}):
        fail("a migrated profile was written back without its default scope")
    if rewritten.get("layout") != "n64" or "a" not in (rewritten.get("buttons") or {}):
        fail("a migrated profile was written back without the flat mirror, so "
             "the migration is one-way and a rollback loses the mapping")
    again = profiles.Profile.from_json(rewritten)
    if set(again.buttons) != {"a", "dpup"} or again.layout != "n64":
        fail("migrating twice is not the same as migrating once")
    print("  ok  stable under a second pass")

    print("\na profile older still, with no layout key at all:")
    older = profiles.Profile.from_json(
        {"signature": "x", "buttons": {"a": {"kind": "button", "index": 1}}})
    if older.layout != "":
        fail(f"an absent layout read as {older.layout!r}; empty is what "
             f"resolves to the generic pad, so such a profile emits exactly "
             f"what it emitted before")
    if not older.has_bindings():
        fail("a pre-layout profile stopped counting as configured")
    print("  ok  empty layout, still configured")

    print("\na half-upgraded profile: scoped captures plus a flat default:")
    # Written by a build that had scopes, then edited by one that did not, or
    # any other order that leaves the pair without a "" entry. The flat pair
    # is the default and must be folded in, not discarded.
    half = {
        "signature": "x",
        "layout": "gamecube",
        "buttons": {"a": {"kind": "button", "index": 0}},
        "mappings": {"console:n64": {"layout": "n64", "buttons": {
            "a": {"kind": "button", "index": 9}}}},
    }
    merged = profiles.Profile.from_json(half)
    if merged.layout != "gamecube" or merged.buttons["a"].index != 0:
        fail("the flat default was discarded when scoped captures were "
             "present, so the mapping used everywhere but N64 is gone")
    if merged.mappings[profiles.console_scope("n64")].buttons["a"].index != 9:
        fail("the scoped capture was overwritten by the flat mirror")
    print("  ok  flat pair folded into the default, N64 scope kept")

    print("\nand a stale flat mirror never overrides a real default scope:")
    both = {
        "signature": "x",
        "layout": "gamecube",
        "buttons": {"a": {"kind": "button", "index": 0}},
        "mappings": {"": {"layout": "snes", "buttons": {
            "a": {"kind": "button", "index": 3}}}},
    }
    kept = profiles.Profile.from_json(both)
    if kept.layout != "snes" or kept.buttons["a"].index != 3:
        fail("the compatibility mirror won over the real default scope, so a "
             "mapping made after a rollback-and-return is silently reverted")
    print("  ok  mappings[''] wins over the flat pair")

    print("\njunk inside a profile is skipped, not fatal:")
    junk = profiles.Profile.from_json({
        "signature": "x",
        "axes": {"nope": {"center": 1}, "0": {"center": 128}},
        "mappings": {
            "": {"layout": "n64", "buttons": {
                "a": {"kind": "button", "index": 2},
                "b": "not a binding"}},
            "console:snes": 5,
        },
    })
    if set(junk.buttons) != {"a"}:
        fail(f"one unreadable binding took the rest of the capture with it "
             f"({sorted(junk.buttons)}); a partly damaged file must keep the "
             f"buttons that are still readable")
    if profiles.console_scope("snes") in junk.mappings:
        fail("a scope whose capture is not a mapping at all was kept, so it "
             "could shadow a good one")
    if junk.axes:
        fail("an axis entry missing its declared range was kept, which would "
             "rescale the stick against numbers that were never measured")
    print("  ok  bad binding, bad scope and bad axis dropped; 'a' survives")


def check_files() -> None:
    print("\nsave then load, through a real file:")
    directory = store()
    profile = populated()
    profile.signature = profiles.signature(GC_PAD)
    path = profiles.save(profile, directory=directory)
    if not path.exists() or path.parent != directory:
        fail(f"save did not write inside the directory it was given ({path})")
    loaded = profiles.load(GC_PAD, directory=directory)
    if loaded is None:
        fail("a profile that was just saved could not be loaded back, so "
             "every controller is new on every boot")
    if sorted(loaded.mappings) != sorted(profile.mappings):
        fail(f"scopes were lost on the way through the file: "
             f"{sorted(loaded.mappings)}")
    if not profiles.is_known(GC_PAD, directory=directory):
        fail("a saved controller is not recognised, so calibration would run "
             "again every session")
    print(f"  ok  {path.name}, {len(loaded.mappings)} scopes")

    print("\nsave creates the store directory if it is not there:")
    nested = store() / "deeper" / "still"
    profiles.save(profile, directory=nested)
    if not (nested / path.name).exists():
        fail("saving into a store that does not exist yet failed, so the "
             "first controller ever configured loses its mapping")
    print("  ok  parents created")

    print("\nthe same model on another port is the same profile:")
    if not profiles.is_known(GC_PAD_REPLUGGED, directory=directory):
        fail("replugging into a different USB port presents the controller as "
             "new, so the wizard opens on the television again")
    replugged = profiles.load(GC_PAD_REPLUGGED, directory=directory)
    if replugged is None or sorted(replugged.mappings) != sorted(profile.mappings):
        fail("a replugged controller did not find its own scoped mappings")
    print("  ok  event node and phys are not part of the identity")

    print("\na different product sharing the vid:pid is not:")
    if profiles.is_known(OTHER_PAD, directory=directory):
        fail("two unrelated adapters that share a vendor id share one "
             "profile, so mapping one silently rewires the other")
    print("  ok  the device name is part of the signature")

    print("\nan unknown controller:")
    empty_dir = store()
    if profiles.load(GC_PAD, directory=empty_dir) is not None:
        fail("a controller with no stored profile loaded something anyway")
    if profiles.is_known(GC_PAD, directory=empty_dir):
        fail("a controller never seen before reports itself known, so it is "
             "never offered the setup it needs")
    if profiles.load(GC_PAD, directory=empty_dir / "not" / "there") is not None:
        fail("a missing store directory did not read as 'nothing stored'")
    print("  ok  missing file and missing directory both read as None")


def check_damaged_files() -> None:
    """A damaged profile must read as "no profile", never as a crash.

    `load` is called from device discovery. Anything it raises takes the
    daemon's handling of that controller with it, and the user sees a pad that
    simply never appears.
    """
    print("\nfiles that are not a profile:")
    directory = store()
    target = directory / profiles._filename(profiles.signature(GC_PAD))
    directory.mkdir(parents=True, exist_ok=True)

    for label, text in [
        ("empty file", ""),
        ("truncated write", '{"signature": "057e:0337:May'),
        ("not json at all", "\x00\x01binary junk"),
        ("whitespace only", "   \n  "),
    ]:
        target.write_text(text)
        if profiles.load(GC_PAD, directory=directory) is not None:
            fail(f"{label}: a damaged profile loaded as a usable one, so the "
                 f"pad would be driven from whatever survived")
        if profiles.is_known(GC_PAD, directory=directory):
            fail(f"{label}: a damaged profile reports the controller as "
                 f"configured, so it is never offered setup and stays broken")
        print(f"  ok  {label:<18} -> no profile, controller counts as new")

    print("\nvalid json that is not an object:")
    # Known gap, recorded rather than asserted away: `from_json` calls
    # .get on whatever it is handed, so these raise AttributeError instead of
    # returning None. Neither may ever produce a *usable* profile, which is
    # what would silently mark a wiped file as a configured controller.
    for text in ["null", "[1, 2]", '"a string"', "42"]:
        target.write_text(text)
        try:
            got = profiles.load(GC_PAD, directory=directory)
            raised = ""
        except Exception as error:  # noqa: BLE001 - see the note above
            got, raised = None, type(error).__name__
        if got is not None:
            fail(f"{text!r} loaded as a usable profile, so a wiped file "
                 f"presents as a configured controller with no bindings")
        print(f"  ok  {text!r:<12} -> {raised or 'None'}"
              f"{'  (gap: should be None)' if raised else ''}")

    print("\na profile file that is an object but holds nothing useful:")
    target.write_text("{}")
    recovered = profiles.load(GC_PAD, directory=directory)
    if recovered is None:
        fail("an empty json object did not load at all")
    if recovered.has_bindings():
        fail("an empty profile reports bindings it does not have")
    scope, resolved = recovered.resolve("n64", "n64/x")
    if scope != profiles.SCOPE_UNIVERSAL or resolved.buttons:
        fail("resolving against an empty profile did not give the empty "
             "default, so callers meet a case the contract denies")
    print("  ok  loads, reports unconfigured, resolves to the empty default")

    print("\nand a damaged file can be overwritten by a fresh capture:")
    target.write_text("{ not json")
    profile = profiles.Profile(signature=profiles.signature(GC_PAD))
    profile.record(profiles.SCOPE_UNIVERSAL, DEFAULT)
    profiles.save(profile, directory=directory)
    again = profiles.load(GC_PAD, directory=directory)
    if again is None or not again.has_bindings():
        fail("re-running the wizard over a damaged profile did not repair it, "
             "so the controller can never be configured again")
    print("  ok  re-capturing repairs the file")


def check_forget() -> None:
    print("\nforget takes every scope, not just the default:")
    directory = store()
    profile = populated()
    profile.signature = profiles.signature(GC_PAD)
    profiles.save(profile, directory=directory)
    if len(profile.mappings) < 3:
        fail("the fixture lost a scope before forget was even called")

    if not profiles.forget(GC_PAD, directory=directory):
        fail("forget reported it removed nothing when there was a profile, so "
             "'reset this controller' looks like it did nothing")
    if profiles.load(GC_PAD, directory=directory) is not None:
        fail("a profile survived forget")
    if profiles.is_known(GC_PAD, directory=directory):
        fail("a forgotten controller still reports itself known, so the setup "
             "prompt never comes back")
    leftovers = [p.name for p in directory.iterdir()]
    if leftovers:
        fail(f"forget left {leftovers} behind. A per-console mapping the user "
             f"had forgotten was there would then still be applied after a "
             f"reset, and they would meet old behaviour from a fresh wizard")
    print("  ok  the file and all three scopes are gone")

    print("\nand forgetting twice is honest about it:")
    if profiles.forget(GC_PAD, directory=directory):
        fail("forget claimed to have removed a profile that was not there, so "
             "'reset' reports success on a controller it never touched")
    if profiles.forget(GC_PAD, directory=store()):
        fail("forget claimed success against an empty store")
    if profiles.forget(GC_PAD, directory=store() / "no" / "such" / "dir"):
        fail("forget claimed success against a store that does not exist")
    print("  ok  False the second time, and against a missing store")

    print("\nforgetting one controller does not touch another:")
    directory = store()
    first = profiles.Profile(signature=profiles.signature(GC_PAD))
    first.record(profiles.console_scope("n64"), FOR_N64)
    second = profiles.Profile(signature=profiles.signature(OTHER_PAD))
    second.record(profiles.SCOPE_UNIVERSAL, DEFAULT)
    profiles.save(first, directory=directory)
    profiles.save(second, directory=directory)
    profiles.forget(GC_PAD, directory=directory)
    if profiles.load(OTHER_PAD, directory=directory) is None:
        fail("forgetting one controller deleted another's mapping, so the "
             "player has to set up a pad they never asked to reset")
    if profiles.load(GC_PAD, directory=directory) is not None:
        fail("forget removed the wrong file")
    print("  ok  only the named controller is cleared")

    print("\nand a controller can be set up again after being forgotten:")
    fresh = profiles.Profile(signature=profiles.signature(GC_PAD))
    fresh.record(profiles.console_scope("snes"),
                 profiles.Mapping(layout="snes", buttons={"a": Binding("button", 1)}))
    profiles.save(fresh, directory=directory)
    back = profiles.load(GC_PAD, directory=directory)
    if back is None or profiles.console_scope("n64") in back.mappings:
        fail("the forgotten N64 scope reappeared after re-configuring, which "
             "is exactly the old behaviour a reset is meant to remove")
    print("  ok  the new mapping stands alone")


def main() -> int:
    # Any check that forgot its `directory=` would fall back here rather than
    # to the machine's real store, and the trap is asserted empty below. A
    # live padmap daemon reads the real one.
    trap = store()
    os.environ[profiles.ENV_DIR] = str(trap)
    try:
        check_game_key_is_not_the_path()
        check_game_key_odd_filenames()
        check_game_key_unusable_names()
        check_game_key_console_qualified()
        check_scope_round_trip()
        check_scope_order()
        check_resolve()
        check_record_replaces()
        check_json_round_trip()
        check_migration_from_flat()
        check_files()
        check_damaged_files()
        check_forget()
    finally:
        os.environ.pop(profiles.ENV_DIR, None)

    print("\nnothing was written outside the temporary stores:")
    stray = sorted(p.name for p in trap.iterdir()) if trap.exists() else []
    if stray:
        fail(f"a check wrote to the default profile directory ({stray}); "
             f"without the trap that would be the real store, and this "
             f"machine has a live daemon reading it")
    print("  ok  the default profile directory was never used")

    print("\na damaged profile reads as nothing stored, not as a crash:")
    # Found by this file's first run: Profile.from_json called raw.get() on
    # whatever json.loads returned, and load() only caught OSError and
    # ValueError -- so a file holding valid JSON that is not an object came
    # back as AttributeError. is_known() calls load() for every pad during
    # discovery, so one corrupted file stopped that controller being handled
    # at all. And it must be *None*, not an empty Profile: is_known is
    # "load() is not None", so an empty one marks the pad as already set up
    # and it is never offered the wizard again.
    with tempfile.TemporaryDirectory() as tmp:
        directory = Path(tmp)
        broken = Pad(path="/dev/input/event77", name="Corrupt Pad",
                     phys="corrupt", uniq="", vid=0x1234, pid=0x0001,
                     syspath="")
        target = directory / profiles._filename(profiles.signature(broken))
        for junk in ("null", "[1, 2]", '"a string"', "42", "not json",
                     "", "   "):
            target.write_text(junk)
            try:
                got = profiles.load(broken, directory)
            except Exception as error:                  # noqa: BLE001
                raise SystemExit(
                    f"FAIL: a profile file holding {junk!r} raised "
                    f"{type(error).__name__} -- discovery reads every pad's "
                    f"profile, so one damaged file stops that controller "
                    f"being handled at all")
            if got is not None:
                raise SystemExit(
                    f"FAIL: {junk!r} loaded as a profile, so is_known() says "
                    f"the controller is already set up and it is never "
                    f"offered the wizard")
            if profiles.is_known(broken, directory):
                raise SystemExit(
                    f"FAIL: is_known() is True for {junk!r}")
        # Object-shaped junk is different: the file *is* a profile, just an
        # unusable one, so it may load with nothing in it -- but it still
        # must not raise.
        for junk in ('{"axes": "nope"}', '{"axes": {"0": "x"}}',
                     '{"mappings": 5}', '{"mappings": {"": {"buttons": "x"}}}',
                     '{"mappings": {"": 5}}'):
            target.write_text(junk)
            try:
                profiles.load(broken, directory)
            except Exception as error:                  # noqa: BLE001
                raise SystemExit(
                    f"FAIL: {junk!r} raised {type(error).__name__} -- junk in "
                    f"one slot must cost that slot, not the whole profile")
    print("  ok  every damaged shape reads as absent, and none of them raise")

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
