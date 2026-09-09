#!/usr/bin/env python3
"""Playing a game: which pad is which player, and what each one is bound to.

The stories under test are the ones that only fail once someone is holding a
controller:

  S14  launching a game resolves the most specific mapping -- this game, else
       this console, else the controller default -- and writes it where
       RetroArch will read it.
  S15  only padmap's virtual pads reach RetroArch; the physical ones are
       hidden.
  S16  unassigned core ports are emptied, so one controller is not four
       players.
  S17  two players each get their own pad, slot and profile.

Every one of these is decided by *predicting* what RetroArch's udev driver is
about to enumerate. So the fixture here is not a stubbed `visible_order()`: it
is a fake `/sys/class/input` that `padmap.devices.discover` walks for real,
with the two facts that make the prediction non-obvious built in --

  * libudev sorts by **syspath**, so padmap's virtual pads (under
    `/sys/devices/virtual/...`) enumerate *after* the physical ones on the USB
    bus, even though their event nodes are numbered lower. The fixture gives
    every virtual pad a lower event number than every physical one, so a check
    that quietly sorted by event node would come out backwards.
  * `padmap hide` clears ID_INPUT_JOYSTICK rather than removing the device, so
    a hidden pad is still discoverable by padmap and simply absent from
    RetroArch's enumeration. The index prediction has to count one and not the
    other.

Nothing here opens a device: `devices.open_device` is replaced with something
that raises, because a real controller on this machine would otherwise be one
`identity_for` away, and a live padmap daemon is holding those pads.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_journey_launch.py
"""

from __future__ import annotations

import atexit
import glob as _glob
import io
import json
import os as _os
import re
import shutil
import sys
import tempfile
from contextlib import redirect_stderr
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

# Redirected before padmap is imported: `retroarch.CONFIG_DIR` is read at
# import time, and XDG_RUNTIME_DIR is where the LIVE daemon on this machine
# keeps the autoconfig profiles and launch.cfg a running game is using.
_SANDBOX = Path(tempfile.mkdtemp(prefix="padmap-check-journey-launch-"))
_os.environ["XDG_RUNTIME_DIR"] = str(_SANDBOX / "run")
_os.environ["XDG_CONFIG_HOME"] = str(_SANDBOX / "config")
_os.environ["XDG_DATA_HOME"] = str(_SANDBOX / "data")
_os.environ["RETROARCH_CONFIG_DIR"] = str(_SANDBOX / "retroarch")
_os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "devices")
# Pinned so that nothing here has to open a device to learn what a virtual pad
# advertises. `mirror` (the default) reads the ids off the physical pad, and
# the pads below are fictional -- but the *paths* they carry are ordinary
# /dev/input nodes, and opening one of those for real is exactly what this
# check must never do.
_os.environ["PADMAP_PAD_IDENTITY"] = "padmap"
# An empty escape hatch: PADMAP_ONLY_DEVICE would filter discovery by name and
# silently shrink every enumeration below.
_os.environ.pop("PADMAP_ONLY_DEVICE", None)
for _name in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    Path(_os.environ[_name]).mkdir(parents=True, exist_ok=True)
atexit.register(shutil.rmtree, _SANDBOX, ignore_errors=True)

from padmap import (controllercfg, devices, launch, profiles,  # noqa: E402
                    protocol, retroarch, virtual)
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

# A database of one (empty) directory, so `find_profile` never scans the real
# libretro autoconfig database: what libretro happens to ship must not decide
# whether these checks pass, and it holds several hundred files.
_FAKE_DB = _SANDBOX / "autoconfig-db"
_FAKE_DB.mkdir(parents=True, exist_ok=True)
retroarch.autoconfig_dirs = lambda: [_FAKE_DB]  # type: ignore[assignment]


def _no_devices(pad: Pad):  # noqa: ANN202
    raise AssertionError(
        f"this check tried to open {pad.path}, which is a real device node on "
        f"this machine and may be a controller a live daemon is grabbing")


devices.open_device = _no_devices  # type: ignore[assignment]


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def ok(message: str) -> None:
    print(f"  ok  {message}")


def gap(message: str) -> None:
    """Something that is not right but is not asserted, so this file stays green.

    Written out loud rather than left in a comment: the maintainer turns these
    into assertions once the source is fixed.
    """
    print(f"  gap: {message}")


def scenario(title: str) -> None:
    print(f"\n{title}:")


# -- a fake /sys/class/input -------------------------------------------------
#
# devices.discover reads three things: the glob of /sys/class/input/input*, the
# attribute files under each, and whether the node exists in /dev/input. The
# first two are redirected into a temp tree; the third and the two udev-facing
# predicates are answered from the fixture, since ID_INPUT_JOYSTICK is set by
# udev rules this check must not install.

_MACHINE: "Machine | None" = None


class _FakePath:
    def __getattr__(self, name):  # noqa: ANN001, ANN202
        return getattr(_os.path, name)

    def exists(self, target) -> bool:  # noqa: ANN001
        text = str(target)
        if text.startswith("/dev/input/"):
            return _MACHINE is not None and text in _MACHINE.nodes
        return _os.path.exists(text)


class _FakeOs:
    path = _FakePath()

    def __getattr__(self, name):  # noqa: ANN001, ANN202
        return getattr(_os, name)


class _FakeGlob:
    def glob(self, pattern: str) -> list[str]:
        if pattern.startswith("/sys/class/input") and _MACHINE is not None:
            pattern = pattern.replace(
                "/sys/class/input", str(_MACHINE.class_dir), 1)
        return _glob.glob(pattern)


devices.os = _FakeOs()  # type: ignore[assignment]
devices.glob = _FakeGlob()  # type: ignore[assignment]
devices._retroarch_sees = (  # type: ignore[assignment]
    lambda node, input_dir: _MACHINE is not None and node in _MACHINE.visible)
devices._looks_like_joypad = (  # type: ignore[assignment]
    lambda node: _MACHINE is not None and node in _MACHINE.joypads)


class Machine:
    """One arrangement of plugged-in devices, as /sys would present it.

    Physical pads hang off USB ports under `/sys/devices/pci0000:00/...`;
    virtual pads live under `/sys/devices/virtual/input`, which is where
    uinput puts them and is why they sort last. Event numbers are deliberately
    the other way round -- virtual pads get lower ones -- because that is the
    arrangement on the machine this was written against (padmap Player 1 on
    event26, adapter ports on event27..30) and the one that catches an
    ordering taken from the node name.
    """

    def __init__(self, name: str) -> None:
        self.root = Path(tempfile.mkdtemp(dir=_SANDBOX, prefix=f"sys-{name}-"))
        self.class_dir = self.root / "class" / "input"
        self.class_dir.mkdir(parents=True)
        self.nodes: set[str] = set()
        self.visible: set[str] = set()
        self.joypads: set[str] = set()
        self.physical: list[str] = []
        self.virtual_paths: dict[int, str] = {}

    def _add(self, ordinal: int, event: int, subpath: str, name: str,
             phys: str, vid: int, pid: int, visible: bool,
             joypad: bool) -> str:
        target = self.root / "devices" / subpath / f"input{ordinal}"
        target.mkdir(parents=True)
        (target / f"event{event}").write_text("")
        entry = self.class_dir / f"input{ordinal}"
        entry.mkdir()
        (entry / "name").write_text(name + "\n")
        (entry / "phys").write_text(phys + "\n")
        (entry / "uniq").write_text("\n")
        (entry / "id").mkdir()
        (entry / "id" / "vendor").write_text(f"{vid:04x}\n")
        (entry / "id" / "product").write_text(f"{pid:04x}\n")
        (entry / f"event{event}").symlink_to(target / f"event{event}")

        node = f"/dev/input/event{event}"
        self.nodes.add(node)
        if visible:
            self.visible.add(node)
        if joypad:
            self.joypads.add(node)
        return node

    def add_physical(self, port: int, name: str = "padmap-check Adapter Port",
                     vid: int = 0x0079, pid: int = 0x1843,
                     hidden: bool = False) -> str:
        """One controller on USB port `port`. `hidden` = `padmap hide` ran."""
        node = self._add(
            ordinal=300 + port, event=920 + port,
            subpath=f"pci0000:00/0000:00:14.0/usb3/3-4/3-4.{port:02d}/input",
            name=f"{name} {port}", phys=f"usb-0000:00:14.0-4.{port:02d}/input0",
            vid=vid, pid=pid, visible=not hidden, joypad=True)
        self.physical.append(node)
        return node

    def add_adapter_port(self, port: int, hidden: bool = False) -> str:
        """A port of a multi-port adapter: same name, vendor and product.

        The four ports of a Mayflash adapter are byte-identical upstream, so
        this is the case where nothing but the virtual pad tells two players
        apart.
        """
        node = self._add(
            ordinal=300 + port, event=920 + port,
            subpath="pci0000:00/0000:00:14.0/usb3/3-4/3-4.09/input",
            name="padmap-check 4-Port Adapter", phys="usb-0000:00:14.0-4.9/input0",
            vid=0x057E, pid=0x0337, visible=not hidden, joypad=True)
        self.physical.append(node)
        return node

    def add_virtual(self, player: int) -> str:
        node = self._add(
            ordinal=470 + player, event=900 + player, subpath="virtual/input",
            name=virtual.virtual_name(player), phys=virtual.virtual_phys(player),
            vid=virtual.PADMAP_VID, pid=virtual.PADMAP_PID,
            visible=True, joypad=True)
        self.virtual_paths[player] = node
        return node

    def add_keyboard(self) -> str:
        """Not a joypad at all, and must never take an index."""
        return self._add(
            ordinal=7, event=903, subpath="platform/i8042/serio0/input",
            name="padmap-check AT Keyboard", phys="isa0060/serio0/input0",
            vid=0x0001, pid=0x0001, visible=False, joypad=False)

    def install(self) -> "Machine":
        global _MACHINE
        _MACHINE = self
        return self


def bench(players: int, hidden: bool, extras: int = 0,
          adapter: bool = False) -> Machine:
    """`players` assigned pads, republished, with the physicals hidden or not.

    `extras` adds visible joysticks padmap does not manage -- a wheel, a
    controller plugged in after setup -- which shift every index after them.
    """
    machine = Machine(f"p{players}-{'hidden' if hidden else 'visible'}")
    for port in range(1, players + 1):
        if adapter:
            machine.add_adapter_port(port, hidden=hidden)
        else:
            machine.add_physical(port, hidden=hidden)
    for extra in range(extras):
        machine.add_physical(50 + extra, name="padmap-check Wheel", hidden=False)
    machine.add_keyboard()
    for player in range(1, players + 1):
        machine.add_virtual(player)
    return machine.install()


# -- fixtures ----------------------------------------------------------------

# Two captures of one physical pad, told apart by RetroArch keys only one of
# them can produce. The GameCube layout binds its A to input_a_btn; the N64
# layout has no A override, so its 'a' lands on the canonical input_b_btn and
# input_a_btn cannot appear at all. The N64 layout puts *b* on input_y_btn.
GC_DEFAULT = profiles.Mapping(
    layout="gamecube",
    buttons={
        "a": Binding("button", 0), "b": Binding("button", 1),
        "x": Binding("button", 2), "y": Binding("button", 3),
        "start": Binding("button", 7),
    },
)
GC_FOR_N64 = profiles.Mapping(
    layout="n64",
    buttons={
        "a": Binding("button", 0), "b": Binding("button", 1),
        "start": Binding("button", 7),
        "lefttrigger": Binding("axis", 4, 1),
        "rightstick_up": Binding("button", 3),
    },
)
GC_FOR_THIS_GAME = profiles.Mapping(
    layout="n64",
    buttons={"a": Binding("button", 5), "start": Binding("button", 7)},
)

CORE = "/nix/store/zzz/lib/retroarch/cores/mupen64plus_next_libretro.so"
UNKNOWN_CORE = "/nix/store/zzz/lib/retroarch/cores/genesis_plus_gx_libretro.so"


def store(pad: Pad, mappings: dict[str, profiles.Mapping] | None = None,
          axes: dict[int, profiles.AxisCalibration] | None = None) -> None:
    profile = profiles.Profile(signature=profiles.signature(pad), name=pad.name)
    profile.axes = dict(axes or {})
    profile.mappings = dict(mappings or {})
    profiles.save(profile)


def pads_by_path() -> dict[str, Pad]:
    return {pad.path: pad for pad in devices.discover()}


def assign_all(machine: Machine) -> list[Assignment]:
    """Assignments for every physical pad the fixture republished, in order."""
    found = pads_by_path()
    out: list[Assignment] = []
    for player, node in enumerate(machine.physical, start=1):
        if player not in machine.virtual_paths:
            break
        pad = found.get(node)
        if pad is None:
            fail(f"the fixture's own pad {node} was not discovered")
        out.append(Assignment(player=player, pad=pad, button=0))
    return out


def settings(text: str) -> dict[str, str]:
    """key -> value for every `key = "value"` line, rejecting duplicates."""
    out: dict[str, str] = {}
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or " = " not in stripped:
            continue
        key, _, value = stripped.partition(" = ")
        key = key.strip()
        if key in out:
            fail(f"{key} is written twice in one config; RetroArch takes the "
                 f"last one, so which value wins is not decided here")
        out[key] = value.strip().strip('"')
    return out


def override(assignments: list[Assignment],
             virtual_paths: dict[int, str]) -> dict[str, str]:
    return settings(retroarch.launch_config(assignments, virtual_paths))


def indices(cfg: dict[str, str]) -> dict[int, int]:
    out: dict[int, int] = {}
    for player in range(1, retroarch.MAX_PLAYERS + 1):
        key = f"input_player{player}_joypad_index"
        if key not in cfg:
            fail(f"player {player} has no {key}. A slot left unwritten keeps "
                 f"whatever retroarch.cfg holds, and RetroArch's own default "
                 f"there is joypad_index = N-1 -- which is how one controller "
                 f"ended up driving players 1 and 3 of an N64 game")
        try:
            out[player] = int(cfg[key])
        except ValueError:
            fail(f"player {player}'s pad index is {cfg[key]!r}, not a number")
    return out


def emptied_ports(assignments: list[Assignment],
                  virtual_paths: dict[int, str]) -> list[int]:
    flags = retroarch.launch_args(assignments, virtual_paths)
    if len(flags) % 2:
        fail(f"launch_args emitted an odd number of tokens ({flags}); "
             f"RetroArch would read the next flag as a port number")
    for i in range(0, len(flags), 2):
        if flags[i] != "--nodevice":
            fail(f"launch_args emitted {flags[i]!r}. --nodevice is the only "
                 f"spelling RetroArch honours: input_libretro_device_pN is "
                 f"read only from .rmp remap files and ignored from a config")
    return [int(flags[i + 1]) for i in range(0, len(flags), 2)]


def profile_dir() -> Path:
    return retroarch.runtime_autoconfig_dir() / "udev"


def profile_text(player: int, directory: Path | None = None) -> str:
    path = (directory or profile_dir()) / f"{virtual.virtual_name(player)}.cfg"
    if not path.exists():
        fail(f"no autoconfig profile for player {player}; RetroArch would fall "
             f"back to libretro's database or to nothing at all")
    return path.read_text()


# -- S15: what RetroArch can see --------------------------------------------

def check_enumeration_is_syspath_order() -> None:
    scenario("S15: the pad indices RetroArch will use, read off /sys")
    machine = bench(players=2, hidden=False)
    order = retroarch.visible_order()

    expected = machine.physical + [machine.virtual_paths[1],
                                   machine.virtual_paths[2]]
    if [order[i] for i in sorted(order)] != expected:
        fail(f"the predicted enumeration is {[order[i] for i in sorted(order)]}, "
             f"not {expected}. RetroArch's udev driver takes libudev's list, "
             f"which is sorted by syspath, and gives each device the first "
             f"vacant slot -- so this order *is* the pad index")
    ok(f"{len(order)} pads, in syspath order")

    # The fact that makes the prediction non-obvious.
    virtual_event = int(Path(machine.virtual_paths[1]).name[len("event"):])
    physical_event = max(
        int(Path(node).name[len("event"):]) for node in machine.physical)
    if virtual_event >= physical_event:
        fail("the fixture no longer numbers the virtual pads below the "
             "physical ones, so it cannot tell a syspath sort from a node sort")
    by_path = {path: index for index, path in order.items()}
    if by_path[machine.virtual_paths[1]] <= by_path[machine.physical[-1]]:
        fail(f"event{virtual_event} (a virtual pad, under /sys/devices/virtual) "
             f"was enumerated before event{physical_event} (a physical pad on "
             f"the USB bus). libudev sorts by syspath, where 'virtual' comes "
             f"after 'pci', so every predicted index would be wrong")
    ok(f"event{virtual_event} (virtual) sorts after event{physical_event} "
       f"(physical), because syspath decides, not the node number")

    if any(path.endswith("event903") for path in order.values()):
        fail("the keyboard took a pad index; RetroArch enumerates only devices "
             "udev tagged ID_INPUT_JOYSTICK, and a keyboard counted here would "
             "push every controller along by one")
    ok("a keyboard takes no index")


def check_hidden_pads_leave_the_enumeration() -> None:
    scenario("S15: `padmap hide` removes the physical pads from RetroArch's "
             "list, not from padmap's")
    machine = bench(players=2, hidden=True)
    order = retroarch.visible_order()

    if list(order.values()) != [machine.virtual_paths[1],
                               machine.virtual_paths[2]]:
        fail(f"RetroArch would still see {list(order.values())}; the point of "
             f"the hide rules is that only padmap's virtual pads reach it, so "
             f"a physical pad cannot claim a player slot of its own")
    ok("RetroArch sees the two virtual pads and nothing else")

    discovered = {pad.path for pad in devices.discover()}
    for node in machine.physical:
        if node not in discovered:
            fail(f"padmap can no longer see {node} either. `padmap hide` "
                 f"clears ID_INPUT_JOYSTICK, which padmap deliberately does "
                 f"not filter on -- sharing that filter would make `padmap "
                 f"setup` impossible to run again once the rules are installed")
    ok(f"padmap still discovers all {len(machine.physical)} hidden pads")

    assignments = assign_all(machine)
    cfg = override(assignments, machine.virtual_paths)
    got = indices(cfg)
    if got[1] != 0 or got[2] != 1:
        fail(f"with the physicals hidden the virtual pads are the whole "
             f"enumeration, so players 1 and 2 must be indices 0 and 1, not "
             f"{got[1]} and {got[2]}")
    ok("players 1 and 2 -> indices 0 and 1")


def check_unhidden_pads_shift_every_index() -> None:
    scenario("S15: with the physical pads visible, every predicted index moves")
    machine = bench(players=2, hidden=False)
    assignments = assign_all(machine)
    got = indices(override(assignments, machine.virtual_paths))
    if got[1] != 2 or got[2] != 3:
        fail(f"two visible physical pads did not push the virtual ones to "
             f"indices 2 and 3 (got {got[1]} and {got[2]}). Counting only the "
             f"pads padmap manages is what makes player 1 pick up somebody "
             f"else's controller")
    ok("2 physical + 2 virtual -> players 1 and 2 at indices 2 and 3")

    machine = bench(players=1, hidden=True, extras=3)
    assignments = assign_all(machine)
    got = indices(override(assignments, machine.virtual_paths))
    if got[1] != 3:
        fail(f"three unmanaged joysticks ahead of the virtual pad left player "
             f"1 at index {got[1]}, not 3; a wheel plugged in after setup "
             f"would silently take the player's controller")
    ok("3 unmanaged joysticks + 1 virtual -> player 1 at index 3")


# -- S16: all sixteen slots --------------------------------------------------

def check_every_slot_is_spoken_for() -> None:
    scenario("S16: all sixteen slots are written, for every player count")
    for players, hidden in ((0, True), (1, True), (2, True), (4, True),
                            (16, True), (1, False), (2, False), (4, False),
                            (16, False)):
        machine = bench(players=players, hidden=hidden)
        assignments = assign_all(machine)
        cfg = override(assignments, machine.virtual_paths)
        got = indices(cfg)

        for key in ("reserved_device", "device_reservation_type"):
            missing = [p for p in range(1, retroarch.MAX_PLAYERS + 1)
                       if f"input_player{p}_{key}" not in cfg]
            if missing:
                fail(f"{players} players: slots {missing} have no "
                     f"input_playerN_{key}; a reservation left over from a "
                     f"session with more players holds a slot open for a "
                     f"virtual pad that no longer exists")

        order = retroarch.visible_order()
        live = {index: path for index, path in order.items()}
        managed = set(range(1, players + 1))

        # No live pad index is shared by two players.
        taken: dict[int, int] = {}
        for player in managed:
            index = got[player]
            if index in taken:
                fail(f"{players} players: slots {taken[index]} and {player} "
                     f"both use pad index {index}. RetroArch does not reject "
                     f"that -- both ports receive the pad's input, which is "
                     f"how one N64 controller became four players")
            taken[index] = player
            if live.get(index) != machine.virtual_paths[player]:
                fail(f"{players} players: player {player} was given index "
                     f"{index}, which is {live.get(index)!r} rather than its "
                     f"own pad {machine.virtual_paths[player]!r}")

        # An unmanaged slot must not quietly point at somebody's controller
        # with its core port still live. RetroArch's own N-1 default would.
        emptied = set(emptied_ports(assignments, machine.virtual_paths))
        for player in range(1, retroarch.MAX_PLAYERS + 1):
            if player in managed:
                continue
            if got[player] in taken and player not in emptied:
                fail(f"{players} players: unmanaged slot {player} points at "
                     f"pad index {got[player]}, which belongs to player "
                     f"{taken[got[player]]}, and its core port is not emptied")
            if live.get(got[player]) in machine.physical and player not in emptied:
                fail(f"{players} players: unmanaged slot {player} was given "
                     f"index {got[player]}, a physical controller, with its "
                     f"core port left live")
        ok(f"{players:>2} players, physicals {'hidden ' if hidden else 'visible'}"
           f" -> 16 slots, {len(managed)} bound, no shared live index")

    gap("_empty_indices clamps its vacant indices at MAX_PLAYERS - 1, so once "
        "RetroArch can see 16 or more pads -- 8 unhidden physical plus their "
        "8 virtual counterparts is enough -- every unmanaged slot is handed "
        "index 15, which by then is a real controller. Only --nodevice keeps "
        "those slots off a core port; the distinct *vacant* index the comment "
        "promises does not exist to be handed out")


def check_unassigned_slots_get_vacant_indices() -> None:
    scenario("S16: an unassigned slot gets a vacant index, not RetroArch's "
             "N-1 default")
    machine = bench(players=1, hidden=False, extras=3)
    assignments = assign_all(machine)
    cfg = override(assignments, machine.virtual_paths)
    got = indices(cfg)
    order = retroarch.visible_order()

    if got[1] != 4:
        fail(f"player 1's own pad is at index 4 in this enumeration, not "
             f"{got[1]}")
    for player in range(2, retroarch.MAX_PLAYERS + 1):
        if got[player] < len(order):
            fail(f"unassigned slot {player} was given index {got[player]}, "
                 f"which is {order[got[player]]!r} -- a controller nobody "
                 f"assigned to it. With the physical pads unhidden, "
                 f"RetroArch's own N-1 default is a real device")
    ok(f"slots 2..16 sit at or above {len(order)}, where no device is")

    spares = [got[p] for p in range(2, retroarch.MAX_PLAYERS + 1)]
    if any(index > retroarch.MAX_PLAYERS - 1 for index in spares):
        fail(f"a vacant index of {max(spares)} is outside RetroArch's own "
             f"0..{retroarch.MAX_PLAYERS - 1} pad range")
    repeated = sorted({index for index in spares if spares.count(index) > 1})
    for index in repeated:
        if index < len(order):
            fail(f"unassigned slots share pad index {index}, and there is a "
                 f"device at it ({order[index]!r}). Two slots on one pad is "
                 f"not rejected by RetroArch -- both ports receive its input")
    ok(f"they run {spares[0]}..{max(spares)}, and every repeat "
       f"({repeated or 'none'}) is on an index with no device behind it")

    if [got[p] for p in range(2, 6)] == [1, 2, 3, 4]:
        fail("the unassigned slots got RetroArch's N-1 default back")
    ok("no slot fell back to N-1")

    gap(f"the vacant indices are capped at {retroarch.MAX_PLAYERS - 1}, so "
        f"with {len(order)} pads visible {spares.count(max(spares))} "
        f"unassigned slots share index {max(spares)} rather than each getting "
        f"a distinct one. Harmless only while that index is vacant -- see the "
        f"note under the sixteen-slot check for when it is not")


def check_missing_virtual_pad_is_emptied() -> None:
    scenario("S16: an assignment whose virtual pad is not enumerated is "
             "emptied, not bound to whatever holds that index")
    # Two assigned players, but only player 1's uinput node ever appeared --
    # the daemon believes it republished both. Anything that quietly kept the
    # stored index would hand player 2 whatever is at that index instead.
    machine = Machine("missing-pad")
    for port in (1, 2):
        machine.add_physical(port, hidden=True)
    machine.add_virtual(1)
    machine.install()
    found = pads_by_path()
    assignments = [Assignment(player=1, pad=found[machine.physical[0]], button=0),
                   Assignment(player=2, pad=found[machine.physical[1]], button=0)]
    paths = {1: machine.virtual_paths[1], 2: "/dev/input/event799"}

    cfg = override(assignments, paths)
    got = indices(cfg)
    order = retroarch.visible_order()

    if got[1] != 0:
        fail(f"player 1 moved to index {got[1]} because player 2's pad "
             f"vanished")
    if got[2] < len(order):
        fail(f"player 2 was given index {got[2]}, which is "
             f"{order.get(got[2])!r} -- the pad padmap could not find is now "
             f"whatever else was at that index")
    if cfg["input_player2_reserved_device"] != "":
        fail(f"player 2 keeps a reservation for "
             f"{cfg['input_player2_reserved_device']!r}, a pad that is not in "
             f"the enumeration; that holds the slot open for nothing")
    if cfg["input_player2_device_reservation_type"] != str(
            retroarch.RESERVATION_NONE):
        fail("player 2's reservation type is still RESERVED")
    if 2 not in emptied_ports(assignments, paths):
        fail("player 2's core port was left live for a pad RetroArch cannot "
             "see, so the core reports a player who cannot press anything")
    ok(f"player 2 -> vacant index {got[2]}, reservation cleared, port emptied")

    if any(key.startswith("input_player2_") and key.endswith(("_btn", "_axis"))
           for key in cfg):
        fail("padmap rewrote the user's binds for a slot it does not manage")
    ok("and its binds are left alone, since padmap has no pad there")


# -- S16: the core ports -----------------------------------------------------

def check_core_ports() -> None:
    scenario("S16: --nodevice empties exactly the unassigned core ports")
    for players in (0, 1, 2, 4, 16):
        machine = bench(players=players, hidden=True)
        assignments = assign_all(machine)
        got = emptied_ports(assignments, machine.virtual_paths)
        want = list(range(players + 1, retroarch.MAX_PLAYERS + 1))
        if got != want:
            fail(f"{players} assigned players emptied ports {got}, not {want}. "
                 f"A core that declares four ports hands the unemptied ones a "
                 f"controller nobody assigned")
        ok(f"{players:>2} players -> {len(got)} port(s) emptied "
           f"{got[:3]}{'...' if len(got) > 3 else ''}")

    scenario("S16: and the flags follow which slots are taken, not how many")
    machine = bench(players=3, hidden=True)
    found = pads_by_path()
    assignments = [
        Assignment(player=player, pad=found[machine.physical[i]], button=0)
        for i, player in enumerate((1, 3, 4))
    ]
    paths = {1: machine.virtual_paths[1], 3: machine.virtual_paths[2],
             4: machine.virtual_paths[3]}
    got = emptied_ports(assignments, paths)
    if got != [2] + list(range(5, 17)):
        fail(f"players 1, 3 and 4 assigned emptied ports {got}; port 2 is the "
             f"only gap and ports 1, 3 and 4 must stay live")
    ok("players 1, 3, 4 -> port 2 emptied, theirs kept")

    scenario("S16: input_libretro_device_pN is not the mechanism")
    machine = bench(players=1, hidden=True)
    assignments = assign_all(machine)
    cfg = override(assignments, machine.virtual_paths)
    stray = [key for key in cfg if key.startswith("input_libretro_device")]
    if stray:
        fail(f"{stray} is in the launch override. RetroArch reads that key "
             f"only from .rmp remap files -- configuration.c touches it in "
             f"input_remapping_load_file/_save_file and nowhere else -- so a "
             f"port emptied this way is not emptied at all")
    ok("absent from the override; the command line does the job")

    scenario("S16: the override and the flags agree about who is managed")
    machine = bench(players=2, hidden=False)
    assignments = assign_all(machine)
    cfg = override(assignments, machine.virtual_paths)
    emptied = set(emptied_ports(assignments, machine.virtual_paths))
    reserved = {p for p in range(1, retroarch.MAX_PLAYERS + 1)
                if cfg[f"input_player{p}_reserved_device"]}
    if emptied & reserved:
        fail(f"slots {sorted(emptied & reserved)} have a reserved pad and an "
             f"emptied core port; the pad is bound to a player the core will "
             f"never ask about")
    if emptied | reserved != set(range(1, retroarch.MAX_PLAYERS + 1)):
        fail("some slot is neither reserved nor emptied, so it keeps whatever "
             "retroarch.cfg happens to hold")
    ok(f"reserved {sorted(reserved)}, emptied {len(emptied)}, no overlap, "
       f"no gaps")


def check_args_file() -> None:
    scenario("S16: the wrapper reads the flags back one token per line")
    machine = bench(players=2, hidden=True)
    assignments = assign_all(machine)
    target = _SANDBOX / "launch.args"
    retroarch.write_launch_args(assignments, machine.virtual_paths, target)
    raw = target.read_text()
    tokens = raw.split("\n")
    if tokens[-1] != "":
        fail("the args file does not end in a newline, so padmap-play joins "
             "the last token to whatever it reads next")
    if tokens[:-1] != retroarch.launch_args(assignments, machine.virtual_paths):
        fail(f"the file holds {tokens[:-1]}, not what launch_args produced")
    if any(" " in token for token in tokens[:-1]):
        fail("a token contains a space; the wrapper reads whole lines, so a "
             "split token would reach RetroArch as one argument")
    ok(f"{len(tokens) - 1} tokens, {len(tokens) - 1} lines, newline-terminated")

    machine = bench(players=16, hidden=True)
    assignments = assign_all(machine)
    retroarch.write_launch_args(assignments, machine.virtual_paths, target)
    if target.read_text() != "":
        fail(f"with all sixteen slots assigned the args file should be empty, "
             f"not {target.read_text()!r} -- every core port has a player")
    ok("16 players -> an empty args file, no port emptied")


# -- S17: two players --------------------------------------------------------

def check_two_players_are_two_players() -> None:
    scenario("S17: two ports of one adapter, identical upstream in every field")
    machine = bench(players=2, hidden=True, adapter=True)
    found = pads_by_path()
    one, two = (found[machine.physical[0]], found[machine.physical[1]])

    if (one.name, one.vid, one.pid, one.phys) != (two.name, two.vid, two.pid,
                                                  two.phys):
        fail("the fixture's two adapter ports are distinguishable, so this "
             "check is not exercising the case it exists for")
    if profiles.signature(one) != profiles.signature(two):
        fail("two ports of one adapter got different profile signatures; "
             "identical controllers are meant to share a stored mapping")
    ok("same name, vendor, product and phys -- and one shared profile")

    store(one, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})
    assignments = [Assignment(player=1, pad=one, button=0),
                   Assignment(player=2, pad=two, button=0)]
    cfg = override(assignments, machine.virtual_paths)

    names = {p: cfg[f"input_player{p}_reserved_device"] for p in (1, 2)}
    if names[1] == names[2]:
        fail(f"both players are reserved for {names[1]!r}; RetroArch's "
             f"reservation matcher compares device names exactly, so two "
             f"slots naming one pad is not a two-player setup")
    if names != {1: "padmap Player 1", 2: "padmap Player 2"}:
        fail(f"the reservations are {names}, not each player's own pad")
    if indices(cfg)[1] == indices(cfg)[2]:
        fail("both players were given the same pad index")
    ok(f"distinct reservations {list(names.values())} and distinct indices")

    guids = {p: controllercfg.virtual_guid(p, pad)
             for p, pad in ((1, one), (2, two))}
    if guids[1] == guids[2]:
        fail(f"both virtual pads compute the SDL GUID {guids[1]}; the ids are "
             f"identical, so the name checksum is the only field that differs "
             f"-- one mapping line would then match both players")
    if guids[1][:4] != guids[2][:4] or guids[1][8:] != guids[2][8:]:
        fail(f"the GUIDs differ somewhere other than the name checksum "
             f"({guids[1]} vs {guids[2]}), which means the identity padmap "
             f"advertises is not what it thinks it is")
    ok(f"GUIDs differ only in the name CRC: {guids[1][4:8]} vs {guids[2][4:8]}")

    dest = Path(tempfile.mkdtemp(dir=_SANDBOX))
    retroarch.install_profiles(assignments, dest=dest)
    written = sorted(p.name for p in dest.glob("*.cfg"))
    if written != ["padmap Player 1.cfg", "padmap Player 2.cfg"]:
        fail(f"the autoconfig directory holds {written}; each player needs a "
             f"profile of its own, matched by device name")
    first, second = settings(profile_text(1, dest)), settings(profile_text(2, dest))
    if first["input_device"] == second["input_device"]:
        fail("both profiles claim the same device name, so RetroArch would "
             "apply one of them to both pads")
    if first.get("input_a_btn") != second.get("input_a_btn"):
        fail("the two ports of one adapter got different bindings; a mapping "
             "follows the controller model, and these are the same model")
    ok("two profiles, one shared mapping, two identities")


def check_two_players_keep_their_own_scopes() -> None:
    scenario("S14/S17: one player on an N64 mapping, the other on its default, "
             "in the same launch")
    machine = bench(players=2, hidden=True)
    found = pads_by_path()
    mapped, plain = (found[machine.physical[0]], found[machine.physical[1]])
    store(mapped, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT,
                   profiles.console_scope("n64"): GC_FOR_N64})
    store(plain, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})

    dest = Path(tempfile.mkdtemp(dir=_SANDBOX))
    retroarch.install_profiles(
        [Assignment(player=1, pad=mapped, button=0),
         Assignment(player=2, pad=plain, button=0)],
        dest=dest, console="n64", game="n64/goldeneye-007",
        context="GoldenEye 007")

    one, two = settings(profile_text(1, dest)), settings(profile_text(2, dest))
    if "input_a_btn" in one:
        fail("player 1 was given the GameCube capture for an N64 launch; "
             "nothing on that console reads RetroPad A, so input_a_btn is a "
             "button that does nothing")
    if one.get("input_y_btn") != "1":
        fail(f"player 1's N64 B (which mupen64plus-next reads from RetroPad Y) "
             f"is {one.get('input_y_btn')!r}")
    if two.get("input_a_btn") != "0" or two.get("input_x_btn") != "2":
        fail(f"player 2 has no N64 mapping and must fall back to its default "
             f"(input_a_btn=0, input_x_btn=2), got "
             f"{two.get('input_a_btn')!r}/{two.get('input_x_btn')!r}")
    if "console:n64" not in profile_text(1, dest):
        fail("player 1's profile does not record the scope it came from")
    if "default (any game)" not in profile_text(2, dest):
        fail("player 2's profile does not say it fell back to the default")
    ok("player 1 -> console:n64, player 2 -> its default, one launch")

    scenario("S14: game beats console beats default, for one pad")
    store(mapped, {
        profiles.SCOPE_UNIVERSAL: GC_DEFAULT,
        profiles.console_scope("n64"): GC_FOR_N64,
        profiles.game_scope("n64/super-mario-64"): GC_FOR_THIS_GAME,
    })
    cases = [
        (("n64", "n64/super-mario-64"), "game:n64/super-mario-64",
         "input_b_btn", "5"),
        (("n64", "n64/goldeneye-007"), "console:n64", "input_y_btn", "1"),
        (("n64", ""), "console:n64", "input_y_btn", "1"),
        (("snes", "snes/super-metroid"), "default (any game)",
         "input_a_btn", "0"),
        (("", ""), "default (any game)", "input_a_btn", "0"),
    ]
    for (console, game), expected, key, value in cases:
        retroarch.install_profiles([Assignment(player=1, pad=mapped, button=0)],
                                   dest=dest, console=console, game=game)
        text = profile_text(1, dest)
        if f"# Mapping scope: {expected}" not in text:
            got = [l for l in text.splitlines() if "Mapping scope" in l]
            fail(f"console={console!r} game={game!r} resolved to {got}, "
                 f"expected {expected!r}")
        if settings(text).get(key) != value:
            fail(f"console={console!r} game={game!r} produced "
                 f"{key}={settings(text).get(key)!r}, not {value!r}; the "
                 f"header and the bindings disagree about which scope won")
        print(f"  ok  console={console or '-'!r:<7} game={game or '-'!r:<20} "
              f"-> {expected}")


# -- S14: the launcher itself ------------------------------------------------

def state_file(assignments: list[Assignment], path: Path | None = None) -> Path:
    """assignments.json exactly as the daemon writes it."""
    target = path or (protocol.runtime_dir() / "assignments.json")
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps([
        {"player": a.player, "path": a.pad.path, "name": a.pad.name,
         "phys": a.pad.phys, "vid": a.pad.vid, "pid": a.pad.pid}
        for a in assignments
    ], indent=2))
    return target


def run_launch(argv: list[str]) -> tuple[int, str]:
    noise = io.StringIO()
    with redirect_stderr(noise):
        code = launch.main(argv)
    return code, noise.getvalue()


def check_launcher_resolves_the_running_game() -> None:
    scenario("S14: the launcher resolves the mapping from the real command line")
    machine = bench(players=2, hidden=True)
    found = pads_by_path()
    mapped, plain = (found[machine.physical[0]], found[machine.physical[1]])

    roms = _SANDBOX / "roms" / "n64"
    roms.mkdir(parents=True, exist_ok=True)
    rom = roms / "GoldenEye 007 (U) [!].z64"
    rom.write_text("not really a rom")
    other = roms / "Super Mario 64 (U) [!].z64"
    other.write_text("not really a rom either")
    key = profiles.game_key("n64", str(rom))

    store(mapped, {
        profiles.SCOPE_UNIVERSAL: GC_DEFAULT,
        profiles.console_scope("n64"): GC_FOR_N64,
        profiles.game_scope(key): GC_FOR_THIS_GAME,
    })
    store(plain, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})
    assignments = [Assignment(player=1, pad=mapped, button=0),
                   Assignment(player=2, pad=plain, button=0)]
    state_file(assignments)

    code, noise = run_launch([
        "--", "--appendconfig", "/nonexistent/launch.cfg",
        "--nodevice", "3", "-L", CORE, str(rom)])
    if code != 0:
        fail(f"padmap-play's resolver returned {code}; a launch must never "
             f"fail over a mapping it could not narrow")
    one = profile_text(1)
    if f"# Mapping scope: game:{key}" not in one:
        fail(f"the launcher did not resolve the per-game scope for "
             f"{rom.name} (core {Path(CORE).name}); the profile says "
             f"{[l for l in one.splitlines() if 'Mapping scope' in l]}. The "
             f"ROM path is the only thing that identifies the game, and this "
             f"is the one place it is known")
    if settings(one).get("input_b_btn") != "5":
        fail("the per-game capture's own binding never reached the file "
             "RetroArch reads")
    if "input_a_btn = \"0\"" not in profile_text(2):
        fail("player 2's default mapping was not written")
    ok(f"player 1 on game:{key}, player 2 on its default")

    code, _ = run_launch(["--", "-L", CORE, str(other)])
    if code != 0:
        fail(f"the resolver returned {code} for {other.name}")
    one = profile_text(1)
    if "# Mapping scope: console:n64" not in one:
        fail(f"a different N64 game did not fall back to the console mapping; "
             f"the profile says "
             f"{[l for l in one.splitlines() if 'Mapping scope' in l]}")
    if settings(one).get("input_y_btn") != "1":
        fail("the console capture's bindings are missing")
    ok(f"another N64 game -> console:n64 ({other.name})")

    if "--appendconfig" in noise or "/nonexistent" in noise:
        fail("the launcher mistook a flag's value for the ROM")
    run_launch(["--", "--appendconfig", "/nonexistent/launch.cfg",
                "--nodevice", "3", "-L", CORE, str(rom)])
    recorded = protocol.read_last_game()
    if recorded.get("key") != key or recorded.get("console") != "n64":
        fail(f"the launch was recorded as {recorded!r}, so the scope picker "
             f"could not offer 'for the game you just played' -- and the "
             f"daemon would regenerate these profiles with no context at all")
    if recorded.get("title") != "GoldenEye 007 (U) [!]":
        fail(f"the recorded title is {recorded.get('title')!r}")
    ok(f"recorded as {key!r} for the scope picker")

    scenario("S14: an unrecognised core still resolves and still records")
    code, _ = run_launch(["--", "-L", UNKNOWN_CORE, str(rom)])
    if code != 0:
        fail(f"an unknown core made the launcher return {code}")
    if "default (any game)" not in profile_text(1):
        fail("with no console known, player 1 must fall back to its default "
             "mapping rather than to some other console's capture")
    recorded = protocol.read_last_game()
    if recorded.get("key") != profiles.game_key("", str(rom)):
        fail(f"the game was not recorded under the unknown console "
             f"({recorded!r}); a launch nobody recognised is exactly the one "
             f"whose controls are most likely to have felt wrong")
    ok(f"console unknown -> default mapping, recorded as {recorded['key']!r}")


def check_launch_never_takes_the_game_down() -> None:
    scenario("S14: nothing about a mapping can stop a game starting")
    machine = bench(players=1, hidden=True)
    assignments = assign_all(machine)
    store(assignments[0].pad, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})
    state = protocol.runtime_dir() / "assignments.json"
    state_file(assignments)

    rom = _SANDBOX / "roms" / "n64" / "GoldenEye 007 (U) [!].z64"
    good = ["--", "-L", CORE, str(rom)]

    cases: list[tuple[str, list[str]]] = [
        ("no arguments at all", ["--"]),
        ("a ROM that does not exist", ["--", "-L", CORE, "/no/such/rom.z64"]),
        ("-L with no value", ["--", "-L"]),
        ("a bare flag salad", ["--", "--fullscreen", "-v", "--", "-L", CORE]),
        ("a ROM that is a directory", ["--", "-L", CORE, str(rom.parent)]),
    ]
    for label, argv in cases:
        code, _ = run_launch(argv)
        if code != 0:
            fail(f"{label}: the resolver returned {code}, which is a game that "
                 f"refuses to start because a mapping could not be narrowed")
        ok(f"{label} -> 0")

    broken = [
        ("not JSON at all", "{ not json"),
        ("valid JSON that is not a list", '{"player": 1}'),
        ("a list of strings", '["player 1"]'),
        ("an entry with no player", '[{"path": "/dev/input/event921"}]'),
        ("a player that is not a number",
         '[{"player": "two", "path": "/dev/input/event921"}]'),
        ("a null entry", "[null]"),
    ]
    for label, text in broken:
        state.write_text(text)
        code, noise = run_launch(good)
        if code != 0:
            fail(f"{label} in assignments.json made the launcher return {code}")
        found = launch.load_assignments(state)
        if found:
            fail(f"{label} produced {found}, which is a player padmap invented "
                 f"out of a damaged file")
        if "no assigned controllers" not in noise:
            fail(f"{label}: the launcher did not report that it had nothing to "
                 f"resolve for ({noise.strip()!r})")
        ok(f"assignments.json {label} -> no assignments, exit 0")

    state.write_text(json.dumps(
        [{"player": 1, "path": "/dev/input/event799", "name": "gone"},
         {"player": "2", "path": assignments[0].pad.path, "name": "here"}]))
    found = launch.load_assignments(state)
    if len(found) != 1 or found[0].player != 2:
        fail(f"an entry whose pad has been unplugged should be skipped and a "
             f"numeric string player accepted, got {found}")
    ok("a pad that has gone is skipped; the rest of the file still loads")


def check_out_of_range_players() -> None:
    scenario("S16: a player number outside 1..16 in the state file")
    machine = bench(players=1, hidden=True)
    assignments = assign_all(machine)
    state = _SANDBOX / "out-of-range.json"
    pad_path = assignments[0].pad.path
    crashed: list[int] = []

    for player in (0, -1, 17, 99):
        state.write_text(json.dumps(
            [{"player": player, "path": pad_path, "name": "x"}]))
        found = launch.load_assignments(state)
        if [a.player for a in found] != [player]:
            fail(f"load_assignments turned player {player} into "
                 f"{[a.player for a in found]}; whatever it does with an "
                 f"out-of-range slot, it has to be the same answer every time")
        paths = {player: machine.virtual_paths[1]}
        # The flags are computable and correct: no slot in 1..16 is claimed by
        # this assignment, so every core port is emptied.
        ports = emptied_ports(found, paths)
        if ports != list(range(1, retroarch.MAX_PLAYERS + 1)):
            fail(f"player {player} left core ports {ports} live; nothing in "
                 f"1..16 is bound to it, so every port must be emptied")
        try:
            cfg = override(found, paths)
        except StopIteration:
            crashed.append(player)
            ok(f"player {player:>3} -> loaded verbatim, all core ports emptied")
            continue
        got = indices(cfg)
        if len(got) != retroarch.MAX_PLAYERS:
            fail(f"player {player}: {len(got)} slots written, not 16")
        ok(f"player {player:>3} -> loaded verbatim, 16 slots written, all "
           f"core ports emptied")

    if crashed:
        gap(f"but launch_config raises StopIteration for player(s) {crashed}: "
            f"_empty_indices is asked for MAX_PLAYERS - len(managed) vacant "
            f"indices, while the loop needs one for every unmanaged slot in "
            f"1..16 -- and an assignment outside that range is counted in "
            f"`managed` without ever consuming one. Neither "
            f"launch.load_assignments nor Server.restore bounds the player "
            f"number, and Server._start_republisher writes this file during "
            f"restore, outside any try/except")


# -- error paths that reach the generated files ------------------------------

def check_profile_lines_are_only_settings() -> None:
    scenario("S14: the profile RetroArch parses holds nothing but comments "
             "and settings")
    machine = bench(players=1, hidden=True)
    assignments = assign_all(machine)
    store(assignments[0].pad, {profiles.SCOPE_UNIVERSAL: GC_DEFAULT})
    state_file(assignments)

    roms = _SANDBOX / "roms" / "n64"
    roms.mkdir(parents=True, exist_ok=True)

    def stray_lines(text: str) -> list[str]:
        out = []
        for line in text.splitlines():
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            if re.fullmatch(r'[A-Za-z0-9_]+ = "[^"\n]*"', stripped):
                continue
            out.append(line)
        return out

    ordinary = roms / "Legend of Zelda, The - Ocarina of Time (U) [!].z64"
    ordinary.write_text("x")
    run_launch(["--", "-L", CORE, str(ordinary)])
    text = profile_text(1)
    if stray_lines(text):
        fail(f"an ordinary ROM name produced {stray_lines(text)} in the "
             f"autoconfig profile; RetroArch parses every line of that file")
    if settings(text).get("input_a_btn") != "0":
        fail("the captured binding is missing from the profile")
    ok(f"{ordinary.name!r} -> comments and settings only")

    # A filename nobody would type on purpose, and one a bad extractor can
    # produce: the title is interpolated into the profile's header comment.
    odd = roms / 'Mario\ninput_analog_sensitivity = "0.1"\n(U).z64'
    odd.write_text("x")
    run_launch(["--", "-L", CORE, str(odd)])
    text = profile_text(1)
    if settings(text).get("input_a_btn") != "0":
        fail("a strangely named ROM cost the controller its captured mapping, "
             "which is the part that must survive whatever the file is called")
    ok("a ROM name containing a newline still gets its captured bindings")
    if stray_lines(text):
        gap(f"...but the header comment is interpolated without stripping "
            f"newlines, so that name adds {len(stray_lines(text))} "
            f"uncommented line(s) to the file RetroArch parses: "
            f"{stray_lines(text)[0]!r}")


def check_two_enumerations() -> None:
    scenario("S16: the override and the flags are two separate enumerations")
    machine = bench(players=2, hidden=True)
    assignments = assign_all(machine)

    # Both artefacts are written back to back by the daemon, each calling
    # visible_order() for itself. Simulated here: player 2's pad appears --
    # or disappears -- between the two calls.
    real = retroarch.visible_order
    partial = {0: machine.virtual_paths[1]}

    def flapping(sequence: list[dict[int, str]]):  # noqa: ANN202
        remaining = list(sequence)

        def order() -> dict[int, str]:
            return remaining.pop(0) if remaining else real()
        return order

    for label, sequence in (
        ("a pad appearing between the two writes", [partial]),
        ("a pad disappearing between them", [real(), partial]),
    ):
        retroarch.visible_order = flapping(sequence)  # type: ignore[assignment]
        try:
            cfg = override(assignments, machine.virtual_paths)
            emptied = set(emptied_ports(assignments, machine.virtual_paths))
        finally:
            retroarch.visible_order = real  # type: ignore[assignment]
        reserved = {p for p in range(1, retroarch.MAX_PLAYERS + 1)
                    if cfg[f"input_player{p}_reserved_device"]}
        both = sorted(emptied & reserved)
        neither = sorted(set(range(1, retroarch.MAX_PLAYERS + 1))
                         - emptied - reserved)
        if both:
            gap(f"{label}: slot(s) {both} end up reserved for a pad whose core "
                f"port is emptied -- the pad is bound to a player the core "
                f"never asks about")
        elif neither:
            gap(f"{label}: slot(s) {neither} are neither reserved nor emptied, "
                f"so that core port keeps whatever retroarch.cfg holds while "
                f"launch.cfg says padmap is not managing it")
        else:
            ok(f"{label}: the two artefacts still agree")

    # What is unambiguous either way: a launch computed from ONE enumeration
    # never disagrees with itself.
    order = retroarch.visible_order()
    managed = retroarch.managed_players(assignments, machine.virtual_paths, order)
    flags = retroarch.launch_args(assignments, machine.virtual_paths, order)
    ports = {int(flags[i + 1]) for i in range(0, len(flags), 2)}
    if ports & set(managed):
        fail(f"from a single enumeration, slots {sorted(ports & set(managed))} "
             f"are both managed and emptied")
    if ports | set(managed) != set(range(1, retroarch.MAX_PLAYERS + 1)):
        fail("from a single enumeration, some slot is neither managed nor "
             "emptied")
    ok("passing one enumeration to both makes them agree by construction")
    gap("launch_args takes an `order` argument so a caller can reuse the "
        "enumeration it already has, and write_launch_args -- the only caller "
        "that writes the file the wrapper reads -- does not pass one")


def main() -> int:
    check_enumeration_is_syspath_order()
    check_hidden_pads_leave_the_enumeration()
    check_unhidden_pads_shift_every_index()
    check_every_slot_is_spoken_for()
    check_unassigned_slots_get_vacant_indices()
    check_missing_virtual_pad_is_emptied()
    check_core_ports()
    check_args_file()
    check_two_players_are_two_players()
    check_two_players_keep_their_own_scopes()
    check_launcher_resolves_the_running_game()
    check_launch_never_takes_the_game_down()
    check_out_of_range_players()
    check_profile_lines_are_only_settings()
    check_two_enumerations()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
