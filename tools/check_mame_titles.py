#!/usr/bin/env python3
"""The MAME title table: how it is built, and what a damaged one costs.

Story S22 (`padmap export-pegasus` regenerates the collections) and S23 (games
are browsed with real names and non-working titles marked).

The table is a *build product read at runtime*: Nix runs titles.parse_mame_xml
over a 43MB MAME dump, titles.dump_json writes the compact rows, and every
export reads them back through PADMAP_MAME_TITLES. Three things follow, and
this file is about all three.

  * The reader is fed a file nobody validated on the way in. A build that was
    interrupted, a file copied while it was being written, or a table someone
    hand-edited all arrive at load_json, and pegasus.export calls find_titles
    on its *first line* -- so anything that raises there costs the user every
    collection, not merely the arcade tab's real names. The docstring has
    always promised "the best available title table, or an empty dict".

  * Silence is worse than a crash. A table whose rows were plain strings
    ({"pacman": "Pac-Man"}) loaded without complaint as
    Title(title="P", year="a", ...) -- a string subscripts exactly like a
    list -- so every arcade game got a one-character name and nothing said
    why. A dropped row is reported; a wrong one is not.

  * The parser reads <description> with re.S, so a description the dump
    wrapped across two lines used to reach the table with a newline in it.
    metadata.pegasus.txt is line oriented, which makes such a title a *key*:
    pegasus.one_line stops that on the way out, and the table is normalised
    here so it never holds a title no human typed in the first place.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \\
        nix develop --command python3 tools/check_mame_titles.py
"""

from __future__ import annotations

import json
import logging
import os
import sys
import tempfile
from pathlib import Path

# Every XDG root is redirected *before* padmap is imported: a live daemon owns
# the pads on this machine and export writes into XDG_DATA_HOME, so a check
# that regenerated the real collections would cost more than the bugs it
# looks for.
SANDBOX = Path(tempfile.mkdtemp(prefix="padmap-mame-titles-"))
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    _dir = SANDBOX / _var.lower()
    _dir.mkdir(parents=True, exist_ok=True)
    _dir.chmod(0o700)
    os.environ[_var] = str(_dir)

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import pegasus, titles  # noqa: E402
from padmap.titles import Title  # noqa: E402

# A launcher inside the sandbox, so the export below never installs a symlink
# over the user's own.
os.environ[pegasus.ENV_PLAY] = str(SANDBOX / "store" / "padmap-play")

ARCADE_CORE = "/cores/mame2010_libretro.so"


class Recorder(logging.Handler):
    """Every warning padmap.titles emits, kept instead of printed.

    Two jobs. It keeps the deliberately damaged tables below from writing
    scary lines into a run whose whole point is that they are handled -- and,
    more importantly, it lets the checks assert that damage is *reported*.
    The one shape that used to load silently is the one that hurt: nobody
    looking at a library full of one-letter arcade names had anything to read
    that said the table was the wrong shape.
    """

    def __init__(self) -> None:
        super().__init__()
        self.messages: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.messages.append(record.getMessage())

    def take(self) -> list[str]:
        messages, self.messages = self.messages, []
        return messages


LOG = logging.getLogger("padmap.titles")
WARNINGS = Recorder()
LOG.addHandler(WARNINGS)
LOG.propagate = False
LOG.setLevel(logging.WARNING)


def ok(what: str) -> None:
    print(f"  ok  {what}")


def need(condition: bool, complaint: str) -> None:
    if not condition:
        raise SystemExit(f"FAIL: {complaint}")


def same(what: str, got: object, want: object) -> None:
    if got != want:
        raise SystemExit(f"FAIL: {what} -- got {got!r}, wanted {want!r}")
    ok(what)


def heading(what: str) -> None:
    print(f"\n{what}")


def playlist(path: Path, labels: list[str]) -> Path:
    path.write_text(json.dumps({
        "version": "1.5",
        "default_core_path": ARCADE_CORE,
        "default_core_name": "Arcade (MAME 2010)",
        "scan_file_exts": "zip",
        "items": [
            {"path": f"/roms/arcade/{label}.zip", "label": label,
             "core_path": "DETECT", "core_name": "DETECT",
             "crc32": "00000000|crc", "db_name": "x.lpl"}
            for label in labels
        ],
    }), encoding="utf-8")
    return path


# ---------------------------------------------------------------------------


def check_damaged_table(work: Path) -> None:
    heading("S22: a title table that will not load is not an export failure")

    table = work / "titles.json"
    os.environ[titles.ENV_TITLES] = str(table)

    # Every one of these raised straight out of find_titles before the fix:
    # JSONDecodeError, AttributeError, IndexError, TypeError, KeyError. Since
    # pegasus.export calls find_titles first, each of them ended the export
    # and left the library as it was.
    unloadable = {
        "a build interrupted mid-write": '{"pacman": ["Pac',
        "a file that is empty": "",
        "a file of zero-length JSON": "   ",
        "a top-level list": "[1, 2, 3]",
        "a top-level string": '"pacman"',
        "a top-level number": "17",
        "a file that is not JSON at all": "<mame><game/></mame>",
        "a NUL-padded copy": "\x00\x00\x00",
    }
    for label, text in unloadable.items():
        table.write_text(text, encoding="utf-8")
        try:
            got = titles.find_titles()
        except Exception as error:  # noqa: BLE001
            raise SystemExit(
                f"FAIL: {label} raises {type(error).__name__} out of "
                f"find_titles ({error}); export-pegasus calls it on its first "
                "line, so the user loses every collection over a table that "
                "only costs the arcade tab its real names") from error
        same(f"{label} loads as no titles at all", got, {})

    heading("S22: one bad row costs that set, not the table")

    rows_dropped = {
        "a row with no columns": '{"pacman": []}',
        "a null row": '{"pacman": null}',
        "a row that is an object": '{"pacman": {"title": "Pac-Man"}}',
        "a row that is a number": '{"pacman": 1980}',
        "a row that is a bool": '{"pacman": true}',
        "a title that is a list": '{"pacman": [["Pac-Man"], "1980"]}',
        "a title that is null": '{"pacman": [null, "1980"]}',
        "a title that is empty": '{"pacman": ["", "1980"]}',
        "a title that is only whitespace": '{"pacman": ["   ", "1980"]}',
    }
    for label, text in rows_dropped.items():
        table.write_text(text, encoding="utf-8")
        got = titles.find_titles()
        need("pacman" not in got,
             f"{label} became a title: {got.get('pacman')!r}. A row that is "
             "not a row must leave the set showing its raw name, never a "
             "name invented out of the damage")
    ok(f"none of {len(rows_dropped)} unreadable rows invents a title")

    table.write_text(json.dumps({
        "pacman": ["Pac-Man", "1980", "Namco", "good"],
        "torn": None,
        "10yard": ["10-Yard Fight", "1983", "Irem", "good"],
    }), encoding="utf-8")
    same("the readable rows of a partly damaged table still name their games",
         sorted(titles.find_titles()), ["10yard", "pacman"])

    heading("S22: a row shaped differently, but still readable")

    readable = {
        "the four columns padmap writes":
            ('{"pacman": ["Pac-Man", "1980", "Namco", "good"]}',
             Title("Pac-Man", "1980", "Namco", "good")),
        "three columns, from before driver status was recorded":
            ('{"pacman": ["Pac-Man", "1980", "Namco"]}',
             Title("Pac-Man", "1980", "Namco")),
        "two columns":
            ('{"pacman": ["Pac-Man", "1980"]}', Title("Pac-Man", "1980")),
        "one column":
            ('{"pacman": ["Pac-Man"]}', Title("Pac-Man")),
        "a fifth column from a future format":
            ('{"pacman": ["Pac-Man", "1980", "Namco", "good", "extra"]}',
             Title("Pac-Man", "1980", "Namco", "good")),
        "a year written as a number":
            ('{"pacman": ["Pac-Man", 1980]}', Title("Pac-Man", "1980")),
        # The dangerous one. This shape used to load as Title(title="P",
        # year="a", manufacturer="c", status="-"), because "Pac-Man"[0] is
        # "P" -- every arcade game a single letter, with nothing logged.
        "a bare string, which is a title and nothing else":
            ('{"pacman": "Pac-Man"}', Title("Pac-Man")),
    }
    for label, (text, wanted) in readable.items():
        table.write_text(text, encoding="utf-8")
        same(label, titles.find_titles().get("pacman"), wanted)

    table.write_text('{"pacman": "Pac-Man"}', encoding="utf-8")
    got = titles.find_titles()["pacman"]
    need(len(got.title) > 1,
         f"a string row is read a character at a time: {got!r}. Every arcade "
         "game would be named with one letter and the export report would "
         "say the table loaded fine")
    ok("a string row is never read a character at a time")

    heading("S22: the damage is said out loud")

    WARNINGS.take()
    table.write_text('{"pacman": ["Pac', encoding="utf-8")
    titles.find_titles()
    said = WARNINGS.take()
    need(any(str(table) in line for line in said),
         "a title table that will not parse is ignored without a word; the "
         "arcade tab silently shows set names and nobody can tell whether "
         "the table is missing, stale or broken")
    ok("a table that will not parse names itself in a warning")

    table.write_text(json.dumps({
        "pacman": ["Pac-Man", "1980", "Namco", "good"],
        "torn": None,
    }), encoding="utf-8")
    titles.find_titles()
    said = WARNINGS.take()
    need(any("torn" in line for line in said),
         "a row that had to be dropped is dropped in silence; the set it "
         "named keeps its raw name in the library with nothing to explain it")
    ok("a dropped row names the set it lost")

    WARNINGS.take()
    titles.dump_json({"pacman": Title("Pac-Man", "1980", "Namco", "good")},
                     table)
    titles.find_titles()
    same("a healthy table warns about nothing", WARNINGS.take(), [])

    heading("S22: a table that is not there at all")

    os.environ[titles.ENV_TITLES] = str(work / "never-built.json")
    same("a table that was never built means raw set names, not a crash",
         titles.find_titles(), {})
    os.environ[titles.ENV_TITLES] = str(work)
    same("a directory in the override means the same", titles.find_titles(),
         {})
    del os.environ[titles.ENV_TITLES]
    same("and no override at all means the same", titles.find_titles(), {})


def check_export_survives(work: Path) -> None:
    heading("S22: export-pegasus against a half-written table")

    playlists = work / "playlists"
    playlists.mkdir()
    playlist(playlists / "Arcade.lpl", ["pacman", "10yard"])
    out = work / "collections"

    table = work / "half-written.json"
    table.write_text('{"pacman": ["Pac', encoding="utf-8")
    os.environ[titles.ENV_TITLES] = str(table)

    try:
        results, written = pegasus.export(playlists, out)
    except Exception as error:  # noqa: BLE001
        raise SystemExit(
            f"FAIL: export-pegasus dies with {type(error).__name__} "
            f"({error}) when the MAME table is half-written. The user asked "
            "for their library to be regenerated and got no collections at "
            "all, over a file that is only there to improve arcade names"
        ) from error
    same("every collection is still written", len(results), 1)
    need(written and (written[0] / "metadata.pegasus.txt").is_file(),
         "export reported success but wrote no metadata file")
    text = (written[0] / "metadata.pegasus.txt").read_text(encoding="utf-8")
    need("game: pacman" in text,
         "the games fell out of the collection with the table; without "
         "titles they must keep their raw set names, which is what "
         "RetroArch shows today")
    ok("and its games keep their raw set names")

    titles.dump_json({"pacman": Title("Pac-Man", "1980", "Namco", "good")},
                     table)
    results, written = pegasus.export(playlists, out)
    text = (written[0] / "metadata.pegasus.txt").read_text(encoding="utf-8")
    need("game: Pac-Man" in text,
         "a repaired table does not reach the collection: rebuilding the "
         "table has to be enough to fix the names")
    ok("a repaired table names the games on the next export")
    del os.environ[titles.ENV_TITLES]


def check_xml(work: Path) -> None:
    heading("S22: building the table out of a MAME dump")

    xml = work / "mame.xml"
    xml.write_text(
        '<mame><game name="10yard"><description>10-Yard Fight</description>'
        "<year>1983</year><manufacturer>Irem</manufacturer>"
        '<driver status="good"/></game></mame>', encoding="utf-8")
    same("a complete entry keeps all four fields",
         titles.parse_mame_xml(xml)["10yard"],
         Title("10-Yard Fight", "1983", "Irem", "good"))

    xml.write_text(
        '<mame><game name="10yard"><description>10-Yard Fight</description>'
        '<driver status="good"/></game>'
        '<game name="pacman"><description>PuckMan</desc', encoding="utf-8")
    same("a dump truncated mid-entry keeps the entries that closed",
         sorted(titles.parse_mame_xml(xml)), ["10yard"])

    xml.write_text(
        '<mame><game name="a"><year>1980</year></game>'
        '<game name="b"><description></description></game>'
        '<game name="c"><description>   </description></game></mame>',
        encoding="utf-8")
    same("an entry with no usable description is skipped, not named \"\"",
         titles.parse_mame_xml(xml), {})

    xml.write_text(
        '<mame><game name="dup"><description>First Dump</description></game>'
        '<game name="dup"><description>Second Dump</description>'
        '<driver status="preliminary"/></game></mame>', encoding="utf-8")
    table = titles.parse_mame_xml(xml)
    same("a set name listed twice is one entry", len(table), 1)
    same("and it is the last one seen", table["dup"].title, "Second Dump")
    same("with its own driver grade", table["dup"].working, False)

    xml.write_text(
        '<mame><game name="a"><description>A</description>'
        '<rom name="a.1" status="baddump"/><rom name="a.2" status="nodump"/>'
        '<driver status="imperfect"/></game></mame>', encoding="utf-8")
    same("a ROM's own status is not mistaken for the driver's",
         titles.parse_mame_xml(xml)["a"].status, "imperfect")
    same("an imperfect driver is still a game that runs",
         titles.parse_mame_xml(xml)["a"].working, True)

    xml.write_text(
        '<mame><game name="a"><description>A</description>'
        '<driver status="banana"/></game>'
        '<game name="b"><description>B</description></game></mame>',
        encoding="utf-8")
    table = titles.parse_mame_xml(xml)
    same("a driver grade this padmap has not heard of is carried through",
         table["a"].status, "banana")
    same("and is not read as \"does not work\"", table["a"].working, True)
    same("a set with no <driver> carries no grade", table["b"].status, "")
    same("which is also not \"does not work\"", table["b"].working, True)

    heading("S22: a description the dump wrapped across two lines")

    # <description> is matched with re.S, so the newline used to survive into
    # the table. metadata.pegasus.txt is line oriented -- "Evil\nlaunch: x"
    # is a *launch command*, and Pegasus keeps the first one it sees -- so a
    # title holding a newline is not cosmetic, it is what that game runs.
    xml.write_text(
        '<mame><game name="wrap"><description>Wrapped\n'
        "            Over Two Lines</description>\n"
        "<year>19\n80</year><manufacturer>Na\nmco</manufacturer>"
        '<driver status="good"/></game></mame>', encoding="utf-8")
    entry = titles.parse_mame_xml(xml)["wrap"]
    same("the wrap is joined the way a reader would join it",
         entry.title, "Wrapped Over Two Lines")
    same("and the other wrapped fields too",
         (entry.year, entry.manufacturer), ("19 80", "Na mco"))
    for field in (entry.title, entry.year, entry.manufacturer):
        need(not any(c in field for c in "\r\n  "),
             f"{field!r} still holds a line break; rendered into "
             "metadata.pegasus.txt the rest of it is read as another key, "
             "which is how a game title became a launch command")
    ok("no field out of the XML holds a line break")

    xml.write_text(
        '<mame><game name="dd"><description>Double  Dragon\t3</description>'
        "</game></mame>", encoding="utf-8")
    same("spacing inside one line is left exactly as MAME wrote it",
         titles.parse_mame_xml(xml)["dd"].title, "Double  Dragon\t3")

    xml.write_bytes(b'<mame><game name="a"><description>Caf\xe9'
                    b"</description></game></mame>")
    same("a byte that is not UTF-8 costs a character, not the file",
         len(titles.parse_mame_xml(xml)), 1)

    xml.write_text("", encoding="utf-8")
    same("an empty dump is an empty table", titles.parse_mame_xml(xml), {})
    xml.write_bytes(b"\x00\x01\x02 not xml at all")
    same("a binary file is an empty table too", titles.parse_mame_xml(xml), {})

    heading("S22: the table survives the round trip Nix does")

    xml.write_text(
        '<mame><game name="wrap"><description>Wrapped\n  Title</description>'
        "<year>1983</year><manufacturer>Irem</manufacturer>"
        '<driver status="preliminary"/></game></mame>', encoding="utf-8")
    built = titles.parse_mame_xml(xml)
    dumped = work / "round-trip.json"
    titles.dump_json(built, dumped)
    os.environ[titles.ENV_TITLES] = str(dumped)
    same("what the build writes is what the runtime reads",
         titles.find_titles(), built)
    del os.environ[titles.ENV_TITLES]


def main() -> None:
    work = SANDBOX / "work"
    work.mkdir()
    check_damaged_table(work)
    check_export_survives(work)
    check_xml(work)
    print("\nall checks passed")


if __name__ == "__main__":
    main()
