"""A Switch pad stuck in simple report mode must be noticed and re-asked.

The Pro Controller powers up sending report 0x3f: buttons and a hat, no
usable analogue data, and a layout that has nothing to do with 0x30's. padmap
asks it for 0x30 once, as the node opens, and `read` filters everything else
out.

That one request is not reliable. A pad that has just finished associating
over Bluetooth can ignore it -- the write succeeds, so nothing fails, and the
pad goes on sending 0x3f indefinitely. Every symptom padmap can see says the
controller is fine: the node exists, the descriptor is live and unmoved,
reports arrive at sixty-odd a second, `read` never raises, `alive()` is true.
They are simply all dropped by the report-id filter, so not one button works.

Measured on the pad here while it was in that state: 3001 hidraw reports in 45
seconds, 0 events on the clone, and nothing in the log. It reads exactly like
a broken mapping, and was reported as one twice.

Four properties:

  * 0x3f reports produce no events -- they cannot be decoded as 0x30 and
    guessing at them would emit garbage
  * the first one says so in the log, naming the mode, at the moment it happens
  * a pad that stays in 0x3f gets asked again, on the wire
  * a 0x30 report resets the count, so a healthy pad is never re-asked and a
    pad that drops back later is caught again

Nothing here opens a controller: the pad is a socketpair, which lets the check
read back the subcommand padmap writes.

    python3 tests/check_report_mode.py
"""

from __future__ import annotations

import logging
import os
import socket
import sys
import tempfile
from pathlib import Path

_SANDBOX = tempfile.mkdtemp(prefix="padmap-reportmode-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import hidraw  # noqa: E402
from padmap.devices import Pad  # noqa: E402


def pad() -> Pad:
    return Pad(path="/dev/input/event900", name="Pro Controller", phys="bt",
               uniq="", vid=0x057E, pid=0x2009, syspath="")


class Captured(logging.Handler):
    def __init__(self) -> None:              # noqa: D107
        super().__init__()
        self.lines: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.lines.append(record.getMessage())


def simple_report() -> bytes:
    """A real 0x3f, copied off the pad here while it was stuck."""
    return bytes.fromhex("3f 00 00 08 a0 82 4f 8b c0 81 2f 8b".replace(" ", ""))


def full_report() -> bytes:
    """A 0x30 of the right length. The contents do not matter here."""
    body = bytearray(49)
    body[0] = hidraw.REPORT_FULL
    return bytes(body)


class Harness:
    """A Source wired to a socketpair instead of a controller.

    Built without __init__ on purpose: the real one opens a device node and
    writes to it, and neither is available or wanted here. Every attribute the
    read path touches is set explicitly, so a new one added upstream shows up
    as an AttributeError rather than being silently defaulted.
    """

    def __init__(self) -> None:
        self.pad_side, self.test_side = socket.socketpair()
        self.pad_side.setblocking(False)
        self.test_side.setblocking(False)

        source = object.__new__(hidraw.Source)
        source.pad = pad()
        source.path = "/dev/hidraw8"
        source.name = source.pad.name
        source._fd = self.pad_side.fileno()
        source._buttons, source._axes, source._hat = {}, {}, (0, 0)
        source._counter = 0
        source._simple_seen = 0
        source._pending = []
        source._rdev = None
        self.source = source

    def deliver(self, payload: bytes) -> None:
        self.test_side.send(payload)

    def read(self) -> list:
        try:
            return self.source.read()
        except BlockingIOError:
            return []

    def written(self) -> list[bytes]:
        """Whatever padmap wrote towards the pad, as whole packets."""
        out = []
        while True:
            try:
                out.append(self.test_side.recv(4096))
            except BlockingIOError:
                return out

    def close(self) -> None:
        self.pad_side.close()
        self.test_side.close()


def is_mode_request(packet: bytes) -> bool:
    return (len(packet) >= 12 and packet[0] == 0x01
            and packet[10] == hidraw.SUBCMD_REPORT_MODE
            and packet[11] == hidraw.REPORT_FULL)


def check_simple_reports_decode_to_nothing() -> None:
    print("\na 0x3f report produces no events:")
    h = Harness()
    try:
        h.deliver(simple_report())
        events = h.read()
        if events:
            raise SystemExit(
                f"FAIL: a 0x3f was decoded into {len(events)} event(s). Its "
                f"layout is not 0x30's, so anything read out of it is noise "
                f"-- buttons pressing themselves, sticks drifting")
        print("  ok  dropped, as it must be")
    finally:
        h.close()


def check_the_first_one_is_logged() -> None:
    print("\n...and says so, naming the mode:")
    h = Harness()
    handler = Captured()
    log = logging.getLogger("padmap.hidraw")
    previous = log.level
    log.setLevel(logging.INFO)
    log.addHandler(handler)
    try:
        h.deliver(simple_report())
        h.read()
    finally:
        log.removeHandler(handler)
        log.setLevel(previous)
        h.close()

    if not handler.lines:
        raise SystemExit(
            "FAIL: a pad delivering nothing usable logged nothing at all. "
            "Every other signal says the controller is healthy, so with no "
            "line here the fault is invisible and gets diagnosed as a broken "
            "mapping -- which is what happened, twice")
    said = " ".join(handler.lines)
    if "3f" not in said.lower():
        raise SystemExit(
            f"FAIL: the warning does not name the mode, so a reader cannot "
            f"tell this from any other silence.\n  logged: {said}")
    print(f"  ok  {handler.lines[0]}")


def check_a_stuck_pad_is_asked_again() -> None:
    print("\na pad that stays in 0x3f is re-asked, on the wire:")
    h = Harness()
    try:
        h.written()                      # discard anything already queued
        for _ in range(hidraw.SIMPLE_REPORTS_BEFORE_RETRY):
            h.deliver(simple_report())
            h.read()
        packets = [p for p in h.written() if is_mode_request(p)]
        if not packets:
            raise SystemExit(
                f"FAIL: after {hidraw.SIMPLE_REPORTS_BEFORE_RETRY} unusable "
                f"reports padmap never asked again. The request at open is "
                f"the only one there is, and a pad that ignored it stays "
                f"dead for the whole session")
        print(f"  ok  {len(packets)} re-request(s) written")
    finally:
        h.close()


def check_a_good_report_resets_it() -> None:
    print("\na 0x30 resets the count, so a healthy pad is left alone:")
    h = Harness()
    try:
        for _ in range(hidraw.SIMPLE_REPORTS_BEFORE_RETRY - 1):
            h.deliver(simple_report())
            h.read()
        h.deliver(full_report())
        h.read()
        if h.source._simple_seen != 0:
            raise SystemExit(
                f"FAIL: a usable report left the count at "
                f"{h.source._simple_seen}. A pad that blips one 0x3f every so "
                f"often would eventually be re-asked for no reason, mid-game")

        h.written()                      # discard
        h.deliver(full_report())
        h.read()
        if [p for p in h.written() if is_mode_request(p)]:
            raise SystemExit(
                "FAIL: a pad in full mode was sent a mode request anyway")
        print("  ok  count cleared, nothing written")
    finally:
        h.close()


def main() -> int:
    check_simple_reports_decode_to_nothing()
    check_the_first_one_is_logged()
    check_a_stuck_pad_is_asked_again()
    check_a_good_report_resets_it()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
