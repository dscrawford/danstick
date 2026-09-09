#!/usr/bin/env python3
"""S22: the exporter -- what lands in the collections Pegasus actually reads.

`padmap export-pegasus` is the only part of padmap whose output outlives the
process by months. Everything else is recomputed on the next run; a
`metadata.pegasus.txt` is written once and then read by Pegasus at every boot
until somebody exports again. That is why this module shipped the worst bug in
FINDINGS: the launch lines named a Nix store path, so collections exported one
afternoon went on invoking *that* padmap-play forever. The daemon was current,
`launch.cfg` was correct, `launch.args` was correct, and every N64 game still
came up with four players, because the binary being run was not the binary
that had been fixed. `install_player_link()` and `stale_collections()` exist
solely to make that class of failure impossible and then visible.

So the checks here are about the two properties that are invisible on the day
they break:

  * **What is in the file.** One directory per collection and never one file
    with several `collection:` blocks -- Pegasus's parser adds every `game:`
    to every collection seen so far in the same file and only fills a launch
    command when it is still empty, so a merged file makes every game inherit
    the *first* collection's launcher. That is how an N64 ROM was launched
    with the MAME arcade core. Every field on one line, `x-console` and
    `x-gamekey` present exactly when they are derivable, `assets.*` only for
    art that is really there, and the whole thing parseable by the rules it
    was written to.

  * **What the launch line names.** The stable symlink, never the store path
    behind it -- through a fresh install, a rebuild, a link that is already
    right, one that points somewhere else, one left dangling and one replaced
    by a regular file. Plus the detector: a collection that names a store path
    is reported, and one that names the link is not.

Nothing here touches real user state or hardware. XDG_RUNTIME_DIR,
XDG_CONFIG_HOME and XDG_DATA_HOME are redirected into a temp tree *before*
padmap is imported, PADMAP_PLAY and PADMAP_THUMBNAILS point into it, no
command is dispatched to the live daemon, `devices.discover` is replaced, and
the real `~/.local/share/padmap/bin/padmap-play` and
`~/.config/pegasus-frontend/game_dirs.txt` are fingerprinted at the start and
re-checked at the end so a check that forgot the redirect cannot pass quietly.

Lines marked "gap:" are defects this file found and reports rather than fixes;
each names its reproduction. Everything else is asserted.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \\
        nix develop --command python3 tools/check_exporter.py
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import os
import re
import shlex
import shutil
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# ---------------------------------------------------------------------------
# Sandbox. Everything below must be in place before padmap is imported: cli
# resolves its state directory from XDG_RUNTIME_DIR at import time, and a live
# daemon owns the controllers and the real launcher symlink on this machine.
# ---------------------------------------------------------------------------

_SANDBOX = Path(tempfile.mkdtemp(prefix="padmap-exporter-"))


def _fingerprint(path: Path) -> object:
    """Enough of a file's identity to notice this run having touched it."""
    try:
        if path.is_symlink():
            return ("symlink", os.readlink(path))
        if path.is_file():
            return ("file", hashlib.md5(path.read_bytes()).hexdigest())
    except OSError as error:
        return ("error", type(error).__name__)
    return ("absent",)


_REAL_LINK = Path(
    os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share")
) / "padmap" / "bin" / "padmap-play"
_REAL_GAME_DIRS = Path(
    os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")
) / "pegasus-frontend" / "game_dirs.txt"
_REAL_BEFORE = (_fingerprint(_REAL_LINK), _fingerprint(_REAL_GAME_DIRS))

for _var, _sub in (("XDG_RUNTIME_DIR", "runtime"),
                   ("XDG_CONFIG_HOME", "config"),
                   ("XDG_DATA_HOME", "data")):
    _path = _SANDBOX / _sub
    _path.mkdir(parents=True, exist_ok=True)
    os.environ[_var] = str(_path)

# A store path of exactly the shape the real wrapper has, so "the launch line
# must not be this" is testing the thing that actually went wrong.
STORE_PLAY = "/nix/store/0000000000000000000000000000000a-padmap-play/bin/padmap-play"
REBUILT_PLAY = "/nix/store/0000000000000000000000000000000b-padmap-play/bin/padmap-play"

os.environ["PADMAP_PLAY"] = STORE_PLAY
os.environ["PADMAP_THUMBNAILS"] = str(_SANDBOX / "thumbnails")

# A tiny MAME table instead of the 8302-entry one the dev shell exports, so
# release/developer/x-mame-status are asserted against known values rather
# than against whatever the store happens to hold.
TITLES = _SANDBOX / "mame-titles.json"
TITLES.write_text(json.dumps({
    "pacman": ["Pac-Man (Midway)", "1980", "Namco (Midway license)", "good"],
    "polyplay": ["Poly-Play", "1985", "VEB Polytechnik", "preliminary"],
}))
os.environ["PADMAP_MAME_TITLES"] = str(TITLES)

sys.path.insert(0, str(REPO / "src"))

from padmap import cli, devices, pegasus  # noqa: E402

# Belt and braces: nothing here dispatches a command, but a stray discover()
# would open the real event nodes the live daemon is holding.
devices.discover = lambda *args, **kwargs: []  # type: ignore[assignment]

N64_CORE = "/nix/store/cores/mupen64plus_next_libretro.so"
ARCADE_CORE = "/nix/store/cores/mame2010_libretro.so"
UNKNOWN_CORE = "/nix/store/cores/frobnicator_libretro.so"

CHECKS = 0


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def heading(text: str) -> None:
    print(f"\n{text}:")


def ok(text: str) -> None:
    global CHECKS
    CHECKS += 1
    print(f"  ok    {text}")


def gap(text: str) -> None:
    print(f"  gap:  {text}")


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def holds(got: object, want: object, held: str, broken: str) -> None:
    """Assert, saying what the user loses when it does not hold."""
    if got != want:
        fail(f"{broken} (got {got!r}, wanted {want!r})")
    ok(held)


def true(got: object, held: str, broken: str) -> None:
    if not got:
        fail(broken)
    ok(held)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


def fresh(name: str) -> Path:
    """A private XDG tree for one scenario.

    Per scenario rather than per run, because half of these are about what
    `~/.local/share/padmap/bin` already contained when install_player_link ran.
    """
    root = _SANDBOX / name
    if root.exists():
        # A scenario that made a directory unwritable restores it itself; this
        # is only here so a re-run of one is not poisoned by the last.
        for directory, _, _ in os.walk(root):
            os.chmod(directory, 0o700)
        shutil.rmtree(root, ignore_errors=True)
    for sub in ("config", "data"):
        (root / sub).mkdir(parents=True)
    os.environ["XDG_CONFIG_HOME"] = str(root / "config")
    os.environ["XDG_DATA_HOME"] = str(root / "data")
    os.environ["PADMAP_THUMBNAILS"] = str(root / "thumbnails")
    os.environ["PADMAP_PLAY"] = STORE_PLAY
    return root


def write_playlist(
    directory: Path,
    stem: str,
    core: str,
    core_name: str = "",
    items: list[dict] | None = None,
    exts: str = "",
) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    body: dict = {
        "default_core_path": core,
        "default_core_name": core_name,
        "items": items if items is not None else [],
    }
    if exts:
        body["scan_file_exts"] = exts
    path = directory / f"{stem}.lpl"
    path.write_text(json.dumps(body, indent=2))
    return path


def art(root: Path, playlist_stem: str, kind: str, name: str) -> Path:
    """One thumbnail where RetroArch would have put it."""
    directory = root / "thumbnails" / playlist_stem / kind
    directory.mkdir(parents=True, exist_ok=True)
    image = directory / f"{name}.png"
    image.write_bytes(b"\x89PNG\r\n\x1a\n")
    return image


def install(collection_dir: Path, text: str) -> Path:
    """A collection already on disk, listed in game_dirs.txt."""
    collection_dir.mkdir(parents=True, exist_ok=True)
    metadata = collection_dir / "metadata.pegasus.txt"
    metadata.write_text(text)
    config = pegasus.game_dirs_file()
    config.parent.mkdir(parents=True, exist_ok=True)
    listed = config.read_text().splitlines() if config.is_file() else []
    listed.append(str(collection_dir))
    config.write_text("\n".join(listed) + "\n")
    return metadata


# ---------------------------------------------------------------------------
# A parser for the format the exporter writes, by the rules it writes it to.
#
# Deliberately strict and independent of render(): a line that does not match
# is a line Pegasus would drop or misread, which is exactly the failure that
# has to be visible here rather than as a missing game on a TV.
# ---------------------------------------------------------------------------

FIELD = re.compile(r"^([A-Za-z][A-Za-z0-9]*(?:[.-][A-Za-z0-9]+)*): ?(.*)$")

# Every key the exporter is allowed to emit. A new one that nothing reads is
# as invisible as a missing one, so the set is pinned rather than sampled.
KNOWN_KEYS = {
    "collection", "extensions", "launch",
    "game", "file", "release", "developer",
    "x-mame-status", "x-console", "x-gamekey",
    "assets.boxFront", "assets.titlescreen", "assets.screenshot",
}


def parse(text: str) -> list[list[tuple[str, str]]]:
    """Blocks of key/value pairs, split on blank lines, in order."""
    blocks: list[list[tuple[str, str]]] = []
    current: list[tuple[str, str]] = []
    for number, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            if current:
                blocks.append(current)
                current = []
            continue
        found = FIELD.match(line)
        if not found:
            raise ValueError(f"line {number} is not a metadata field: {line!r}")
        current.append((found.group(1), found.group(2)))
    if current:
        blocks.append(current)
    return blocks


def keys(block: list[tuple[str, str]]) -> list[str]:
    return [key for key, _ in block]


def value(block: list[tuple[str, str]], key: str) -> str | None:
    for name, text in block:
        if name == key:
            return text
    return None


# ---------------------------------------------------------------------------
# Where the exporter keeps things
# ---------------------------------------------------------------------------


def check_paths_are_where_the_story_says() -> None:
    heading("S22: the two paths everything else depends on")

    root = fresh("paths")

    holds(pegasus.data_dir(), root / "data" / "padmap",
          "padmap's data directory follows XDG_DATA_HOME",
          "padmap wrote its launcher symlink outside XDG_DATA_HOME, so a "
          "test or a sandboxed run silently repoints the real one")
    holds(pegasus.player_link(),
          root / "data" / "padmap" / "bin" / "padmap-play",
          "the stable launcher is ~/.local/share/padmap/bin/padmap-play",
          "the stable launcher path moved: every collection ever exported "
          "names the old one and would keep invoking a launcher padmap no "
          "longer maintains")
    holds(pegasus.game_dirs_file(),
          root / "config" / "pegasus-frontend" / "game_dirs.txt",
          "game_dirs.txt is ~/.config/pegasus-frontend/game_dirs.txt",
          "padmap listed its collections somewhere Pegasus does not read, so "
          "an export appears to succeed and the library stays empty")


# ---------------------------------------------------------------------------
# install_player_link
# ---------------------------------------------------------------------------


def check_link_absent_without_a_launcher() -> None:
    heading("S22: no PADMAP_PLAY means no link, not a broken one")

    root = fresh("link-unset")
    for value_ in (None, ""):
        if value_ is None:
            os.environ.pop("PADMAP_PLAY", None)
        else:
            os.environ["PADMAP_PLAY"] = value_
        label = "unset" if value_ is None else "empty"
        holds(pegasus.install_player_link(), None,
              f"install_player_link returns None when PADMAP_PLAY is {label}",
              f"install_player_link invented a link with PADMAP_PLAY {label}, "
              f"so every exported launch line points at nothing")
    true(not (root / "data" / "padmap" / "bin").exists(),
         "no bin directory is created when there is nothing to point it at",
         "install_player_link created ~/.local/share/padmap/bin containing no "
         "launcher, which reads as an install that worked")
    holds(pegasus.player_command(), "retroarch",
          "player_command falls back to a bare retroarch in a dev shell",
          "player_command returned something other than retroarch with no "
          "launcher available, so the fallback path is untested guesswork")
    os.environ["PADMAP_PLAY"] = STORE_PLAY


def check_link_is_created() -> None:
    heading("S22: a first export installs the stable launcher")

    fresh("link-create")
    link = pegasus.install_player_link()

    holds(link, pegasus.player_link(),
          "install_player_link returns the stable path",
          "install_player_link returned a path that is not player_link(), so "
          "the launch lines and the link the daemon repoints disagree")
    true(link is not None and link.is_symlink(),
         "the stable path is a symlink",
         "the stable launcher was installed as something other than a "
         "symlink, so a rebuild cannot repoint it")
    holds(os.readlink(pegasus.player_link()), STORE_PLAY,
          "it points at PADMAP_PLAY",
          "the stable launcher points somewhere other than the current "
          "padmap-play, so games run through the wrong wrapper")
    holds(sorted(p.name for p in pegasus.player_link().parent.iterdir()),
          ["padmap-play"],
          "no staging file is left behind",
          "install_player_link left padmap-play.new in the bin directory, "
          "which is a half-finished install a user has to clean up")


def check_link_is_idempotent() -> None:
    heading("S22: an already-correct link is left alone")

    fresh("link-same")
    pegasus.install_player_link()
    before = pegasus.player_link().lstat().st_ino
    pegasus.install_player_link()

    holds(pegasus.player_link().lstat().st_ino, before,
          "a second call does not replace an already-correct link",
          "install_player_link recreates the link on every call, so Pegasus "
          "can spawn a game through a path that is being unlinked")
    holds(os.readlink(pegasus.player_link()), STORE_PLAY,
          "and it still points at the current padmap-play",
          "an idempotent call broke the link it was meant to leave alone")


def check_link_repoints_after_a_rebuild() -> None:
    heading("S22: a rebuild repoints the link -- the four-players fix")

    fresh("link-repoint")
    pegasus.install_player_link()
    os.environ["PADMAP_PLAY"] = REBUILT_PLAY
    link = pegasus.install_player_link()

    holds(os.readlink(pegasus.player_link()), REBUILT_PLAY,
          "the link follows PADMAP_PLAY to the rebuilt padmap-play",
          "the link kept pointing at the padmap-play from before the rebuild: "
          "this is the failure that left N64 games with four controllers "
          "while every fix for it was already on disk")
    true(link is not None and link.is_symlink(),
         "and it is still a symlink afterwards",
         "repointing left the stable path as something other than a symlink")
    holds(sorted(p.name for p in pegasus.player_link().parent.iterdir()),
          ["padmap-play"],
          "repointing leaves no staging file behind",
          "padmap-play.new survived a repoint")
    os.environ["PADMAP_PLAY"] = STORE_PLAY


def check_link_over_hostile_targets() -> None:
    heading("S22: the stable path is claimed whatever was there before")

    cases = {
        "a regular file": lambda link: link.write_text("#!/bin/sh\nexit 1\n"),
        "a dangling symlink": lambda link: os.symlink("/gone/padmap-play", link),
        "a symlink to a stale store path": lambda link: os.symlink(
            REBUILT_PLAY, link),
        "a stale staging file": lambda link: (
            link.with_name("padmap-play.new").write_text("junk")),
    }
    for label, prepare in cases.items():
        root = fresh("link-over")
        link = pegasus.player_link()
        link.parent.mkdir(parents=True)
        prepare(link)
        pegasus.install_player_link()

        true(link.is_symlink() and os.readlink(link) == STORE_PLAY,
             f"the link is installed over {label}",
             f"install_player_link did not claim the stable path when it "
             f"already held {label}: games keep launching through whatever "
             f"was there instead of the current padmap-play")
        holds(sorted(p.name for p in link.parent.iterdir()), ["padmap-play"],
              f"and nothing is left beside it after {label}",
              f"install_player_link left debris beside the link after {label}")
        assert root.exists()


def check_link_when_the_bin_directory_is_read_only() -> None:
    heading("S22: a launcher directory that cannot be written")

    if os.geteuid() == 0:
        print("  --    skipped: running as root, permissions do not apply")
        return

    root = fresh("link-readonly")
    pegasus.install_player_link()
    link = pegasus.player_link()
    binary_dir = link.parent
    os.chmod(binary_dir, 0o500)
    try:
        # Nothing to do: the link is already right, so the early return means
        # a read-only directory is survivable when it does not need writing.
        holds(pegasus.install_player_link(), link,
              "an already-correct link is returned without writing anything",
              "install_player_link tried to rewrite a link that was already "
              "correct, so a read-only ~/.local/share fails even when there "
              "is nothing to change")

        os.environ["PADMAP_PLAY"] = REBUILT_PLAY
        raised: BaseException | None = None
        try:
            pegasus.install_player_link()
        except BaseException as error:  # noqa: BLE001 - reporting, not handling
            raised = error

        if isinstance(raised, PermissionError):
            gap("install_player_link() raises PermissionError when "
                "~/.local/share/padmap/bin is not writable -- os.symlink() "
                "and os.replace() have no guard, and the docstring's "
                "contract is that the caller gets None and falls back. It "
                "propagates through player_command() -> launch_line() -> "
                "render() -> export(), so `padmap export-pegasus` dies with "
                "a traceback and writes nothing, and `padmap ensure-daemon` "
                "dies before the daemon or the front-end is started at all. "
                "Repro: chmod 500 ~/.local/share/padmap/bin, rebuild so "
                "PADMAP_PLAY changes, run `padmap ensure-daemon`.")
            # What is unambiguous either way: the launcher that was already
            # installed must survive the attempt. A half-done repoint that
            # unlinked the old one would leave Pegasus with no launcher.
            true(link.is_symlink() and os.readlink(link) == STORE_PLAY,
                 "the previously installed launcher survives the failed "
                 "repoint",
                 "a failed repoint destroyed the working launcher: every "
                 "game in the library stops starting, and the collections "
                 "still name the path that is now gone")
            holds(sorted(p.name for p in binary_dir.iterdir()),
                  ["padmap-play"],
                  "and no staging file is left in the way of the next repoint",
                  "a failed repoint left padmap-play.new behind, so the next "
                  "attempt has to clean up after this one")
        elif raised is None:
            fail("install_player_link repointed the link inside a directory "
                 "with no write permission, which cannot have happened -- the "
                 "read-only case is not being exercised at all")
        else:
            fail(f"install_player_link raised {type(raised).__name__} rather "
                 f"than PermissionError on a read-only bin directory; "
                 f"`padmap ensure-daemon` fails in a way nothing has looked at")
    finally:
        os.chmod(binary_dir, 0o700)
        os.environ["PADMAP_PLAY"] = STORE_PLAY
    assert root.exists()


def check_player_command_prefers_the_link() -> None:
    heading("S22: what a launch line is allowed to name")

    fresh("player-command")
    command = pegasus.player_command()

    holds(command, str(pegasus.player_link()),
          "player_command returns the stable link, not PADMAP_PLAY",
          "player_command handed back the raw store path: this is precisely "
          "the bug that baked a Nix store path into every collection and made "
          "months of launcher fixes invisible")
    true("/nix/store/" not in command,
         "and there is no store path in it",
         "a Nix store path reached the launch line, which cannot survive the "
         "next rebuild")


# ---------------------------------------------------------------------------
# launch_line and render
# ---------------------------------------------------------------------------


def check_launch_line_shape() -> None:
    heading("S22: the launch line a front-end runs")

    fresh("launch-line")
    link = str(pegasus.player_link())

    holds(pegasus.launch_line(N64_CORE),
          f'{link} -L {N64_CORE} "{{file.path}}"',
          "a core is passed with -L and the ROM as {file.path}",
          "the launch line does not name the core and the ROM the way "
          "RetroArch expects, so nothing starts")
    holds(pegasus.launch_line(pegasus.DETECT), f'{link} "{{file.path}}"',
          "the literal DETECT is never passed as a core",
          "'DETECT' was emitted as a core path, which produces an "
          "unlaunchable entry for every game in the playlist")
    holds(pegasus.launch_line(""), f'{link} "{{file.path}}"',
          "a playlist with no default core launches without -L",
          "an empty core path produced a -L with nothing after it")
    holds(pegasus.launch_line("/cores/my core.so"),
          f'{link} -L \'/cores/my core.so\' "{{file.path}}"',
          "a core path with a space is quoted",
          "a core path containing a space was emitted unquoted, so RetroArch "
          "is handed two broken arguments instead of one core")


def n64_collection() -> pegasus.Collection:
    return pegasus.Collection(
        name="Nintendo 64",
        core_path=N64_CORE,
        console="n64",
        extensions="z64|n64|v64",
        entries=[pegasus.Entry(
            title="Super Mario 64 (USA)",
            path="/roms/n64/Super Mario 64 (USA).z64",
            year="1996",
            manufacturer="Nintendo",
            status="good",
            key="n64/super-mario-64-usa",
            assets={"boxFront": "/art/box.png", "screenshot": "/art/snap.png"},
        )],
    )


def check_render_emits_every_field_once_and_on_one_line() -> None:
    heading("S22: every field the theme reads, one per line")

    fresh("render-fields")
    text = pegasus.render(n64_collection())
    blocks = parse(text)

    holds(len(blocks), 2,
          "a collection renders as a collection block and one game block",
          "the rendered collection did not split into a header and a game, so "
          "Pegasus cannot tell the collection from its games")
    holds(keys(blocks[0]), ["collection", "extensions", "launch"],
          "the header carries collection, extensions and launch in that order",
          "the collection header is missing a field or emits them out of "
          "order, so the collection loses its name, its extensions or its "
          "launcher")
    holds(keys(blocks[1]),
          ["game", "file", "release", "developer", "x-mame-status",
           "x-console", "x-gamekey", "assets.boxFront", "assets.screenshot"],
          "a fully populated game carries all nine fields, once each",
          "a game block dropped or duplicated a field: the theme reads a "
          "missing x-gamekey as 'this game has no mapping' and offers the "
          "wizard a question it cannot file the answer to")
    holds(value(blocks[0], "collection"), "Nintendo 64",
          "the collection name is emitted verbatim",
          "the collection name was mangled, so the console tab is wrong")
    holds(value(blocks[0], "extensions"), "z64|n64|v64",
          "scan_file_exts is passed through",
          "the extensions line was lost, so Pegasus rejects the ROMs")
    holds(value(blocks[1], "file"), "/roms/n64/Super Mario 64 (USA).z64",
          "the ROM path is emitted unquoted and whole",
          "the ROM path was quoted or truncated, so the game does not launch")
    holds(value(blocks[1], "release"), "1996",
          "the year is emitted as release:",
          "the release year was lost from the library")
    holds(value(blocks[1], "developer"), "Nintendo",
          "the manufacturer is emitted as developer:",
          "the developer was lost from the library")
    holds(value(blocks[1], "x-mame-status"), "good",
          "the MAME driver grade is emitted as x-mame-status",
          "the driver grade was lost, so the theme cannot warn about a game "
          "that does not run")
    holds(value(blocks[1], "x-console"), "n64",
          "x-console names the layout the launcher will scope by",
          "x-console is missing or wrong, so a mapping made from the library "
          "is filed under a scope the launcher never looks in")
    holds(value(blocks[1], "x-gamekey"), "n64/super-mario-64-usa",
          "x-gamekey is profiles.game_key for the same game",
          "x-gamekey does not match the key the launcher computes, so a "
          "per-game mapping is written where nothing will ever read it")

    unknown = {key for block in blocks for key in keys(block)} - KNOWN_KEYS
    holds(sorted(unknown), [],
          "no key outside the set Pegasus and the theme read",
          "the exporter emitted a key nothing reads, which is invisible "
          "until somebody wonders why the theme ignores it")
    true(all("\n" not in v and "\r" not in v
             for block in blocks for _, v in block),
         "no emitted value contains a line break",
         "a value spanning two lines makes its remainder parse as another "
         "metadata key")


def check_render_omits_what_it_cannot_derive() -> None:
    heading("S22: absent means unknown, not empty")

    fresh("render-omit")
    bare = pegasus.Collection(
        name="Frobnicator",
        core_path=UNKNOWN_CORE,
        entries=[pegasus.Entry(title="A Game", path="/roms/x/a.bin")],
    )
    blocks = parse(pegasus.render(bare))

    holds(keys(blocks[0]), ["collection", "launch"],
          "a playlist with no scan_file_exts emits no extensions line",
          "an empty extensions line was emitted, so Pegasus is told the "
          "collection accepts no file types at all")
    holds(keys(blocks[1]), ["game", "file"],
          "a game with nothing known carries only game and file",
          "the exporter emitted empty release/developer/x- lines, and the "
          "theme cannot tell an unknown year from a blank one")
    holds(value(blocks[1], "x-console"), None,
          "an unrecognised core emits no x-console",
          "x-console was emitted for a core padmap knows nothing about, so "
          "the launcher scopes mappings to a console that does not exist")

    # x-console without x-gamekey: the console is known, the ROM name is not
    # sluggable. The two are independently derived and must be independently
    # emitted.
    console_only = pegasus.Collection(
        name="Nintendo 64", core_path=N64_CORE, console="n64",
        entries=[pegasus.Entry(title="?", path="/roms/n64/---.z64",
                               key=pegasus.profiles.game_key("n64",
                                                             "/roms/n64/---.z64"))],
    )
    blocks = parse(pegasus.render(console_only))
    holds(value(blocks[1], "x-console"), "n64",
          "x-console is still emitted when the game key is empty",
          "a ROM with no sluggable name cost the whole game its console, so "
          "the console mapping stops applying to it")
    holds(value(blocks[1], "x-gamekey"), None,
          "and x-gamekey is omitted rather than emitted empty",
          "an empty x-gamekey was emitted, which the theme reads as a real "
          "key and files a per-game mapping under nothing")


def check_render_output_parses_by_its_own_rules() -> None:
    heading("S22: the file is readable by the format it claims to be")

    fresh("render-parse")
    text = pegasus.render(n64_collection())

    true(text.endswith("\n") and not text.endswith("\n\n\n"),
         "the file ends with exactly one blank separator",
         "the rendered file does not end cleanly, so appending to it or "
         "re-reading it depends on trailing whitespace")
    holds([line for line in text.splitlines()
           if line.strip() and not FIELD.match(line)], [],
          "every non-blank line is a key: value field",
          "a line that is not a metadata field reached the file; Pegasus "
          "drops it and the game it belonged to loses that fact silently")
    holds([line for line in text.splitlines() if line != line.strip()], [],
          "no line is indented or padded",
          "an indented line is a continuation in Pegasus's format, so it "
          "joins the previous value instead of standing on its own")


def check_render_pins_the_newline_injection() -> None:
    heading("S22: a title containing a newline")

    fresh("render-injection")
    hostile = pegasus.Collection(
        name="Nintendo 64", core_path=N64_CORE, console="n64",
        entries=[pegasus.Entry(
            title="Mario 64\nlaunch: /bin/sh -c 'echo pwned'",
            path="/roms/n64/mario.z64", key="n64/mario")],
    )
    text = pegasus.render(hostile)
    blocks = parse(text)
    game = blocks[1]

    # Promoted from a gap once pegasus.one_line landed. render() used to
    # interpolate every value into "key: value" with no escaping, and
    # read_playlist takes the title straight from the playlist's label -- a
    # JSON string a user can edit and a scanner can fill from a filename.
    # Because Pegasus honours a per-game launch: line, a label of
    # "Mario 64\nlaunch: /bin/sh -c ..." replaced the launch command for that
    # game, and stale_collections could not see it: it stops at the first
    # launch line, which is the collection's correct one.
    holds(value(game, "game"), "Mario 64 launch: /bin/sh -c 'echo pwned'",
          "the whole hostile label stays on the game's own line",
          "a newline in a title no longer stays on one line -- if the value "
          "is now escaped or rejected instead, assert that; if it injects "
          "again, the launch command for that game has been replaced")
    holds(keys(game), ["game", "file", "x-console", "x-gamekey"],
          "and no key was injected into the block",
          "an injected key appeared in the game's block, which means a "
          "hostile label can still write metadata -- including a launch: "
          "line, which Pegasus honours ahead of the collection's own")

    # And the half that is unambiguous: whatever the format does with the
    # value, the launcher line the *collection* declares must still be ours.
    holds(value(blocks[0], "launch"),
          pegasus.launch_line(N64_CORE),
          "the collection's own launch line is untouched by the injection",
          "a hostile title rewrote the collection's launch command, so every "
          "game in that collection runs through it")


def check_render_pins_a_newline_in_a_collection_name() -> None:
    heading("S22: a collection name containing a newline")

    fresh("render-injection-name")
    hostile = pegasus.Collection(
        name="Arcade\nlaunch: /bin/false", core_path=ARCADE_CORE,
        entries=[pegasus.Entry(title="Pac-Man", path="/roms/a/pacman.zip")],
    )
    blocks = parse(pegasus.render(hostile))

    # Promoted from a gap once pegasus.one_line landed. A newline in the
    # collection name injected a line *ahead* of the exporter's own launch
    # line, and Pegasus's game_add_to only fills a launch command when it is
    # still empty -- so the injected one won and every game in the collection
    # launched through it. The name comes from the playlist's
    # default_core_name or its filename, so naming a playlist
    # $'Arcade\nlaunch: /bin/false.lpl' was enough.
    holds(keys(blocks[0]), ["collection", "launch"],
          "the collection header has exactly its own launch line",
          "a newline in a collection name injected a line again -- it decides "
          "which binary every game in that collection runs, because Pegasus "
          "keeps the first launch command it sees")
    launcher = value(blocks[0], "launch")
    if "/bin/false" in launcher:
        raise SystemExit(
            f"FAIL: the collection's launch line is the injected one "
            f"({launcher!r}). Pegasus keeps the first launch command it "
            f"sees, so every game in this collection would run it")
    if "padmap-play" not in launcher:
        raise SystemExit(
            f"FAIL: the launch line is neither the injection nor padmap-play "
            f"({launcher!r}) -- whatever it now names is what every game in "
            f"the collection will run")
    print("  ok  the launch line is padmap's own, not the injected one")


def check_render_assets_only_for_art_that_exists() -> None:
    heading("S22: assets are emitted only for art on disk")

    root = fresh("render-assets")
    playlists = root / "playlists"
    write_playlist(
        playlists, "Nintendo 64", N64_CORE,
        "Nintendo - Nintendo 64 (Mupen64Plus-Next)",
        [{"path": "/roms/n64/Illustrated.z64", "label": "Illustrated"},
         {"path": "/roms/n64/Bare.z64", "label": "Bare"},
         {"path": "/roms/n64/Jpeg.z64", "label": "Jpeg"},
         {"path": "/roms/n64/renamed.z64", "label": "A Label Nobody Kept"},
         {"path": "/roms/n64/Ampersand.z64", "label": "Ratchet & Clank"}],
    )
    art(root, "Nintendo 64", "Named_Boxarts", "Illustrated")
    art(root, "Nintendo 64", "Named_Titles", "Illustrated")
    (root / "thumbnails" / "Nintendo 64" / "Named_Boxarts"
     / "Jpeg.jpg").write_bytes(b"not a png")
    art(root, "Nintendo 64", "Named_Boxarts", "renamed")
    art(root, "Nintendo 64", "Named_Boxarts", "Ratchet _ Clank")

    collection = pegasus.read_playlist(playlists / "Nintendo 64.lpl")
    assert collection is not None
    blocks = parse(pegasus.render(collection))
    by_title = {value(block, "game"): block for block in blocks[1:]}

    holds([k for k in keys(by_title["Illustrated"]) if k.startswith("assets.")],
          ["assets.boxFront", "assets.titlescreen"],
          "both kinds of art found on disk are emitted, boxFront first",
          "art that exists was not offered to the theme, or the order "
          "changed: boxFront is what a grid actually shows")
    holds([k for k in keys(by_title["Bare"]) if k.startswith("assets.")], [],
          "a game with no art emits no assets line",
          "an assets line pointing at a file that is not there makes the "
          "theme pick its 'has art' layout and draw a blank tile")
    holds([k for k in keys(by_title["Jpeg"]) if k.startswith("assets.")], [],
          "a non-PNG in the thumbnail tree is not offered as art",
          "a .jpg was emitted as a PNG asset, which Pegasus will not draw")
    true(value(by_title["A Label Nobody Kept"], "assets.boxFront", ) is not None,
         "art is still found by ROM basename when the label was renamed",
         "renaming a playlist label lost the thumbnails already downloaded "
         "for that game")
    true(value(by_title["Ratchet & Clank"], "assets.boxFront", ) is not None,
         "art is found through RetroArch's underscore escaping",
         "a title with a character RetroArch escapes in filenames lost its "
         "art, which is most of the arcade library")
    true(all(Path(v).is_file()
             for block in blocks for k, v in block if k.startswith("assets.")),
         "every emitted asset path resolves to a real file",
         "an asset path that does not resolve reached the metadata: the "
         "theme shows an empty frame where a box should be")


def check_render_assets_pin_a_dangling_image() -> None:
    heading("S22: a thumbnail that is not really there")

    root = fresh("render-assets-dangling")
    playlists = root / "playlists"
    write_playlist(playlists, "Nintendo 64", N64_CORE,
                   "Nintendo - Nintendo 64 (Mupen64Plus-Next)",
                   [{"path": "/roms/n64/Ghost.z64", "label": "Ghost"}])
    boxarts = root / "thumbnails" / "Nintendo 64" / "Named_Boxarts"
    boxarts.mkdir(parents=True)
    os.symlink("/gone/Ghost.png", boxarts / "Ghost.png")

    collection = pegasus.read_playlist(playlists / "Nintendo 64.lpl")
    assert collection is not None
    blocks = parse(pegasus.render(collection))
    emitted = value(blocks[1], "assets.boxFront")

    if emitted is not None and not Path(emitted).exists():
        gap("art_index() indexes any entry named *.png in a Named_* "
            "directory, including a dangling symlink and a directory, so "
            "`assets.boxFront` is emitted for art that cannot be opened -- "
            "against the contract in render() that assets appear 'only when "
            "the file exists', which the theme relies on to choose between "
            "its grid and its list layout. A half-finished `padmap fetch-art` "
            "or a thumbnail pack on an unmounted drive produces exactly this. "
            "Repro: ln -s /gone/X.png ~/.config/retroarch/thumbnails/"
            "<playlist>/Named_Boxarts/X.png and run `padmap export-pegasus`.")
        holds(emitted, str(boxarts / "Ghost.png"),
              "the dangling thumbnail is emitted by name (pinned, not "
              "endorsed)",
              "what a dangling thumbnail does has changed and the note above "
              "no longer describes it")
    else:
        holds(emitted, None,
              "a dangling thumbnail is not offered to the theme",
              "a dangling thumbnail was emitted as art")


# ---------------------------------------------------------------------------
# export
# ---------------------------------------------------------------------------


def two_playlists(root: Path) -> Path:
    playlists = root / "playlists"
    write_playlist(
        playlists, "Nintendo - Nintendo 64", N64_CORE,
        "Nintendo - Nintendo 64 (Mupen64Plus-Next)",
        [{"path": "/roms/n64/Super Mario 64 (USA).z64",
          "label": "Super Mario 64 (USA)"},
         {"path": "/roms/n64/Zelda.z64", "label": "Zelda"}],
        exts="z64|n64|v64")
    write_playlist(
        playlists, "MAME 2010", ARCADE_CORE, "Arcade (MAME 2010)",
        [{"path": "/roms/arcade/pacman.zip", "label": "pacman"},
         {"path": "/roms/arcade/polyplay.zip", "label": "polyplay"}])
    return playlists


def check_export_writes_one_directory_per_playlist() -> None:
    heading("S22: one directory per collection")

    root = fresh("export-layout")
    playlists = two_playlists(root)
    out = root / "collections"

    results, written = pegasus.export(playlists, out)

    holds(sorted(p.name for p in written), ["MAME 2010", "Nintendo - Nintendo 64"],
          "one directory per playlist, named by the playlist stem",
          "the exporter did not give each collection its own directory; "
          "Pegasus never looks below a listed directory, so the collections "
          "that share one are simply not there")
    holds(sorted(str(p.relative_to(out)) for p in out.rglob("*")),
          ["MAME 2010", "MAME 2010/metadata.pegasus.txt",
           "Nintendo - Nintendo 64",
           "Nintendo - Nintendo 64/metadata.pegasus.txt"],
          "and nothing else is written into the output directory",
          "the exporter wrote a file outside the per-collection directories, "
          "which either Pegasus reads as an extra collection or nothing "
          "reads at all")
    holds([name for name, *_ in results], ["Mame 2010", "Nintendo 64"],
          "the display names come from default_core_name, playlists in order",
          "a collection is named after its file rather than its platform, so "
          "the console tabs read like a directory listing")
    true(all((directory / "metadata.pegasus.txt").is_file()
             for directory in written),
         "every returned directory holds a metadata.pegasus.txt",
         "export returned a directory with no metadata in it, so game_dirs "
         "lists a directory Pegasus finds nothing in")


def check_export_never_merges_collections_into_one_file() -> None:
    heading("S22: never one file with several collection blocks")

    root = fresh("export-merge")
    playlists = two_playlists(root)
    out = root / "collections"
    _, written = pegasus.export(playlists, out)

    for directory in written:
        text = (directory / "metadata.pegasus.txt").read_text()
        blocks = parse(text)
        declared = [line for line in text.splitlines()
                    if line.startswith("collection:")]
        holds(len(declared), 1,
              f"{directory.name} declares exactly one collection",
              f"{directory.name} holds more than one collection block: "
              f"Pegasus adds every game to every collection in the file and "
              f"only fills a launch command while it is empty, so every game "
              f"inherits the FIRST collection's launcher -- this is how N64 "
              f"ROMs were launched with the MAME arcade core")
        holds(len([line for line in text.splitlines()
                   if line.startswith("launch:")]), 1,
              f"{directory.name} declares exactly one launch command",
              f"{directory.name} holds a second launch line, and which one "
              f"Pegasus keeps decides what every game in it runs")
        holds(len([b for b in blocks if value(b, "game") is not None]), 2,
              f"{directory.name} carries both of its games",
              f"{directory.name} lost a game between the playlist and the "
              f"metadata")

    n64 = (out / "Nintendo - Nintendo 64" / "metadata.pegasus.txt").read_text()
    arcade = (out / "MAME 2010" / "metadata.pegasus.txt").read_text()
    true("mupen64plus" in n64 and "mame2010" not in n64,
         "the N64 collection names only the N64 core",
         "the N64 collection carries the arcade core: N64 ROMs launch under "
         "MAME, which is the exact report this file layout exists to prevent")
    true("mame2010" in arcade and "mupen64plus" not in arcade,
         "the arcade collection names only the arcade core",
         "the arcade collection carries the N64 core")


def check_export_removes_a_legacy_merged_file() -> None:
    heading("S22: a merged file from an older padmap is removed")

    root = fresh("export-legacy")
    playlists = two_playlists(root)
    out = root / "collections"
    out.mkdir(parents=True)
    legacy = out / "metadata.pegasus.txt"
    legacy.write_text("collection: Everything\nlaunch: /nix/store/old/play\n")
    hand_written = out / "notes.txt"
    hand_written.write_text("mine\n")

    pegasus.export(playlists, out)

    true(not legacy.exists(),
         "the merged metadata.pegasus.txt is deleted",
         "a merged file from an earlier export survived: Pegasus still reads "
         "it, and it reintroduces the every-game-inherits-the-first-launcher "
         "bug the per-directory layout exists to avoid")
    true(hand_written.is_file(),
         "and a user's own file in the output directory is left alone",
         "the exporter deleted a file it did not write")


def check_export_skips_what_it_cannot_use() -> None:
    heading("S22: an unusable playlist costs only itself")

    root = fresh("export-skip")
    playlists = two_playlists(root)
    write_playlist(playlists, "Empty", N64_CORE, "Nintendo - Nintendo 64", [])
    (playlists / "Garbage.lpl").write_text("{{{ not json")
    (playlists / "NotAnObject.lpl").write_text("[1, 2, 3]")
    (playlists / "NoItems.lpl").write_text('{"default_core_path": "/x.so"}')
    (playlists / "NotAPlaylist.txt").write_text('{"items": []}')
    out = root / "collections"

    results, written = pegasus.export(playlists, out)

    holds(sorted(p.name for p in written),
          ["MAME 2010", "Nintendo - Nintendo 64"],
          "an empty, an unparseable and a wrongly shaped playlist are skipped",
          "one unusable playlist cost the user the collections that were "
          "fine, or produced a directory Pegasus finds nothing in")
    holds(len(results), 2,
          "and only usable collections are reported",
          "the export reported a collection it did not write")
    true(not (out / "NotAPlaylist").exists(),
         "a file that is not an .lpl is not exported",
         "the exporter read something that is not a playlist")


def check_export_reports_what_it_wrote() -> None:
    heading("S22: the counts the user is shown")

    root = fresh("export-counts")
    playlists = two_playlists(root)
    art(root, "MAME 2010", "Named_Boxarts", "pacman")
    out = root / "collections"

    results, _ = pegasus.export(playlists, out)
    by_name = {name: rest for name, *rest in results}

    holds(by_name["Mame 2010"], [2, 2, 1],
          "arcade reports 2 games, 2 MAME titles resolved, 1 with artwork",
          "the export's own report does not match what it wrote; a title "
          "table that stopped resolving looks exactly like one that worked")
    holds(by_name["Nintendo 64"], [2, 0, 0],
          "a console playlist reports no resolved titles and no artwork",
          "the counts for a playlist with real labels are wrong, so 'no "
          "titles resolved' stops meaning anything")

    arcade = (out / "MAME 2010" / "metadata.pegasus.txt").read_text()
    blocks = parse(arcade)
    by_title = {value(b, "game"): b for b in blocks[1:]}
    holds(sorted(by_title), ["Pac-Man (Midway)", "Poly-Play"],
          "arcade set names are replaced by their MAME descriptions",
          "the arcade collection still reads like a directory listing of set "
          "names")
    holds(value(by_title["Poly-Play"], "x-mame-status"), "preliminary",
          "a preliminary driver is labelled for the theme",
          "a game MAME says does not work is indistinguishable from one that "
          "does")
    holds(value(by_title["Pac-Man (Midway)"], "x-gamekey"), "arcade/pacman",
          "the game key is filed under the arcade console",
          "the arcade game key is not what the launcher will compute, so a "
          "mapping made for it is never found")


def check_export_replaces_a_stale_collection_in_place() -> None:
    heading("S22: re-exporting overwrites rather than appends")

    root = fresh("export-overwrite")
    playlists = two_playlists(root)
    out = root / "collections"
    pegasus.export(playlists, out)
    target = out / "Nintendo - Nintendo 64" / "metadata.pegasus.txt"
    target.write_text(target.read_text()
                      + "\ngame: Ghost\nfile: /roms/n64/ghost.z64\n")

    pegasus.export(playlists, out)
    blocks = parse(target.read_text())

    holds([value(b, "game") for b in blocks[1:]],
          ["Super Mario 64 (USA)", "Zelda"],
          "a re-export replaces the file rather than growing it",
          "a game that is no longer in the playlist survived a re-export, so "
          "the library keeps titles whose ROMs are gone")
    holds(len([line for line in target.read_text().splitlines()
               if line.startswith("collection:")]), 1,
          "and the file still declares exactly one collection",
          "a re-export appended a second collection block to the file")


def check_exported_launch_lines_survive_a_rebuild() -> None:
    heading("S22: collections keep working across a rebuild")

    root = fresh("export-rebuild")
    playlists = two_playlists(root)
    out = root / "collections"
    _, written = pegasus.export(playlists, out)
    link = str(pegasus.player_link())

    for directory in written:
        text = (directory / "metadata.pegasus.txt").read_text()
        launch = value(parse(text)[0], "launch")
        true(launch is not None and launch.startswith(link + " "),
             f"{directory.name} launches through the stable link",
             f"{directory.name} names something other than "
             f"~/.local/share/padmap/bin/padmap-play, so a rebuild cannot "
             f"reach it")
        # The core legitimately lives in the store; the launcher must not.
        holds(shlex.split(launch or "")[:1], [link],
              f"and the binary {directory.name} runs is the link itself",
              f"{directory.name} baked a Nix store path into its launch "
              f"line: this is the bug that kept four controllers in N64 "
              f"games for months while every fix for it was already "
              f"installed")

    before = {d: (d / "metadata.pegasus.txt").read_bytes() for d in written}

    # The rebuild: PADMAP_PLAY moves, ensure-daemon repoints the link, and
    # nothing re-exports.
    os.environ["PADMAP_PLAY"] = REBUILT_PLAY
    pegasus.install_player_link()

    holds(os.readlink(pegasus.player_link()), REBUILT_PLAY,
          "after a rebuild the link points at the new padmap-play",
          "the link did not follow the rebuild")
    holds({d: (d / "metadata.pegasus.txt").read_bytes() for d in written},
          before,
          "and the collections on disk did not have to change",
          "the collections would need re-exporting after every rebuild, "
          "which is the state that made the launcher bug invisible")
    holds(pegasus.stale_collections(), [],
          "and nothing is reported stale",
          "collections that point at the stable link were reported stale "
          "after a rebuild, so the warning cries wolf and gets ignored")
    os.environ["PADMAP_PLAY"] = STORE_PLAY


# ---------------------------------------------------------------------------
# stale_collections
# ---------------------------------------------------------------------------


def check_stale_flags_a_baked_in_store_path() -> None:
    heading("S22: a collection that names a store path is reported")

    root = fresh("stale-flag")
    link = str(pegasus.player_link())
    baked = install(root / "colls" / "n64", (
        f"collection: Nintendo 64\n"
        f"launch: {STORE_PLAY} -L {N64_CORE} \"{{file.path}}\"\n"
        f"\ngame: Zelda\nfile: /roms/n64/Zelda.z64\n"))
    current = install(root / "colls" / "arcade", (
        f"collection: Arcade\n"
        f"launch: {link} -L {ARCADE_CORE} \"{{file.path}}\"\n"))
    bare = install(root / "colls" / "snes",
                   'collection: SNES\nlaunch: retroarch "{file.path}"\n')

    stale = pegasus.stale_collections()

    holds(sorted(stale), sorted([baked, bare]),
          "a store path and a bare retroarch are both reported, the link is "
          "not",
          "a collection launching through a hard-coded path went unreported: "
          "nothing else reveals it, and every launcher fix stays invisible "
          "for as long as it does")
    true(current not in stale,
         "a collection using the stable link is not reported",
         "a correct collection was reported stale, so the warning is noise "
         "and a user learns to ignore the one that matters")
    true(all(p.name == "metadata.pegasus.txt" for p in stale),
         "the report names the metadata file to re-export",
         "the warning does not say which file is wrong, so the user cannot "
         "act on it")


def check_stale_is_quiet_about_what_it_cannot_read() -> None:
    heading("S22: staleness detection over an awkward library")

    root = fresh("stale-quiet")
    link = str(pegasus.player_link())
    config = pegasus.game_dirs_file()
    config.parent.mkdir(parents=True, exist_ok=True)

    cases: list[tuple[str, str, bool]] = [
        ("a collection with no launch line at all",
         "collection: N64\n\ngame: Zelda\nfile: /roms/z.z64\n", False),
        ("an empty launch line",
         "collection: N64\nlaunch:\n", False),
        ("a launch line naming the stable link",
         f'collection: N64\nlaunch: {link} "{{file.path}}"\n', False),
        ("a launch line naming the link with no arguments",
         f"collection: N64\nlaunch: {link}\n", False),
        ("a launch line naming the link, quoted",
         f"collection: N64\nlaunch: '{link}' \"{{file.path}}\"\n", False),
        ("a launch line naming a store path",
         f'collection: N64\nlaunch: {STORE_PLAY} "{{file.path}}"\n', True),
    ]
    for label, text, expected in cases:
        directory = root / "one"
        directory.mkdir(parents=True, exist_ok=True)
        metadata = directory / "metadata.pegasus.txt"
        metadata.write_text(text)
        config.write_text(f"{directory}\n")
        holds(bool(pegasus.stale_collections()), expected,
              f"{label} is {'reported' if expected else 'not reported'}",
              f"{label} produced the wrong answer, so the only signal a user "
              f"gets about a hard-coded launcher is wrong")

    # Nothing in a listed directory, and directories that are not there.
    empty = root / "empty"
    empty.mkdir()
    config.write_text(f"{root / 'gone'}\n{empty}\n\n   \n")
    holds(pegasus.stale_collections(), [],
          "a missing directory, an empty one and blank lines are ignored",
          "a game_dirs.txt listing a directory that has gone away broke the "
          "check `padmap ensure-daemon` runs before it starts anything")

    # A collection file RetroArch or an editor left in another encoding.
    directory = root / "latin"
    directory.mkdir()
    (directory / "metadata.pegasus.txt").write_bytes(
        b"collection: Caf\xe9\nlaunch: /nix/store/old/padmap-play \"{file.path}\"\n")
    config.write_text(f"{directory}\n")
    holds([p.parent.name for p in pegasus.stale_collections()], ["latin"],
          "a metadata file that is not UTF-8 is still checked, not skipped",
          "a collection whose name is not UTF-8 was skipped, so the one file "
          "most likely to be old is the one never checked")

    # And a game_dirs.txt that is not UTF-8 either.
    config.write_bytes(b"\xff\xfe\n" + str(directory).encode() + b"\n")
    holds([p.parent.name for p in pegasus.stale_collections()], ["latin"],
          "a game_dirs.txt that is not UTF-8 does not hide the collections "
          "after the bad line",
          "one undecodable byte in game_dirs.txt cost the staleness check "
          "every directory listed in it")

    config.unlink()
    holds(pegasus.stale_collections(), [],
          "no game_dirs.txt at all reports nothing",
          "a fresh machine with no Pegasus config crashed the check that "
          "runs before the front-end starts")


def check_stale_pins_what_it_cannot_see() -> None:
    heading("S22: what the staleness check does not look at")

    root = fresh("stale-blind")
    link = str(pegasus.player_link())

    # A per-game launch line after the collection's own. Pegasus honours it,
    # and it is exactly what a hostile or hand-edited title produces.
    install(root / "colls" / "pergame", (
        f'collection: N64\nlaunch: {link} "{{file.path}}"\n'
        f"\ngame: Zelda\nfile: /roms/z.z64\n"
        f'launch: {STORE_PLAY} "{{file.path}}"\n'))
    found = pegasus.stale_collections()

    if not found:
        gap("stale_collections() stops at the FIRST launch line in a "
            "metadata file, so a per-game launch line naming a store path is "
            "never seen -- and Pegasus honours a per-game launch line over "
            "the collection's. Combined with the newline injection above, an "
            ".lpl label is enough to install a launcher padmap's own "
            "staleness check reports as clean. Repro: append 'launch: "
            "/nix/store/old/padmap-play \"{file.path}\"' to a game block in "
            "an installed metadata.pegasus.txt and run `padmap "
            "ensure-daemon`.")
        holds(found, [],
              "a stale per-game launch line is not reported (pinned, not "
              "endorsed)",
              "the pinned blind spot changed and the note above no longer "
              "describes it")
    else:
        holds([p.parent.name for p in found], ["pergame"],
              "a stale per-game launch line is reported",
              "a per-game launch line naming a store path was missed")

    # The one that matters most is still caught in the same file shape: the
    # collection's own launcher, first line, wrong.
    fresh("stale-blind-control")
    control = install(root / "colls2" / "first", (
        f'collection: N64\nlaunch: {STORE_PLAY} "{{file.path}}"\n'
        f"\ngame: Zelda\nfile: /roms/z.z64\n"
        f'launch: {str(pegasus.player_link())} "{{file.path}}"\n'))
    holds(pegasus.stale_collections(), [control],
          "a stale collection launcher is caught even when a later line is "
          "correct",
          "the check was satisfied by a correct launch line further down the "
          "file, so the launcher Pegasus actually uses goes unchecked")


def check_stale_survives_a_hand_edited_launch_line() -> None:
    heading("S22: a hand-edited collection must not stop padmap starting")

    root = fresh("stale-quote")
    install(root / "colls" / "quoted",
            'collection: N64\nlaunch: /nix/store/old/padmap-play "unclosed\n')

    raised: BaseException | None = None
    try:
        found = pegasus.stale_collections()
    except BaseException as error:  # noqa: BLE001 - reporting, not handling
        raised = error
        found = []

    if isinstance(raised, ValueError):
        gap("stale_collections() runs shlex.split() on the launch line with "
            "no guard, and an unbalanced quote raises ValueError('No closing "
            "quotation'). `padmap ensure-daemon` calls it unconditionally "
            "before the daemon or the front-end is started, so one "
            "hand-edited metadata.pegasus.txt stops padmap starting at all. "
            "Repro: put 'launch: /x/padmap-play \"unclosed' in a "
            "metadata.pegasus.txt listed in game_dirs.txt and run `padmap "
            "ensure-daemon`. (Also reported by tools/check_hostile_files.py.)")
    elif raised is not None:
        fail(f"stale_collections raised {type(raised).__name__} on a launch "
             f"line with an unbalanced quote; `padmap ensure-daemon` dies "
             f"before the front-end starts")
    else:
        holds([p.parent.name for p in found], ["quoted"],
              "an unparseable launch line naming a store path is reported",
              "a launch line that cannot be split was treated as current, so "
              "the collection most likely to be broken is the one not "
              "checked")

    # Unambiguous either way: a sound collection listed *after* the broken one
    # must still be examined, or one bad file hides the rest of the library.
    install(root / "colls" / "sound",
            f'collection: SNES\nlaunch: {STORE_PLAY} "{{file.path}}"\n')
    try:
        after = [p.parent.name for p in pegasus.stale_collections()]
    except ValueError:
        gap("and the same ValueError abandons every collection listed after "
            "the broken one, so a single bad file hides staleness across the "
            "whole library.")
        after = None  # type: ignore[assignment]
    if after is not None:
        true("sound" in after,
             "a sound collection listed after a broken one is still checked",
             "one unparseable collection cost the check every collection "
             "listed after it")


# ---------------------------------------------------------------------------
# game_dirs.txt, which is what makes the directories above visible at all
# ---------------------------------------------------------------------------


def export_command(
    playlists: Path, out: Path, no_game_dirs: bool = False
) -> tuple[int, str]:
    """Run the command, capturing what it tells the user."""
    captured = io.StringIO()
    with contextlib.redirect_stdout(captured):
        code = cli.cmd_export_pegasus(argparse.Namespace(
            playlists=str(playlists), out=str(out), no_game_dirs=no_game_dirs))
    return code, captured.getvalue()


def check_game_dirs_lists_every_collection() -> None:
    heading("S22: game_dirs.txt lists each collection directory")

    root = fresh("game-dirs")
    playlists = two_playlists(root)
    out = root / "collections"
    config = pegasus.game_dirs_file()

    code, report = export_command(playlists, out)
    holds(code, 0,
          "export-pegasus succeeds on a sound playlist directory",
          "export-pegasus reported failure on playlists it exported")
    true("Nintendo 64" in report and "Mame 2010" in report,
         "the report names every collection it wrote",
         "export-pegasus wrote a collection it did not mention, so a user "
         "cannot tell what the library now holds")
    holds(report.count("0 with artwork"), 2,
          "artwork is reported even when there is none",
          "the export said nothing about artwork for a collection that has "
          "none: silence there looks exactly like an export that lost the "
          "art it used to have")
    true(str(out / "MAME 2010") in report,
         "and the directories added to game_dirs.txt are echoed",
         "export-pegasus changed game_dirs.txt without saying which lines it "
         "added")
    listed = config.read_text().splitlines()
    holds(listed, [str(out / "MAME 2010"), str(out / "Nintendo - Nintendo 64")],
          "every exported directory is listed, one per line",
          "a collection directory was not listed in game_dirs.txt: Pegasus "
          "never looks below a listed directory, so that console simply does "
          "not appear in the library")
    true(config.read_text().endswith("\n"),
         "and the file ends with a newline",
         "game_dirs.txt has no trailing newline, so the next line appended "
         "to it joins the last directory")

    export_command(playlists, out)
    holds(config.read_text().splitlines(), listed,
          "a second export leaves the file identical",
          "re-exporting duplicates every directory in game_dirs.txt, and the "
          "file grows without limit")


def check_game_dirs_drops_only_padmaps_own_entries() -> None:
    heading("S22: game_dirs.txt keeps what the user put there")

    root = fresh("game-dirs-keep")
    playlists = two_playlists(root)
    out = root / "collections"
    config = pegasus.game_dirs_file()
    config.parent.mkdir(parents=True, exist_ok=True)

    hand_added = str(root / "my-own-collection")
    sibling = str(root / "collections-extra")
    previous = str(out / "Retired Console")
    merged = str(out)
    config.write_text("\n".join([hand_added, previous, merged, sibling]) + "\n")

    export_command(playlists, out)
    listed = config.read_text().splitlines()

    true(hand_added in listed,
         "a directory the user added by hand survives an export",
         "export-pegasus deleted a collection the user added to "
         "game_dirs.txt themselves, and it disappears from Pegasus with no "
         "message")
    true(previous not in listed,
         "a collection padmap exported before and no longer writes is dropped",
         "a directory from a previous export lingers in game_dirs.txt, so a "
         "console whose playlist is gone stays in the library forever")
    true(merged not in listed,
         "and the old merged output directory is dropped",
         "the pre-per-directory output stayed listed, so Pegasus keeps "
         "reading the merged file and every game inherits the first "
         "collection's launcher")

    if sibling not in listed:
        gap("cmd_export_pegasus keeps a game_dirs.txt line only when it does "
            "not `startswith(str(out_dir))`, which is a string prefix test "
            "and not a path test: a directory the user added by hand that "
            "merely SHARES A PREFIX with the export directory -- "
            "'~/collections-extra' beside '~/collections' -- is silently "
            "dropped, and that collection vanishes from Pegasus with the "
            "message '(1 pre-existing entr(y/ies) kept)'. Repro: add "
            "$HOME/collections-extra to game_dirs.txt and run `padmap "
            "export-pegasus --out $HOME/collections`.")
        holds(sibling in listed, False,
              "a hand-added sibling sharing the output prefix is dropped "
              "(pinned, not endorsed)",
              "the pinned behaviour changed and the note above no longer "
              "describes what a user gets")
    else:
        true(sibling in listed,
             "a hand-added sibling sharing the output prefix survives",
             "a hand-added directory sharing a prefix with the output "
             "directory was dropped")


def check_game_dirs_is_left_alone_when_asked() -> None:
    heading("S22: --no-game-dirs prints instead of writing")

    root = fresh("game-dirs-none")
    playlists = two_playlists(root)
    out = root / "collections"
    config = pegasus.game_dirs_file()
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text("/somewhere/else\n")

    code, report = export_command(playlists, out, no_game_dirs=True)
    holds(code, 0,
          "export-pegasus --no-game-dirs succeeds",
          "--no-game-dirs reported failure")
    true(str(out / "MAME 2010") in report,
         "and it prints the directories the user must add themselves",
         "--no-game-dirs neither wrote game_dirs.txt nor said what to put in "
         "it, so the export is unreachable from Pegasus")
    holds(config.read_text(), "/somewhere/else\n",
          "and game_dirs.txt is untouched",
          "--no-game-dirs rewrote the file it promised not to touch")
    true((out / "MAME 2010" / "metadata.pegasus.txt").is_file(),
         "while the collections are still written",
         "--no-game-dirs skipped the export as well as the listing")


def check_export_command_refuses_what_it_cannot_export() -> None:
    heading("S22: export-pegasus says so rather than writing nothing")

    root = fresh("export-cli-refuse")
    out = root / "collections"
    config = pegasus.game_dirs_file()
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text("/somewhere/else\n")

    code, report = export_command(root / "not-there", out)
    holds(code, 1,
          "a missing playlist directory exits non-zero",
          "export-pegasus reported success having found no playlists, so a "
          "wrong --playlists looks like an empty library")
    true("No playlist directory" in report,
         "and says which directory it could not find",
         "export-pegasus failed without naming the directory it was given")
    empty = root / "no-playlists"
    empty.mkdir()
    code, report = export_command(empty, out)
    holds(code, 1,
          "a playlist directory with nothing usable in it exits non-zero",
          "export-pegasus reported success having exported nothing")
    true("No usable playlists" in report,
         "and says the playlists were unusable rather than absent",
         "a directory of unusable playlists is reported the same as a "
         "missing one, so the user looks in the wrong place")
    holds(config.read_text(), "/somewhere/else\n",
          "and neither case rewrites game_dirs.txt",
          "a failed export emptied game_dirs.txt, so the library the user "
          "already had disappeared too")


# ---------------------------------------------------------------------------


def check_real_user_state_was_not_touched() -> None:
    heading("S22: the machine this ran on")

    for name, value_ in (("XDG_CONFIG_HOME", os.environ["XDG_CONFIG_HOME"]),
                         ("XDG_DATA_HOME", os.environ["XDG_DATA_HOME"]),
                         ("XDG_RUNTIME_DIR", os.environ["XDG_RUNTIME_DIR"])):
        if not value_.startswith(str(_SANDBOX)):
            fail(f"{name} was left pointing at {value_}, so these checks were "
                 f"reading and writing the real user's state")
    ok("the XDG directories never left the sandbox")

    after = (_fingerprint(_REAL_LINK), _fingerprint(_REAL_GAME_DIRS))
    holds(after, _REAL_BEFORE,
          "the real launcher symlink and game_dirs.txt are unchanged",
          "this check altered the running system's launcher or its Pegasus "
          "game directories -- a live daemon owns those")


def main() -> int:
    check_paths_are_where_the_story_says()

    check_link_absent_without_a_launcher()
    check_link_is_created()
    check_link_is_idempotent()
    check_link_repoints_after_a_rebuild()
    check_link_over_hostile_targets()
    check_link_when_the_bin_directory_is_read_only()
    check_player_command_prefers_the_link()

    check_launch_line_shape()
    check_render_emits_every_field_once_and_on_one_line()
    check_render_omits_what_it_cannot_derive()
    check_render_output_parses_by_its_own_rules()
    check_render_pins_the_newline_injection()
    check_render_pins_a_newline_in_a_collection_name()
    check_render_assets_only_for_art_that_exists()
    check_render_assets_pin_a_dangling_image()

    check_export_writes_one_directory_per_playlist()
    check_export_never_merges_collections_into_one_file()
    check_export_removes_a_legacy_merged_file()
    check_export_skips_what_it_cannot_use()
    check_export_reports_what_it_wrote()
    check_export_replaces_a_stale_collection_in_place()
    check_exported_launch_lines_survive_a_rebuild()

    check_stale_flags_a_baked_in_store_path()
    check_stale_is_quiet_about_what_it_cannot_read()
    check_stale_pins_what_it_cannot_see()
    check_stale_survives_a_hand_edited_launch_line()

    check_game_dirs_lists_every_collection()
    check_game_dirs_drops_only_padmaps_own_entries()
    check_game_dirs_is_left_alone_when_asked()
    check_export_command_refuses_what_it_cannot_export()

    check_real_user_state_was_not_touched()

    shutil.rmtree(_SANDBOX, ignore_errors=True)
    print(f"\n{CHECKS} assertions held")
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
