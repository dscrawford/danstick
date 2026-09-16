#!/usr/bin/env python3
"""The daemon writes Cemu's, ares' and Ryujinx's config files, or says why not.

padmap is an abstraction layer: a program that cannot see a controller is
padmap's problem, not the user's. Three emulators cannot be reached through the
SDL database padmap already writes --

  * Cemu reads no mapping database at all, so a pad SDL does not already
    recognise never appears in its device list;
  * ares binds raw SDL joystick indices, so a wrong index does not fail, it
    binds a different button;
  * Ryujinx blanks the name checksum out of the GUID it uses as a device id.

-- so each gets a file of its own, written by `padmap_input::emulators` and
driven from the daemon through `padmap.emulators`.

What is asserted here:

  THE WIRE FORMAT IS THE ONE RUST READS. The Python builds the request and the
    Rust parses it, and nothing but a real round trip catches a renamed field:
    a JSON key Rust does not know is silently defaulted, so a renamed `sdl_line`
    would leave every emulator with a pad and no mapping and nothing would
    say so. Run against the real binary when there is one.

  A MISSING EMULATOR IS NOT A FAILURE. Most machines have none of the three
    installed. `emit` must exit 0 and write what it can.

  NOTHING HERE CAN TAKE THE DAEMON DOWN. No binary, a binary that fails, a
    binary that hangs, a binary that is not executable: False and a log line,
    never an exception. The caller has already republished the pads and
    written the SDL database; losing Cemu's profile is not worth a crash.

  THE DAEMON ASKS FOR EVERY ASSIGNED PLAYER. Including one with no capture,
    which gets an entry with an empty mapping rather than no entry -- Cemu
    still needs the profile, and a pad missing from the request is a pad the
    user has to bind by hand.

Stories: S4 (a finished wizard writes what the front-end reads).

Nothing here touches real user state or hardware: XDG_* and PADMAP_PROFILE_DIR
are redirected into a temp tree before padmap is imported, every emulator path
is overridden into it, and no device is opened.
"""

from __future__ import annotations

import json
import logging
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

_SANDBOX = Path(tempfile.mkdtemp(prefix="check-emulators-"))
os.environ["XDG_RUNTIME_DIR"] = str(_SANDBOX / "run")
os.environ["XDG_CONFIG_HOME"] = str(_SANDBOX / "config")
os.environ["XDG_DATA_HOME"] = str(_SANDBOX / "data")
os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "devices")
for _sub in ("run/padmap", "config", "data", "devices", "bin"):
    (_SANDBOX / _sub).mkdir(parents=True, exist_ok=True)

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import devices, emulators  # noqa: E402

devices.discover = lambda *a, **k: []

logging.disable(logging.CRITICAL)

CHECKS = 0


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def heading(text: str) -> None:
    print(f"\n{text}:")


def ok(text: str) -> None:
    global CHECKS
    CHECKS += 1
    print(f"  ok  {text}")


def note(text: str) -> None:
    print(f"  --  {text}")


# -- fixtures ----------------------------------------------------------------

def pad(player: int, *, mapped: bool = True) -> emulators.Published:
    return emulators.Published(
        player=player,
        guid=f"0300000{player}5e040000e00200000{player}000000",
        name=f"padmap Player {player}",
        keys=[0x130, 0x131, 0x133, 0x134],
        axes=[0x00, 0x01, 0x10, 0x11],
        sdl_line=(f"guid{player},padmap Player {player},a:b0," if mapped
                  else ""),
    )


def stub_binary(name: str, script: str) -> str:
    """A fake `padmap-rs` that does whatever the scenario needs."""
    path = _SANDBOX / "bin" / name
    path.write_text(script)
    path.chmod(0o755)
    return str(path)


# -- the wire format ---------------------------------------------------------

def real_binary() -> str | None:
    """The built padmap-rs, if this machine has one.

    `PADMAP_RS` is set by the dev shell. Without it the round trip is skipped
    rather than the suite failing: the Python half is still worth checking on
    a machine with no Rust toolchain.
    """
    found = os.environ.get("PADMAP_RS") or shutil.which("padmap-rs")
    if found and Path(found).exists():
        return found
    return None


def check_the_request_is_what_rust_reads() -> None:
    """The one coupling a unit test on either side cannot see.

    serde defaults an unknown key rather than rejecting it, so a field renamed
    on this side produces a request Rust accepts and misreads -- every pad
    present, no mapping on any of them, no error anywhere.
    """
    binary = real_binary()
    if binary is None:
        note("skipped: no padmap-rs built; set PADMAP_RS or run in the dev "
             "shell to check the wire format")
        return

    run = _SANDBOX / "emit"
    cemu = run / "Cemu" / "controllerProfiles"
    ares = run / "ares.bml"
    ryujinx = run / "Config.json"
    run.mkdir(parents=True, exist_ok=True)
    ares.write_text("Video\n  Driver: OpenGL\n")
    ryujinx.write_text('{"version": 50}\n')

    env = dict(
        os.environ,
        XDG_RUNTIME_DIR=str(run),
        PADMAP_CEMU_DIR=str(cemu),
        PADMAP_ARES_SETTINGS=str(ares),
        PADMAP_RYUJINX_CONFIG=str(ryujinx),
    )
    result = subprocess.run(
        [binary, "emit"], input=emulators.payload([pad(1), pad(2)]),
        text=True, capture_output=True, env=env, timeout=300, check=False)
    if result.returncode != 0:
        fail(f"padmap-rs emit rejected the daemon's request: "
             f"{result.stderr.strip()}")
    ok("padmap-rs accepts the request the daemon builds")

    profile = (cemu / "controller0.xml").read_text()
    if pad(1).guid not in profile:
        fail(f"Cemu's profile does not name player 1's GUID:\n{profile}")
    ok("the GUID reached Cemu's profile")

    settings = ares.read_text()
    if "Driver: OpenGL" not in settings:
        fail("ares' own settings were lost")
    if "VirtualPad1" not in settings or "VirtualPad2" not in settings:
        fail(f"ares is missing a port:\n{settings}")
    ok("both ports reached ares, and its other settings survived")

    config = json.loads(ryujinx.read_text())
    if config.get("version") != 50:
        fail("Ryujinx's own settings were lost")
    ids = {entry["id"] for entry in config["input_config"]}
    if len(ids) != 2:
        fail(f"Ryujinx cannot tell the two pads apart: {ids}")
    ok("Ryujinx got two distinct device ids")

    script = (run / "padmap" / "env.sh").read_text()
    for player in (1, 2):
        if f"padmap Player {player}" not in script:
            fail(f"player {player}'s mapping is not in the environment file")
    ok("both mappings reached the environment file Cemu is launched with")


def check_an_absent_emulator_is_not_a_failure() -> None:
    binary = real_binary()
    if binary is None:
        note("skipped: no padmap-rs built")
        return
    run = _SANDBOX / "bare"
    run.mkdir(parents=True, exist_ok=True)
    env = dict(
        os.environ,
        XDG_RUNTIME_DIR=str(run),
        PADMAP_CEMU_DIR=str(run / "cemu"),
        PADMAP_ARES_SETTINGS=str(run / "nowhere" / "ares.bml"),
        PADMAP_RYUJINX_CONFIG=str(run / "nowhere" / "Config.json"),
    )
    result = subprocess.run(
        [binary, "emit"], input=emulators.payload([pad(1)]), text=True,
        capture_output=True, env=env, timeout=300, check=False)
    if result.returncode != 0:
        fail("an emulator that has never run was treated as a failure; most "
             f"machines have none of the three: {result.stderr.strip()}")
    if not (run / "cemu" / "controller0.xml").exists():
        fail("a skip stopped the emulators that could be written")
    ok("ares and Ryujinx absent: exit 0, and Cemu still written")
    if (run / "nowhere").exists():
        fail("padmap invented a settings file for an emulator that has never "
             "run, which would give it padmap's ports and defaults for "
             "everything else")
    ok("no settings file was invented")


# -- nothing here can take the daemon down -----------------------------------

def check_no_binary_is_survivable() -> None:
    os.environ["PADMAP_RS"] = str(_SANDBOX / "bin" / "not-installed")
    try:
        if emulators.binary() is not None:
            fail("a PADMAP_RS pointing at nothing was accepted")
        if emulators.publish([pad(1)]) is not False:
            fail("publish claimed to have written something with no binary")
    finally:
        os.environ.pop("PADMAP_RS", None)
    ok("no binary: False, no exception")


def check_a_failing_binary_is_survivable() -> None:
    os.environ["PADMAP_RS"] = stub_binary(
        "failing", "#!/bin/sh\necho 'bad field' >&2\nexit 1\n")
    try:
        if emulators.publish([pad(1)]) is not False:
            fail("a failing emit was reported as success")
    finally:
        os.environ.pop("PADMAP_RS", None)
    ok("a non-zero exit: False, no exception")


def check_a_hanging_binary_is_survivable() -> None:
    # The daemon calls this on the thread that answers its socket. A hang
    # there is a daemon that stops responding, holding EVIOCGRAB on every pad.
    os.environ["PADMAP_RS"] = stub_binary("hanging", "#!/bin/sh\nsleep 30\n")
    previous = emulators.TIMEOUT_SECONDS
    emulators.TIMEOUT_SECONDS = 0.5
    try:
        if emulators.publish([pad(1)]) is not False:
            fail("a hanging emit was reported as success")
    finally:
        emulators.TIMEOUT_SECONDS = previous
        os.environ.pop("PADMAP_RS", None)
    ok("a hang is bounded by the timeout: False, no exception")


def check_a_binary_that_is_not_executable_is_survivable() -> None:
    path = _SANDBOX / "bin" / "not-executable"
    path.write_text("#!/bin/sh\n")
    path.chmod(0o644)
    os.environ["PADMAP_RS"] = str(path)
    try:
        if emulators.publish([pad(1)]) is not False:
            fail("a PermissionError escaped publish")
    finally:
        os.environ.pop("PADMAP_RS", None)
    ok("a non-executable binary: False, no exception")


def check_no_pads_writes_nothing() -> None:
    # An empty roster means every controller was unplugged. Rewriting the
    # emulators' files to bind nothing would take away the profiles a user may
    # be about to plug a pad back into.
    os.environ["PADMAP_RS"] = stub_binary(
        "counting", f"#!/bin/sh\ntouch {_SANDBOX / 'bin' / 'was-called'}\n")
    try:
        if emulators.publish([]) is not False:
            fail("an empty roster was published")
    finally:
        os.environ.pop("PADMAP_RS", None)
    if (_SANDBOX / "bin" / "was-called").exists():
        fail("padmap-rs was run for an empty roster")
    ok("an empty roster runs nothing")


# -- what the daemon asks for ------------------------------------------------

def check_the_daemon_asks_for_every_assigned_player() -> None:
    """Including a player with no capture, which still needs a profile."""
    from padmap import server

    requests: list[list[emulators.Published]] = []
    # Reached through the module object: the daemon calls `emulators.publish`
    # by attribute, so replacing it here is what it will find.
    patched: Any = emulators
    real_publish = patched.publish
    patched.publish = requests.append
    cfg: Any = server.controllercfg
    real_caps, real_guid = cfg.pad_capabilities, cfg.virtual_guid
    try:
        daemon: Any = server.Server.__new__(server.Server)
        daemon._assignments = [_FakeAssignment(1), _FakeAssignment(2)]
        cfg.pad_capabilities = lambda _pad: ([0x131, 0x130], [0x01, 0x00])
        cfg.virtual_guid = lambda player, _pad: f"guid-{player}"
        daemon._write_emulator_configs({1: "line-1"})
    finally:
        patched.publish = real_publish
        cfg.pad_capabilities, cfg.virtual_guid = real_caps, real_guid

    if len(requests) != 1:
        fail(f"the daemon made {len(requests)} requests, expected 1")
    published = requests[0]
    if [entry.player for entry in published] != [1, 2]:
        fail(f"the daemon skipped a player: {[e.player for e in published]}")
    ok("every assigned player is in the request")
    if published[1].sdl_line != "":
        fail("a player with no capture was given somebody else's mapping")
    ok("a player with no capture gets an entry with an empty mapping")
    if published[0].keys != [0x130, 0x131] or published[0].axes != [0x00, 0x01]:
        fail(f"capabilities reached ares unsorted: {published[0]}")
    ok("capabilities are sorted, which is what ares' indices count over")


class _FakeAssignment:
    """Just enough of an Assignment: nothing below opens the pad."""

    def __init__(self, player: int) -> None:
        self.player = player
        self.pad = object()


def main() -> int:
    heading("the wire format between the daemon and padmap-rs")
    check_the_request_is_what_rust_reads()
    check_an_absent_emulator_is_not_a_failure()

    heading("nothing here can take the daemon down")
    check_no_binary_is_survivable()
    check_a_failing_binary_is_survivable()
    check_a_hanging_binary_is_survivable()
    check_a_binary_that_is_not_executable_is_survivable()
    check_no_pads_writes_nothing()

    heading("what the daemon asks for")
    check_the_daemon_asks_for_every_assigned_player()

    heading("the sandbox")
    for name in ("XDG_CONFIG_HOME", "XDG_RUNTIME_DIR", "XDG_DATA_HOME",
                 "PADMAP_PROFILE_DIR"):
        if not os.environ[name].startswith(str(_SANDBOX)):
            fail(f"{name} escaped the sandbox and now points at "
                 f"{os.environ[name]}, which may be real user state")
    ok("every redirected path still points into the temp sandbox")

    print(f"\n{CHECKS} assertions held")
    print("all checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
