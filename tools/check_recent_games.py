#!/usr/bin/env python3
"""Recently launched games, and the scope pickers built on top of them.

"The controls were wrong in the game I just played" is the moment someone
wants a per-game mapping, and it is the one moment the controller setup screen
cannot answer by itself: that screen is reached from the front-end, never from
inside a game, so nothing on it knows which game is meant. `lastgame.json` --
written by padmap-play, read by the daemon -- is the only bridge. Everything
below guards a way that bridge can quietly stop carrying the right game:

* a list that loses its order, or fills with copies of one replayed game and
  pushes out everything else someone might want to correct;
* a file written by an older padmap that stops being readable, so a user who
  upgrades mid-session loses the scope for the game running right now;
* a damaged file that raises instead of simply offering fewer options -- this
  decorates a picker, so a bad file must cost options, never the screen;
* a picker strip that offers a game twice, draws it as the wrong console, or
  fails to mark a scope that already holds a capture. Re-mapping a scope
  *replaces* it, and without the mark there is no way to tell which one that
  would destroy.

Never touches real state: `protocol.last_game_path` is monkeypatched to a temp
file for every scenario, so the live daemon's lastgame.json is not read,
written or deleted.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \\
        nix develop --command python3 tools/check_recent_games.py
"""

from __future__ import annotations

import json
import sys
import tempfile
from contextlib import contextmanager
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import capture, layouts, profiles, protocol  # noqa: E402


@contextmanager
def store():
    """A temp lastgame.json, in place of the real one.

    The daemon on this machine is live and owns the real file under
    XDG_RUNTIME_DIR. Reading it would make these checks depend on whatever was
    played last; writing it would rewrite a user's recent list.
    """
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "runtime" / "lastgame.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        original = protocol.last_game_path
        protocol.last_game_path = lambda: path   # type: ignore[assignment]
        try:
            yield path
        finally:
            protocol.last_game_path = original  # type: ignore[assignment]


def keys() -> list[str]:
    return [game["key"] for game in protocol.read_recent_games()]


def ids(options) -> list[str]:
    return [option.id for option in options]


def check_order_and_dedup() -> None:
    print("\nlaunches come back newest first:")
    with store():
        protocol.write_last_game("n64", "n64/goldeneye-007-usa", "GoldenEye")
        protocol.write_last_game("snes", "snes/super-metroid", "Super Metroid")
        protocol.write_last_game("gamecube", "gamecube/melee", "Melee")
        if keys() != ["gamecube/melee", "snes/super-metroid",
                      "n64/goldeneye-007-usa"]:
            raise SystemExit(
                f"FAIL: recent games came back as {keys()}. The picker offers "
                f"them in this order, so the game just played must be the "
                f"first one reachable, not buried under older launches.")
        print(f"  ok  {keys()}")

        if protocol.read_last_game()["key"] != "gamecube/melee":
            raise SystemExit(
                "FAIL: read_last_game is not the newest launch. The daemon "
                "reinstalls RetroArch profiles for this game on every "
                "republish, so an older one silently overrides the mapping "
                "for the game actually running.")
        if protocol.read_last_game()["title"] != "Melee":
            raise SystemExit(
                "FAIL: read_last_game lost the title, so the picker entry "
                "for the game just played has nothing to name it")
        print("  ok  read_last_game is the newest entry, title intact")

    print("\nreplaying a game moves it up rather than duplicating it:")
    with store():
        protocol.write_last_game("n64", "n64/goldeneye", "GoldenEye")
        protocol.write_last_game("n64", "n64/mario-64", "Super Mario 64")
        protocol.write_last_game("n64", "n64/goldeneye", "GoldenEye")
        if keys() != ["n64/goldeneye", "n64/mario-64"]:
            raise SystemExit(
                f"FAIL: after a replay the list is {keys()}. Copies of one "
                f"game fill a five-entry strip and push out the other games "
                f"the user might want to fix.")
        print(f"  ok  {keys()} -- one entry, moved to the front")

        # Dedup is by key, and the freshly written console/title win: a game
        # re-exported under a nicer title should read with that title, not the
        # one recorded the first time it was played.
        protocol.write_last_game("n64", "n64/mario-64", "Super Mario 64 (USA)")
        titles = [game["title"] for game in protocol.read_recent_games()]
        if titles != ["Super Mario 64 (USA)", "GoldenEye"]:
            raise SystemExit(
                f"FAIL: a replay kept a stale title ({titles}); the picker "
                f"would name the game something the library no longer shows")
        print(f"  ok  {titles} -- the newest launch supplies the title")

    print("\ntwo different games that share a title both survive:")
    with store():
        protocol.write_last_game("n64", "n64/pac-man", "Pac-Man")
        protocol.write_last_game("arcade", "arcade/pac-man", "Pac-Man")
        if keys() != ["arcade/pac-man", "n64/pac-man"]:
            raise SystemExit(
                f"FAIL: {keys()} -- deduplicating by title would hide the "
                f"arcade version of a game from the scope picker entirely")
        print(f"  ok  {keys()}")


def check_cap() -> None:
    print(f"\nthe list stops at protocol.RECENT_GAMES ({protocol.RECENT_GAMES}):")
    with store():
        for index in range(protocol.RECENT_GAMES + 4):
            protocol.write_last_game("n64", f"n64/game{index}", f"Game {index}")
        recent = protocol.read_recent_games()
        if len(recent) != protocol.RECENT_GAMES:
            raise SystemExit(
                f"FAIL: {len(recent)} games kept, cap is "
                f"{protocol.RECENT_GAMES}. The strip is worked from the pad "
                f"one step at a time and games sit after every console, so a "
                f"long tail turns 'map this for N64' into a scrolling chore.")
        newest = protocol.RECENT_GAMES + 3
        if recent[0]["key"] != f"n64/game{newest}":
            raise SystemExit(
                f"FAIL: capping dropped the newest game ({recent[0]['key']}), "
                f"which is the one the user just played")
        if any(game["key"] == "n64/game0" for game in recent):
            raise SystemExit(
                "FAIL: the oldest launch survived the cap, so the list grows "
                "from the wrong end and stale games crowd out fresh ones")
        print(f"  ok  {keys()}")

        # Replaying the oldest survivor promotes it without shortening the
        # list: dedup must run before the cap, not instead of it.
        oldest = recent[-1]["key"]
        protocol.write_last_game("n64", oldest, "Promoted")
        if keys()[0] != oldest or len(keys()) != protocol.RECENT_GAMES:
            raise SystemExit(
                f"FAIL: replaying the oldest entry gave {keys()}; it must "
                f"move to the front and the list must stay full")
        print(f"  ok  replaying {oldest!r} promotes it, list stays full")

    print("\na file already longer than the cap is trimmed on read:")
    with store() as path:
        path.write_text(json.dumps({"games": [
            {"console": "n64", "key": f"n64/old{n}", "title": f"Old {n}"}
            for n in range(protocol.RECENT_GAMES + 6)
        ]}))
        if len(protocol.read_recent_games()) != protocol.RECENT_GAMES:
            raise SystemExit(
                f"FAIL: a file written by a build with a larger cap offers "
                f"{len(protocol.read_recent_games())} games; the picker must "
                f"stay short whatever is on disk")
        print(f"  ok  {protocol.RECENT_GAMES} offered from an oversized file")


def check_old_format() -> None:
    print("\nthe pre-list format is still read:")
    with store() as path:
        # What earlier versions wrote: a bare object, no "games" list. Someone
        # upgrading mid-session would otherwise lose the per-game scope for
        # the game they are playing right now -- exactly when they want it.
        path.write_text(json.dumps(
            {"console": "n64", "key": "n64/goldeneye", "title": "GoldenEye"}))
        recent = protocol.read_recent_games()
        if [game["key"] for game in recent] != ["n64/goldeneye"]:
            raise SystemExit(
                "FAIL: a lastgame.json written by the previous padmap reads "
                "as nothing, so upgrading mid-game loses the scope for the "
                "game currently running")
        if recent[0]["console"] != "n64" or recent[0]["title"] != "GoldenEye":
            raise SystemExit(
                f"FAIL: the old format lost its console/title ({recent[0]}); "
                f"the picker would draw the wrong pad and have no name to show")
        if protocol.read_last_game()["key"] != "n64/goldeneye":
            raise SystemExit(
                "FAIL: read_last_game does not see the old format, so the "
                "daemon reinstalls context-free RetroArch profiles over the "
                "per-game ones for the running game")
        print(f"  ok  {recent[0]}")

        # The next launch migrates the file without discarding what was there.
        protocol.write_last_game("snes", "snes/super-metroid", "Super Metroid")
        if keys() != ["snes/super-metroid", "n64/goldeneye"]:
            raise SystemExit(
                f"FAIL: migrating gave {keys()}; the game played before the "
                f"upgrade must still be offered afterwards")
        if "games" not in json.loads(path.read_text()):
            raise SystemExit(
                "FAIL: the file was not migrated to the list format, so the "
                "next launch has nowhere to keep the previous games")
        print(f"  ok  migrated in place -> {keys()}")

    print("\nan old-format file with no key offers nothing:")
    with store() as path:
        path.write_text(json.dumps({"console": "n64", "title": "Untitled"}))
        if protocol.read_recent_games():
            raise SystemExit(
                "FAIL: a launch with no game key became a picker entry; its "
                "scope is 'game:' which no launch will ever resolve, so the "
                "mapping captured under it would be inert")
        if protocol.read_last_game() != {}:
            raise SystemExit(
                "FAIL: read_last_game invented a game out of a keyless record")
        print("  ok  no key, no entry")


def check_bad_files() -> None:
    print("\na damaged or missing file costs options, never the screen:")
    cases = {
        "missing": None,
        "empty": "",
        "truncated json": '{"games": [{"key": "n64/x"',
        "not json at all": "<html>404</html>",
        "top-level null": "null",
        "top-level list": '[{"console": "n64", "key": "n64/x"}]',
        "top-level number": "17",
        "top-level string": '"n64/goldeneye"',
        "games is not a list": '{"games": {"key": "n64/x"}}',
        "games is a string": '{"games": "n64/goldeneye"}',
    }
    for name, text in cases.items():
        with store() as path:
            if text is None:
                path.unlink(missing_ok=True)
            else:
                path.write_text(text)
            try:
                recent = protocol.read_recent_games()
                last = protocol.read_last_game()
            except Exception as exc:   # noqa: BLE001 -- that is the bug
                raise SystemExit(
                    f"FAIL: a {name} lastgame.json raised {exc!r}. This "
                    f"decorates the scope picker: a bad file must mean fewer "
                    f"options, not a controller setup screen that dies.")
            if recent != [] or last != {}:
                raise SystemExit(
                    f"FAIL: a {name} lastgame.json produced {recent}, so the "
                    f"picker offers a game scope built from garbage")
        print(f"  ok  {name} -> no games offered")

    print("\njunk entries are dropped, good ones beside them are kept:")
    with store() as path:
        path.write_text(json.dumps({"games": [
            "n64/just-a-string",
            None,
            {"console": "n64", "title": "No Key At All"},
            {"console": "n64", "key": "", "title": "Empty Key"},
            {"console": "n64", "key": "n64/goldeneye", "title": "GoldenEye"},
            {"key": "snes/metroid"},
        ]}))
        recent = protocol.read_recent_games()
        if [game["key"] for game in recent] != ["n64/goldeneye", "snes/metroid"]:
            raise SystemExit(
                f"FAIL: {[g.get('key') for g in recent]} -- an entry with no "
                f"key files a mapping under 'game:', a scope the launcher "
                f"never looks up, so the capture would do nothing")
        if recent[1]["console"] != "" or recent[1]["title"] != "":
            raise SystemExit(
                f"FAIL: missing fields did not default to '' ({recent[1]}); "
                f"the picker reads them straight and would raise on None")
        print(f"  ok  {[g['key'] for g in recent]} kept, 4 junk entries dropped")

    print("\nnon-string fields are coerced rather than handed on raw:")
    with store() as path:
        path.write_text(json.dumps({"games": [
            {"console": 64, "key": "n64/x", "title": ["Weird"]},
        ]}))
        game = protocol.read_recent_games()[0]
        if not all(isinstance(value, str) for value in game.values()):
            raise SystemExit(
                f"FAIL: {game} -- a non-string field reaches the picker's "
                f"label and layout lookup, where it fails at draw time "
                f"instead of here")
        print(f"  ok  {game}")


def check_write_side() -> None:
    print("\nwriting a launch creates the runtime dir and reports the path:")
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "never" / "made" / "lastgame.json"
        original = protocol.last_game_path
        protocol.last_game_path = lambda: path   # type: ignore[assignment]
        try:
            written = protocol.write_last_game("n64", "n64/x", "X")
            if written != path or not path.exists():
                raise SystemExit(
                    "FAIL: write_last_game did not create lastgame.json under "
                    "a fresh XDG_RUNTIME_DIR, so the first launch of a login "
                    "session records nothing and no game scope is offered")
            if json.loads(path.read_text())["games"][0]["key"] != "n64/x":
                raise SystemExit("FAIL: the launch was not recorded on disk")
        finally:
            protocol.last_game_path = original   # type: ignore[assignment]
        print(f"  ok  {path.name} created, path returned")

    print("\nan unrecognised console is still recorded:")
    with store():
        # padmap.launch records a launch whose core it cannot name, because
        # that is exactly the launch whose controls are most likely to have
        # felt wrong.
        protocol.write_last_game("", "unknown/some-rom", "Some ROM")
        game = protocol.read_last_game()
        if game.get("key") != "unknown/some-rom" or game.get("console") != "":
            raise SystemExit(
                f"FAIL: {game} -- a game launched through an unrecognised "
                f"core cannot be offered a per-game mapping at all")
        print(f"  ok  {game}")


def check_scope_options() -> None:
    print("\nthe setup-screen strip: any game, every console, recent games:")
    recent = [("n64", "n64/goldeneye", "GoldenEye 007"),
              ("gamecube", "gamecube/melee", "Melee")]
    options = capture.scope_options(
        scopes=set(), default_layout="gamecube", recent=recent)
    expected = (
        [profiles.SCOPE_UNIVERSAL]
        + [profiles.console_scope(c) for c in layouts.CONSOLES]
        + ["game:n64/goldeneye", "game:gamecube/melee"]
    )
    if ids(options) != expected:
        raise SystemExit(
            f"FAIL: the scope strip is {ids(options)}, expected {expected}. "
            f"A missing console means a pad can never be mapped for it; a "
            f"missing game means the user cannot fix the game they just "
            f"played without launching it again.")
    print(f"  ok  {ids(options)}")

    if options[0].label != "Any game":
        raise SystemExit(
            f"FAIL: the first entry reads {options[0].label!r}; the default "
            f"scope must be the one a hurried user confirms without reading")
    if options[0].layout != "gamecube":
        raise SystemExit(
            "FAIL: 'Any game' is not drawn with the pad's best guess, so the "
            "picker opens showing a controller nobody is holding")
    print(f"  ok  {options[0].label!r} first, drawn as gamecube")

    if any(option.id == "console:generic" for option in options):
        raise SystemExit(
            "FAIL: 'generic' was offered as a console scope. No core ever "
            "reports it, so a mapping filed there could never resolve.")
    print("  ok  generic is a layout but not an offered console")

    n64 = next(o for o in options if o.id == "console:n64")
    if n64.label != "Nintendo 64 games" or n64.layout != "n64":
        raise SystemExit(
            f"FAIL: the N64 console entry is {n64}; the picture and the "
            f"layout the wizard then walks must be the same pad")
    arcade = next(o for o in options if o.id == "console:arcade")
    if arcade.label != "Arcade games":
        raise SystemExit(
            f"FAIL: the arcade scope reads {arcade.label!r}. console_label "
            f"exists so the strip says 'Arcade games' rather than 'Arcade "
            f"stick games', which describes the pad, not the games.")
    print(f"  ok  {n64.label!r} drawn as n64, {arcade.label!r}")

    games = [o for o in options if o.id.startswith("game:")]
    if [g.layout for g in games] != ["n64", "gamecube"]:
        raise SystemExit(
            f"FAIL: recent games draw {[g.layout for g in games]}; a game is "
            f"captured against its console's control set, so drawing another "
            f"console's pad asks for buttons this one does not have")
    if [g.label for g in games] != ["GoldenEye 007", "Melee"]:
        raise SystemExit(
            f"FAIL: game entries read {[g.label for g in games]}")
    print(f"  ok  {[g.label for g in games]} drawn as their own consoles")

    print("\nevery entry appears once, even a game replayed twice:")
    options = capture.scope_options(
        scopes=set(), default_layout="",
        recent=[("n64", "n64/goldeneye", "GoldenEye"),
                ("gamecube", "gamecube/melee", "Melee"),
                ("n64", "n64/goldeneye", "GoldenEye")])
    games = [o.id for o in options if o.id.startswith("game:")]
    if games != ["game:n64/goldeneye", "game:gamecube/melee"]:
        raise SystemExit(
            f"FAIL: {games} -- a game offered twice wastes a slot on a strip "
            f"worked one step at a time from the pad, and the second copy "
            f"means the same thing as the first")
    if len(set(ids(options))) != len(ids(options)):
        raise SystemExit(
            f"FAIL: duplicate ids on the strip ({ids(options)}); two entries "
            f"that file a capture in the same place cannot both be right")
    print(f"  ok  {games}")

    print("\na recent entry with no key is not offered:")
    options = capture.scope_options(
        scopes=set(), default_layout="",
        recent=[("n64", "", "Nameless"), ("n64", "n64/x", "X")])
    games = [o.id for o in options if o.id.startswith("game:")]
    if games != ["game:n64/x"]:
        raise SystemExit(
            f"FAIL: {games} -- 'game:' with no key is a scope no launch will "
            f"ever look up, so a mapping captured under it does nothing")
    print(f"  ok  {games}")

    print("\na game with no title falls back to its key:")
    options = capture.scope_options(
        scopes=set(), default_layout="",
        recent=[("n64", "n64/unnamed-rom", "")])
    entry = options[-1]
    if entry.label != "n64/unnamed-rom":
        raise SystemExit(
            f"FAIL: an untitled game reads {entry.label!r}; a blank entry on "
            f"the strip is one the user cannot tell apart from any other")
    print(f"  ok  {entry.label!r}")

    print("\nno recent games at all is not an error:")
    for label, value in (("recent=None", None), ("recent=[]", [])):
        options = capture.scope_options(scopes=set(), default_layout="",
                                        recent=value)
        if any(o.id.startswith("game:") for o in options):
            raise SystemExit(
                f"FAIL: {label} still offered a per-game scope with no game "
                f"to attach it to")
        if len(options) != 1 + len(layouts.CONSOLES):
            raise SystemExit(
                f"FAIL: {label} gave {len(options)} entries; the consoles must "
                f"still be offered when nothing has been played")
    print("  ok  consoles only, both spellings of 'nothing played'")


def check_mapped_marks() -> None:
    print("\nscopes that already hold a capture are marked:")
    held = {profiles.SCOPE_UNIVERSAL, "console:n64", "game:gamecube/melee"}
    options = capture.scope_options(
        scopes=held, default_layout="n64",
        recent=[("n64", "n64/goldeneye", "GoldenEye"),
                ("gamecube", "gamecube/melee", "Melee")])
    marked = {o.id for o in options if o.mapped}
    if marked != held:
        raise SystemExit(
            f"FAIL: marked {sorted(marked)}, this pad has captures under "
            f"{sorted(held)}. Re-mapping a scope replaces what is stored, and "
            f"without the mark there is no way to see which one that destroys.")
    print(f"  ok  {sorted(marked)} marked, {len(options) - len(marked)} clear")

    unmapped = capture.scope_options(scopes=set(), default_layout="n64",
                                     recent=[("n64", "n64/x", "X")])
    if any(o.mapped for o in unmapped):
        raise SystemExit(
            "FAIL: a controller with nothing stored shows scopes as already "
            "mapped, so every entry looks destructive and the mark means "
            "nothing")
    print("  ok  a pad with no captures marks nothing")

    payload = [o.to_json() for o in options]
    if not payload[0]["mapped"] or payload[0]["layout"]["id"] != "n64":
        raise SystemExit(
            "FAIL: the mark or the drawn layout is lost on the way to the "
            "front-end, which is the only place the user ever sees it")
    print("  ok  the mark and the layout survive to_json")


def check_game_scope_options() -> None:
    print("\nasked from the library, the question is two entries wide:")
    pair = capture.game_scope_options(
        console="n64", key="n64/goldeneye-007-usa",
        title="GoldenEye 007 (USA)", scopes=set())
    if set(ids(pair)) != {"console:n64", "game:n64/goldeneye-007-usa"}:
        raise SystemExit(
            f"FAIL: offered {ids(pair)}, wanted exactly the console and the "
            f"game. Both facts are known here, so there is nothing to scroll "
            f"past and nothing to get wrong.")
    if pair[0].id != "console:n64":
        raise SystemExit(
            "FAIL: the game is offered before the console. The console is the "
            "answer that is right more often -- a pad needing a remap for one "
            "N64 game usually needs it for all of them -- and the first entry "
            "is what a hurried user confirms.")
    if [o.label for o in pair] != ["Nintendo 64 games", "GoldenEye 007 (USA)"]:
        raise SystemExit(
            f"FAIL: the two entries read {[o.label for o in pair]}")
    print(f"  ok  {[o.label for o in pair]}")

    if [o.layout for o in pair] != ["n64", "n64"]:
        raise SystemExit(
            f"FAIL: drawn as {[o.layout for o in pair]}. A mapping for one "
            f"N64 game is still a capture of the N64 control set, so both "
            f"entries must show -- and the wizard must then walk -- that pad.")
    print("  ok  both drawn with the console layout")

    print("\nthe existing capture is marked here too:")
    pair = capture.game_scope_options(
        "n64", "n64/goldeneye", "GoldenEye", {"console:n64"})
    if not pair[0].mapped or pair[1].mapped:
        raise SystemExit(
            f"FAIL: marks are {[o.mapped for o in pair]}; confirming the "
            f"console entry would overwrite the N64 mapping this pad already "
            f"has with no warning that it existed")
    pair = capture.game_scope_options(
        "n64", "n64/goldeneye", "GoldenEye", {"game:n64/goldeneye"})
    if pair[0].mapped or not pair[1].mapped:
        raise SystemExit(
            f"FAIL: marks are {[o.mapped for o in pair]} when only the "
            f"per-game capture exists")
    print("  ok  each mark follows the scope that actually holds a capture")

    print("\nan untitled game is still named on its entry:")
    pair = capture.game_scope_options("snes", "snes/unknown-rom", "", set())
    if pair[1].label != "snes/unknown-rom":
        raise SystemExit(
            f"FAIL: the game entry reads {pair[1].label!r}; a blank second "
            f"entry gives the user nothing to choose between")
    print(f"  ok  {pair[1].label!r}")

    print("\nno console means no console scope:")
    # `profiles.scope_order` only ever looks up "console:<id>" for a console
    # the launcher could name, so an entry built from an empty console id is a
    # scope nothing will ever resolve: the user maps their pad, the capture is
    # filed, and nothing changes in the game. The front-end can genuinely send
    # this -- the exporter omits x-console for a collection whose core it does
    # not recognise, while still emitting x-gamekey.
    orphan = capture.game_scope_options("", "unknown/rom", "Mystery", set())
    if any(o.id.startswith("console:") for o in orphan):
        raise SystemExit(
            f"FAIL: {ids(orphan)} -- a console scope was offered for a game "
            f"whose console padmap cannot name. Nothing ever looks that scope "
            f"up, so the mapping the user captures under it is inert.")
    print(f"  ok  {ids(orphan)} -- nothing filed under 'console:'")

    if capture.game_scope_options("", "", "Mystery", set()):
        raise SystemExit(
            "FAIL: a scope was offered for a game with neither a console nor "
            "a key. The daemon relies on an empty list here to say 'no "
            "console known for this game' instead of recording something "
            "that can never be resolved.")
    print("  ok  neither fact known -> no options, so the daemon can say so")

    print("\na console with no game key offers the console alone:")
    only = capture.game_scope_options("n64", "", "", set())
    if ids(only) != ["console:n64"]:
        raise SystemExit(
            f"FAIL: {ids(only)} -- a game with no key can still be mapped "
            f"for its console, which is the useful half of the answer")
    print(f"  ok  {ids(only)}")

    print("\nan unknown console still draws a pad rather than raising:")
    odd = capture.game_scope_options("dreamcast", "dc/soulcalibur",
                                     "Soulcalibur", set())
    drawn = [o.to_json()["layout"]["id"] for o in odd]
    if drawn != ["generic", "generic"]:
        raise SystemExit(
            f"FAIL: an id no layout matches drew {drawn}; refusing to show a "
            f"wizard at all is a worse answer than showing the ordinary pad")
    print(f"  ok  console:dreamcast offered, drawn as {drawn[0]}")


def check_end_to_end() -> None:
    print("\nwhat the daemon actually builds from a session's launches:")
    with store():
        # Exactly the sequence _begin_scope_choice goes through: read the
        # launches padmap-play recorded, hand them to scope_options.
        protocol.write_last_game("n64", "n64/goldeneye", "GoldenEye 007")
        protocol.write_last_game("gamecube", "gamecube/melee", "Melee")
        protocol.write_last_game("n64", "n64/goldeneye", "GoldenEye 007")
        options = capture.scope_options(
            scopes={"console:n64"},
            default_layout="gamecube",
            recent=[(game["console"], game["key"], game["title"])
                    for game in protocol.read_recent_games()],
        )
        games = [(o.id, o.label, o.layout) for o in options
                 if o.id.startswith("game:")]
        if games != [("game:n64/goldeneye", "GoldenEye 007", "n64"),
                     ("game:gamecube/melee", "Melee", "gamecube")]:
            raise SystemExit(
                f"FAIL: the strip a real session produces is {games}; the "
                f"game just replayed must be first, each game once, each "
                f"drawn as its own console")
        if not next(o for o in options if o.id == "console:n64").mapped:
            raise SystemExit(
                "FAIL: the pad's existing N64 capture is unmarked on the "
                "strip the daemon sends")
        print(f"  ok  {[g[1] for g in games]}, N64 console marked")

        # The library route ignores the recent list entirely: any game there
        # can be mapped for, played this session or not.
        last = protocol.read_last_game()
        pair = capture.game_scope_options(
            "snes", "snes/never-played", "Never Played", {"console:n64"})
        if ids(pair) != ["console:snes", "game:snes/never-played"]:
            raise SystemExit(
                f"FAIL: {ids(pair)} -- the library route must not depend on "
                f"the game having been launched this session")
        if any(o.mapped for o in pair):
            raise SystemExit(
                "FAIL: an unrelated console's capture marked the SNES entries")
        if last["key"] != "n64/goldeneye":
            raise SystemExit(
                "FAIL: asking the library question disturbed the recorded "
                "last game, which the daemon uses to pick the RetroArch "
                "profile context on its next republish")
        print(f"  ok  {[o.label for o in pair]} regardless of what was played")


def main() -> int:
    check_order_and_dedup()
    check_cap()
    check_old_format()
    check_bad_files()
    check_write_side()
    check_scope_options()
    check_mapped_marks()
    check_game_scope_options()
    check_end_to_end()

    print("\na game whose console is unknown offers no scope at all:")
    # The exporter writes x-gamekey for every game but omits x-console when
    # the collection's core is not one padmap recognises, so a front-end can
    # really send a key with no console. Both entries are captured against the
    # console's control set, so without one there is nothing coherent to
    # offer -- and the daemon relies on an empty list here to say "no console
    # known for this game" rather than record a mapping under a scope it
    # cannot draw.
    if capture.game_scope_options("", "unknown/mystery", "Mystery", set()):
        raise SystemExit(
            "FAIL: offered a scope for a game with no console -- the daemon "
            "would record a capture it cannot draw instead of saying it does "
            "not know the console")
    if capture.game_scope_options("", "", "", set()):
        raise SystemExit("FAIL: offered a scope knowing neither fact")
    print("  ok  no console, no options")

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
