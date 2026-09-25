"""Watch a controller and its clone at the same time, live.

Built because too much of this was guessed. When a pad "does not work in game"
there are four separate places it can die, and no amount of reading code says
which:

    physical pad  --grab-->  danstick daemon  --write-->  virtual pad  --> RetroArch

This shows the two ends side by side. Press a button and you see whether the
physical device emitted it, whether the clone emitted it, and which RetroArch
bind that button resolves to in the profile danstick actually wrote. If the left
column moves and the right does not, the daemon is not forwarding. If both move
and the game still does nothing, the fault is downstream of danstick.

    nix develop --command python3 tools/padmon.py

    --physical   also try to open the physical pad. The daemon holds
                 EVIOCGRAB on it, so this usually shows nothing -- which is
                 itself the expected, correct state.
"""

from __future__ import annotations

import selectors
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(Path(__file__).resolve().parent))

# Line-buffered: piping this into `tee` block-buffers stdout, and a
# Ctrl-C then discards everything the run had printed.
sys.stdout.reconfigure(line_buffering=True)

import evdev  # noqa: E402
from evdev import ecodes  # noqa: E402

import _danstick as protocol  # noqa: E402

VIRTUAL_PREFIX = "danstick Player"


def code_name(kind: int, code: int) -> str:
    table = {ecodes.EV_KEY: ecodes.KEY, ecodes.EV_ABS: ecodes.ABS}.get(kind)
    if not table:
        return f"code {code}"
    name = table.get(code, f"code {code}")
    return name[0] if isinstance(name, (list, tuple)) else name


def load_profile() -> dict[str, str]:
    """button/axis -> RetroArch key, from the profile danstick last wrote.

    Read rather than assumed: this is the file RetroArch is actually matching,
    and a mapping that looks right in the store can still be absent here.
    """
    directory = protocol.runtime_dir() / "autoconfig" / "udev"
    out: dict[str, str] = {}
    for path in sorted(directory.glob("*.cfg")):
        for line in path.read_text(errors="replace").splitlines():
            if " = " not in line or line.startswith("#"):
                continue
            key, value = line.split(" = ", 1)
            out.setdefault(value.strip().strip('"'), key.strip())
    return out


def retro_bind(profile: dict[str, str], kind: int, code: int,
               index_of: dict[int, int]) -> str:
    """What RetroArch would call this event, per the written profile."""
    if kind == ecodes.EV_KEY:
        index = index_of.get(code)
        if index is None:
            return "not a numbered button"
        return profile.get(str(index), f"button {index}: unbound")
    if kind == ecodes.EV_ABS:
        return "(axis -- see +N/-N binds)"
    return ""


def button_indices(device: evdev.InputDevice) -> dict[int, int]:
    """evdev code -> the index RetroArch numbers it as (contiguous from 0)."""
    caps = device.capabilities().get(ecodes.EV_KEY, [])
    return {code: i for i, code in enumerate(sorted(caps))}


def find(names_contain: str) -> list[evdev.InputDevice]:
    found = []
    for path in evdev.list_devices():
        try:
            device = evdev.InputDevice(path)
        except OSError:
            continue
        if names_contain in device.name:
            found.append(device)
        else:
            device.close()
    return found


def daemon_state() -> str:
    """Whether a daemon is running, and what it last said it was doing."""
    pids = protocol.daemon_pids()
    if not pids:
        return "NOT RUNNING -- nothing is republishing anything"
    log = protocol.runtime_dir() / "danstick.log"
    last = ""
    try:
        lines = [ln for ln in log.read_text(errors="replace").splitlines()
                 if ln.strip()]
        last = lines[-1] if lines else ""
    except OSError:
        pass
    return f"pid {pids[0]}; last log line: {last!r}"


def main() -> int:
    want_physical = "--physical" in sys.argv

    virtual = find(VIRTUAL_PREFIX)
    if not virtual:
        print("No 'danstick Player N' device exists at all.")
        print("The daemon is not republishing; nothing downstream can work.")
        return 1

    profile = load_profile()
    print("watching:")
    watched: list[evdev.InputDevice] = []
    for device in virtual:
        print(f"  CLONE     {device.name!r}  {device.path}")
        watched.append(device)

    if want_physical:
        for device in evdev.list_devices():
            try:
                candidate = evdev.InputDevice(device)
            except OSError:
                continue
            if VIRTUAL_PREFIX in candidate.name or not candidate.capabilities().get(ecodes.EV_KEY):
                candidate.close()
                continue
            print(f"  PHYSICAL  {candidate.name!r}  {candidate.path}")
            watched.append(candidate)

    print()
    print(f"profile binds loaded: {len(profile)} "
          f"(from {protocol.runtime_dir()}/autoconfig/udev)")
    print(f"daemon: {daemon_state()}")
    print()
    print("Press buttons on the controller. Ctrl-C to stop.")
    print("If nothing appears here, the clone is emitting nothing -- the "
          "daemon is not forwarding.")
    print("-" * 78)

    indices = {d.path: button_indices(d) for d in watched}
    selector = selectors.DefaultSelector()
    for device in watched:
        selector.register(device, selectors.EVENT_READ)

    seen = 0
    started = time.monotonic()
    try:
        while True:
            for key, _mask in selector.select(timeout=1.0):
                device = key.fileobj
                try:
                    events = list(device.read())
                except OSError:
                    continue
                for event in events:
                    if event.type not in (ecodes.EV_KEY, ecodes.EV_ABS):
                        continue
                    if event.type == ecodes.EV_ABS and event.value in (0,):
                        pass
                    seen += 1
                    tag = ("CLONE   " if VIRTUAL_PREFIX in device.name
                           else "PHYSICAL")
                    bind = retro_bind(profile, event.type, event.code,
                                      indices[device.path])
                    print(f"{time.monotonic() - started:7.2f}  {tag}  "
                          f"{code_name(event.type, event.code):<16} "
                          f"value={event.value:<7} {bind}")
    except KeyboardInterrupt:
        print("-" * 78)
        print(f"{seen} event(s) seen.")
        if seen == 0:
            print()
            print("NOTHING came through. In order of likelihood:")
            print("  * the daemon is not forwarding (republisher paused, or "
                  "the source was dropped)")
            print("  * the pad is not assigned to a player")
            print("  * the physical pad disconnected")
            print("Check the daemon log: "
                  f"{protocol.runtime_dir()}/danstick.log")
    return 0


if __name__ == "__main__":
    sys.exit(main())
