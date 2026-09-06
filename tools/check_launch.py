"""Exercise launch_config over the shapes that produced the bugs.

Every player slot is written, not only the assigned ones; unassigned ones get
a vacant pad index and a cleared reservation; and RetroArch is pointed at
padmap's own autoconfig directory rather than libretro's database.

    python3 tools/check_launch.py
"""
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import retroarch
from padmap.assign import Assignment
from padmap.devices import Pad


def pad(path):
    return Pad(path=path, name="x", phys="", uniq="", vid=1, pid=2,
               syspath="", retroarch_visible=True)


def parse(text):
    out = {}
    for line in text.splitlines():
        m = re.match(r'^([a-z0-9_]+)\s*=\s*"(.*)"$', line)
        if m:
            assert m.group(1) not in out, f"duplicate key {m.group(1)}"
            out[m.group(1)] = m.group(2)
    return out


def scenario(name, n_assigned, n_visible):
    """n_visible virtual pads enumerated at indices 0..n_visible-1."""
    paths = {p: f"/dev/input/event{100 + p}" for p in range(1, n_assigned + 1)}
    order = {i: f"/dev/input/event{101 + i}" for i in range(n_visible)}
    retroarch.visible_order = lambda: order
    assignments = [
        Assignment(player=p, pad=pad(paths[p]), button=0)
        for p in range(1, n_assigned + 1)
    ]
    cfg = parse(retroarch.launch_config(assignments, paths))

    idx = {p: cfg[f"input_player{p}_joypad_index"]
           for p in range(1, retroarch.MAX_PLAYERS + 1)}
    res = {p: cfg[f"input_player{p}_reserved_device"]
           for p in range(1, retroarch.MAX_PLAYERS + 1)}

    # 1. every slot is written
    assert len(idx) == 16 and len(res) == 16

    # 2. no live pad index is shared by two players
    live = [int(v) for v in idx.values() if int(v) < n_visible]
    assert len(live) == len(set(live)), f"{name}: shared index {live}"

    # 3. players with a pad are exactly the assigned ones
    got = sorted(p for p, v in idx.items() if int(v) < n_visible)
    assert got == list(range(1, n_assigned + 1)), f"{name}: bound {got}"

    # 4. core ports are emptied on the command line, not in the config
    assert not any(k.startswith("input_libretro_device") for k in cfg), name
    flags = retroarch.launch_args(assignments, paths)
    ports = [int(flags[i + 1]) for i in range(0, len(flags), 2)]
    assert all(flags[i] == "--nodevice" for i in range(0, len(flags), 2)), name
    assert ports == list(range(n_assigned + 1, 17)), f"{name}: {ports}"

    # 5. reservations cleared outside the assigned range
    assert all(res[p] == "" for p in range(n_assigned + 1, 17)), name
    assert all(res[p] == f"padmap Player {p}"
               for p in range(1, n_assigned + 1)), name

    assert cfg["config_save_on_exit"] == "false", name

    # RetroArch scans exactly one autoconfig directory, and by default that is
    # libretro's database -- so padmap's own profiles were never read, and the
    # database could outscore a mapping the user recorded.
    # A per-player bind in retroarch.cfg beats the autoconfig profile, so an
    # assigned player must have them all cleared or the captured mapping is
    # silently ignored while the pad still reports as configured.
    for bind in ("start", "a", "b", "select"):
        for form in ("btn", "axis"):
            key = f"input_player1_{bind}_{form}"
            assert cfg.get(key) == "nul", \
                f"{name}: {key} is {cfg.get(key)!r}, which would outrank " \
                f"the autoconfig profile"

    assert "joypad_autoconfig_dir" in cfg, \
        f"{name}: nothing points RetroArch at padmap's profiles"
    assert cfg["joypad_autoconfig_dir"].endswith("padmap/autoconfig"), \
        f"{name}: autoconfig dir is {cfg['joypad_autoconfig_dir']!r}"
    print(f"  ok  {name}: idx1-4={[idx[p] for p in (1,2,3,4)]} "
          f"nodevice={ports[:4]}{'...' if len(ports) > 4 else ''}")


print("launch_config:")
scenario("1 player, 1 virtual pad (the reported case)", 1, 1)
scenario("2 players, 2 virtual pads", 2, 2)
scenario("4 players, 4 virtual pads", 4, 4)
scenario("1 player, physicals NOT hidden (10 visible pads)", 1, 10)

# The dropped-assignment path: assignment exists but its pad is not enumerated.
retroarch.visible_order = lambda: {}
cfg = parse(retroarch.launch_config(
    [Assignment(player=1, pad=pad("/dev/input/event101"), button=0)],
    {1: "/dev/input/event101"},
))
assert cfg["input_player1_reserved_device"] == ""
assert retroarch.launch_args(
    [Assignment(player=1, pad=pad("/dev/input/event101"), button=0)],
    {1: "/dev/input/event101"},
)[:2] == ["--nodevice", "1"]
print("  ok  assignment with no enumerated pad -> slot emptied, not skipped")

print("all checks passed")
