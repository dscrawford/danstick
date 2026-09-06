"""Generate the RetroArch config that pins players to padmap's virtual pads.

Two artefacts:

1. **Autoconfig profiles.** RetroArch looks up button mappings by device name.
   Renaming a pad to "padmap Player 1" would otherwise lose the mapping, so we
   copy the source pad's profile with the identity fields rewritten.

2. **A launch override.** `input_playerN_reserved_device` +
   `input_playerN_device_reservation_type` pin each player to a uniquely named
   virtual pad. Fed to RetroArch via `--appendconfig`, so it applies for one
   launch and leaves the user's config untouched.

The reservation matcher already compares device names exactly (see
task_autodetect.c, reallocate_port_if_needed), so this needs no patched
RetroArch -- the unique names our virtual pads carry are enough.

"Leaves the user's config untouched" was, for a while, false. `--appendconfig`
is merged into the *live* config, and `config_save_on_exit` -- on by default --
writes that merged result back over retroarch.cfg on quit. padmap's per-launch
values were therefore being persisted, and stale ones from earlier sessions
outlived the assignments that produced them. `launch_config` now turns
save-on-exit off for the launch, and `clean_user_config` (the one function here
that does write to retroarch.cfg, and only when asked) undoes what already
leaked.
"""

from __future__ import annotations

import glob
import os
import re
from pathlib import Path

from . import protocol, virtual
from .assign import Assignment
from .virtual import PADMAP_PID, PADMAP_VID, VIRTUAL_PREFIX, virtual_name

CONFIG_DIR = Path(os.environ.get(
    "RETROARCH_CONFIG_DIR",
    Path.home() / ".config" / "retroarch",
))

# Reservation type 2 = INPUT_DEVICE_RESERVATION_RESERVED (input_defines.h):
# the slot is held for this device and nothing else may claim it.
RESERVATION_RESERVED = 2
RESERVATION_NONE = 0

# RetroArch keeps MAX_USERS = 16 player slots and retroarch.cfg carries a full
# set of bindings for every one of them, whatever input_max_users says.
#
# The override must therefore write *all sixteen*, not just the assigned ones.
# Anything left unwritten keeps whatever retroarch.cfg happens to hold, and
# those values are neither empty nor inert: RetroArch's own default is
# input_playerN_joypad_index = N-1, and a config that has been through an
# earlier padmap session (or the hand-edited pre-padmap one) holds arbitrary
# leftovers. Observed here with a single assigned player and an N64 core:
#
#   input_player1_joypad_index = "0"   <- written by padmap, correct
#   input_player3_joypad_index = "0"   <- stale, same pad, still honoured
#
# so the one controller drove players 1 and 3, and the core -- Mupen64Plus
# declares four ports -- reported four players. Silence is not a safe default
# for a slot; every slot has to be said out loud.
MAX_PLAYERS = 16

# Every per-player bind RetroArch keeps in retroarch.cfg, by its short name.
#
# These override autoconfig. `input_driver.c`:
#
#     joykey = (bind_joykey != NO_BTN) ? bind_joykey : autobind_joykey;
#
# so a value left in retroarch.cfg wins over the profile padmap generates, and
# the pad reports itself as configured while behaving as though it is not.
# Observed with `input_player1_start_btn = "9"` against a captured Start of 8:
# RetroArch said "configured in port 1" and Start did nothing.
PLAYER_BINDS = (
    "b", "y", "select", "start", "up", "down", "left", "right",
    "a", "x", "l", "r", "l2", "r2", "l3", "r3",
    "l_x_plus", "l_x_minus", "l_y_plus", "l_y_minus",
    "r_x_plus", "r_x_minus", "r_y_plus", "r_y_minus",
)

# libretro.h device ids. Ports padmap does not manage are set to NONE so a
# core that declares four ports does not hand three of them a controller
# nobody assigned.
RETRO_DEVICE_NONE = 0
RETRO_DEVICE_JOYPAD = 1

# Copied profiles must keep the source's *button mapping* and none of its
# identity. input_device_display_name in particular is what RetroArch shows in
# its input menus and also compares against reservation values, so leaving the
# source's value makes "padmap Player 2" announce itself as the pad it wraps.
# input_phys is scored +/-10 during profile matching (task_autodetect.c) and
# would only ever mismatch our synthetic phys.
_IDENTITY_KEYS = (
    "input_device",
    "input_device_display_name",
    "input_vendor_id",
    "input_product_id",
    "input_phys",
)


def runtime_autoconfig_dir() -> Path:
    """The autoconfig directory padmap hands RetroArch for a launch.

    RetroArch scans exactly one autoconfig directory -- `joypad_autoconfig_dir`
    -- and on this machine that pointed into the Nix store copy of libretro's
    database. Everything padmap wrote to ~/.config/retroarch/autoconfig was
    therefore never read. It only appeared to work while the virtual pads
    mirrored the physical vid/pid, because libretro's *own* entry for the
    underlying controller then matched on vid/pid and scored 50; giving the
    pads padmap's identity removed that accidental match and left them
    unconfigured.

    So padmap points the setting at its own directory for the launch. That
    also stops the database competing with a mapping the user recorded: an
    entry there can outscore ours, or bind controls that were never captured.
    Since the physical pads are hidden, the only pads RetroArch can see are
    padmap's, and a directory holding just their profiles is complete.

    Under XDG_RUNTIME_DIR, so it is rebuilt from scratch each session and
    cannot accumulate profiles for players that no longer exist.
    """
    return protocol.runtime_dir() / "autoconfig"


def autoconfig_dirs() -> list[Path]:
    """Where to look for existing joypad profiles, most specific first."""
    dirs: list[Path] = []
    override = os.environ.get("PADMAP_AUTOCONFIG_DIRS")
    if override:
        dirs.extend(Path(p) for p in override.split(":") if p)

    dirs.append(CONFIG_DIR / "autoconfig")
    dirs.append(Path("/run/current-system/sw/share/libretro/autoconfig"))
    # Nix has no global share dir; fall back to the store path directly.
    dirs.extend(
        Path(p) for p in sorted(glob.glob(
            "/nix/store/*retroarch-joypad-autoconfig-*/share/libretro/autoconfig"
        ), reverse=True)
    )
    return [d for d in dirs if d.is_dir()]


def parse_profile(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    try:
        text = path.read_text(errors="replace")
    except OSError:
        return values
    for line in text.splitlines():
        match = re.match(r'\s*([A-Za-z0-9_]+)\s*=\s*"?([^"]*)"?\s*$', line)
        if match:
            values[match.group(1)] = match.group(2)
    return values


def find_profile(name: str, vid: int, pid: int) -> Path | None:
    """Best existing profile for a physical pad.

    Exact name match wins; a vid/pid match is the fallback, mirroring how
    RetroArch scores autoconfig candidates.
    """
    vid_pid_match: Path | None = None
    for base in autoconfig_dirs():
        for path in sorted(base.rglob("*.cfg")):
            values = parse_profile(path)
            if values.get("input_device") == name:
                return path
            if vid_pid_match is None and vid and pid:
                try:
                    if (int(values.get("input_vendor_id", "0")) == vid
                            and int(values.get("input_product_id", "0")) == pid):
                        vid_pid_match = path
                except ValueError:
                    pass
    return vid_pid_match


def derive_profile(
    source: Path | None, player: int, vid: int = 0, pid: int = 0
) -> str:
    """A profile for the virtual pad, keeping the source's button mapping.

    vid/pid must be whatever the virtual pad actually advertises, which
    depends on the identity mode -- see virtual.identity_for. RetroArch
    matches autoconfig primarily by device name, but a disagreeing vid/pid
    scores against us, and affinity is what decides whether the profile is
    used at all.
    """
    header = (
        f"# Generated by padmap for {virtual_name(player)}.\n"
        f"# Button mapping copied from: "
        f"{source.name if source else '<none found>'}\n"
    )
    lines = [
        'input_driver = "udev"',
        f'input_device = "{virtual_name(player)}"',
        f'input_device_display_name = "{virtual_name(player)}"',
        f'input_vendor_id = "{vid or PADMAP_VID}"',
        f'input_product_id = "{pid or PADMAP_PID}"',
    ]
    if source is not None:
        values = parse_profile(source)
        for key, value in values.items():
            if key in _IDENTITY_KEYS or key == "input_driver":
                continue
            lines.append(f'{key} = "{value}"')
    return header + "\n".join(lines) + "\n"


def install_profiles(
    assignments: list[Assignment], dest: Path | None = None
) -> list[Path]:
    """Write a profile per virtual pad. Returns the paths written.

    Into padmap's own directory, which the launch override points RetroArch
    at -- see runtime_autoconfig_dir. The `udev` subdirectory matters:
    RetroArch looks in `<dir>/<driver>` first and only falls back to the base
    directory when that is empty.
    """
    target = dest or (runtime_autoconfig_dir() / "udev")
    target.mkdir(parents=True, exist_ok=True)

    # Clear previous generations. A profile for a player who no longer exists
    # would still be scanned, and could match a pad it was never meant for.
    for stale in target.glob("*.cfg"):
        if stale.name.startswith(VIRTUAL_PREFIX):
            stale.unlink()

    from . import controllercfg

    written: list[Path] = []
    for assignment in assignments:
        source = find_profile(
            assignment.pad.name, assignment.pad.vid, assignment.pad.pid
        )
        path = target / f"{virtual_name(assignment.player)}.cfg"

        bindings = controllercfg.stored_bindings(assignment.pad)
        if bindings:
            # The user pressed these buttons themselves. Copying libretro's
            # entry instead would give two sets of bindings for one
            # controller, differing in ways nobody is told about.
            path.write_text(controllercfg.retroarch_profile(
                assignment.player, assignment.pad, bindings,
                source=source.name if source else "",
                # Re-resolved here rather than at capture time, so a
                # correction to a console's key table reaches controllers
                # already mapped under it.
                layout=controllercfg.stored_layout(assignment.pad),
            ))
        else:
            # The ids the pad will actually advertise, not the physical
            # ones. They coincide in mirror mode and do not in padmap mode,
            # and a profile claiming a vid/pid the device does not report
            # scores against itself in RetroArch's autoconfig matching.
            identity = virtual.identity_for(assignment.pad)
            path.write_text(derive_profile(
                source, assignment.player,
                vid=identity.vendor, pid=identity.product,
            ))
        written.append(path)
    return written


def visible_order() -> dict[int, str]:
    """Pad index -> device path, exactly as RetroArch's udev driver will see it.

    udev_joypad_init enumerates ID_INPUT_JOYSTICK=1 devices and assigns each
    the first vacant slot, and libudev returns that list sorted by syspath.
    Verified against a live RetroArch run: an 8-pad setup mapped event23..30
    to pad indices 0..7 exactly as this predicts.

    Computed at launch time, so it reflects whatever is plugged in right now
    rather than a stale assumption baked into a config file.
    """
    from . import devices

    # retroarch_only: pads hidden by the `padmap hide` rules are absent from
    # RetroArch's enumeration, so counting them here would shift every index.
    return {
        index: pad.path
        for index, pad in enumerate(
            devices.discover(include_virtual=True, retroarch_only=True)
        )
    }


def compute_pad_indices(
    virtual_paths: dict[int, str], order: dict[int, str] | None = None
) -> dict[int, int]:
    """Player -> the pad index RetroArch's udev driver will give its pad.

    `order` lets a caller that already enumerated reuse the result, so a
    single launch does not discover twice and risk the two disagreeing.
    """
    by_path = {
        path: index
        for index, path in (visible_order() if order is None else order).items()
    }
    return {
        player: by_path[path]
        for player, path in virtual_paths.items()
        if path in by_path
    }


def managed_players(
    assignments: list[Assignment],
    virtual_paths: dict[int, str],
    order: dict[int, str] | None = None,
) -> dict[int, int]:
    """Player -> pad index, for the players padmap is actually binding.

    An assignment whose virtual pad is missing from the enumeration is *not*
    managed: padmap cannot bind a pad RetroArch will not see, and pretending
    otherwise leaves that slot on whatever retroarch.cfg holds.
    """
    indices = compute_pad_indices(virtual_paths, order)
    return {
        a.player: indices[a.player]
        for a in assignments
        if a.player in indices
    }


def launch_args(
    assignments: list[Assignment],
    virtual_paths: dict[int, str],
    order: dict[int, str] | None = None,
) -> list[str]:
    """RetroArch command-line flags emptying every unassigned core port.

    This is the *only* working way to do it. `input_libretro_device_pN` reads
    like the config setting for a port's device type, and RetroArch ignores it
    from retroarch.cfg and --appendconfig alike: configuration.c only touches
    that key in `input_remapping_load_file`/`_save_file`, i.e. inside `.rmp`
    remap files. Setting it in the launch override changed nothing at all.

    `--nodevice PORT` does work. It runs `input_config_set_device(port,
    RETRO_DEVICE_NONE)` during argument parsing (retroarch.c, case 'N'), which
    is what `command_event_init_controllers` later reads back per core port:

        for (port = 0; port < num_core_ports; port++)
           for (i = 0; i < num_active_users; i++)
              if (port == input_remap_ports[i])
                 device = input_config_get_device(port);
           core_set_controller_port_device(port, device);

    Verified with a probe core that logs every
    `retro_set_controller_port_device` call: without these flags an N64 core's
    four ports all get RETRO_DEVICE_JOYPAD, with them only the assigned ones
    do.

    `input_max_users` reaches the same result via `num_active_users`, and is
    deliberately not used: RetroArch skips reserved slots when looking for the
    first free player slot and bails out if that index reaches
    `input_max_users`, so constraining it would break the reservations that
    are padmap's order-independent binding. A command-line flag has no such
    interaction.

    Ports above the core's own port count simply do not exist -- the loop is
    bounded by `num_core_ports` -- so emitting flags up to MAX_PLAYERS costs
    nothing and needs no knowledge of which core is about to run.
    """
    managed = managed_players(assignments, virtual_paths, order)
    args: list[str] = []
    for player in range(1, MAX_PLAYERS + 1):
        if player not in managed:
            args += ["--nodevice", str(player)]
    return args


def write_launch_args(
    assignments: list[Assignment],
    virtual_paths: dict[int, str],
    path: Path,
) -> Path:
    """Persist the flags so the padmap-play wrapper can pass them on.

    A front-end spawns `padmap-play <rom>` and knows nothing about players, so
    the wrapper has to pick these up the same way it picks up launch.cfg. One
    token per line: every token here is a flag or a small integer, so there is
    nothing to quote.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    args = launch_args(assignments, virtual_paths)
    path.write_text("".join(f"{arg}\n" for arg in args))
    return path


def _empty_indices(pad_count: int, wanted: int) -> list[int]:
    """Pad indices with no device behind them, for the unmanaged slots.

    RetroArch enumerates `pad_count` pads into indices 0..pad_count-1, so
    everything from there to MAX_PLAYERS-1 is vacant. Handing each unmanaged
    player a *distinct* vacant index is what stops two slots sharing one pad;
    reusing RetroArch's own N-1 default would not, because with the physical
    pads unhidden index N-1 is a real controller.

    Degrades rather than collides if every index is occupied: the unmanaged
    slots are set to RETRO_DEVICE_NONE regardless, so a repeated index there
    reaches no core port.
    """
    ceiling = MAX_PLAYERS - 1
    return [min(pad_count + n, ceiling) for n in range(wanted)]


def launch_config(
    assignments: list[Assignment],
    virtual_paths: dict[int, str],
    verbose: bool = False,
) -> str:
    """The full override handed to RetroArch via --appendconfig.

    Belt and braces. The joypad_index lines are the mechanism that is
    verified to work; the reservation lines are a second, order-independent
    binding that costs nothing if RetroArch honours it and is harmless if not.

    Every one of the sixteen player slots is written, assigned or not -- see
    MAX_PLAYERS. An unmanaged slot gets a vacant pad index, a cleared
    reservation, and RETRO_DEVICE_NONE, which together are the difference
    between "padmap said nothing about player 3" and "player 3 has no
    controller".
    """
    order = visible_order()
    managed = managed_players(assignments, virtual_paths, order)
    dropped = [a.player for a in assignments if a.player not in managed]

    lines = [
        "# Generated by padmap. Passed to RetroArch with --appendconfig.",
        "# Regenerated at every launch, since pad indices depend on what is",
        "# currently plugged in.",
        "#",
        f"# {len(managed)} of {MAX_PLAYERS} player slots are assigned; the rest",
        "# are explicitly emptied rather than left to retroarch.cfg.",
    ]
    for player in sorted(dropped):
        lines.append(
            f"# player {player}: virtual pad not in the enumeration, emptied"
        )

    lines.append("")
    lines.append("# Primary binding: explicit pad indices, computed from the")
    lines.append("# live udev enumeration order.")
    spare = iter(_empty_indices(len(order), MAX_PLAYERS - len(managed)))
    for player in range(1, MAX_PLAYERS + 1):
        index = managed.get(player)
        lines.append(
            f'input_player{player}_joypad_index = '
            f'"{next(spare) if index is None else index}"'
        )

    lines.append("")
    lines.append("# Secondary binding: reservation by the unique names we")
    lines.append("# gave the virtual pads, which does not depend on ordering.")
    lines.append("# Unmanaged slots have theirs cleared, so a reservation left")
    lines.append("# over from a session with more players cannot hold a slot")
    lines.append("# open for a virtual pad that no longer exists.")
    lines.append(_reservation_lines(sorted(managed)))

    lines.append("")
    lines.append("# Hand the managed players back to autoconfig.")
    lines.append("#")
    lines.append("# A per-player bind in retroarch.cfg beats the autoconfig")
    lines.append("# profile -- RetroArch only falls back to the profile when")
    lines.append("# the explicit bind is `nul`. Leftovers there make a pad")
    lines.append("# report as configured while the buttons do nothing.")
    lines.append("# Only managed slots: an unmanaged one has no pad, so what")
    lines.append("# its binds say cannot matter.")
    for player in sorted(managed):
        for bind in PLAYER_BINDS:
            lines.append(f'input_player{player}_{bind}_btn = "nul"')
            lines.append(f'input_player{player}_{bind}_axis = "nul"')

    lines.append("")
    lines.append("# Autoconfig comes from padmap's own directory, not")
    lines.append("# libretro's database. RetroArch scans exactly one, and")
    lines.append("# the database was both outscoring these profiles and")
    lines.append("# binding controls the user never mapped.")
    lines.append(
        f'joypad_autoconfig_dir = "{runtime_autoconfig_dir()}"')

    lines.append("")
    lines.append("# Core-side ports are NOT set here. input_libretro_device_pN")
    lines.append("# looks like the setting for it and is silently ignored from")
    lines.append("# a config file -- see launch_args(), which does the job on")
    lines.append("# the command line instead.")

    lines.append("")
    lines.append("# --appendconfig is merged into the live config, and with")
    lines.append("# config_save_on_exit the whole merged result is written")
    lines.append("# back to retroarch.cfg on quit. That is how the stale")
    lines.append("# reservations and duplicate joypad indices this file now")
    lines.append("# has to override got there in the first place. Turning it")
    lines.append("# off is what actually makes the override per-launch.")
    lines.append('config_save_on_exit = "false"')

    if verbose:
        # RetroArch is quiet at the default frontend_log_level, which hides
        # exactly the [udev] and [Autoconf] lines worth reading here.
        lines.extend([
            "",
            "# Diagnostics, enabled by `padmap launch --log`.",
            'log_verbosity = "true"',
            'frontend_log_level = "0"',
        ])

    lines.append("")
    return "\n".join(lines) + "\n"


def _reservation_lines(managed: list[int]) -> str:
    """Reservation settings for all MAX_PLAYERS slots.

    Managed slots are reserved for their virtual pad by name; every other slot
    is cleared, because an uncleared reservation naming a pad that is no
    longer republished still occupies the slot.
    """
    lines = []
    for player in range(1, MAX_PLAYERS + 1):
        reserved = player in managed
        name = virtual_name(player) if reserved else ""
        kind = RESERVATION_RESERVED if reserved else RESERVATION_NONE
        lines.append(f'input_player{player}_reserved_device = "{name}"')
        lines.append(
            f'input_player{player}_device_reservation_type = "{kind}"'
        )
    return "\n".join(lines)


def reservation_config(assignments: list[Assignment]) -> str:
    """Config text pinning each player to its virtual pad by name."""
    lines = [
        "# Generated by padmap. Pass to RetroArch with --appendconfig.",
        "# Players are pinned by device name, so pad enumeration order",
        "# no longer matters.",
        "",
        _reservation_lines(sorted(a.player for a in assignments)),
        "",
    ]
    # Deliberately NOT setting input_max_users.
    #
    # reallocate_port_if_needed() computes first_free_player_slot while
    # skipping RESERVED slots, then early-returns if that index is >=
    # input_max_users -- before it ever reaches the reservation matching
    # loop. Setting input_max_users to the number of reserved players makes
    # every slot below it reserved, so the early return fires and no
    # reservation is ever matched. There must be at least one free,
    # unreserved slot below input_max_users for this to work at all.
    return "\n".join(lines) + "\n"


_SETTING = re.compile(r'^(\s*)([A-Za-z0-9_]+)(\s*=\s*)"(.*)"(\s*)$')


def _cleaned_value(key: str, value: str, reserved: dict[int, str]) -> str | None:
    """The stock RetroArch value for a key padmap may have leaked into, or None.

    Only keys already present are rewritten; nothing is added. A key padmap
    never wrote is left exactly as the user had it.
    """
    match = re.fullmatch(r"input_player(\d+)_reserved_device", key)
    if match:
        return "" if value.startswith(VIRTUAL_PREFIX) else None

    match = re.fullmatch(r"input_player(\d+)_device_reservation_type", key)
    if match:
        # Paired with the line above: a type left at RESERVED while its
        # device name is empty holds the slot for nothing at all.
        player = int(match.group(1))
        stale = reserved.get(player, "").startswith(VIRTUAL_PREFIX)
        if stale or (not reserved.get(player) and value != str(RESERVATION_NONE)):
            return str(RESERVATION_NONE)
        return None

    match = re.fullmatch(
        r"input_player\d+_(" + "|".join(PLAYER_BINDS) + r")_(btn|axis)", key)
    if match:
        # Same reason as the launch override: these outrank autoconfig, and
        # RetroArch saved them here itself. "nul" is how it spells unbound.
        return None if value == "nul" else "nul"

    match = re.fullmatch(r"input_player(\d+)_joypad_index", key)
    if match:
        # RetroArch's own default is N-1, one pad per player in order. Any
        # other value is either padmap's or a hand-edit that padmap now
        # overrides at every launch anyway, so N-1 is the honest reset.
        return str(int(match.group(1)) - 1)

    # Deliberately no rule for input_libretro_device_pN. RetroArch neither
    # loads nor saves that key in retroarch.cfg -- it lives only in `.rmp`
    # remap files -- so it cannot have leaked here, and a rule for it would
    # only be able to damage a remap file someone pointed --config at.
    return None


def clean_user_config(
    path: Path | None = None, dry_run: bool = False
) -> tuple[list[str], Path | None]:
    """Strip persisted padmap values out of retroarch.cfg.

    Only needed for configs written before `launch_config` started disabling
    config_save_on_exit; after that no new leakage occurs. Returns the changes
    made (as "key: old -> new" strings) and the backup path, if one was taken.

    Deliberately a separate, explicitly invoked command rather than something
    `launch` does: this is the only code in padmap that writes to the user's
    RetroArch config, and it should stay something they asked for.
    """
    target = path or (CONFIG_DIR / "retroarch.cfg")
    lines = target.read_text(errors="replace").splitlines(keepends=True)

    # Reservation type and device name have to be judged together, and the
    # file is sorted alphabetically so the type comes first. Collect names up
    # front rather than relying on the order.
    reserved: dict[int, str] = {}
    for line in lines:
        match = _SETTING.match(line)
        if not match:
            continue
        slot = re.fullmatch(r"input_player(\d+)_reserved_device", match.group(2))
        if slot:
            reserved[int(slot.group(1))] = match.group(4)

    changes: list[str] = []
    out: list[str] = []
    for line in lines:
        match = _SETTING.match(line)
        if match is None:
            out.append(line)
            continue
        indent, key, sep, value, trail = match.groups()
        new = _cleaned_value(key, value, reserved)
        if new is None or new == value:
            out.append(line)
            continue
        changes.append(f'{key}: "{value}" -> "{new}"')
        out.append(f'{indent}{key}{sep}"{new}"{trail}')

    if not changes or dry_run:
        return changes, None

    # RetroArch's config is 3000-odd lines the user did not write by hand, but
    # it is still theirs and this rewrite is not reconstructible from padmap
    # state. Back it up before touching it.
    backup = target.with_suffix(target.suffix + ".padmap-backup")
    backup.write_text("".join(lines))
    target.write_text("".join(out))
    return changes, backup


def write_launch_config(
    assignments: list[Assignment],
    virtual_paths: dict[int, str],
    path: Path,
    verbose: bool = False,
) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(launch_config(assignments, virtual_paths, verbose=verbose))
    return path
