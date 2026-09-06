"""Fetch box art for RetroArch playlists from libretro's thumbnail server.

The upstream source is <https://thumbnails.libretro.com>, the same host
RetroArch's own "Download Thumbnails" menu uses. It is a plain Apache
autoindex over the contents of the github.com/libretro-thumbnails
repositories, laid out as::

    /<System Name>/Named_Boxarts/<Game Name>.png
    /<System Name>/Named_Snaps/<Game Name>.png
    /<System Name>/Named_Titles/<Game Name>.png

Preferred over cloning the git repositories, which are enormous:
libretro-thumbnails/Nintendo_-_Nintendo_64 alone reports 960 MB for 1115
games, and the whole thing would be fetched to satisfy the 23 in this
playlist.

Everything here is driven by the *directory index* rather than by guessing
URLs. One index request per system covers the whole catalogue -- 5824 names
in 1.4 MB for MAME -- and then matching happens locally. The alternative,
requesting one URL per playlist entry and treating 404 as "no art", is 8302
round trips for the arcade playlist of which a third are misses. Indexing
first also means "this game has no upstream art" is known *before* any
bytes are spent, so the size estimate printed up front is real.

Two naming problems stand between a playlist and a URL:

  * Arcade playlists label entries by MAME set name (`10yard`, `11beat`)
    but the MAME thumbnails are named by the real game title
    ("10-Yard Fight (Japan)"). Matching set names directly finds 3 of 8302.
    Going through titles.py's set-name table first finds 3639.
  * Console labels follow whatever DAT the ROMs came from. The N64
    playlist here is GoodN64 ("Banjo-Kazooie (U) [!]") while upstream is
    No-Intro ("Banjo-Kazooie (USA)"), so 7 of 23 miss on an exact compare.

`base_title` handles the second by falling back to the part of the name
before the first parenthesis and picking a preferred regional variant --
which is sound for box art specifically, since regional variants of a game
mostly share a box, and an arcade clone set shares the parent's cabinet
art. Measured on this machine that lifts arcade from 3639 to 5154 of 8302
and closes N64 and GameCube completely.

Downloaded files are stored under the *playlist* name and under the
escaping `pegasus.art_name` produces, not under the upstream name, so
`pegasus.art_index` finds them with no knowledge of any of the above.
"""

from __future__ import annotations

import os
import re
import shutil
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path
from typing import TextIO

from .pegasus import art_name, thumbnail_dir
from .titles import Title, find_titles

# Override so tests can point at a fixture server instead of the internet.
ENV_SERVER = "PADMAP_THUMBNAIL_SERVER"

DEFAULT_SERVER = "https://thumbnails.libretro.com"

# The three kinds RetroArch stores and pegasus.ART_KINDS consumes. Boxarts
# is the default and usually the only one worth fetching: it is what a grid
# shows, and the full arcade set is ~2.4 GB per kind.
KINDS = ("Named_Boxarts", "Named_Snaps", "Named_Titles")

# Apache autoindex rows. Size is captured too -- it is the only way to tell
# the user what a run will cost before starting it -- but is optional,
# because a differently configured index would still list the names.
_ROW = re.compile(
    r'<a href="(?P<href>[^"?/][^"]*)">.*?</a>\s*</td>'
    r'(?:\s*<td[^>]*>[^<]*</td>)?'
    r'\s*<td[^>]*>\s*(?P<size>[\d.]+)(?P<unit>[KMGT]?)\s*</td>',
    re.I | re.S,
)
_HREF = re.compile(r'<a href="([^"?/][^"]*)">', re.I)

_UNITS = {"": 1.0, "K": 1024.0, "M": 1024.0 ** 2, "G": 1024.0 ** 3,
          "T": 1024.0 ** 4}

# Regional variants in preference order, for picking one upstream name when
# a playlist label matches several. Box art differs least across these, and
# an English-language box is the better default for an unknown library.
_REGIONS = ("(usa", "(world", "(europe", "(japan")

# Arcade cores are not named after a libretro system, so `default_core_name`
# gives "Arcade (MAME 2010)" where the thumbnail directory is "MAME". Keyed
# on substrings of the whole core name, longest-specific first.
_ARCADE_CORES = (
    ("fbneo", "FBNeo - Arcade Games"),
    ("fb alpha", "FBNeo - Arcade Games"),
    ("finalburn", "FBNeo - Arcade Games"),
    ("final burn", "FBNeo - Arcade Games"),
    ("mame", "MAME"),
)


@dataclass
class Image:
    """One upstream file: what to request, and where it must land."""
    url: str
    dest: Path
    size: int = 0


@dataclass
class Plan:
    """What a single playlist would fetch, decided before any download."""
    playlist: str
    system: str = ""
    # Entries matched to an upstream file that is not on disk yet.
    wanted: list[Image] = field(default_factory=list)
    entries: int = 0
    matched: int = 0
    present: int = 0
    # Entries with no upstream art at all. Normal, not an error.
    missing: int = 0
    note: str = ""

    @property
    def bytes(self) -> int:
        return sum(i.size for i in self.wanted)


@dataclass
class Report:
    plan: Plan
    downloaded: int = 0
    failed: int = 0
    # First failure, for a message that says why rather than only how many.
    reason: str = ""


def _unique(values: list[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for value in values:
        if value not in seen:
            seen.add(value)
            out.append(value)
    return out


def server() -> str:
    return (os.environ.get(ENV_SERVER) or DEFAULT_SERVER).rstrip("/")


def _url(*parts: str) -> str:
    # Every path segment needs quoting: system names contain spaces and the
    # game names contain almost everything else.
    quoted = "/".join(urllib.parse.quote(p) for p in parts)
    return f"{server()}/{quoted}/"


def fetch(url: str, timeout: float = 30.0) -> bytes:
    request = urllib.request.Request(
        url,
        # The default Python agent is refused by some CDN configurations.
        headers={"User-Agent": "padmap/1 (+libretro-thumbnails)"},
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        data = response.read()
    return bytes(data)


def parse_index(html: str, suffix: str = ".png") -> dict[str, int]:
    """Names (suffix stripped) to byte size, from an Apache autoindex page.

    Sizes are best effort; a name whose row does not parse still appears,
    with size 0, because a wrong estimate is much better than a missing
    download.
    """
    out: dict[str, int] = {}
    for row in html.split("<tr"):
        match = _ROW.search(row)
        href = match.group("href") if match else None
        if href is None:
            plain = _HREF.search(row)
            if plain is None:
                continue
            href, size = plain.group(1), 0
        else:
            size = int(float(match.group("size"))  # type: ignore[union-attr]
                       * _UNITS[match.group("unit").upper()])  # type: ignore[union-attr]
        name = urllib.parse.unquote(href)
        if not name.lower().endswith(suffix):
            continue
        out[name[: -len(suffix)]] = size
    return out


def list_systems(timeout: float = 30.0) -> list[str]:
    """Directory names at the server root, i.e. libretro's system names."""
    html = fetch(server() + "/", timeout).decode("utf-8", "replace")
    return sorted(parse_index(html, suffix="/"))


def list_images(system: str, kind: str, timeout: float = 60.0) -> dict[str, int]:
    html = fetch(_url(system, kind), timeout).decode("utf-8", "replace")
    return parse_index(html)


def system_candidates(core_name: str, playlist: str) -> list[str]:
    """Plausible libretro system names for a playlist, best first.

    `default_core_name` reads "Nintendo - Nintendo 64 (Mupen64Plus-Next)":
    the trailing parenthesis is the core, the rest is very nearly the
    libretro system name. Two ways it is not exactly:

      * "Nintendo - GameCube / Wii" is one core covering two systems, which
        upstream splits into "Nintendo - GameCube" and "Nintendo - Wii".
      * arcade cores give "Arcade (MAME 2010)" -- see _ARCADE_CORES.
    """
    low = core_name.lower()
    out: list[str] = []
    for needle, system in _ARCADE_CORES:
        if needle in low:
            out.append(system)
            break

    platform = core_name.split("(", 1)[0].strip()
    if platform:
        out.append(platform)
        if " - " in platform:
            vendor, rest = platform.split(" - ", 1)
            if "/" in rest:
                out += [f"{vendor} - {p.strip()}" for p in rest.split("/")]

    # A playlist named the libretro way ("Nintendo - Nintendo 64.lpl") is
    # its own answer, and beats a mis-set core.
    out.append(playlist)
    return _unique(out)


def resolve_system(
    core_name: str, playlist: str, available: list[str]
) -> str | None:
    lookup = {s.casefold(): s for s in available}
    for candidate in system_candidates(core_name, playlist):
        found = lookup.get(candidate.casefold())
        if found is not None:
            return found
    return None


def base_title(name: str) -> str:
    """A name stripped of its regional/revision suffix, for loose matching.

    "Banjo-Kazooie (USA) (Rev 1)" and "Banjo-Kazooie (U) [!]" both reduce to
    "banjo-kazooie". Deliberately crude: this is only ever consulted after
    an exact match has failed, and box art is the one asset where collapsing
    variants is defensible.
    """
    cut = name.find(" (")
    if cut < 0:
        cut = name.find(" [")
    return (name if cut < 0 else name[:cut]).strip().casefold()


def _variant_rank(name: str) -> tuple[int, int, str]:
    low = name.casefold()
    for index, region in enumerate(_REGIONS):
        if region in low:
            return (index, len(name), name)
    return (len(_REGIONS), len(name), name)


def index_by_base(names: dict[str, int]) -> dict[str, str]:
    """base title -> the single upstream name to use for it."""
    grouped: dict[str, list[str]] = {}
    for name in names:
        grouped.setdefault(base_title(name), []).append(name)
    return {
        base: min(variants, key=_variant_rank)
        for base, variants in grouped.items()
    }


def candidate_names(
    label: str, rom: str, titles: dict[str, Title]
) -> list[str]:
    """Names worth looking up for one playlist entry, best first.

    The resolved MAME title leads, because for an arcade playlist the label
    is a set name that upstream has never heard of.
    """
    out: list[str] = []
    resolved = titles.get(Path(rom).stem)
    if resolved is not None and resolved.title:
        out.append(resolved.title)
    if label:
        out.append(label)
    stem = Path(rom).stem
    if stem:
        out.append(stem)
    return _unique(out)


def match_upstream(
    names: list[str], exact: dict[str, int], by_base: dict[str, str]
) -> str | None:
    """The upstream file name for one entry, or None if there is no art."""
    for name in names:
        escaped = art_name(name)
        if escaped in exact:
            return escaped
    for name in names:
        found = by_base.get(base_title(art_name(name)))
        if found is not None:
            return found
    return None


def plan_playlist(
    playlist: Path,
    items: list[dict[str, str]],
    core_name: str,
    kind: str,
    available: list[str],
    titles: dict[str, Title],
    dest_root: Path,
    system: str | None = None,
) -> Plan:
    out = Plan(playlist=playlist.stem, entries=len(items))

    found = system or resolve_system(core_name, playlist.stem, available)
    if found is None:
        out.note = (
            f"no upstream system matches core {core_name!r}"
            " -- pass --system to name one"
        )
        return out
    out.system = found

    try:
        upstream = list_images(found, kind)
    except (urllib.error.URLError, OSError, ValueError) as error:
        out.note = f"could not list {found}/{kind}: {error}"
        return out

    by_base = index_by_base(upstream)
    # Stored under the playlist stem because that is where art_index looks:
    # RetroArch files thumbnails by playlist, not by libretro system.
    target = dest_root / playlist.stem / kind

    for item in items:
        label = item.get("label", "")
        rom = item.get("path", "")
        if not rom:
            continue
        remote = match_upstream(
            candidate_names(label, rom, titles), upstream, by_base
        )
        if remote is None:
            out.missing += 1
            continue
        out.matched += 1
        # The local name is the one pegasus.entry_assets will ask for, which
        # is derived from the label -- never from the upstream name, which
        # may be a differently-regioned variant.
        local = target / (art_name(label or Path(rom).stem) + ".png")
        if local.is_file() and local.stat().st_size > 0:
            out.present += 1
            continue
        out.wanted.append(Image(
            url=_url(found, kind) + urllib.parse.quote(remote + ".png"),
            dest=local,
            size=upstream.get(remote, 0),
        ))

    return out


def _download(image: Image, timeout: float) -> str:
    """Fetch one image, or return a message describing the failure.

    Written to a sibling and renamed so an interrupted run cannot leave a
    truncated PNG behind -- a later run skips whatever is already present,
    so a half file would be permanent.
    """
    data = fetch(image.url, timeout)
    if not data:
        return "empty response"
    image.dest.parent.mkdir(parents=True, exist_ok=True)
    staging = image.dest.with_name(image.dest.name + ".part")
    staging.write_bytes(data)
    os.replace(staging, image.dest)
    return ""


def human(size: float) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024 or unit == "GB":
            return f"{size:.0f} {unit}" if unit == "B" else f"{size:.1f} {unit}"
        size /= 1024
    return f"{size:.1f} GB"


def run_plan(
    plan: Plan,
    workers: int = 8,
    timeout: float = 30.0,
    progress: bool = True,
    out: TextIO | None = None,
) -> Report:
    """Download everything a plan wants, tolerating per-file failure.

    Failures are counted rather than raised: a playlist of 8302 entries is
    long enough that losing the network partway through is the expected
    case, not an exceptional one, and every file already written stays
    valid and is skipped on the next run.
    """
    stream: TextIO = out if out is not None else sys.stderr
    report = Report(plan=plan)
    total = len(plan.wanted)
    if not total:
        return report

    done = 0
    last = 0.0
    interactive = bool(getattr(stream, "isatty", lambda: False)())

    def emit(final: bool = False) -> None:
        nonlocal last
        if not progress:
            return
        line = (f"  {plan.playlist}: {done}/{total}"
                f"  {report.downloaded} ok  {report.failed} failed")
        if interactive:
            # Rate-limited, because a redraw per file is most of the work at
            # 8 workers against a fast mirror.
            now = time.monotonic()
            if not final and now - last < 0.5:
                return
            last = now
            print("\r" + line.ljust(70), end="", file=stream, flush=True)
            if final:
                print(file=stream)
            return
        # Redirected to a log or a pipe: gate on count only. Gating on time
        # as well silently drops nearly every milestone, because the two
        # conditions almost never coincide -- which is how a 5000-file run
        # came to print no progress at all.
        if final or done % 250 == 0:
            print(line, file=stream, flush=True)

    with ThreadPoolExecutor(max_workers=max(1, workers)) as pool:
        for image, result in zip(
            plan.wanted,
            pool.map(lambda i: _safe(i, timeout), plan.wanted),
        ):
            done += 1
            if result:
                report.failed += 1
                if not report.reason:
                    report.reason = f"{image.dest.name}: {result}"
            else:
                report.downloaded += 1
            emit()
    emit(final=True)
    return report


def _safe(image: Image, timeout: float) -> str:
    try:
        return _download(image, timeout)
    except (urllib.error.URLError, OSError, ValueError) as error:
        return str(error)


def prune_partials(root: Path) -> int:
    """Remove `.part` leftovers from a run killed mid-write."""
    removed = 0
    for stray in root.rglob("*.png.part"):
        try:
            stray.unlink()
            removed += 1
        except OSError:
            pass
    return removed


def read_playlist_items(path: Path) -> tuple[list[dict[str, str]], str]:
    import json

    try:
        data = json.loads(path.read_text(errors="replace"))
    except (OSError, ValueError):
        return [], ""
    if not isinstance(data, dict):
        return [], ""
    items = data.get("items")
    if not isinstance(items, list):
        return [], ""
    return items, str(data.get("default_core_name", ""))


def plan_all(
    playlist_dir: Path,
    kind: str = KINDS[0],
    dest_root: Path | None = None,
    only: list[str] | None = None,
    system: str | None = None,
) -> tuple[list[Plan], str]:
    """Plan every playlist. Second element is a fatal error, if any."""
    dest = dest_root if dest_root is not None else thumbnail_dir()
    try:
        available = list_systems()
    except (urllib.error.URLError, OSError, ValueError) as error:
        return [], f"cannot reach {server()}: {error}"

    titles = find_titles()
    plans: list[Plan] = []
    for playlist in sorted(playlist_dir.glob("*.lpl")):
        if only and playlist.stem not in only:
            continue
        items, core = read_playlist_items(playlist)
        if not items:
            continue
        plans.append(plan_playlist(
            playlist, items, core, kind, available, titles, dest, system,
        ))
    return plans, ""


def free_space(path: Path) -> int:
    probe = path
    while not probe.exists() and probe != probe.parent:
        probe = probe.parent
    try:
        return shutil.disk_usage(probe).free
    except OSError:
        return 0
