"""Minimal client for the danstick daemon.

Exists so the protocol can be exercised without Pegasus, and so the C++ patch
has a reference to check against. It is also the fastest way to see what a
front-end will actually receive.

    danstick serve &                     # in one terminal
    python3 tools/padctl.py begin      # then hold buttons on the pads
    python3 tools/padctl.py watch      # just observe
"""

import json
import socket
import sys
from pathlib import Path
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))

import _danstick as protocol  # noqa: E402


def connect() -> socket.socket:
    path = protocol.socket_path()
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(str(path))
    except OSError as exc:
        print(f"cannot reach the daemon at {path}: {exc}")
        print("start it with:  danstick serve")
        raise SystemExit(1)
    return sock


def pump(sock: socket.socket, seconds: float | None = None) -> None:
    """Print events until the socket closes or `seconds` elapses."""
    reader = protocol.LineReader()
    deadline = None if seconds is None else time.monotonic() + seconds
    sock.settimeout(0.2)
    while deadline is None or time.monotonic() < deadline:
        try:
            data = sock.recv(65536)
        except socket.timeout:
            continue
        except OSError:
            break
        if not data:
            break
        for message in reader.feed(data):
            kind = message.get("event")
            if kind == "progress":
                print(f"  hold {message['frac']:.2f}", end="\r", flush=True)
            elif kind == "confirm":
                print(f"  confirm {message['frac']:.2f}", end="\r", flush=True)
            elif kind == "claim":
                print(f"claim: Player {message['player']} = "
                      f"{message['name']} [{message['node']}]")
            elif kind == "state":
                players = ", ".join(
                    f"P{p['player']}={p['name']}" for p in message["players"]
                ) or "none"
                print(f"state: {message['state']}  ({players})")
            else:
                print(json.dumps(message))


def main() -> int:
    command = sys.argv[1] if len(sys.argv) > 1 else "watch"
    sock = connect()

    if command == "watch":
        print("watching; Ctrl-C to stop")
        pump(sock)
        return 0

    if command == "begin":
        players = int(sys.argv[2]) if len(sys.argv) > 2 else 4
        sock.sendall(protocol.encode({"cmd": "begin", "players": players}))
        print("session started -- hold a button on each pad in order,")
        print("then hold again on an assigned pad to confirm")
        pump(sock)
        return 0

    if command in ("reset", "accept", "cancel", "status"):
        sock.sendall(protocol.encode({"cmd": command}))
        pump(sock, seconds=0.5)
        return 0

    print(f"unknown command {command!r}")
    print("usage: padctl.py [watch|begin [N]|reset|accept|cancel|status]")
    return 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print()
        sys.exit(0)
