"""The few things a diagnostic tool needs that padmap's own binary does not print.

padmap is a Rust binary now, so there is no package to import. These are the
three facts a client needs to talk to the daemon -- where the socket is, how a
message is framed, and which processes are daemons -- restated here rather
than shelled out for, because a tool that exists to diagnose the daemon should
not depend on the daemon answering.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any


def runtime_dir() -> Path:
    return Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "padmap"


def socket_path() -> Path:
    return runtime_dir() / "padmap.sock"


def encode(message: dict[str, Any]) -> bytes:
    """One message, newline-terminated, with no stray whitespace in it."""
    return (json.dumps(message, separators=(",", ":")) + "\n").encode("utf-8")


class LineReader:
    """Accumulates socket reads and yields whole JSON messages.

    A stream socket splits messages anywhere, so a client that assumes one
    recv() is one message works until it doesn't.
    """

    def __init__(self) -> None:
        self._buffer = b""

    def feed(self, data: bytes) -> list[dict[str, Any]]:
        self._buffer += data
        out: list[dict[str, Any]] = []
        while b"\n" in self._buffer:
            line, self._buffer = self._buffer.split(b"\n", 1)
            line = line.strip()
            if not line:
                continue
            try:
                decoded = json.loads(line)
            except (ValueError, RecursionError):
                continue
            if isinstance(decoded, dict):
                out.append(decoded)
        return out


def daemon_pids(runtime: str | None = None) -> list[int]:
    """Pids of padmap daemons serving a given XDG_RUNTIME_DIR.

    Matches argv structurally rather than as a substring: `pgrep -f` also
    matches any shell whose command line mentions the string, and signalling
    that kills the shell instead of a daemon.
    """
    wanted = runtime if runtime is not None else os.environ.get(
        "XDG_RUNTIME_DIR", "/tmp")
    uid = os.getuid()
    pids: list[int] = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        try:
            if entry.stat().st_uid != uid:
                continue
            argv = [part.decode("utf-8", "replace") for part in
                    (entry / "cmdline").read_bytes().split(b"\0") if part]
            environ = [part.decode("utf-8", "replace") for part in
                       (entry / "environ").read_bytes().split(b"\0") if part]
        except OSError:
            continue
        if len(argv) < 2 or argv[-1] != "serve":
            continue
        if Path(argv[-2]).name not in ("padmap", "padmap-rs"):
            continue
        theirs = "/tmp"
        for item in environ:
            key, sep, value = item.partition("=")
            if sep and key == "XDG_RUNTIME_DIR":
                theirs = value
        if os.path.normpath(theirs) == os.path.normpath(wanted):
            pids.append(pid)
    return sorted(pids)
