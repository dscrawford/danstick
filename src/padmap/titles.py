"""Resolve MAME set names to real game titles.

RetroArch's arcade playlists label entries by ROM set name -- `10yard`,
`ckongjeu`, `1941j` -- because that is the filename. A library built on those
reads like a directory listing. MAME's own XML carries the real description
per set, so this maps one to the other.

Matching is by **set name**, not CRC. The playlist here records
`crc32 = "00000000|crc"` for all 8302 entries, a single distinct value, so
checksum matching against libretro-database cannot work at all.

Use the XML that matches the *core*, not the newest one available. Set names
drift between MAME versions, and the playlist was scanned for MAME 2010
(0.139). Measured against that XML: 8215/8302 = 99.0% resolved; the remainder
are clone and bootleg sets absent from that build, which fall back to the
raw name.
"""

from __future__ import annotations

import json
import logging
import os
import re
from dataclasses import dataclass
from pathlib import Path

log = logging.getLogger("padmap.titles")

# Environment override so the generated Nix package can be found without
# the caller having to pass a path.
ENV_TITLES = "PADMAP_MAME_TITLES"

_GAME = re.compile(r'<game name="([^"]+)"')
_DESCRIPTION = re.compile(r"<description>(.*?)</description>", re.S)
# re.S on all three, and each result run through _flatten: a dump that
# wrapped one of these elements across two lines otherwise loses the field
# entirely, silently, since `.` alone will not cross the newline. The wrap is
# a property of how the XML was written, not of the game.
_YEAR = re.compile(r"<year>(.*?)</year>", re.S)
_MANUFACTURER = re.compile(r"<manufacturer>(.*?)</manufacturer>", re.S)
# MAME grades each driver: "good", "imperfect", or "preliminary". Preliminary
# means the game does not really run -- it is the status behind MAME's own red
# "THIS GAME DOES NOT WORK" warning. Deliberately matched on `<driver ` and
# not on `status="..."` anywhere: ROM elements carry their own status
# ("baddump", "nodump") and there are thousands of those per file.
_DRIVER = re.compile(r'<driver\s+status="([a-z]+)"')

STATUS_GOOD = "good"
STATUS_IMPERFECT = "imperfect"
STATUS_PRELIMINARY = "preliminary"

# A run of whitespace that contains a line break, anywhere in a field read out
# of the XML.
_WRAPPED = re.compile(r"[ \t]*[\r\n\u2028\u2029]+[ \t]*")


def _flatten(value: str) -> str:
    """One line, with a wrapped description joined the way a reader would.

    <description> is matched with re.S, so a MAME description that the dump
    wrapped across two lines arrives here as "Puck\\n            Man". That
    newline is not cosmetic: a title reaches line-oriented files and
    line-oriented reports, so one holding a newline can write a line of its
    own. Normalising here means the *table* never holds a title no human
    typed, which is what artwork matching and the fetch-art report read.

    Only runs of whitespace that span a line break collapse. Spaces inside a
    single line are left exactly as they are, because "Double  Dragon" is a
    real MAME description and rewriting it to "Double Dragon" would lose a
    match that the set-name table exists to make.
    """
    return _WRAPPED.sub(" ", value).strip()


@dataclass(frozen=True)
class Title:
    title: str
    year: str = ""
    manufacturer: str = ""
    # MAME's driver grade: "good", "imperfect", "preliminary", or "" when the
    # set is not in the table at all. Only meaningful for arcade.
    status: str = ""

    @property
    def working(self) -> bool:
        """False only when MAME says the driver does not work.

        An unknown status counts as working: most of the library is not MAME,
        and marking every console game as broken because it has no driver
        grade would be worse than saying nothing.
        """
        return self.status != STATUS_PRELIMINARY


def parse_mame_xml(path: Path) -> dict[str, Title]:
    """Extract set name -> Title from a MAME -listxml dump.

    Read whole and scanned with regexes rather than parsed as a DOM: the
    MAME 2010 XML is 43MB and an ElementTree of it costs several hundred MB
    of RSS for three fields per entry.
    """
    text = path.read_text(encoding="utf-8", errors="replace")
    out: dict[str, Title] = {}

    for match in _GAME.finditer(text):
        name = match.group(1)
        end = text.find("</game>", match.end())
        if end < 0:
            continue
        block = text[match.end():end]

        description = _DESCRIPTION.search(block)
        if not description:
            continue
        year = _YEAR.search(block)
        manufacturer = _MANUFACTURER.search(block)
        driver = _DRIVER.search(block)
        # _flatten, not .strip(): a description the dump wrapped across two
        # lines keeps its newline through a strip, and a title with a newline
        # in it is a key injection in the exported metadata file.
        title = _flatten(description.group(1))
        if not title:
            # Same reason an entry with no <description> is skipped: a set
            # named "" resolves to a blank row in the arcade tab, which is
            # worse than falling back to the raw set name.
            continue
        out[name] = Title(
            title=title,
            year=_flatten(year.group(1)) if year else "",
            manufacturer=(
                _flatten(manufacturer.group(1)) if manufacturer else ""),
            status=driver.group(1) if driver else "",
        )
    return out


def dump_json(titles: dict[str, Title], path: Path) -> None:
    """Write the compact form the runtime actually loads."""
    path.write_text(json.dumps(
        {
            name: [t.title, t.year, t.manufacturer, t.status]
            for name, t in titles.items()
        },
        separators=(",", ":"),
    ))


def _column(value: object) -> str:
    """One cell of a row, or "" when it is not something to show a player."""
    if isinstance(value, str):
        return _flatten(value)
    if isinstance(value, bool):
        # A JSON true would otherwise print as the year "True".
        return ""
    if isinstance(value, (int, float)):
        return str(value)
    return ""


def load_json(path: Path) -> dict[str, Title]:
    """Read a dumped table, dropping any row that is not one.

    Every row is checked instead of trusted. The table is a build product
    read at runtime, so a truncated write, a half-copied file or a
    hand-edited one all arrive here, and the shapes they arrive in are not
    theoretical: a row that is a plain string ({"pacman": "Pac-Man"}) used to
    load as Title(title="P", year="a", ...) because a string subscripts just
    like a list -- every arcade game named with one character, and nothing
    said so. That shape is now read as the one thing it can only mean, a
    title with no year; a row that cannot be read at all is dropped and
    counted, and the rest of the table still names its games.
    """
    raw = json.loads(path.read_text())
    if not isinstance(raw, dict):
        raise ValueError(
            f"title table is a {type(raw).__name__}, not an object of "
            "set name -> row")

    out: dict[str, Title] = {}
    dropped: list[str] = []
    for name, value in raw.items():
        # Tested for explicitly, never by "is it subscriptable": str and dict
        # are subscriptable too, and both of them produced a wrong Title
        # rather than an error.
        if isinstance(value, str):
            row: list[object] = [value]
        elif isinstance(value, list):
            row = list(value)
        else:
            dropped.append(str(name))
            continue
        # Tolerates the three-element rows written before driver status was
        # recorded, so an already-built table keeps working rather than making
        # every arcade entry fall back to its raw set name. Short and long
        # rows are read the same way: missing columns are empty, extra ones
        # from a future format are ignored.
        columns = [_column(cell) for cell in row[:4]]
        columns += [""] * (4 - len(columns))
        if not columns[0]:
            # No title is not a title. Keeping the row would replace the set
            # name in the arcade tab with a blank line.
            dropped.append(str(name))
            continue
        out[str(name)] = Title(
            title=columns[0], year=columns[1], manufacturer=columns[2],
            status=columns[3],
        )

    if dropped:
        log.warning(
            "title table %s: %d of %d rows are the wrong shape and were "
            "dropped (e.g. %r); those sets keep their raw names",
            path, len(dropped), len(raw), dropped[0])
    return out


def find_titles() -> dict[str, Title]:
    """Best available title table, or an empty dict.

    An empty result is not an error: without it, entries simply keep their
    raw set names, which is what RetroArch shows today.

    Damage is therefore reported, not raised. A caller runs this on
    its first line, so a table caught half-written -- the Nix build
    interrupted, the file copied while it was being generated -- used to end
    the whole export with a JSONDecodeError, and the user lost every
    collection instead of the arcade tab's real names.
    """
    override = os.environ.get(ENV_TITLES)
    if override and Path(override).is_file():
        try:
            return load_json(Path(override))
        except Exception as error:              # noqa: BLE001
            log.warning("ignoring title table %s: %s: %s",
                        override, type(error).__name__, error)
    return {}


def resolve(label: str, path: str, titles: dict[str, Title]) -> Title:
    """Title for one playlist entry.

    Keyed on the ROM basename rather than the playlist label, because the
    label is only incidentally the same string and users can edit it.
    """
    stem = Path(path).stem
    found = titles.get(stem)
    if found is not None:
        return found
    return Title(title=label or stem)
