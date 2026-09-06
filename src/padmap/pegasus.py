"""Generate Pegasus collections from RetroArch playlists.

RetroArch `.lpl` files already hold everything Pegasus needs -- ROM paths, a
per-playlist default core, and labels -- so a library can be built without
rescanning or hand-writing metadata.

Two details the playlists make you handle:

  * `core_path` is often the literal string `"DETECT"`, meaning "use the
    playlist default". Emitting that verbatim produces an unlaunchable entry.
  * arcade labels are MAME set names; see titles.py.

Launch lines point at the `padmap-play` wrapper rather than at `retroarch`.
A front-end spawns games with its own environment, and under Nix RetroArch is
generally not on the system PATH, so a bare `retroarch` fails with "could not
launch". The wrapper carries RetroArch and the --appendconfig override that
binds the assigned controller order.

Artwork, when there is any, is borrowed from RetroArch's thumbnail tree
rather than downloaded again; see `art_index`.
"""

from __future__ import annotations

import json
import os
import shlex
from dataclasses import dataclass, field
from pathlib import Path

from .titles import Title, find_titles, resolve

DETECT = "DETECT"

# Set by the flake wrapper to an absolute padmap-play path.
ENV_PLAY = "PADMAP_PLAY"

# Override for the RetroArch thumbnail tree, mainly so tests can point at a
# fixture. RetroArch's own default is $XDG_CONFIG_HOME/retroarch/thumbnails
# and its `thumbnails_directory` setting is left at that here.
ENV_THUMBNAILS = "PADMAP_THUMBNAILS"

# RetroArch's thumbnail kinds, paired with the Pegasus asset key each one
# feeds. Order matters only in that boxFront is what a grid actually shows.
#
# All three keys were confirmed against the Pegasus binary this flake builds,
# by loading a fixture collection that declared them and reading them back
# from a theme. Worth doing rather than trusting the documentation: the
# binary contains the literal string "assets.boxFront" but not
# "assets.titlescreen", because the parser assembles some keys at runtime --
# so grepping it suggests two of these three are unsupported, and they are
# not.
ART_KINDS = (
    ("Named_Boxarts", "boxFront"),
    ("Named_Titles", "titlescreen"),
    ("Named_Snaps", "screenshot"),
)


@dataclass
class Entry:
    title: str
    path: str
    year: str = ""
    manufacturer: str = ""
    # MAME driver grade, "" for anything not in the table. Surfaced to the
    # theme rather than acted on here: whether a preliminary driver is worth
    # hiding, dimming or just labelling is a presentation decision.
    status: str = ""
    # Pegasus asset key -> absolute image path, for whatever art was found.
    assets: dict[str, str] = field(default_factory=dict)


@dataclass
class Collection:
    name: str
    core_path: str
    entries: list[Entry]
    extensions: str = ""


def _display_name(playlist: Path, data: dict) -> str:
    """A human name for the collection.

    `default_core_name` reads like "Nintendo - Nintendo 64 (Mupen64Plus-Next)";
    the part before the dash is the platform, which is what belongs on a
    collection. Falls back to the file stem.
    """
    core_name = data.get("default_core_name", "")
    if " - " in core_name:
        platform = core_name.split(" - ", 1)[1]
        # Drop the trailing "(Core Name)" if present.
        if "(" in platform:
            platform = platform.split("(", 1)[0]
        platform = platform.strip()
        if platform:
            return platform
    return playlist.stem.replace("_", " ").title()


def thumbnail_dir() -> Path:
    return Path(
        os.environ.get(ENV_THUMBNAILS)
        or Path(
            os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")
        ) / "retroarch" / "thumbnails"
    )


def art_name(label: str) -> str:
    """RetroArch's on-disk name for a playlist label.

    RetroArch stores thumbnails under the label, with the characters that are
    awkward in a filename replaced by underscore. This mirrors that so a
    thumbnail pack already downloaded through RetroArch is reused as-is.
    """
    out = label
    for bad in "&*/:`<>?\\|":
        out = out.replace(bad, "_")
    return out


def art_index(playlist_stem: str, root: Path | None = None) -> dict[str, dict[str, str]]:
    """Available art for one playlist: on-disk name -> {asset key: path}.

    Built by listing each Named_* directory once instead of stat-ing per
    game. The arcade playlist has 8302 entries and three kinds of art, so
    the probe-per-entry version is 25k syscalls to answer a question three
    readdir calls answer.

    Indexing what is *actually* present, rather than trusting `art_name` to
    reproduce RetroArch's escaping exactly, is deliberate: if the convention
    is wrong the result is no art, never a metadata file full of paths that
    do not resolve. Entries are also keyed by their unescaped stem, so a pack
    laid down by hand still matches.
    """
    root = root if root is not None else thumbnail_dir()
    found: dict[str, dict[str, str]] = {}

    for directory, asset in ART_KINDS:
        try:
            files = list((root / playlist_stem / directory).iterdir())
        except OSError:
            continue
        for image in files:
            if image.suffix.lower() != ".png":
                continue
            found.setdefault(image.stem, {}).setdefault(asset, str(image))

    return found


def entry_assets(
    label: str, rom: str, index: dict[str, dict[str, str]]
) -> dict[str, str]:
    """Art for one playlist entry, by label and then by ROM basename.

    Both are tried because they routinely differ: arcade labels are MAME set
    names while the ROM is `<set>.zip`, and a user who renamed a label has
    thumbnails under the old one.
    """
    if not index:
        return {}
    for key in (art_name(label), label, Path(rom).stem):
        if key and key in index:
            return index[key]
    return {}


def read_playlist(
    path: Path, titles: dict[str, Title] | None = None
) -> Collection | None:
    try:
        data = json.loads(path.read_text(errors="replace"))
    except (OSError, ValueError):
        return None
    if not isinstance(data, dict) or "items" not in data:
        return None

    titles = titles if titles is not None else {}
    default_core = data.get("default_core_path", "")
    # RetroArch files thumbnails under the playlist name, so the tree is
    # keyed by the same stem regardless of what the collection ends up
    # called for display.
    art = art_index(path.stem)

    entries: list[Entry] = []
    for item in data["items"]:
        rom = item.get("path", "")
        if not rom:
            continue
        label = item.get("label", "")
        info = resolve(label, rom, titles)
        entries.append(Entry(
            title=info.title,
            path=rom,
            year=info.year,
            manufacturer=info.manufacturer,
            status=info.status,
            assets=entry_assets(label, rom, art),
        ))

    return Collection(
        name=_display_name(path, data),
        core_path=default_core,
        entries=entries,
        extensions=data.get("scan_file_exts", ""),
    )


def data_dir() -> Path:
    return Path(
        os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share")
    ) / "padmap"


def player_link() -> Path:
    """A stable path that always points at the current padmap-play.

    Launch lines must not contain a Nix store path. `export-pegasus` writes
    them once, but the store path changes on every rebuild, so the
    collections keep invoking whichever padmap-play existed when they were
    exported. That failed silently and expensively: a launcher predating
    `--nodevice` went on being used long after the flake had moved on, so
    every N64 game still got four controllers while the daemon, launch.cfg
    and launch.args were all perfectly current.

    Indirecting through a symlink padmap maintains means a rebuild fixes the
    collections without re-exporting them.
    """
    return data_dir() / "bin" / "padmap-play"


def install_player_link() -> Path | None:
    """Point the stable launcher path at the current padmap-play.

    Returns the link, or None when there is nothing to point it at (a dev
    shell with PADMAP_PLAY unset), in which case callers fall back to the
    raw value.
    """
    target = os.environ.get(ENV_PLAY)
    if not target:
        return None

    link = player_link()
    link.parent.mkdir(parents=True, exist_ok=True)
    if link.is_symlink() and os.readlink(link) == target:
        return link

    # Replace atomically: Pegasus may be spawning a game through this very
    # path, and an unlink/symlink pair leaves a window where it does not
    # exist.
    staging = link.with_name(link.name + ".new")
    staging.unlink(missing_ok=True)
    os.symlink(target, staging)
    os.replace(staging, link)
    return link


def player_command() -> str:
    """Absolute path to the launcher a front-end should spawn.

    Pegasus inherits only its own environment, so a bare "retroarch" in the
    launch line fails with "could not launch 'retroarch'" unless RetroArch
    happens to be on the system PATH -- which, under Nix, it typically is not.
    PADMAP_PLAY points at a wrapper carrying RetroArch and the --appendconfig
    override with it.

    Prefers the stable symlink over PADMAP_PLAY itself, so what lands in the
    metadata survives a rebuild. See player_link().
    """
    link = install_player_link()
    if link is not None:
        return str(link)
    return os.environ.get(ENV_PLAY) or "retroarch"


def launch_line(core_path: str) -> str:
    """The command a front-end runs for a game in this collection."""
    parts = [player_command()]
    if core_path and core_path != DETECT:
        parts += ["-L", core_path]
    # Quoted because ROM paths routinely contain spaces, brackets and commas.
    return " ".join(shlex.quote(p) for p in parts) + ' "{file.path}"'


def game_dirs_file() -> Path:
    return Path(
        os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")
    ) / "pegasus-frontend" / "game_dirs.txt"


def stale_collections() -> list[Path]:
    """Installed collections whose launch line bypasses the stable link.

    Anything exported before `player_link()` existed still names the
    padmap-play that was current that day, and will go on invoking it
    forever. Nothing else reveals this: the daemon can be current, launch.cfg
    and launch.args correct, and every game still launched by a wrapper from
    months ago.
    """
    config = game_dirs_file()
    if not config.is_file():
        return []

    wanted = str(player_link())
    stale: list[Path] = []
    for line in config.read_text(errors="replace").splitlines():
        directory = line.strip()
        if not directory:
            continue
        metadata = Path(directory) / "metadata.pegasus.txt"
        if not metadata.is_file():
            continue
        for entry in metadata.read_text(errors="replace").splitlines():
            if not entry.startswith("launch:"):
                continue
            parts = shlex.split(entry[len("launch:"):].strip())
            if parts and parts[0] != wanted:
                stale.append(metadata)
            break
    return stale


def render(collection: Collection) -> str:
    lines = [
        f"collection: {collection.name}",
    ]
    if collection.extensions:
        lines.append(f"extensions: {collection.extensions}")
    lines.append(f"launch: {launch_line(collection.core_path)}")
    lines.append("")

    for entry in collection.entries:
        lines.append(f"game: {entry.title}")
        lines.append(f"file: {entry.path}")
        if entry.year:
            lines.append(f"release: {entry.year}")
        if entry.manufacturer:
            lines.append(f"developer: {entry.manufacturer}")
        # `x-` is Pegasus's own extension escape hatch: PegasusMetadata.cpp
        # keeps any key starting with it and exposes the rest of the name
        # under `game.extra`. Nothing built in carries "this driver does not
        # work", and inventing a use for `genre` or `description` would put
        # it somewhere a user might reasonably want to edit.
        #
        # Only emitted when MAME actually graded the set. Absent means
        # unknown, which the theme must not read as broken -- almost nothing
        # outside arcade has a grade at all.
        if entry.status:
            lines.append(f"x-mame-status: {entry.status}")
        # Emitted only when the file exists, so the theme can treat "has a
        # boxFront" as "has art to show" and pick its layout from that.
        for _, asset in ART_KINDS:
            image = entry.assets.get(asset)
            if image:
                lines.append(f"assets.{asset}: {image}")
        lines.append("")

    return "\n".join(lines)


def export(
    playlist_dir: Path,
    out_dir: Path,
) -> tuple[list[tuple[str, int, int, int]], list[Path]]:
    """Write one metadata.pegasus.txt per collection, each in its own dir.

    Emphatically *not* one file with several `collection:` blocks. Pegasus's
    parser adds every `game:` to every collection declared so far in the same
    file (PegasusMetadata.cpp, `for (coll : ps.all_colls) game_add_to(...)`),
    and `game_add_to` only fills a launch command when it is still empty. So
    in a merged file every game inherits the *first* collection's launcher --
    which silently ran N64 and GameCube ROMs with the arcade core.

    Separate files keep each parser run's `all_colls` to a single collection,
    which is the only way the association is unambiguous.

    The cost is that each directory must be listed in game_dirs.txt, because
    PegasusProvider builds its QDirIterator without QDirIterator::
    Subdirectories and never looks below a listed directory. Returns those
    paths so the caller can write that file.
    """
    titles = find_titles()
    results: list[tuple[str, int, int, int]] = []
    written: list[Path] = []

    for playlist in sorted(playlist_dir.glob("*.lpl")):
        collection = read_playlist(playlist, titles)
        if collection is None or not collection.entries:
            continue

        target = out_dir / playlist.stem
        target.mkdir(parents=True, exist_ok=True)
        (target / "metadata.pegasus.txt").write_text(render(collection))
        written.append(target)

        resolved = sum(
            1 for e in collection.entries
            if Path(e.path).stem in titles
        )
        illustrated = sum(1 for e in collection.entries if e.assets)
        results.append((
            collection.name, len(collection.entries), resolved, illustrated,
        ))

    # A stale merged file from an earlier export would still be read, and
    # would reintroduce exactly the bug this layout avoids.
    legacy = out_dir / "metadata.pegasus.txt"
    if legacy.is_file():
        legacy.unlink()

    return results, written
