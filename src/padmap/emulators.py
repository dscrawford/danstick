"""Emulator config files, written by the Rust side.

Cemu, ares and Ryujinx each need something the SDL database cannot give them,
and the code that knows their three formats lives in `padmap_input::emulators`
-- written there because that is where padmap is going, and because Ryujinx's
device id and ares' raw joystick indices are the kind of thing that must have
exactly one implementation or the two will drift and only one of them will be
the one a user's emulator reads.

So this does not reimplement any of it. It builds the pad list and hands it to
`padmap-rs emit` on stdin. The cost is a subprocess per republish -- a few per
hour, off the forwarding path -- and the benefit is that when the daemon
finishes moving to Rust this module goes away rather than being ported.

Never fatal. A missing binary, a crash, a timeout: the pads are already
republished and the SDL database is already written, and losing Cemu's profile
is not a reason to take a working game down.
"""

from __future__ import annotations

import json
import logging
import os
import shutil
import subprocess
from dataclasses import asdict, dataclass
from pathlib import Path

log = logging.getLogger("padmap")

# How long `padmap-rs emit` gets. It writes four small files and does no
# device I/O, so anything near this is a hang rather than a slow disk.
TIMEOUT_SECONDS = 10.0

# Set by the dev shell and the packaged daemon. Checked before PATH so a
# running daemon keeps using the binary it was installed with.
ENV_BINARY = "PADMAP_RS"


@dataclass(frozen=True)
class Published:
    """One republished pad, as `padmap_input::emulators::Published` reads it.

    The capabilities are the *clone's*. They happen to equal the controller's
    -- the clone mirrors them -- but ares numbers its bindings over the device
    SDL opened, and SDL opens the clone.
    """

    player: int
    guid: str
    name: str
    keys: list[int]
    axes: list[int]
    sdl_line: str


def binary() -> str | None:
    """Where `padmap-rs` is, or None if it is not installed."""
    override = os.environ.get(ENV_BINARY, "")
    if override:
        return override if Path(override).exists() else None
    return shutil.which("padmap-rs")


def payload(pads: list[Published]) -> str:
    """The JSON `padmap-rs emit` reads. Separate so a test needs no binary."""
    return json.dumps([asdict(pad) for pad in pads])


def publish(pads: list[Published]) -> bool:
    """Write every emulator's config for these pads. True if it ran.

    False means nothing was written and the reason is logged: no binary, or a
    failure inside it. Not an exception, because the only caller is the daemon
    finishing a republish and there is nothing useful it could do differently.
    """
    if not pads:
        return False
    found = binary()
    if found is None:
        log.debug("no padmap-rs on PATH; emulator configs not written")
        return False
    try:
        result = subprocess.run(
            [found, "emit"], input=payload(pads), text=True,
            capture_output=True, timeout=TIMEOUT_SECONDS, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        log.warning("could not write emulator configs: %s", error)
        return False
    if result.returncode != 0:
        # stderr rather than the code: `emit` exits non-zero only on a
        # malformed request, and the message says which field.
        log.warning("padmap-rs emit failed: %s",
                    result.stderr.strip() or f"exit {result.returncode}")
        return False
    written = [line for line in result.stdout.splitlines() if line]
    if written:
        log.info("wrote %d emulator config file(s)", len(written))
    return True
