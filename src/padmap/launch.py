"""Resolve a controller's mapping for the game that is about to start.

The one moment anything knows what is being played. A front-end spawns
`padmap-play`, which is handed RetroArch's own arguments -- `-L <core.so>` and
a ROM path -- and that is the only place the console and the game are both
available. The daemon writes its RetroArch autoconfig profiles at republish
time, which is long before, when nothing can know either.

So the split is: the daemon writes each controller's *default* mapping into
`$XDG_RUNTIME_DIR/padmap/autoconfig`, and this module rewrites that same
directory immediately before RetroArch starts, with whichever of the
controller's mappings actually applies -- this game, else this console, else
the default.

Rewriting in place, rather than pre-building one directory per console and
picking between them at launch:

  * The launch override names exactly one `joypad_autoconfig_dir` and
    RetroArch scans exactly one. Selecting a directory would mean editing the
    override too, so two files would have to agree instead of one.
  * The directory is per-session runtime state and is cleared on every write
    anyway, so there is nothing to accumulate.
  * It degrades the right way. If this never runs -- padmap-play bypassed,
    Python missing, an unrecognised core -- what is on disk is the default
    mapping, which is exactly what padmap did before scopes existed.

Nothing here talks to the daemon. It reads the assignments the daemon
persisted and the profiles on disk, both of which are files, so a launch is
not one more thing that fails when the socket is busy or the daemon is
mid-restart.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from . import devices, layouts, profiles, protocol, retroarch
from .assign import Assignment

# RetroArch's own spellings for "use this core". `-L` is what every front-end
# emits; the long form is accepted because a hand-written launch command is a
# perfectly ordinary thing to point padmap-play at.
CORE_FLAGS = ("-L", "--libretro")

# Flags that take a value, so the value is never mistaken for the ROM.
# Deliberately a short list of the ones padmap or a front-end actually emits
# rather than an attempt at RetroArch's whole grammar: an unlisted flag's
# value can only be misread as a ROM if it also happens to be an existing
# file path, and the fallback for "no ROM identified" is the console mapping,
# which is still better than the default.
VALUE_FLAGS = (
    "-L", "--libretro", "-c", "--config", "--appendconfig", "-s", "--save",
    "-S", "--savestate", "-N", "--nodevice", "-A", "--dualanalog",
    "-d", "--device", "-P", "--bsvplay", "-R", "--bsvrecord",
    "--record", "--recordconfig", "--size", "--log-file", "--subsystem",
    "--eof-exit", "--max-frames",
)


def split_args(argv: list[str]) -> tuple[str, str]:
    """(core path, ROM path) out of a RetroArch command line.

    The ROM is identified by being a path that exists rather than by
    position. RetroArch takes it as a positional argument, but padmap-play
    prepends its own flags and a front-end may append more, so counting from
    either end is wrong sooner or later -- and a ROM that does not exist is
    not a game whose mapping is worth resolving.
    """
    core = ""
    rom = ""
    index = 0
    while index < len(argv):
        item = argv[index]
        if item in CORE_FLAGS and index + 1 < len(argv):
            core = argv[index + 1]
            index += 2
            continue
        if item in VALUE_FLAGS:
            index += 2
            continue
        if item.startswith("-"):
            index += 1
            continue
        # Last one wins: a launch naming several files is a subsystem load,
        # where the last is still a game and any of them identifies it about
        # equally well.
        if Path(item).exists():
            rom = item
        index += 1
    return core, rom


def title_for(rom: str) -> str:
    """A human-readable name for the scope picker's entry.

    Deliberately derived from the filename rather than looked up. The daemon
    shows this on a strip beside four console names, where "close enough to
    recognise" is the whole requirement, and a lookup would make the picker
    depend on a metadata table that may not have this game in it.
    """
    stem = Path(rom).name
    if "." in stem:
        stem = stem.rsplit(".", 1)[0]
    return stem.replace("_", " ").strip()


def load_assignments(state: Path | None = None) -> list[Assignment]:
    """The player order the daemon persisted, matched to live devices.

    Matched by path against a fresh enumeration, the same way the daemon's
    own `restore` does, because the stored record is a snapshot and a pad
    that has gone needs skipping rather than faking: a profile written for a
    player whose pad no longer exists would still be scanned by RetroArch.
    """
    import json

    path = state or (protocol.runtime_dir() / "assignments.json")
    try:
        raw = json.loads(path.read_text())
    except (OSError, ValueError):
        return []
    if not isinstance(raw, list):
        return []

    by_path = {pad.path: pad for pad in devices.discover()}
    found: list[Assignment] = []
    for entry in raw:
        if not isinstance(entry, dict):
            continue
        pad = by_path.get(str(entry.get("path", "")))
        if pad is None:
            continue
        try:
            player = int(entry["player"])
        except (KeyError, TypeError, ValueError):
            continue
        found.append(Assignment(player=player, pad=pad, button=0))
    return found


def resolve(argv: list[str]) -> int:
    core, rom = split_args(argv)
    console = layouts.for_core(core)
    key = profiles.game_key(console, rom) if rom else ""
    title = title_for(rom) if rom else ""

    # Recorded even when nothing about this launch is recognised, because the
    # scope picker offers "...for the game you just played" and a launch with
    # an unknown core is exactly the one whose controls are most likely to
    # have felt wrong.
    if key:
        protocol.write_last_game(console, key, title)

    assignments = load_assignments()
    if not assignments:
        # Not an error. Running a game with no padmap assignment is what
        # happens before anyone has been through the setup screen, and there
        # is nothing to resolve for.
        print("padmap: no assigned controllers; leaving autoconfig alone",
              file=sys.stderr)
        return 0

    context = title or Path(rom).name or "an unidentified game"
    written = retroarch.install_profiles(
        assignments, console=console, game=key, context=context)

    from . import controllercfg

    for assignment in assignments:
        scope, _ = controllercfg.resolved_mapping(
            assignment.pad, console, key)
        print(
            f"padmap: player {assignment.player} using the "
            f"{scope or 'default'} mapping"
            + (f" (console {console})" if console else " (unknown console)"),
            file=sys.stderr,
        )
    print(f"padmap: wrote {len(written)} autoconfig profile(s)",
          file=sys.stderr)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m padmap.launch",
        description="Rewrite padmap's RetroArch autoconfig profiles for the "
                    "game about to run.",
    )
    parser.add_argument(
        "args", nargs=argparse.REMAINDER,
        help="the RetroArch command line, verbatim",
    )
    parsed = parser.parse_args(sys.argv[1:] if argv is None else argv)
    rest = parsed.args
    # argparse.REMAINDER keeps a leading `--`, which is how padmap-play
    # separates its own arguments from RetroArch's.
    if rest and rest[0] == "--":
        rest = rest[1:]
    try:
        return resolve(rest)
    except Exception as error:  # noqa: BLE001
        # Never take a launch down. The profiles the daemon wrote are already
        # in place and are a working, if less specific, answer -- a game that
        # refuses to start because a mapping could not be narrowed is a far
        # worse outcome than one played on the default mapping.
        print(f"padmap: could not resolve mappings ({error}); "
              f"using the default", file=sys.stderr)
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
