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
import os
import re
from dataclasses import dataclass
from pathlib import Path

# Environment override so the generated Nix package can be found without
# the caller having to pass a path.
ENV_TITLES = "PADMAP_MAME_TITLES"

_GAME = re.compile(r'<game name="([^"]+)"')
_DESCRIPTION = re.compile(r"<description>(.*?)</description>", re.S)
_YEAR = re.compile(r"<year>(.*?)</year>")
_MANUFACTURER = re.compile(r"<manufacturer>(.*?)</manufacturer>")


@dataclass(frozen=True)
class Title:
    title: str
    year: str = ""
    manufacturer: str = ""


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
        out[name] = Title(
            title=description.group(1).strip(),
            year=year.group(1).strip() if year else "",
            manufacturer=manufacturer.group(1).strip() if manufacturer else "",
        )
    return out


def dump_json(titles: dict[str, Title], path: Path) -> None:
    """Write the compact form the runtime actually loads."""
    path.write_text(json.dumps(
        {name: [t.title, t.year, t.manufacturer] for name, t in titles.items()},
        separators=(",", ":"),
    ))


def load_json(path: Path) -> dict[str, Title]:
    raw = json.loads(path.read_text())
    return {
        name: Title(title=value[0], year=value[1], manufacturer=value[2])
        for name, value in raw.items()
    }


def find_titles() -> dict[str, Title]:
    """Best available title table, or an empty dict.

    An empty result is not an error: without it, entries simply keep their
    raw set names, which is what RetroArch shows today.
    """
    override = os.environ.get(ENV_TITLES)
    if override and Path(override).is_file():
        return load_json(Path(override))
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
