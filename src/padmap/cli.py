"""padmap command line.

    padmap list      what is plugged in, and what RetroArch would make of it
    padmap setup     assign player order by pressing and holding a button
    padmap ui        the same, as a graphical screen
    padmap run       republish assigned pads and keep them alive
    padmap launch    run, then start RetroArch bound to the assigned order
    padmap hide      print udev rules hiding the physical pads
    padmap fetch-art download box art for the playlists from libretro
    padmap ensure-daemon
                     start the daemon, or restart it if it is running old code
    padmap clean-config
                     strip padmap values RetroArch saved into retroarch.cfg
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

from . import devices, hide, retroarch, virtual
from .assign import HOLD_SECONDS, Assigner, Assignment

STATE_DIR = Path(
    os.environ.get("XDG_RUNTIME_DIR", "/tmp")
) / "padmap"
STATE_PATH = STATE_DIR / "assignments.json"
LAUNCH_CONFIG_PATH = STATE_DIR / "launch.cfg"
# Command-line flags to go with launch.cfg. Separate file because they cannot
# be expressed as config settings at all -- see retroarch.launch_args.
LAUNCH_ARGS_PATH = STATE_DIR / "launch.args"
LOG_PATH = STATE_DIR / "retroarch.log"

# Duplicated from artwork.KINDS rather than imported: argparse needs the
# choices while the parser is built, and importing artwork here would pull
# urllib into every `padmap list`.
_ART_KINDS = ("Named_Boxarts", "Named_Snaps", "Named_Titles")


def cmd_list(_args: argparse.Namespace) -> int:
    pads = devices.discover()
    if not pads:
        print("No joypads found.")
        return 1

    # Indices count only the pads RetroArch can see, so a hidden pad gets no
    # index rather than silently shifting the ones below it.
    print(f"{len(pads)} pad(s), in the order RetroArch would enumerate them:\n")
    index = 0
    hidden = 0
    for pad in pads:
        if pad.retroarch_visible:
            label = f"[{index}]"
            index += 1
        else:
            label = " -- "
            hidden += 1
        print(f"  {label} {pad.name}")
        print(f"        {pad.path}   phys={pad.phys or '(none)'}")

    if hidden:
        print(f"\n{hidden} pad(s) marked -- are hidden from RetroArch by udev")
        print("rules (ID_INPUT_JOYSTICK cleared). padmap can still republish")
        print("them; RetroArch sees only the virtual pads.")

    groups = devices.ambiguous_groups(pads)
    if groups:
        print("\nIndistinguishable by every static attribute:")
        for group in groups:
            nodes = ", ".join(p.event for p in group)
            print(f"  {group[0].name}")
            print(f"    {nodes}")
        print("\nNo config-file scheme can tell these apart. This is why")
        print("assignment is done by pressing a button.")
    return 0


def cmd_setup(args: argparse.Namespace) -> int:
    pads = devices.discover()
    if not pads:
        print("No joypads found.")
        return 1

    wanted = args.players
    print(f"{len(pads)} pad(s) detected.")
    print("Hold a button on each controller, in the order you want them")
    print(f"assigned -- hold for about {HOLD_SECONDS:.2f}s, a tap will not register.")
    if wanted:
        print(f"Waiting for {wanted} controller(s); Ctrl-C to stop early.\n")
    else:
        print("Press Ctrl-C when you are done.\n")

    def on_claim(assignment: Assignment) -> None:
        print(f"  Player {assignment.player}: {assignment.pad.name} "
              f"[{assignment.pad.event}]")

    with Assigner(pads, grab=True) as assigner:
        try:
            assignments = assigner.run(wanted=wanted, on_claim=on_claim)
        except KeyboardInterrupt:
            # Ctrl-C is how you finish when -n was not given, so keep what was
            # already claimed instead of discarding it. `run` returns the same
            # list object it appends to, so nothing is lost by the unwind.
            assignments = assigner.assignments
            print()

    if not assignments:
        print("Nothing assigned.")
        return 1

    _save_assignments(assignments)
    print(f"\nSaved {len(assignments)} assignment(s) to {STATE_PATH}")
    print("Start the daemon with: padmap run")
    return 0


def _save_assignments(assignments: list[Assignment]) -> None:
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    STATE_PATH.write_text(json.dumps([
        {"player": a.player, "path": a.pad.path, "name": a.pad.name,
         "phys": a.pad.phys, "vid": a.pad.vid, "pid": a.pad.pid}
        for a in assignments
    ], indent=2))


def cmd_export_pegasus(args: argparse.Namespace) -> int:
    from . import pegasus

    playlist_dir = Path(args.playlists).expanduser()
    if not playlist_dir.is_dir():
        print(f"No playlist directory at {playlist_dir}")
        return 1

    out_dir = Path(args.out).expanduser()
    results, dirs = pegasus.export(playlist_dir, out_dir)
    if not results:
        print(f"No usable playlists in {playlist_dir}")
        return 1

    print(f"Wrote {len(results)} collection(s) to {out_dir}:\n")
    for name, count, resolved, illustrated in results:
        # Only mention title resolution where it actually applied. Console
        # playlists already carry real labels, so "no titles resolved" there
        # is normal rather than a problem worth reporting.
        note = ""
        if resolved:
            note = f"  ({resolved} MAME titles resolved"
            note += f", {count - resolved} raw)" if resolved < count else ")"
        print(f"  {name:<28} {count:>6} games{note}")
        # Art is reported even at zero: an empty thumbnail tree is the normal
        # state until a pack is downloaded, and silence there looks like the
        # export lost artwork it once had.
        print(f"  {'':<28} {illustrated:>6} with artwork"
              f" (from {pegasus.thumbnail_dir()})")

    # Each collection needs its own directory listed: Pegasus never looks
    # below a listed directory, and merging collections into one file makes
    # every game inherit the first collection's launch command.
    config = Path(
        os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")
    ) / "pegasus-frontend" / "game_dirs.txt"

    wanted = [str(d) for d in dirs]
    if args.no_game_dirs:
        print(f"\nAdd these to {config}:")
        for line in wanted:
            print(f"  {line}")
        return 0

    existing = []
    if config.is_file():
        existing = [
            line.strip() for line in config.read_text().splitlines()
            if line.strip()
        ]
    # Drop our own previous entries (including the old merged directory) but
    # keep anything the user added by hand.
    keep = [
        line for line in existing
        if not line.startswith(str(out_dir))
    ]
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text("\n".join(keep + wanted) + "\n")
    print(f"\nUpdated {config}:")
    for line in wanted:
        print(f"  {line}")
    if keep:
        print(f"  ({len(keep)} pre-existing entr(y/ies) kept)")
    return 0


def cmd_calibrate(args: argparse.Namespace) -> int:
    """Walk each controller through centre and range measurement.

    Two phases, because measuring only the resting position is not enough:
    an adapter can declare a 0-255 axis while the stick physically reaches
    far less, and scaling against the declared range then leaves one
    direction with no usable travel.
    """
    from . import calibrate, profiles

    pads = devices.discover()
    if not pads:
        print("No joypads found.")
        return 1

    todo = [
        pad for pad in pads
        if args.force or not profiles.is_known(pad)
    ]
    skipped = len(pads) - len(todo)
    if skipped:
        print(f"{skipped} controller(s) already configured "
              f"(--force to redo).\n")
    if not todo:
        print("Nothing to do.")
        return 0

    for pad in todo:
        name = "".join(c for c in pad.name if c.isprintable()).strip()
        print(f"--- {name} [{pad.event}] ---")

        existing = profiles.load(pad)
        icon = existing.icon if existing else ""

        print("  1. Let go of the sticks. Press Enter when steady.")
        try:
            input()
        except (EOFError, KeyboardInterrupt):
            print("\ncancelled")
            return 1

        axes = calibrate.sample_rest(pad)
        if not axes:
            print("  no centring axes on this device, skipping\n")
            continue

        print(f"  2. Now rotate every stick through its full range for "
              f"{calibrate.REACH_SECONDS:.0f}s. Press Enter to start.")
        try:
            input()
        except (EOFError, KeyboardInterrupt):
            print("\ncancelled")
            return 1

        print("     measuring...", end="", flush=True)
        reach = calibrate.sample_reach(pad)
        axes = calibrate.merge_reach(axes, reach)
        print(" done")

        profile = profiles.Profile(
            signature=profiles.signature(pad), name=name, icon=icon, axes=axes
        )
        path = profiles.save(profile)

        for code, cal in sorted(profile.axes.items()):
            mid = (cal.minimum + cal.maximum) // 2
            drift = cal.center - mid
            span = f"{cal.low}..{cal.high}"
            declared = f"{cal.minimum}..{cal.maximum}"
            note = ""
            if span != declared:
                note = f"  (declared {declared})"
            print(f"      {_abs_name(code):<10} centre={cal.center:<5} "
                  f"reach={span:<12} deadband=+/-{cal.flat}{note}")
            if abs(drift) > cal.flat:
                print(f"      {'':<10} off centre by {drift:+d}")
        print(f"      saved to {path}\n")

    print("Restart the daemon to apply:  padmap serve")
    return 0


def _abs_name(code: int) -> str:
    from evdev import ecodes

    name = ecodes.ABS.get(code, str(code))
    return name[0] if isinstance(name, list) else str(name)


def _forget_prompted(signatures: set[str] | None) -> int:
    """Drop 'already asked about this controller' records.

    None means all of them. Returns how many were removed. The running daemon
    notices the file changing, so this takes effect without a restart.
    """
    from . import protocol

    path = protocol.prompted_path()
    try:
        remembered = [
            line.strip() for line in path.read_text().splitlines()
            if line.strip()
        ]
    except OSError:
        return 0

    keep = [] if signatures is None else [
        line for line in remembered if line not in signatures
    ]
    if len(keep) == len(remembered):
        return 0

    try:
        if keep:
            path.write_text("".join(f"{line}\n" for line in keep))
        else:
            path.unlink(missing_ok=True)
    except OSError:
        return 0
    return len(remembered) - len(keep)


def cmd_forget(args: argparse.Namespace) -> int:
    """Delete stored profiles so controllers are treated as new again."""
    from . import profiles

    directory = profiles.profile_dir()
    stored = sorted(directory.glob("*.json")) if directory.is_dir() else []
    connected = {profiles.signature(p) for p in devices.discover()}

    targets = []
    if args.all:
        targets = stored
    else:
        for path in stored:
            try:
                raw = json.loads(path.read_text())
            except (OSError, ValueError):
                continue
            if raw.get("signature") in connected:
                targets.append(path)

    for path in targets:
        path.unlink()
        print(f"  removed {path.name}")

    # Clear the 'already asked' record for every controller in scope, not
    # only the ones that had a profile to delete.
    #
    # Two separate memories, and the interesting case is a controller stuck
    # in the second with nothing in the first: never configured, so it
    # reports itself as new, but already asked about, so the daemon stays
    # silent. Forgetting it is precisely what someone would try, and keying
    # this off the deleted profiles meant there was nothing to key off and
    # `forget` did nothing at all -- including bailing out early with "no
    # stored profiles" before reaching this.
    cleared = _forget_prompted(None if args.all else connected)

    if not targets and not cleared:
        print(f"Nothing to forget for the connected controllers "
              f"(profiles in {directory}).")
        if stored and not args.all:
            print("Use --all to remove every stored profile.")
        return 0

    if targets:
        print(f"\nForgot {len(targets)} profile(s). They will be set up again")
        print("on the next controller assignment.")
    if cleared:
        print(f"Cleared {cleared} 'already asked' record(s), so setup is")
        print("offered again without waiting for a reboot.")

    # Pegasus keeps its button mappings separately, and padmap has no business
    # deleting them silently -- but they are the other half of "reset this
    # controller", so say where they are.
    sdl_map = Path(
        os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")
    ) / "pegasus-frontend" / "sdl_controllers.txt"
    if sdl_map.is_file():
        print(f"\nPegasus button mappings are separate and still present:")
        print(f"  {sdl_map}")
        print("Delete that file to reset those too.")
    return 0


def cmd_serve(_args: argparse.Namespace) -> int:
    from .server import serve

    return serve()


def cmd_ui(args: argparse.Namespace) -> int:
    # Imported lazily: PySide6 is a heavy dependency and every other
    # subcommand works without it.
    from .ui.app import run_setup

    assignments = run_setup(players=args.players)
    if not assignments:
        print("Cancelled; assignments unchanged.")
        return 1

    _save_assignments(assignments)
    for assignment in assignments:
        print(f"  Player {assignment.player}: {assignment.pad.name} "
              f"[{assignment.pad.event}]")
    print(f"\nSaved {len(assignments)} assignment(s) to {STATE_PATH}")
    return 0


def _load_assignments() -> list[Assignment]:
    if not STATE_PATH.is_file():
        return []
    raw = json.loads(STATE_PATH.read_text())
    by_path = {p.path: p for p in devices.discover()}
    out: list[Assignment] = []
    for entry in raw:
        pad = by_path.get(entry["path"])
        if pad is None:
            print(f"  warning: {entry['name']} ({entry['path']}) is gone")
            continue
        out.append(Assignment(player=entry["player"], pad=pad, button=0))
    return out


def _start(assignments: list[Assignment]) -> tuple[virtual.Republisher, dict[int, str]]:
    vpads = [virtual.create(a.pad, a.player) for a in assignments]
    paths = {vp.player: vp.ui.device.path for vp in vpads}
    return virtual.Republisher(vpads), paths


def cmd_run(_args: argparse.Namespace) -> int:
    assignments = _load_assignments()
    if not assignments:
        print("No assignments. Run `padmap setup` first.")
        return 1

    republisher, paths = _start(assignments)
    retroarch.install_profiles(assignments)
    retroarch.write_launch_config(assignments, paths, LAUNCH_CONFIG_PATH)
    retroarch.write_launch_args(assignments, paths, LAUNCH_ARGS_PATH)
    print(f"\nRepublishing {len(assignments)} pad(s). Launch config at:")
    print(f"  {LAUNCH_CONFIG_PATH}")
    print("Ctrl-C to stop.\n")

    signal.signal(signal.SIGTERM, lambda *_: republisher.stop())
    try:
        republisher.run()
    except KeyboardInterrupt:
        pass
    finally:
        republisher.close()
    return 0


def cmd_launch(args: argparse.Namespace) -> int:
    assignments = _load_assignments()
    if not assignments:
        print("No assignments. Run `padmap setup` first.")
        return 1

    republisher, paths = _start(assignments)
    retroarch.install_profiles(assignments)
    retroarch.write_launch_config(
        assignments, paths, LAUNCH_CONFIG_PATH, verbose=bool(args.log)
    )
    retroarch.write_launch_args(assignments, paths, LAUNCH_ARGS_PATH)

    # The --nodevice flags go before args.rest so a caller can still override
    # a port by hand: RetroArch's option parser takes the last one it sees.
    argv = (
        ["retroarch", "--appendconfig", str(LAUNCH_CONFIG_PATH)]
        + retroarch.launch_args(assignments, paths)
        + args.rest
    )

    # RetroArch logs to stderr and writes no file unless log_to_file is set.
    # Capturing the pipe ourselves is more reliable than depending on that
    # config path, and leaves the user's retroarch.cfg alone either way.
    log_file = None
    if args.log:
        log_path = Path(args.log) if isinstance(args.log, str) else LOG_PATH
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_file = log_path.open("w")
        print(f"logging RetroArch output to {log_path}")
        print(f"  follow it with:  tail -f {log_path}")

    print(f"launching: {' '.join(argv)}\n")
    try:
        proc = subprocess.Popen(
            argv,
            stdout=log_file,
            stderr=subprocess.STDOUT if log_file else None,
        )
        try:
            while proc.poll() is None:
                republisher.pump(timeout=0.1)
        except KeyboardInterrupt:
            proc.terminate()
    finally:
        republisher.close()
        if log_file:
            log_file.close()
    return proc.returncode or 0


def _daemon_state(timeout: float = 1.0) -> dict | None:
    """Ask the running daemon for its state, or None if nothing answers."""
    import socket

    from . import protocol

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.settimeout(timeout)
        sock.connect(str(protocol.socket_path()))
        # The daemon sends a state event on connect, so no command is needed;
        # asking anyway makes this work regardless of that greeting.
        sock.sendall(protocol.encode({"cmd": "status"}))
        reader = protocol.LineReader()
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                data = sock.recv(65536)
            except (TimeoutError, OSError):
                break
            if not data:
                break
            for message in reader.feed(data):
                if message.get("event") == "state":
                    return message
        return None
    except OSError:
        return None
    finally:
        sock.close()


def _spawn_daemon() -> None:
    """Start a daemon detached from this process.

    start_new_session so it outlives whatever asked for it -- a front-end
    launcher, typically, which would otherwise take the controllers down with
    it when it exits.
    """
    subprocess.Popen(
        [sys.executable, "-m", "padmap.cli", "serve"],
        start_new_session=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def _wait_for_daemon(deadline: float) -> dict | None:
    while time.monotonic() < deadline:
        state = _daemon_state()
        if state is not None:
            return state
        time.sleep(0.2)
    return None


def cmd_ensure_daemon(args: argparse.Namespace) -> int:
    """Guarantee a daemon is running *this* code, then return.

    Meant to be run just before a front-end starts. The daemon holds the
    modules it was started with, so after a rebuild it keeps running the old
    ones -- still answering, still writing launch.cfg, just from the previous
    version. That is not visible in any file it produces, which is exactly how
    a fixed controller-port bug went on reproducing.

    Restarting is only cheap because the daemon now restores its assignments
    on startup; before that this would have cost the user their controller
    order on every launch.
    """
    from . import pegasus, protocol

    # Repoint the stable launcher symlink first, and unconditionally. The
    # exported Pegasus collections invoke it by that fixed path, so this is
    # what stops a rebuild leaving them on a padmap-play from before whatever
    # was just fixed -- the failure that kept four controllers appearing in
    # N64 games long after the cause was fixed everywhere else.
    link = pegasus.install_player_link()
    if link is not None:
        print(f"launcher: {link} -> {os.readlink(link)}")

        # Collections exported before the link existed name a store path
        # directly and will keep invoking it whatever the link says.
        stale = pegasus.stale_collections()
        if stale:
            print(f"warning: {len(stale)} collection(s) still launch games "
                  f"through a hard-coded path:")
            for path in stale[:3]:
                print(f"  {path}")
            print("  re-export them with:  padmap export-pegasus")

    ours = protocol.build_id()
    state = _daemon_state()

    if state is None:
        if args.check:
            print("no daemon running")
            return 1
        print("no daemon running; starting one")
        _spawn_daemon()
        state = _wait_for_daemon(time.monotonic() + args.timeout)
        if state is None:
            print(f"daemon did not come up within {args.timeout:.0f}s")
            return 1
        print(f"daemon up, build {state.get('build', '?')}")
        return 0

    # Identity mode as well as build id. Flipping PADMAP_PAD_IDENTITY or
    # PADMAP_ONLY_VIRTUAL changes what the virtual pads advertise, and
    # therefore the SDL GUID every mapping is written under -- but it changes
    # no source, so a build-id comparison alone reports a stale daemon as
    # current and leaves it publishing pads the front-end has no mapping for.
    from . import virtual

    theirs = state.get("build")
    their_identity = state.get("identity")
    our_identity = virtual.identity_mode()
    if theirs == ours and their_identity in (None, our_identity):
        print(f"daemon is current (build {ours}, identity {our_identity})")
        return 0
    if theirs == ours:
        print(f"daemon is running with a different pad identity:")
        print(f"  daemon: {their_identity}")
        print(f"  ours:   {our_identity}")
        if args.check:
            return 1
        if not _stop_daemon(state, args.timeout):
            print("could not stop the running daemon")
            return 1
        _spawn_daemon()
        state = _wait_for_daemon(time.monotonic() + args.timeout)
        if state is None:
            print(f"replacement daemon did not come up within {args.timeout:.0f}s")
            return 1
        print(f"restarted; identity {state.get('identity')}, "
              f"{len(state.get('players') or [])} assignment(s) restored")
        return 0

    print("daemon is running older code:")
    print(f"  daemon: {theirs}")
    print(f"  ours:   {ours}")
    if args.check:
        return 1

    if not _stop_daemon(state, args.timeout):
        print("could not stop the running daemon")
        return 1

    _spawn_daemon()
    state = _wait_for_daemon(time.monotonic() + args.timeout)
    if state is None:
        print(f"replacement daemon did not come up within {args.timeout:.0f}s")
        return 1
    players = len(state.get("players") or [])
    print(f"restarted; build {state.get('build', '?')}, "
          f"{players} assignment(s) restored")
    return 0


def _stop_daemon(state: dict, timeout: float) -> bool:
    """Ask the daemon that answered us to exit, and wait for it to go.

    Signals exactly the pid it reported. Matching on the command line instead
    hits *every* padmap daemon the user is running -- including one on a
    different XDG_RUNTIME_DIR, which is none of this one's business. That is
    not hypothetical: an earlier version of this function, run from a test
    with its own runtime dir, took down the real daemon with it.

    SIGTERM rather than SIGKILL: the daemon's handler releases every
    EVIOCGRAB on the way out, and a killed one leaves the machine with no
    working controllers at all.
    """
    from . import protocol

    pid = state.get("pid")
    if not isinstance(pid, int):
        # A daemon predating the pid field. Fall back to a search, but scoped
        # to processes sharing our XDG_RUNTIME_DIR, which is what decides
        # which socket a daemon is on.
        candidates = protocol.daemon_pids()
        if len(candidates) != 1:
            print(f"  cannot identify the running daemon "
                  f"({len(candidates)} candidates); stop it by hand")
            return False
        pid = candidates[0]

    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        return True
    except OSError as error:
        print(f"  could not signal pid {pid}: {error}")
        return False

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if _daemon_state(timeout=0.3) is None:
            return True
        time.sleep(0.2)
    return False


def cmd_clean_config(args: argparse.Namespace) -> int:
    """Undo padmap values that config_save_on_exit persisted into retroarch.cfg.

    Only needed once, for a config written before padmap started disabling
    save-on-exit for the launch. Symptom it fixes: a stale
    `input_player3_joypad_index` equal to an assigned player's index, so one
    controller drives two ports -- visible as four players in an N64 game.
    """
    target = Path(args.config) if args.config else (
        retroarch.CONFIG_DIR / "retroarch.cfg")
    if not target.is_file():
        print(f"No RetroArch config at {target}")
        return 1

    try:
        changes, backup = retroarch.clean_user_config(target, dry_run=args.dry_run)
    except OSError as error:
        print(f"Could not rewrite {target}: {error}")
        return 1

    if not changes:
        print(f"{target} has no padmap leftovers.")
        return 0

    verb = "Would change" if args.dry_run else "Changed"
    print(f"{verb} {len(changes)} setting(s) in {target}:\n")
    for change in changes:
        print(f"  {change}")
    if backup:
        print(f"\nOriginal saved to {backup}")
    elif args.dry_run:
        print("\nDry run; nothing written. Re-run without --dry-run to apply.")
    return 0


def cmd_fetch_art(args: argparse.Namespace) -> int:
    from . import artwork, pegasus

    playlist_dir = Path(args.playlists).expanduser()
    if not playlist_dir.is_dir():
        print(f"No playlist directory at {playlist_dir}")
        return 1

    dest = Path(args.dest).expanduser() if args.dest else pegasus.thumbnail_dir()
    only = args.playlist or None

    print(f"Source:      {artwork.server()}")
    print(f"Destination: {dest}")
    print(f"Kind:        {args.kind}\n")
    print("Reading upstream indexes...")

    plans, error = artwork.plan_all(
        playlist_dir, args.kind, dest, only, args.system,
    )
    if error:
        print(error)
        return 1
    if not plans:
        print(f"No usable playlists in {playlist_dir}")
        return 1

    for plan in plans:
        if plan.note:
            print(f"  {plan.playlist:<12} skipped: {plan.note}")
            continue
        print(f"  {plan.playlist:<12} {plan.system:<24}"
              f" {plan.matched:>5}/{plan.entries} matched,"
              f" {plan.present} already here,"
              f" {plan.missing} with no upstream art")

    # Without the MAME table an arcade playlist matches on raw set names,
    # which upstream has never heard of: 3 hits out of 8302 here. That is
    # indistinguishable from "there is no art" unless it is said out loud.
    from .titles import ENV_TITLES, find_titles
    if not find_titles() and any(
        p.system in ("MAME", "FBNeo - Arcade Games") for p in plans
    ):
        print(f"\nNote: no MAME title table ({ENV_TITLES} is unset), so arcade"
              "\n  set names are matched literally and almost nothing will"
              "\n  match. Run this through the padmap wrapper, which sets it.")

    wanted = [p for p in plans if p.wanted]
    total = sum(len(p.wanted) for p in wanted)
    size = sum(p.bytes for p in wanted)
    if not total:
        print("\nNothing to download; everything matched is already on disk.")
        return 0

    print(f"\nTo download: {total} image(s), about {artwork.human(size)}.")
    free = artwork.free_space(dest)
    if free and size > free:
        print(f"Not enough free space at {dest}"
              f" ({artwork.human(free)} available).")
        return 1
    if args.dry_run:
        print("Dry run; nothing fetched.")
        return 0

    # Re-running is cheap and safe, so an interrupted run needs no cleanup
    # beyond dropping half-written files.
    artwork.prune_partials(dest)

    failures = 0
    downloaded = 0
    for plan in wanted:
        report = artwork.run_plan(
            plan, workers=args.jobs, out=sys.stdout,
        )
        downloaded += report.downloaded
        failures += report.failed
        if report.failed:
            print(f"  {plan.playlist}: {report.failed} failed"
                  f" (first: {report.reason})")

    print(f"\nFetched {downloaded} image(s); {failures} failed.")
    if failures:
        # Not an error worth a non-zero exit: what did arrive is usable, and
        # re-running picks up only the gaps.
        print("Re-run to retry only what is still missing.")
    print("\nNow run:  padmap export-pegasus")
    return 0


def cmd_hide(_args: argparse.Namespace) -> int:
    assignments = _load_assignments()
    pads = [a.pad for a in assignments] or devices.discover()
    print(hide.install_hint(hide.generate_rules(pads), pads))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="padmap", description=__doc__)
    parser.add_argument("-v", "--verbose", action="store_true")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("list", help="show connected pads").set_defaults(func=cmd_list)

    setup = sub.add_parser("setup", help="assign player order by button press")
    setup.add_argument("-n", "--players", type=int, default=None,
                       help="stop after this many controllers")
    setup.set_defaults(func=cmd_setup)

    sub.add_parser(
        "serve", help="run the daemon front-ends connect to"
    ).set_defaults(func=cmd_serve)

    forget = sub.add_parser(
        "forget", help="delete stored controller profiles")
    forget.add_argument("--all", action="store_true",
                        help="remove every profile, not just connected ones")
    forget.set_defaults(func=cmd_forget)

    cal = sub.add_parser(
        "calibrate", help="measure where each controller's sticks rest")
    cal.add_argument("-f", "--force", action="store_true",
                     help="re-measure controllers that already have a profile")
    cal.set_defaults(func=cmd_calibrate)

    export = sub.add_parser(
        "export-pegasus",
        help="build Pegasus collections from RetroArch playlists",
    )
    export.add_argument(
        "--playlists", default="~/.config/retroarch/playlists",
        help="where the .lpl files are (default %(default)s)",
    )
    export.add_argument(
        "--out", default="~/.local/share/padmap/collections",
        help="where to write the generated metadata (default %(default)s)",
    )
    export.add_argument(
        "--no-game-dirs", action="store_true",
        help="print the directories instead of updating game_dirs.txt",
    )
    export.set_defaults(func=cmd_export_pegasus)

    art = sub.add_parser(
        "fetch-art",
        help="download box art from libretro's thumbnail server",
    )
    art.add_argument(
        "--playlists", default="~/.config/retroarch/playlists",
        help="where the .lpl files are (default %(default)s)",
    )
    art.add_argument(
        "--dest", default="",
        help="thumbnail tree to fill (default: RetroArch's own)",
    )
    art.add_argument(
        "--kind", default="Named_Boxarts", choices=list(_ART_KINDS),
        help="which artwork to fetch (default %(default)s)",
    )
    art.add_argument(
        "--playlist", action="append", default=[], metavar="STEM",
        help="only this playlist; repeatable",
    )
    art.add_argument(
        "--system", default=None,
        help="upstream system name, when the core name does not identify one"
             ' (e.g. "MAME"). Only sensible with a single --playlist.',
    )
    art.add_argument(
        "--jobs", type=int, default=8,
        help="parallel downloads (default %(default)s)",
    )
    art.add_argument(
        "--dry-run", action="store_true",
        help="report what would be fetched, and how big, without fetching",
    )
    art.set_defaults(func=cmd_fetch_art)

    ui = sub.add_parser("ui", help="assign player order in a graphical screen")
    ui.add_argument("-n", "--players", type=int, default=4,
                    help="number of player slots to show (default 4)")
    ui.set_defaults(func=cmd_ui)

    sub.add_parser("run", help="republish assigned pads").set_defaults(func=cmd_run)

    launch = sub.add_parser("launch", help="republish, then start RetroArch")
    launch.add_argument(
        "--log", nargs="?", const=True, default=None, metavar="PATH",
        help=f"capture RetroArch output (default {LOG_PATH}) and enable "
             f"verbose logging",
    )
    # `rest` is filled in by main() rather than declared as a positional --
    # see the note there on why argparse.REMAINDER cannot do this job.
    launch.set_defaults(func=cmd_launch, rest=[])

    sub.add_parser("hide", help="print udev rules hiding physical pads"
                   ).set_defaults(func=cmd_hide)

    ensure = sub.add_parser(
        "ensure-daemon",
        help="start the daemon, or restart it if it is running older code",
    )
    ensure.add_argument("--check", action="store_true",
                        help="report staleness and exit non-zero; change "
                             "nothing")
    ensure.add_argument("--timeout", type=float, default=10.0,
                        help="seconds to wait for the daemon (default "
                             "%(default)s)")
    ensure.set_defaults(func=cmd_ensure_daemon)

    clean = sub.add_parser(
        "clean-config",
        help="remove padmap values persisted into retroarch.cfg",
    )
    clean.add_argument("--dry-run", action="store_true",
                       help="show what would change without writing")
    clean.add_argument("--config", default=None, metavar="PATH",
                       help="retroarch.cfg to clean (default ~/.config/"
                            "retroarch/retroarch.cfg)")
    clean.set_defaults(func=cmd_clean_config)

    # Passthrough args are split off by hand. argparse.REMAINDER looks like
    # the right tool but refuses any leading option: `launch --verbose` and
    # `launch -L core.so` both fail with "unrecognized arguments" rather than
    # collecting them. An explicit `--` separator is honoured, and for
    # convenience unknown args after `launch` are forwarded too.
    argv = list(sys.argv[1:] if argv is None else argv)
    passthrough: list[str] = []
    if "--" in argv:
        index = argv.index("--")
        argv, passthrough = argv[:index], argv[index + 1:]

    args, unknown = parser.parse_known_args(argv)
    if args.command == "launch":
        args.rest = unknown + passthrough
    elif unknown or passthrough:
        parser.error(f"unrecognized arguments: {' '.join(unknown + passthrough)}")

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(message)s",
    )
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
