#!/usr/bin/env python3
"""The library and its generation, fed content nobody would type on purpose.

Stories S22 (`padmap export-pegasus` regenerates collections with art and
console ids) and S23 (games are browsed by console tab, with box art and
non-working MAME titles marked).

Everything the library screen shows is read out of one generated file per
collection, and that file is produced by string concatenation from three
untrusted sources: a RetroArch `.lpl` the user can hand-edit, a MAME title
table built from a 43MB XML by regex, and whatever filenames happen to be in
the thumbnail tree. None of those are validated on the way in, and the output
is a line-oriented format where one stray newline turns a game's *name* into
somebody else's *key*.

So the checks below are not "does a normal playlist render". They are:

  * every rule the rendered file has to obey, applied as a set to every
    hostile input -- one game block per entry, a `game:` and a `file:` line in
    each, `x-console` and `x-gamekey` present exactly when derivable, and no
    line that Pegasus's parser would reject. Modelled with a parser rather
    than with substring checks, because the failure that matters is a value
    that reads as a key;
  * art referenced only when the file is really there, since Library.qml
    picks its layout from `assets.boxFront != ""` -- a path that does not
    resolve is an empty frame, not a fallback;
  * the damaged-input paths beside all of that: a playlist whose JSON is the
    wrong shape, a title table that was truncated mid-write, a launch line
    that will not lex.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \\
        nix develop --command python3 tools/check_hostile_library.py
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path

# Redirect every XDG root *before* padmap is imported. A live daemon runs on
# this machine and export-pegasus writes into XDG_DATA_HOME and
# XDG_CONFIG_HOME; a test that regenerated the real collections would be far
# more expensive than the bugs it looks for.
SANDBOX = Path(tempfile.mkdtemp(prefix="padmap-hostile-library-"))
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    _dir = SANDBOX / _var.lower()
    _dir.mkdir(parents=True, exist_ok=True)
    _dir.chmod(0o700)
    os.environ[_var] = str(_dir)

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import pegasus, profiles, titles  # noqa: E402
from padmap.titles import Title  # noqa: E402

# A fixed launcher path, so every rendered launch line is predictable and the
# stable symlink is installed inside the sandbox rather than over the user's.
PLAY = str(SANDBOX / "store" / "padmap-play")
os.environ[pegasus.ENV_PLAY] = PLAY

ARCADE_CORE = "/cores/mame2010_libretro.so"
N64_CORE = "/cores/mupen64plus_next_libretro.so"
# A core padmap has no layout for: the collection must come out with no
# x-console, and therefore with no per-game key either.
UNKNOWN_CORE = "/cores/vice_x64_libretro.so"


# ---------------------------------------------------------------------------
# reporting


def ok(what: str) -> None:
    print(f"  ok  {what}")


def need(condition: bool, complaint: str) -> None:
    if not condition:
        raise SystemExit(f"FAIL: {complaint}")


def same(what: str, got, want) -> None:
    if got != want:
        raise SystemExit(f"FAIL: {what} -- got {got!r}, wanted {want!r}")
    ok(what)


def gap(what: str) -> None:
    """A defect this file deliberately does not assert away.

    Printed rather than raised so the suite stays green against current
    source; the maintainer turns each of these into an assertion with the fix.
    """
    print(f"  gap: {what}")


def note(what: str) -> None:
    print(f"  note: {what}")


# ---------------------------------------------------------------------------
# a model of Pegasus's own parser


def parse_metadata(
    text: str,
) -> tuple[dict[str, list[str]], list[dict[str, list[str]]], list[str]]:
    """Read a rendered file the way Pegasus reads it.

    The format is line-oriented: `key: value`, a line starting with whitespace
    continues the previous value, `#` is a comment, and `game:` opens a new
    entry. Anything else is a syntax error the parser reports and drops.

    Modelled here instead of checking for substrings because the interesting
    failure is not a missing line, it is a *value* that the parser reads as
    the next key -- which no `"x" in text` assertion can see.
    """
    collection: dict[str, list[str]] = {}
    games: list[dict[str, list[str]]] = []
    junk: list[str] = []
    current = collection
    last_key: str | None = None

    for raw in text.split("\n"):
        if not raw.strip():
            last_key = None
            continue
        if raw.lstrip().startswith("#"):
            continue
        if raw[0] in " \t":
            if last_key is None:
                junk.append(raw)
            else:
                current[last_key][-1] += " " + raw.strip()
            continue
        if ":" not in raw:
            junk.append(raw)
            last_key = None
            continue
        key, value = raw.split(":", 1)
        key = key.strip()
        if key == "game":
            current = {}
            games.append(current)
        current.setdefault(key, []).append(value.strip())
        last_key = key

    return collection, games, junk


def audit(collection: pegasus.Collection) -> list[str]:
    """Every rule the generated file must obey, as a list of what it broke.

    An empty list is the only acceptable answer for any playlist, however
    hostile, because Pegasus has no error reporting a user will ever see: a
    block it rejects is a game that silently is not in the library.
    """
    text = pegasus.render(collection)
    header, games, junk = parse_metadata(text)
    bad = [f"line {line!r} is not key, value or continuation" for line in junk]

    if header.get("collection") != [collection.name]:
        bad.append(f"collection header is {header.get('collection')!r}")
    if len(header.get("launch", [])) != 1:
        bad.append(f"{len(header.get('launch', []))} launch lines for the set")
    if len(games) != len(collection.entries):
        bad.append(
            f"{len(games)} game blocks for {len(collection.entries)} entries")

    for entry, block in zip(collection.entries, games):
        where = repr(entry.path)
        if block.get("game") != [entry.title.strip()]:
            bad.append(f"{where}: game name is {block.get('game')!r}")
        if not block.get("game", [""])[0]:
            bad.append(f"{where}: game block has no name")
        if block.get("file") != [entry.path.strip()]:
            bad.append(f"{where}: file line is {block.get('file')!r}")
        # x-console and x-gamekey are what the theme sends back when the user
        # asks to map a pad for this game or this console (S9, S10). A block
        # carrying one that was not derivable files the capture under a scope
        # nothing ever looks up.
        wanted_console = [collection.console] if collection.console else None
        if block.get("x-console") != wanted_console:
            bad.append(f"{where}: x-console is {block.get('x-console')!r}")
        wanted_key = [entry.key] if entry.key else None
        if block.get("x-gamekey") != wanted_key:
            bad.append(f"{where}: x-gamekey is {block.get('x-gamekey')!r}")
        if entry.status:
            if block.get("x-mame-status") != [entry.status]:
                bad.append(f"{where}: driver grade lost")
        elif "x-mame-status" in block:
            bad.append(f"{where}: invented a driver grade")
        for key, values in block.items():
            if not key.startswith("assets."):
                continue
            for value in values:
                # Library.qml treats a non-empty boxFront as "this game has
                # art" and switches to the grid on it. A path that does not
                # resolve is a blank tile with no fallback.
                if not value:
                    bad.append(f"{where}: empty {key}")
                elif not Path(value).is_file():
                    bad.append(f"{where}: {key} points at no file: {value}")

    return bad


# ---------------------------------------------------------------------------
# fixtures


def playlist(path: Path, core: str, items: list, core_name: str = "") -> Path:
    path.write_text(json.dumps({
        "version": "1.5",
        "default_core_path": core,
        "default_core_name": core_name,
        "scan_file_exts": "zip",
        "items": items,
    }, ensure_ascii=False), encoding="utf-8")
    return path


def item(path: str, label: str = "") -> dict:
    return {"path": path, "label": label, "core_path": "DETECT",
            "core_name": "DETECT", "crc32": "00000000|crc", "db_name": "x.lpl"}


def read(path: Path, table: dict[str, Title] | None = None):
    """read_playlist, but never raising -- damaged input must read as absent."""
    try:
        return pegasus.read_playlist(path, table)
    except Exception as error:  # noqa: BLE001 - the point is what escapes
        return error


# ---------------------------------------------------------------------------


def check_empty_and_missing(work: Path) -> None:
    print("\nS22: a playlist with nothing in it")

    empty = playlist(work / "empty.lpl", ARCADE_CORE, [],
                     "Arcade (MAME 2010)")
    collection = pegasus.read_playlist(empty)
    same("an empty playlist is still a collection, with no games",
         (collection.name, collection.entries), ("Empty", []))
    same("its rendered file has no game blocks",
         parse_metadata(pegasus.render(collection))[1], [])
    same("and it obeys every rule anyway", audit(collection), [])

    out = work / "out-empty"
    results, dirs = pegasus.export(work, out)
    same("export writes nothing for it", (results, dirs), ([], []))
    same("so no empty directory is offered to game_dirs.txt",
         sorted(p.name for p in out.iterdir()) if out.is_dir() else [], [])

    print("\nS22: items with nothing to launch")
    mixed = playlist(work / "mixed.lpl", ARCADE_CORE, [
        {"label": "No Path At All"},
        item("", "Empty Path"),
        item("/roms/arcade/pacman.zip", "Pac-Man"),
    ], "Arcade (MAME 2010)")
    collection = pegasus.read_playlist(mixed)
    same("an item with no path is dropped, not rendered as a nameless game",
         [(e.title, e.path) for e in collection.entries],
         [("Pac-Man", "/roms/arcade/pacman.zip")])
    same("the survivor still renders cleanly", audit(collection), [])


def check_directory_rom(work: Path) -> None:
    print("\nS22: a ROM path that is a directory")

    romdir = work / "roms" / "psx" / "Final Fantasy VII"
    romdir.mkdir(parents=True)
    (romdir / "disc1.bin").write_text("x")

    source = playlist(work / "dir.lpl", ARCADE_CORE, [
        item(str(romdir)),
        item(str(romdir) + "/", "Trailing Slash"),
    ], "Arcade (MAME 2010)")
    collection = pegasus.read_playlist(source)
    same("a directory launches like any other file", audit(collection), [])
    same("with no label, the directory name is the title",
         collection.entries[0].title, "Final Fantasy VII")
    same("a trailing slash does not blank the game key",
         collection.entries[1].key, "arcade/final-fantasy-vii")


def check_hostile_labels(work: Path) -> None:
    print("\nS22: labels a filename can legally contain")

    long_label = "L" + "o" * 497 + "ng"
    labels = [
        ("comma", "Sonic, Knuckles & Tails"),
        ("colon", "Bomberman: The Second Attack!"),
        ("leading dash", "-- unnamed prototype --"),
        ("leading hash", "#1 Racer"),
        ("leading space", "   spaced out"),
        ("500 characters", long_label),
        ("percent and brace", "100% {file.path} Complete"),
        ("quote", 'He said "hi"'),
        ("backslash", "C:\\GAMES\\DOOM"),
        ("equals and bracket", "Zelda [!] = best"),
        ("tab inside", "Double\tDragon"),
        ("unicode", "Pokémon Stadium 2 — 大戦略"),
        ("right to left", "شارع المقاتل"),
        ("emoji", "Puzzle 🧩 Bobble"),
    ]
    source = playlist(
        work / "labels.lpl", ARCADE_CORE,
        [item(f"/roms/arcade/g{i}.zip", text) for i, (_, text) in
         enumerate(labels)],
        "Arcade (MAME 2010)")
    collection = pegasus.read_playlist(source)
    same("every hostile label renders a file that still parses",
         audit(collection), [])

    _, blocks, _ = parse_metadata(pegasus.render(collection))
    for (kind, text), block in zip(labels, blocks):
        same(f"{kind} survives as a name, not as syntax",
             block["game"], [text.strip()])
    same("a 500-character label is not truncated",
         len(blocks[5]["game"][0]), 500)

    print("\nS22: labels that collide with the format's own keys")
    keys = ["game", "file", "launch", "collection", "sort-by", "developer",
            "assets.boxFront", "x-console", "x-gamekey"]
    source = playlist(
        work / "keys.lpl", ARCADE_CORE,
        [item(f"/roms/arcade/k{i}.zip", k) for i, k in enumerate(keys)],
        "Arcade (MAME 2010)")
    collection = pegasus.read_playlist(source)
    same("a game named after a metadata key still renders cleanly",
         audit(collection), [])
    _, blocks, _ = parse_metadata(pegasus.render(collection))
    same("a game called \"file\" does not become the file line",
         (blocks[1]["game"], blocks[1]["file"]),
         (["file"], ["/roms/arcade/k1.zip"]))
    same("a game called \"launch\" does not add a launch line",
         "launch" in blocks[2], False)
    same("a game called \"assets.boxFront\" claims no art",
         [k for k in blocks[6] if k.startswith("assets.")], [])


def check_newline_injection(work: Path) -> None:
    print("\nS22: a label containing a newline")

    source = playlist(work / "newline.lpl", ARCADE_CORE, [
        item("/roms/arcade/evil.zip", "Evil\nlaunch: /bin/sh -c 'rm -rf'"),
        item("/roms/arcade/two.zip", "Two\nLine Title"),
        item("/roms/arcade/carriage.zip", "Carriage\rReturn"),
    ], "Arcade (MAME 2010)")
    collection = read(source)
    need(isinstance(collection, pegasus.Collection),
         "a label with a newline in it stopped the collection being read")
    same("the entry is kept, not dropped", len(collection.entries), 3)
    same("the newline reaches the entry verbatim",
         collection.entries[0].title.count("\n"), 1)
    same("the ROM path, not the label, decides the game key",
         collection.entries[0].key, "arcade/evil")

    _, blocks, junk = parse_metadata(pegasus.render(collection))
    if len(blocks) == 3 and not junk and all(
            b.get("game") == [e.title.strip()]
            for b, e in zip(blocks, collection.entries)):
        raise SystemExit(
            "FAIL: newline injection was fixed -- turn this into an assertion")
    gap("a newline in a label is emitted raw, so the rest of the label is "
        "read as metadata: the first game block gained "
        f"{sorted(set(blocks[0]) - {'game'})} -- including a launch command "
        "Pegasus will run for that game")
    gap(f"a label whose second line has no colon is a syntax error instead: "
        f"{junk!r}")
    gap("a carriage return is emitted raw inside a value too; whether the "
        "reader splits on it is not something render() gets to decide")

    print("\nS22: a newline arriving from the MAME title table")
    xml = work / "wrapped.xml"
    xml.write_text(
        '<mame><game name="evil"><description>Wrapped\nOver Two Lines'
        "</description></game></mame>", encoding="utf-8")
    table = titles.parse_mame_xml(xml)
    same("a description wrapped over two lines keeps both",
         table["evil"].title, "Wrapped\nOver Two Lines")
    gap("so the newline above needs no hand-edited playlist: any MAME XML "
        "with a wrapped <description> feeds one straight into render()")

    print("\nS22: a collection-level field containing a newline")
    source = playlist(work / "ext.lpl", ARCADE_CORE,
                      [item("/roms/arcade/a.zip", "A")], "Arcade (MAME 2010)")
    source.write_text(json.dumps({
        "default_core_path": ARCADE_CORE,
        "default_core_name": "Arcade (MAME 2010)",
        "scan_file_exts": "zip\nlaunch: /bin/false",
        "items": [item("/roms/arcade/a.zip", "A")],
    }), encoding="utf-8")
    collection = pegasus.read_playlist(source)
    header, _, _ = parse_metadata(pegasus.render(collection))
    same("the real launch line is still emitted",
         any(PLAY in line or str(pegasus.player_link()) in line
             for line in header["launch"]), True)
    if len(header["launch"]) == 1:
        raise SystemExit(
            "FAIL: scan_file_exts no longer injects -- assert it instead")
    gap("scan_file_exts is copied verbatim, so a newline in it prepends a "
        f"second launch line ({header['launch'][0]!r}); Pegasus keeps the "
        "first launch command it sees, so that one wins for the whole set")


def check_duplicates(work: Path) -> None:
    print("\nS22: duplicate labels and duplicate paths")

    source = playlist(work / "dups.lpl", N64_CORE, [
        item("/roms/n64/mario64.z64", "Super Mario 64"),
        item("/roms/n64/mario64 (copy).z64", "Super Mario 64"),
        item("/roms/n64/goldeneye.z64", "GoldenEye"),
        item("/roms/n64/goldeneye.z64", "GoldenEye 007"),
    ], "Nintendo - Nintendo 64 (Mupen64Plus-Next)")
    collection = pegasus.read_playlist(source)
    same("nothing is deduplicated away", len(collection.entries), 4)
    same("duplicates render a clean file", audit(collection), [])

    keys = [e.key for e in collection.entries]
    same("two labels for one path share one game key",
         keys[2] == keys[3], True)
    same("one label over two paths does not",
         keys[0] != keys[1], True)
    note("a repeated path is two library rows launching the same ROM, and "
         "both carry the same x-gamekey -- a per-game mapping made on either "
         "row applies to both, which is right, but the row is duplicated")


def check_console_and_key(work: Path) -> None:
    print("\nS22: x-console and x-gamekey appear exactly when derivable")

    known = playlist(work / "known.lpl", N64_CORE,
                     [item("/roms/n64/Banjo-Kazooie (U) [!].z64", "Banjo")],
                     "Nintendo - Nintendo 64 (Mupen64Plus-Next)")
    collection = pegasus.read_playlist(known)
    same("a core padmap knows yields a console", collection.console, "n64")
    _, blocks, _ = parse_metadata(pegasus.render(collection))
    same("and the block carries it", blocks[0].get("x-console"), ["n64"])
    same("with the key launch will compute for the same ROM",
         blocks[0].get("x-gamekey"),
         [profiles.game_key("n64", "/roms/n64/Banjo-Kazooie (U) [!].z64")])

    unknown = playlist(work / "c64.lpl", UNKNOWN_CORE,
                       [item("/roms/c64/elite.d64", "Elite")],
                       "Commodore - 64 (VICE x64)")
    collection = pegasus.read_playlist(unknown)
    same("a core padmap does not know yields no console",
         collection.console, "")
    _, blocks, _ = parse_metadata(pegasus.render(collection))
    same("so no x-console is invented", "x-console" in blocks[0], False)
    # The defect the last sweep fixed in game_scope_options was reachable
    # exactly because render emitted a key with no console. It must not.
    same("and no x-gamekey either, or the theme offers a scope with no "
         "console", "x-gamekey" in blocks[0], False)

    print("\nS22: a name no game key can be made from")
    source = playlist(work / "rtl.lpl", N64_CORE, [
        item("/roms/n64/شارع.z64", "شارع المقاتل"),
        item("/roms/n64/!!!.z64", "Punctuation Only"),
    ], "Nintendo - Nintendo 64 (Mupen64Plus-Next)")
    collection = pegasus.read_playlist(source)
    same("a filename with nothing sluggable gets no key",
         [e.key for e in collection.entries], ["", ""])
    same("the block is still complete without one", audit(collection), [])
    _, blocks, _ = parse_metadata(pegasus.render(collection))
    same("the console scope is still offered",
         blocks[0].get("x-console"), ["n64"])
    note("a ROM whose name is entirely non-ASCII can be mapped for its "
         "console but never for itself (S10), because game_key normalises to "
         "[a-z0-9] and is left with nothing")


def check_empty_title(work: Path) -> None:
    print("\nS22: an entry with no name at all")

    source = playlist(work / "noname.lpl", ARCADE_CORE, [item("/", "")],
                      "Arcade (MAME 2010)")
    collection = pegasus.read_playlist(source)
    same("the entry survives with an empty title",
         (collection.entries[0].title, collection.entries[0].path), ("", "/"))
    _, blocks, junk = parse_metadata(pegasus.render(collection))
    same("the file line is still right", blocks[0].get("file"), ["/"])
    same("no line is unparseable", junk, [])
    problems = audit(collection)
    if not problems:
        raise SystemExit(
            "FAIL: the nameless game is handled now -- assert it instead")
    gap(f"a playlist item with path \"/\" and no label renders `game:` with "
        f"no value ({problems[0]}); Pegasus needs a name, so the entry is "
        "either dropped or shown blank")


def check_wrong_shapes(work: Path) -> None:
    print("\nS22: playlists whose JSON is the wrong shape")

    clearly_absent = {
        "not JSON at all": "this is a RetroArch 1.0 playlist\n/roms/a.zip\n",
        "truncated mid-object": '{"items": [{"path": "/roms/a.zip"',
        "empty file": "",
        "a top-level list": '[{"path": "/roms/a.zip"}]',
        "a top-level string": '"items"',
        "an object with no items": '{"default_core_path": "/c/x.so"}',
    }
    for label, text in clearly_absent.items():
        path = work / "shape.lpl"
        path.write_text(text)
        same(f"{label} reads as absent", read(path), None)

    survives = {
        "items is null": '{"items": null}',
        "items is a string": '{"items": "pacman"}',
        "items is an object": '{"items": {"path": "/roms/a.zip"}}',
        "an item is a string": '{"items": ["pacman"]}',
        "an item is null": '{"items": [null]}',
        "a path that is a number": '{"items": [{"path": 5}]}',
        "a label that is a list": '{"items": [{"path": "/a.zip",'
                                 ' "label": ["x"]}]}',
        "default_core_path is a number":
            '{"items": [{"path": "/a.zip"}], "default_core_path": 7}',
        "default_core_name is a number":
            '{"items": [], "default_core_name": 7}',
    }
    raised: list[str] = []
    accepted: list[str] = []
    for label, text in survives.items():
        path = work / "shape.lpl"
        path.write_text(text)
        result = read(path)
        if isinstance(result, Exception):
            raised.append(f"{label} -> {type(result).__name__}")
        elif isinstance(result, pegasus.Collection) and result.entries:
            accepted.append(f"{label} -> title {result.entries[0].title!r}")
    if not raised and not accepted:
        raise SystemExit(
            "FAIL: hostile playlist shapes are handled -- assert None instead")
    gap(f"{len(raised)} wrong-shaped playlists raise out of read_playlist "
        f"instead of reading as absent ({'; '.join(raised)}); it catches "
        "(OSError, ValueError) and checks that the top level is a dict with "
        "an \"items\" key, but never that items is a list of objects -- the "
        "same gap profiles.load had before this sweep")
    if accepted:
        gap(f"and {len(accepted)} are taken as real library content "
            f"({'; '.join(accepted)}): a label that is not a string becomes "
            "the game's title, and render() writes its Python repr into the "
            "metadata")

    # The one shape that is not hostile at all: RetroArch omits "label" for
    # entries it never resolved, and those must still be listed.
    path = work / "shape.lpl"
    path.write_text(json.dumps({
        "default_core_path": ARCADE_CORE,
        "items": [{"path": "/roms/arcade/dkong.zip"}]}))
    collection = pegasus.read_playlist(path)
    same("an item with a path and no label at all falls back to its basename",
         [(e.title, e.key) for e in collection.entries],
         [("dkong", "arcade/dkong")])

    print("\nS22: one damaged playlist must not cost the others")
    bad_dir = work / "half-bad"
    bad_dir.mkdir()
    playlist(bad_dir / "n64.lpl", N64_CORE,
             [item("/roms/n64/mario.z64", "Mario")],
             "Nintendo - Nintendo 64 (Mupen64Plus-Next)")
    (bad_dir / "arcade.lpl").write_text('{"items": null}')
    try:
        results, _ = pegasus.export(bad_dir, work / "out-half")
        same("export skips the damaged one and writes the rest",
             [r[0] for r in results], ["Nintendo 64"])
    except Exception as error:  # noqa: BLE001
        gap(f"export aborts on the first damaged playlist "
            f"({type(error).__name__}), so `padmap export-pegasus` "
            "regenerates nothing at all -- including collections that were "
            "fine -- and the library keeps whatever was last written")
    same("the good playlist itself is fine",
         [e.title for e in pegasus.read_playlist(bad_dir / "n64.lpl").entries],
         ["Mario"])


def check_display_name(work: Path) -> None:
    print("\nS22: naming a collection from a mangled core name")

    cases = [
        # The PlayStation case is the one that matters: the platform is what
        # goes on the console tab (S23), and taking the whole string would put
        # the vendor and the core name up there with it.
        ("psx", "Sony - PlayStation (Beetle PSX HW)", "PlayStation"),
        ("psx", "Sony - PlayStation", "PlayStation"),
        # No " - " to split on, so the file name is all there is. The real
        # arcade playlist is exactly this shape.
        ("mame_2010", "Arcade (MAME 2010)", "Mame 2010"),
        ("nes", "Nintendo - ", "Nes"),
        ("odd_stem", " - (Core)", "Odd Stem"),
        ("weird", "", "Weird"),
        ("下一个", "Nintendo - ", "下一个"),
    ]
    for stem, core_name, wanted in cases:
        source = playlist(work / f"{stem}.lpl", ARCADE_CORE, [], core_name)
        same(f"{core_name!r} in {stem}.lpl names the collection {wanted!r}",
             pegasus.read_playlist(source).name, wanted)

    source = work / "no_name.lpl"
    source.write_text('{"items": []}')
    same("a playlist with no core name falls back to its stem",
         pegasus.read_playlist(source).name, "No Name")


def check_launch_lines(work: Path) -> None:
    print("\nS22: the launch line")

    same("DETECT is not passed to RetroArch as a core",
         pegasus.launch_line(pegasus.DETECT),
         f"{pegasus.player_link()} \"{{file.path}}\"")
    same("an empty core is not passed either",
         pegasus.launch_line(""),
         f"{pegasus.player_link()} \"{{file.path}}\"")
    spaced = pegasus.launch_line("/nix/store/a b/cores/mame 2010.so")
    same("a core path with spaces is quoted", spaced,
         f"{pegasus.player_link()} -L '/nix/store/a b/cores/mame 2010.so'"
         " \"{file.path}\"")
    same("the ROM placeholder is always the last argument",
         spaced.endswith('"{file.path}"'), True)
    same("launch lines go through the stable symlink, not the store path",
         PLAY in pegasus.launch_line(ARCADE_CORE), False)


def check_stale_collections(work: Path) -> None:
    print("\nS22: finding collections that name a dead launcher")

    config = Path(os.environ["XDG_CONFIG_HOME"]) / "pegasus-frontend"
    config.mkdir(parents=True, exist_ok=True)
    dirs_file = config / "game_dirs.txt"
    games = work / "installed"
    games.mkdir()
    metadata = games / "metadata.pegasus.txt"
    dirs_file.write_text(f"{games}\n\n   \n/does/not/exist\n")

    def stale():
        try:
            return pegasus.stale_collections()
        except Exception as error:  # noqa: BLE001
            return error

    current = pegasus.player_command()
    metadata.write_text(
        f"collection: X\nlaunch: {current} \"{{file.path}}\"\n\n"
        "game: A\nfile: /roms/a.zip\n")
    same("a collection pointing at the stable link is not stale", stale(), [])

    metadata.write_text(
        "collection: X\nlaunch: /nix/store/old-padmap/bin/padmap-play"
        " \"{file.path}\"\n\ngame: A\nfile: /roms/a.zip\n")
    same("one pointing at a store path from a previous build is",
         stale(), [metadata])

    metadata.write_bytes(b"launch: \xff\xfe not utf-8\n")
    result = stale()
    need(not isinstance(result, Exception),
         f"a collection file that is not UTF-8 raised {result!r} out of "
         "ensure-daemon's staleness check")
    ok("a collection file that is not UTF-8 is reported, not raised")

    dirs_file.write_bytes(b"\xff\xfe/not/utf8\n" + str(games).encode() + b"\n")
    need(not isinstance(stale(), Exception),
         "a game_dirs.txt that is not UTF-8 raised out of the check")
    ok("a game_dirs.txt that is not UTF-8 is read past")

    dirs_file.write_text(f"{metadata}\n")
    same("a game_dirs line naming a file rather than a directory is skipped",
         stale(), [])

    dirs_file.write_text(f"{games}\n")
    metadata.write_text('collection: X\nlaunch: "/nix/store/unbalanced -L x\n')
    result = stale()
    if not isinstance(result, Exception):
        raise SystemExit(
            "FAIL: an unlexable launch line is handled -- assert it instead")
    gap(f"a launch line with an unbalanced quote raises "
        f"{type(result).__name__} out of stale_collections, and cli's "
        "ensure-daemon calls it unconditionally, so a hand-edited "
        "metadata.pegasus.txt tracebacks the whole start path -- the same "
        "shape as the hide.unhidden UTF-8 bug")

    metadata.write_text("collection: X\nlaunch:\n\ngame: A\n")
    same("a launch line with no command is not called stale", stale(), [])
    dirs_file.unlink()
    same("no game_dirs.txt at all is not an error",
         pegasus.stale_collections(), [])


def check_art(work: Path) -> None:
    print("\nS23: art is referenced only when there is a file")

    thumbs = work / "thumbnails"
    box = thumbs / "arcade" / "Named_Boxarts"
    titles_dir = thumbs / "arcade" / "Named_Titles"
    box.mkdir(parents=True)
    titles_dir.mkdir(parents=True)
    os.environ[pegasus.ENV_THUMBNAILS] = str(thumbs)

    # RetroArch replaces &*/:`<>?\| with _ when it writes a thumbnail, and
    # nothing else -- a quote, a semicolon or a bracket is stored as typed.
    escaped = "A_B_C_D_E_F_G_H_I_J_K"
    (box / f"{escaped}.png").write_bytes(b"\x89PNG")
    (box / "Bomberman_ The Second Attack!.png").write_bytes(b"\x89PNG")
    (titles_dir / "Bomberman_ The Second Attack!.png").write_bytes(b"\x89PNG")
    (box / "who?.png").write_bytes(b"\x89PNG")
    (box / "Pokémon Stadium.png").write_bytes(b"\x89PNG")
    (box / "notes.txt").write_text("not art")
    (box / "cover.PNG").write_bytes(b"\x89PNG")
    (box / ".png").write_bytes(b"\x89PNG")

    same("art_name mirrors RetroArch's escape set exactly",
         pegasus.art_name("A&B*C/D:E`F<G>H?I\\J|K"), escaped)
    same("and leaves alone what RetroArch leaves alone",
         pegasus.art_name('He said "hi"; [1985] 100%'),
         'He said "hi"; [1985] 100%')
    same("an empty label escapes to nothing rather than to \"_\"",
         pegasus.art_name(""), "")

    index = pegasus.art_index("arcade", thumbs)
    same("only .png files are indexed, case insensitively",
         sorted(index), sorted([
             escaped, "Bomberman_ The Second Attack!", "who?",
             "Pokémon Stadium", "cover"]))
    same("a bare \".png\" is a dotfile, not a game with an empty name",
         "" in index, False)
    same("a thumbnail root that does not exist yields no art, not an error",
         pegasus.art_index("arcade", work / "nowhere"), {})
    a_file = work / "a-file"
    a_file.write_text("x")
    same("a thumbnail root that is a file yields no art either",
         pegasus.art_index("arcade", a_file), {})

    def boxart_for(label: str, rom: str) -> str:
        """The boxart filename picked for one entry, "" for none.

        Not indexed directly: a lookup that finds nothing should read as a
        missing thumbnail, not as a KeyError from the test harness.
        """
        found = pegasus.entry_assets(label, rom, index).get("boxFront", "")
        return Path(found).name if found else ""

    same("a label is found under RetroArch's escaped name",
         boxart_for("A&B*C/D:E`F<G>H?I\\J|K", "/roms/arcade/ab.zip"),
         f"{escaped}.png")
    same("a hand-laid pack under the raw label is found too",
         boxart_for("who?", "/roms/arcade/who.zip"), "who?.png")
    same("and failing both, under the ROM's own basename",
         boxart_for("Renamed By The User", "/roms/arcade/Pokémon Stadium.zip"),
         "Pokémon Stadium.png")
    same("a label whose escaped form collides with another's shares its art",
         boxart_for("A&B&C/D:E`F<G>H?I\\J|K", "/roms/arcade/ab2.zip"),
         f"{escaped}.png")
    same("nothing is matched by prefix",
         boxart_for("who", "/roms/arcade/who.zip"), "")
    same("all three kinds come back for one game",
         sorted(pegasus.entry_assets(
             "Bomberman: The Second Attack!", "/roms/arcade/bomb.zip", index)),
         ["boxFront", "titlescreen"])
    same("a game with no art gets an empty dict, never an empty path",
         pegasus.entry_assets("Nothing At All", "/roms/arcade/none.zip",
                              index), {})
    same("an entry with no label and no stem asks for nothing",
         pegasus.entry_assets("", "/", index), {})

    print("\nS23: what the theme reads off a rendered collection")
    source = playlist(work / "arcade.lpl", ARCADE_CORE, [
        item("/roms/arcade/ab.zip", "A&B*C/D:E`F<G>H?I\\J|K"),
        item("/roms/arcade/bomb.zip", "Bomberman: The Second Attack!"),
        item("/roms/arcade/none.zip", "Nothing At All"),
    ], "Arcade (MAME 2010)")
    collection = pegasus.read_playlist(source)
    same("every referenced image really exists", audit(collection), [])
    _, blocks, _ = parse_metadata(pegasus.render(collection))
    same("the game with no art declares none, so the list layout is chosen",
         [k for k in blocks[2] if k.startswith("assets.")], [])
    same("the game with two kinds declares both",
         sorted(k for k in blocks[1] if k.startswith("assets.")),
         ["assets.boxFront", "assets.titlescreen"])

    print("\nS23: entries in the thumbnail tree that are not images")
    hostile = work / "hostile-thumbs"
    hbox = hostile / "arcade" / "Named_Boxarts"
    hbox.mkdir(parents=True)
    (hbox / "real.png").write_bytes(b"\x89PNG")
    (hbox / "a directory.png").mkdir()
    (hbox / "dangling.png").symlink_to(hostile / "nothing.png")
    hindex = pegasus.art_index("arcade", hostile)
    same("a real file is indexed",
         Path(hindex["real"]["boxFront"]).is_file(), True)
    unreal = sorted(
        name for name, kinds in hindex.items()
        if not Path(kinds["boxFront"]).is_file())
    if not unreal:
        raise SystemExit(
            "FAIL: art_index now checks its entries -- assert that instead")
    gap(f"art_index indexes anything ending .png, including {unreal}: a "
        "directory or a dangling symlink becomes an assets.boxFront the "
        "theme reads as \"this game has art\", and Library.qml switches that "
        "game to the grid layout and draws an empty frame")

    del os.environ[pegasus.ENV_THUMBNAILS]


def check_titles_table(work: Path) -> None:
    print("\nS22: a MAME title table that was damaged")

    good = {"pacman": Title("Pac-Man", "1980", "Namco", "good")}
    table_path = work / "titles.json"
    titles.dump_json(good, table_path)
    os.environ[titles.ENV_TITLES] = str(table_path)
    same("a table written by padmap round-trips", titles.find_titles(), good)

    supported = {
        "a row from before driver status existed":
            ('{"pacman": ["Pac-Man", "1980", "Namco"]}',
             Title("Pac-Man", "1980", "Namco", "")),
        "a row with a column from the future":
            ('{"pacman": ["Pac-Man", "1980", "Namco", "good", "?"]}',
             Title("Pac-Man", "1980", "Namco", "good")),
        "a driver status MAME did not have then":
            ('{"pacman": ["Pac-Man", "1980", "Namco", "banana"]}',
             Title("Pac-Man", "1980", "Namco", "banana")),
        "an empty table":
            ("{}", None),
    }
    for label, (text, wanted) in supported.items():
        table_path.write_text(text)
        got = titles.find_titles()
        same(label, got.get("pacman"), wanted)

    same("an unknown driver status is not read as broken",
         Title("x", status="banana").working, True)
    same("only preliminary is",
         [Title("x", status=s).working for s in
          ("good", "imperfect", "preliminary", "")],
         [True, True, False, True])

    damaged = {
        "truncated mid-write": '{"pacman": ["Pac',
        "a top-level list": "[1, 2, 3]",
        "an empty file": "",
        "a row with one column": '{"pacman": ["Pac-Man"]}',
        "a row with no columns": '{"pacman": []}',
        "a null row": '{"pacman": null}',
        "a row that is an object": '{"pacman": {"title": "Pac-Man"}}',
    }
    raised = []
    for label, text in damaged.items():
        table_path.write_text(text)
        try:
            got = titles.find_titles()
        except Exception as error:  # noqa: BLE001
            raised.append(f"{label} -> {type(error).__name__}")
            continue
        need(got.get("pacman") is None or got["pacman"].title == "Pac-Man",
             f"{label} was loaded as a title table, with a made-up title")
    ok(f"none of {len(damaged)} damaged tables invents a title")
    if not raised:
        raise SystemExit(
            "FAIL: damaged title tables are handled -- assert {} instead")
    gap(f"{len(raised)} damaged title tables raise out of find_titles, e.g. "
        f"{raised[0]}; the docstring promises \"the best available title "
        "table, or an empty dict\", and pegasus.export calls it first, so a "
        "half-written PADMAP_MAME_TITLES stops every collection being "
        "regenerated rather than costing the arcade tab its real names")

    table_path.write_text('{"pacman": "Pac-Man"}')
    got = titles.find_titles()["pacman"]
    if got.title == "Pac-Man":
        raise SystemExit("FAIL: string rows load properly now -- assert it")
    gap(f"a table whose rows are plain strings loads silently as "
        f"{got!r}: every arcade game gets a one-character name, and nothing "
        "in the export report says the table was the wrong shape")

    os.environ[titles.ENV_TITLES] = str(work / "never-built.json")
    same("a table that was never built means no titles, not a crash",
         titles.find_titles(), {})
    os.environ[titles.ENV_TITLES] = str(work)
    same("a directory in the override means no titles either",
         titles.find_titles(), {})
    del os.environ[titles.ENV_TITLES]
    same("no override at all means no titles", titles.find_titles(), {})


def check_mame_xml(work: Path) -> None:
    print("\nS22: a MAME XML that is not what it should be")

    xml = work / "mame.xml"

    xml.write_text(
        '<mame><game name="10yard"><description>10-Yard Fight</description>'
        "<year>1983</year><manufacturer>Irem</manufacturer>"
        '<driver status="good"/></game>'
        '<game name="pacman"><description>PuckMan</description>',
        encoding="utf-8")
    table = titles.parse_mame_xml(xml)
    same("a file truncated mid-entry keeps the entries that closed",
         sorted(table), ["10yard"])
    same("and the complete one is complete",
         table["10yard"],
         Title("10-Yard Fight", "1983", "Irem", "good"))

    xml.write_text('<mame><game name="a"><year>1980</year></game></mame>')
    same("an entry with no description is skipped rather than named \"\"",
         titles.parse_mame_xml(xml), {})

    xml.write_text(
        '<mame><game name="dup"><description>First Dump</description></game>'
        '<game name="dup"><description>Second Dump</description>'
        '<driver status="preliminary"/></game></mame>')
    table = titles.parse_mame_xml(xml)
    same("a duplicated set name is one entry", len(table), 1)
    same("and it is the last one seen", table["dup"].title, "Second Dump")
    same("with its own driver grade", table["dup"].working, False)

    xml.write_text(
        '<mame><game name="a"><description>A</description>'
        '<rom name="a.1" status="baddump"/><rom name="a.2" status="nodump"/>'
        '<driver status="imperfect"/></game></mame>')
    same("a ROM's own status is not mistaken for the driver's",
         titles.parse_mame_xml(xml)["a"].status, "imperfect")

    xml.write_text(
        '<mame><game name="a"><description>A</description>'
        '<driver status="banana"/></game></mame>')
    same("an unrecognised driver grade is carried through, not dropped",
         titles.parse_mame_xml(xml)["a"].status, "banana")

    xml.write_bytes(b"<mame><game name=\"a\"><description>Caf\xe9"
                    b"</description></game></mame>")
    same("a byte that is not UTF-8 costs a character, not the file",
         len(titles.parse_mame_xml(xml)), 1)

    xml.write_text("")
    same("an empty XML is an empty table", titles.parse_mame_xml(xml), {})
    xml.write_bytes(b"\x00\x01\x02 not xml at all")
    same("a binary file is an empty table too", titles.parse_mame_xml(xml), {})

    print("\nS23: resolving one playlist entry against the table")
    table = {"10yard": Title("10-Yard Fight", "1983", "Irem", "good"),
             "broken": Title("Broken Game", "1985", "X", "preliminary")}
    same("the ROM basename is the key, not the label",
         titles.resolve("Whatever The User Renamed It",
                        "/roms/arcade/10yard.zip", table).title,
         "10-Yard Fight")
    # A label the user edited to another set's name must not drag that set's
    # title, year and driver grade onto this game -- which would show the red
    # "does not work" marker on a game that runs perfectly (S23).
    same("a label that happens to be another set's name is still ignored",
         titles.resolve("broken", "/roms/arcade/10yard.zip", table),
         Title("10-Yard Fight", "1983", "Irem", "good"))
    same("a set the table does not have keeps its label",
         titles.resolve("Clone Set", "/roms/arcade/10yardj.zip", table).title,
         "Clone Set")
    same("with no label either, the basename shows",
         titles.resolve("", "/roms/arcade/10yardj.zip", table).title,
         "10yardj")
    same("a non-arcade entry has no grade to show",
         titles.resolve("Mario", "/roms/n64/mario.z64", table).status, "")

    source = playlist(work / "grades.lpl", ARCADE_CORE, [
        item("/roms/arcade/10yard.zip", "10yard"),
        item("/roms/arcade/broken.zip", "broken"),
        item("/roms/arcade/unknown.zip", "unknown"),
    ], "Arcade (MAME 2010)")
    collection = pegasus.read_playlist(source, table)
    same("a graded collection renders cleanly", audit(collection), [])
    _, blocks, _ = parse_metadata(pegasus.render(collection))
    same("the non-working game is marked for the theme",
         blocks[1].get("x-mame-status"), ["preliminary"])
    same("the working one is marked as working",
         blocks[0].get("x-mame-status"), ["good"])
    same("and an ungraded one carries no grade at all, which is not \"broken\"",
         "x-mame-status" in blocks[2], False)
    same("resolved titles reach the metadata",
         [b["game"][0] for b in blocks],
         ["10-Yard Fight", "Broken Game", "unknown"])


def check_scale(work: Path) -> None:
    print("\nS22: the size of the real arcade collection")

    count = 10000
    items = [item(f"/roms/arcade/game{i:05d}.zip", f"game{i:05d}")
             for i in range(count)]
    source = playlist(work / "big.lpl", ARCADE_CORE, items,
                      "Arcade (MAME 2010)")
    table = {f"game{i:05d}": Title(f"Game {i}", "1985", "Namco",
                                   "good" if i % 3 else "preliminary")
             for i in range(0, count, 2)}

    started = time.monotonic()
    collection = pegasus.read_playlist(source, table)
    text = pegasus.render(collection)
    elapsed = time.monotonic() - started
    same(f"{count} items are all read", len(collection.entries), count)
    need(elapsed < 15.0,
         f"reading and rendering {count} games took {elapsed:.1f}s; the real "
         "arcade playlist has 8302 and export-pegasus would appear hung")
    ok(f"read and rendered in {elapsed:.2f}s (budget 15s)")

    started = time.monotonic()
    header, blocks, junk = parse_metadata(text)
    same("every one of them is a well-formed block", len(blocks), count)
    same("with no unparseable lines", junk, [])
    same("under a single collection header", len(header["collection"]), 1)
    same("half of them resolved to a real MAME title",
         sum(1 for b in blocks if b["game"][0].startswith("Game ")),
         count // 2)
    same("and the non-working ones are marked",
         sum(1 for b in blocks if b.get("x-mame-status") == ["preliminary"]),
         len([i for i in range(0, count, 2) if not i % 3]))
    ok(f"parsed back in {time.monotonic() - started:.2f}s")

    out = work / "out-big"
    big_dir = work / "bigdir"
    big_dir.mkdir()
    shutil.copy(source, big_dir / "arcade.lpl")
    started = time.monotonic()
    results, dirs = pegasus.export(big_dir, out)
    elapsed = time.monotonic() - started
    same("export writes the collection", [r[1] for r in results], [count])
    same("in its own directory, for game_dirs.txt",
         [d.name for d in dirs], ["arcade"])
    need(elapsed < 30.0, f"exporting {count} games took {elapsed:.1f}s")
    ok(f"exported in {elapsed:.2f}s (budget 30s)")

    written = (out / "arcade" / "metadata.pegasus.txt")
    same("the file is written as UTF-8",
         len(written.read_text(encoding="utf-8").split("\n")) > count, True)

    legacy = out / "metadata.pegasus.txt"
    legacy.write_text("collection: Stale Merged File\n")
    pegasus.export(big_dir, out)
    same("a merged file from an older padmap is deleted, not left to be read",
         legacy.exists(), False)


CHECKS = (
    check_empty_and_missing,
    check_directory_rom,
    check_hostile_labels,
    check_newline_injection,
    check_duplicates,
    check_console_and_key,
    check_empty_title,
    check_wrong_shapes,
    check_display_name,
    check_launch_lines,
    check_stale_collections,
    check_art,
    check_titles_table,
    check_mame_xml,
    check_scale,
)


def main() -> int:
    try:
        for check in CHECKS:
            # One scratch directory each: several of these glob a directory
            # for *.lpl, and a playlist left behind by an earlier scenario
            # would quietly join the next one's export.
            work = SANDBOX / "work" / check.__name__
            work.mkdir(parents=True)
            check(work)
    finally:
        shutil.rmtree(SANDBOX, ignore_errors=True)

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
