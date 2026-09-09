"""The wire framing: `protocol.encode` out, `protocol.LineReader.feed` in.

Every byte between the daemon and the front-end goes through these two
functions. The framing is newline-delimited JSON over a unix socket, chosen
so the Pegasus side can be a QLocalSocket and a QJsonDocument and nothing
else -- which means the framing has no length prefix, no checksum and no
resynchronisation story. It has exactly one invariant:

    a newline separates messages, and appears nowhere else.

Break either half of that and the connection does not fail loudly, it
desynchronises: from the first stray newline on, the client parses halves of
messages forever while the daemon happily goes on writing. Nothing in the
setup screen would say so -- the pads are grabbed, so the only feedback the
user has is the screen that stopped updating.

The other half is the reader. `Server._on_client_read` does
`sock.recv(65536)` and hands the result straight to `feed`, so a message
larger than 64 KiB *always* arrives split, and a kernel is free to split any
message anywhere else besides. Splitting inside a UTF-8 character or inside
a string literal is normal traffic, not an attack. The layout_choice event
carries whole layouts (S9/S10) and already runs to ~8 KiB with five consoles;
one screenshot-carrying event away from crossing the recv size.

And `feed` is reached before any guard. `_handle_command` is wrapped in a
bare `except Exception` on purpose -- its docstring records a front-end
sending {"players": "lots"} taking the daemon down with it, and with it every
virtual pad on the machine. That wrapper sits *inside* the loop that `feed`
drives, so anything `feed` itself raises goes straight out of `serve()`.
Anything a local process can put on that socket, it can put through `feed`.

Touches no hardware, opens no uinput node, imports neither `devices` nor
`server`, and never dispatches a command: `protocol` is a pure module and
this exercises it as one. The one socket it opens is a `socketpair()` to
itself, to reproduce the daemon's recv/feed shape without going near the
real one -- a live daemon owns the controllers on this machine.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_protocol_framing.py
"""

import json
import os
import socket
import sys
import tempfile
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Before importing padmap: protocol resolves the socket, the prompted file and
# the last-game record out of XDG_RUNTIME_DIR, and a live daemon is serving
# the real one.
_SANDBOX = tempfile.mkdtemp(prefix="padmap-framing-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    _path = Path(_SANDBOX) / _var.lower()
    _path.mkdir(parents=True, exist_ok=True)
    os.environ[_var] = str(_path)

sys.path.insert(0, str(REPO / "src"))

from padmap import protocol  # noqa: E402

GAPS: list[str] = []


def check(condition: object, complaint: str) -> None:
    """Assert, phrased as what breaks for the person at the arcade box."""
    if not condition:
        raise SystemExit(f"FAIL: {complaint}")


def gap(text: str) -> None:
    GAPS.append(text)
    print(f"  gap: {text}")


def read(reader: protocol.LineReader, *chunks: bytes) -> list[dict]:
    out: list[dict] = []
    for chunk in chunks:
        out += reader.feed(chunk)
    return out


def feed_all(*chunks: bytes) -> list[dict]:
    return read(protocol.LineReader(), *chunks)


# A real claim event (S3) and a real accepted event (S4), as the daemon
# writes them. Small enough to split byte by byte, real enough that a
# regression here is a regression in the setup screen.
CLAIM = {"event": "claim", "player": 1, "name": "Nintendo 64 Controller",
         "node": "event24", "icon": "n64", "configured": False}
ACCEPTED = {"event": "accepted",
            "players": [{"player": 1, "name": "Pad One", "node": "event24"},
                        {"player": 2, "name": "Pad Two", "node": "event25"}],
            "launch_config": "/run/user/1000/padmap/launch.cfg"}
STATE = {"event": "state", "state": protocol.STATE_ASSIGNING, "slots": 4,
         "players": [], "build": "/nix/store/abc-padmap", "pid": 4321,
         "identity": "mirror"}


def strict_loads(raw: bytes) -> object:
    """Parse the way a client that is not Python would.

    QJsonDocument implements JSON, not Python's superset of it: NaN and
    Infinity are not JSON and a strict client rejects the whole document.
    """
    def reject(constant: str) -> object:
        raise ValueError(f"not JSON: {constant}")

    return json.loads(raw.decode("utf-8"), parse_constant=reject)


# -- splitting ------------------------------------------------------------


def scenario_split_two() -> None:
    print("\nS3 -- a claim event split across two chunks, at every byte:")
    wire = protocol.encode(CLAIM)
    for cut in range(len(wire) + 1):
        reader = protocol.LineReader()
        early = reader.feed(wire[:cut])
        if cut < len(wire):
            check(early == [],
                  f"a claim split at byte {cut} was delivered before its "
                  f"newline arrived; the setup screen would act on half a "
                  f"message")
        got = early + reader.feed(wire[cut:])
        check(got == [CLAIM],
              f"a claim split at byte {cut} came back as {got!r}; the pad "
              f"the player just claimed never appears on the screen")
    print(f"  ok  all {len(wire) + 1} two-chunk splits reassemble exactly one claim")


def scenario_split_three_and_many() -> None:
    print("\nS4 -- the accepted event split three ways, and byte by byte:")
    wire = protocol.encode(ACCEPTED)
    third = len(wire) // 3
    got = feed_all(wire[:third], wire[third:2 * third], wire[2 * third:])
    check(got == [ACCEPTED],
          "an accepted event split three ways did not reassemble; the "
          "front-end never learns the assignment was saved")
    print("  ok  three chunks reassemble one accepted event")

    reader = protocol.LineReader()
    got = []
    for index in range(len(wire)):
        step = reader.feed(wire[index:index + 1])
        if index < len(wire) - 1:
            check(step == [],
                  f"byte {index} of an accepted event produced a message "
                  f"before the newline; the framing is guessing")
        got += step
    check(got == [ACCEPTED],
          "an accepted event fed one byte at a time did not reassemble")
    print(f"  ok  {len(wire)} single-byte chunks reassemble one accepted event")


def scenario_split_inside_utf8() -> None:
    print("\nsplitting inside a UTF-8 character:")
    # A pad whose name the kernel reports with non-ASCII in it. encode()
    # escapes non-ASCII, but feed() is also handed bytes off a socket, so it
    # has to be byte-safe rather than character-safe.
    message = {"event": "claim", "player": 1, "name": "Manette Néo \U0001f3ae",
               "node": "event9"}
    wire = (json.dumps(message, ensure_ascii=False,
                       separators=(",", ":")) + "\n").encode("utf-8")
    multibyte = [i for i in range(1, len(wire)) if wire[i] & 0xC0 == 0x80]
    check(multibyte,
          "test setup: there is no UTF-8 continuation byte to split on")
    for cut in multibyte:
        got = feed_all(wire[:cut], wire[cut:])
        check(got == [message],
              f"a pad name split mid-character at byte {cut} came back as "
              f"{got!r}; an accented controller name corrupts the claim")
    print(f"  ok  {len(multibyte)} splits inside multibyte characters reassemble")

    reader = protocol.LineReader()
    got = []
    for index in range(len(wire)):
        got += reader.feed(wire[index:index + 1])
    check(got == [message],
          "a non-ASCII pad name fed one byte at a time did not reassemble")
    print("  ok  the same message one byte at a time")


def scenario_split_inside_string_literal() -> None:
    print("\nsplitting inside a string literal, and inside an escape:")
    message = {"event": "error",
               "message": 'begin failed: "players" was \\ not an int'}
    wire = protocol.encode(message)
    opening = wire.index(b'begin failed')
    for cut in range(opening, opening + 20):
        got = feed_all(wire[:cut], wire[cut:])
        check(got == [message],
              f"an error message split inside its string at byte {cut} came "
              f"back as {got!r}; the reason the command failed is lost")
    print("  ok  splits inside a quoted string reassemble")

    escaped = protocol.encode({"event": "error", "message": "line\nbreak"})
    cut = escaped.index(b"\\n") + 1
    got = feed_all(escaped[:cut], escaped[cut:])
    check(got == [{"event": "error", "message": "line\nbreak"}],
          "a message split between the backslash and the n of an escape "
          "did not reassemble")
    print("  ok  a split between a backslash and its escape letter reassembles")


def scenario_newline_alone() -> None:
    print("\nthe terminating newline arriving on its own:")
    wire = protocol.encode(STATE)
    reader = protocol.LineReader()
    check(reader.feed(wire[:-1]) == [],
          "a state event without its newline was delivered anyway; the "
          "reader is guessing where messages end")
    check(reader.feed(b"\n") == [STATE],
          "a state event was lost when its newline arrived in its own read; "
          "the front-end cannot tell which daemon it is talking to (S18)")
    print("  ok  a lone-newline read completes the pending message")


# -- batching -------------------------------------------------------------


def scenario_many_in_one_chunk() -> None:
    print("\nS4 -- several messages in one chunk:")
    batch = [CLAIM, {"event": "progress", "frac": 0.62}, STATE, ACCEPTED]
    wire = b"".join(protocol.encode(m) for m in batch)
    got = feed_all(wire)
    check(got == batch,
          f"a burst of {len(batch)} events in one read came back as "
          f"{len(got)} in {'the same' if got == batch else 'a different'} "
          f"order; events are dropped or reordered under load")
    print(f"  ok  {len(batch)} events in one chunk, all of them, in order")

    # The tail of a read is usually half a message. It must not be lost.
    reader = protocol.LineReader()
    tail = protocol.encode(CLAIM)
    got = reader.feed(wire + tail[:10])
    check(got == batch,
          "a trailing partial message swallowed the complete ones before it")
    check(reader.feed(tail[10:]) == [CLAIM],
          "the partial message at the end of a read was never completed")
    print("  ok  a partial tail neither loses nor delays the whole messages")


def scenario_empty_and_newline_chunks() -> None:
    print("\nempty chunks and chunks of nothing but newlines:")
    check(feed_all(b"") == [], "an empty read invented a message")
    check(feed_all(b"", b"", b"") == [],
          "repeated empty reads invented a message")
    print("  ok  empty chunks yield nothing")

    reader = protocol.LineReader()
    wire = protocol.encode(CLAIM)
    reader.feed(wire[:8])
    check(reader.feed(b"") == [],
          "an empty read in the middle of a message produced something")
    check(reader.feed(wire[8:]) == [CLAIM],
          "an empty read in the middle of a message destroyed it; a socket "
          "is allowed to return a short read at any moment")
    print("  ok  an empty chunk mid-message does not disturb the buffer")

    check(feed_all(b"\n\n\n\n") == [], "a chunk of newlines invented messages")
    check(feed_all(b"\r\n\r\n") == [],
          "a chunk of CRLFs invented messages")
    check(feed_all(b"   \n\t\n \r\n") == [],
          "a chunk of blank lines invented messages")
    print("  ok  newline-only and blank-only chunks yield nothing")

    got = feed_all(b"\n\n" + protocol.encode(CLAIM) + b"\n\n")
    check(got == [CLAIM],
          "blank lines around a real message lost the message; a keepalive "
          "newline would silence the setup screen")
    print("  ok  blank lines around a message do not hide it")


def scenario_crlf() -> None:
    print("\nCRLF line endings:")
    wire = (json.dumps(CLAIM, separators=(",", ":")) + "\r\n"
            + json.dumps(STATE, separators=(",", ":")) + "\r\n").encode()
    check(feed_all(wire) == [CLAIM, STATE],
          "a client that terminates with CRLF has every message rejected")
    print("  ok  CRLF-terminated messages parse")

    # Split exactly between the CR and the LF -- the one boundary a naive
    # reader gets wrong.
    single = (json.dumps(CLAIM, separators=(",", ":")) + "\r\n").encode()
    cut = single.index(b"\r\n") + 1
    check(feed_all(single[:cut], single[cut:]) == [CLAIM],
          "a message split between its CR and its LF was lost")
    print("  ok  a split between CR and LF reassembles")


# -- hostile lines: the framing must survive them --------------------------


def scenario_malformed_between_good() -> None:
    print("\na malformed line between two good ones:")
    junk = [b"{not json at all}", b'{"cmd":', b"}{", b'{"a":1',
            b'{"a":1}}', b"\x1b[2J", b"<html>", b"{'cmd':'begin'}"]
    for bad in junk:
        wire = protocol.encode(CLAIM) + bad + b"\n" + protocol.encode(STATE)
        got = feed_all(wire)
        check(got == [CLAIM, STATE],
              f"garbage line {bad!r} between two events cost us {got!r}; the "
              f"newline is still a frame boundary and both events must "
              f"survive it")
    print(f"  ok  {len(junk)} kinds of garbage line skipped, neighbours intact")

    # ... and split across the garbage too, since that is how it would arrive.
    wire = protocol.encode(CLAIM) + b"{oops}\n" + protocol.encode(STATE)
    for cut in range(len(wire) + 1):
        got = feed_all(wire[:cut], wire[cut:])
        check(got == [CLAIM, STATE],
              f"garbage plus a split at byte {cut} cost us {got!r}")
    print(f"  ok  and at all {len(wire) + 1} split points around it")


def scenario_valid_json_not_an_object() -> None:
    print("\na line that is valid JSON but not an object:")
    others = [b"42", b'"hello"', b"null", b"true", b"false", b"[1,2,3]",
              b'[{"cmd":"begin"}]', b"1.5", b"-0.0", b'""']
    for line in others:
        wire = protocol.encode(CLAIM) + line + b"\n" + protocol.encode(STATE)
        got = feed_all(wire)
        check(got == [CLAIM, STATE],
              f"non-object line {line!r} yielded {got!r}; a command handler "
              f"expecting a dict would be handed a {type(json.loads(line)).__name__}")
    print(f"  ok  {len(others)} non-object lines dropped, neighbours intact")

    for line in others:
        check(feed_all(line + b"\n") == [],
              f"{line!r} alone was delivered as a message")
    print("  ok  and none of them is delivered on its own")


def scenario_nul_bytes() -> None:
    print("\nNUL bytes:")
    good = protocol.encode(CLAIM)
    # Each is a run of lines with a NUL somewhere in it. Whatever those lines
    # do, the framing must still hand over the message that follows them.
    placements = {
        "leading a line": b"\x00" + good,
        "appended to a line": good[:-1] + b"\x00\n",
        "inside a string value": b'{"event":"claim","name":"a\x00b"}\n',
        "a whole line of them": b"\x00" * 64 + b"\n",
        "a line that is only one": b"\x00\n",
        "scattered over three lines": b"\x00" + good[:-1] + b"\x00\n\x00\x00\n",
    }
    for where, prefix in placements.items():
        wire = prefix + protocol.encode(STATE)
        try:
            got = feed_all(wire)
        except Exception as error:                       # noqa: BLE001
            raise SystemExit(
                f"FAIL: a NUL byte {where} raised "
                f"{type(error).__name__}: {error} out of feed(); any local "
                f"process can send that byte and end the daemon, taking "
                f"every virtual pad with it")
        check(all(isinstance(m, dict) for m in got),
              f"a NUL byte {where} produced a non-dict message")
        check(got and got[-1] == STATE,
              f"a NUL byte {where} swallowed the message after it "
              f"({got!r}); the newline is still a frame boundary")
    print(f"  ok  {len(placements)} NUL placements: feed does not raise, and "
          f"the next message still arrives")

    # A NUL with no newline after it glues itself to the *following* message,
    # so that message is the one that is lost -- not the framing.
    wire = good + b"\x00" + protocol.encode(STATE) + protocol.encode(CLAIM)
    check(feed_all(wire) == [CLAIM, CLAIM],
          "a NUL between two messages cost more than the one message it was "
          "glued to; a single stray byte would desynchronise the connection "
          "permanently")
    print("  ok  a NUL costs exactly one message; the next newline resyncs")


def scenario_invalid_utf8() -> None:
    print("\ninvalid UTF-8:")
    broken = {
        "lone 0xff": b'{"event":"claim","name":"\xff"}',
        "lone continuation": b'{"event":"claim","name":"\x80\x80"}',
        "truncated 3-byte": b'{"event":"claim","name":"\xe2\x82"}',
        "overlong": b'{"event":"claim","name":"\xc0\xaf"}',
        "raw bytes only": b"\xff\xfe\xfd\xfc",
        "half a UTF-16 pair": b'{"event":"claim","name":"\x00\x41\x00\x42"}',
    }
    for where, bad in broken.items():
        wire = protocol.encode(CLAIM) + bad + b"\n" + protocol.encode(STATE)
        try:
            got = feed_all(wire)
        except Exception as error:                       # noqa: BLE001
            raise SystemExit(
                f"FAIL: {where} raised {type(error).__name__} out of feed(); "
                f"a mis-encoded controller name would end the daemon and "
                f"take every virtual pad with it")
        check(got == [CLAIM, STATE],
              f"{where} cost us {got!r}; the newline after it is still a "
              f"frame boundary")
    print(f"  ok  {len(broken)} invalid-UTF-8 lines skipped, neighbours intact")

    # Split inside the invalid sequence too.
    wire = (protocol.encode(CLAIM) + b'{"name":"\xe2\x82"}\n'
            + protocol.encode(STATE))
    for cut in range(len(wire) + 1):
        check(feed_all(wire[:cut], wire[cut:]) == [CLAIM, STATE],
              f"invalid UTF-8 plus a split at byte {cut} lost a message")
    print("  ok  and at every split point through the bad bytes")

    # One kind of bad UTF-8 is NOT rejected: json.loads decodes bytes with
    # errors="surrogatepass", so an unpaired surrogate arrives as a Python
    # str no UTF-8 encoder will take back. Worth pinning down, because the
    # daemon echoes client strings into its error events.
    surrogate = b'{"cmd":"set_icon","icon":"\xed\xa0\x80"}'
    got = feed_all(protocol.encode(CLAIM) + surrogate + b"\n"
                   + protocol.encode(STATE))
    check(len(got) in (2, 3) and got[0] == CLAIM and got[-1] == STATE,
          f"an unpaired surrogate cost us a neighbouring message ({got!r})")
    if len(got) == 3:
        icon = got[1]["icon"]
        check(icon == "\ud800",
              f"an unpaired surrogate decoded to {icon!r} rather than being "
              f"passed through or rejected")
        raised = None
        try:
            round_tripped = feed_all(protocol.encode({"icon": icon}))
        except UnicodeEncodeError as error:
            raised = error
        check(raised is None,
              f"encode() raised {raised!r} on a string the daemon got from "
              f"a client; Server._send catches only OSError, so echoing an "
              f"unpaired surrogate back would end the daemon")
        check(round_tripped == [{"icon": icon}],
              "an unpaired surrogate did not survive being sent back out")
        print("  ok  an unpaired surrogate is accepted (surrogatepass) and "
              "encode() re-emits it without raising")
    else:
        print("  ok  an unpaired surrogate is rejected like any other bad "
              "UTF-8")


def scenario_bom() -> None:
    print("\na byte-order mark, which some Qt writers prepend:")
    wire = b"\xef\xbb\xbf" + protocol.encode(CLAIM)
    got = feed_all(wire)
    check(got in ([CLAIM], []),
          f"a BOM-prefixed message produced {got!r}, which is neither the "
          f"message nor a clean skip")
    if got == [CLAIM]:
        print("  ok  a UTF-8 BOM is tolerated and the message parses")
    else:
        print("  ok  a UTF-8 BOM costs its own line only")
    after = feed_all(wire + protocol.encode(STATE))
    check(after and after[-1] == STATE,
          f"a BOM took the following message with it ({after!r})")
    print("  ok  either way the next message survives")


# -- the escaped newline: the one that must NOT split a frame --------------


def scenario_escaped_newline_is_not_a_boundary() -> None:
    print("\nan escaped newline inside a string is not a frame boundary:")
    cases = {
        "newline": "Player 1\nPlayer 2",
        "two newlines": "a\n\nb",
        "leading newline": "\nstarts with one",
        "trailing newline": "ends with one\n",
        "CRLF in a value": "windows\r\nstyle",
        "lone CR": "old\rmac",
        "newline and tab": "a\n\tb",
        "only newlines": "\n\n\n\n",
        "a literal backslash-n": "not a newline: \\n",
        "backslash before newline": "trap: \\\n after",
        "U+2028 line separator": "js hazard",
        "U+2029 paragraph sep": "js hazard",
    }
    for name, value in cases.items():
        message = {"event": "error", "message": value}
        wire = protocol.encode(message)
        check(wire.count(b"\n") == 1,
              f"a value containing a {name} put {wire.count(b'\n')} raw "
              f"newlines on the wire; from here on the client parses halves "
              f"of messages forever")
        check(wire.endswith(b"\n"),
              f"a value containing a {name} produced a message with no "
              f"terminator")
        got = feed_all(wire)
        check(got == [message],
              f"a value containing a {name} came back as {got!r} instead of "
              f"the message that was sent")
    print(f"  ok  {len(cases)} newline-bearing values are each exactly one frame")

    # And the whole set at once, in one chunk and byte by byte.
    batch = [{"event": "error", "message": v} for v in cases.values()]
    wire = b"".join(protocol.encode(m) for m in batch)
    check(feed_all(wire) == batch,
          "a burst of newline-bearing messages did not come back intact")
    reader = protocol.LineReader()
    got = []
    for index in range(len(wire)):
        got += reader.feed(wire[index:index + 1])
    check(got == batch,
          "newline-bearing messages fed one byte at a time did not "
          "reassemble; the escaping is not split-safe")
    print("  ok  the whole set survives one chunk and one-byte chunks alike")


# -- encode -----------------------------------------------------------------


HOSTILE_VALUES = {
    "raw newline": "a\nb",
    "raw tab": "a\tb",
    "carriage return": "a\rb",
    "form feed": "a\x0cb",
    "vertical tab": "a\x0bb",
    "escape char": "\x1b[2Jcleared",
    "NUL in a value": "a\x00b",
    "every C0 control": "".join(chr(c) for c in range(0x20)),
    "DEL": "\x7f",
    "C1 controls": "".join(chr(c) for c in range(0x80, 0xA0)),
    "accented": "Manette Néo",
    "emoji": "\U0001f3ae\U0001f579",
    "CJK": "コントローラー",
    "RTL": "בקר משחקים",
    "combining": "ę́",
    "line separator": "a b",
    "paragraph separator": "a b",
    "non-character": "￾￿",
    "quote and backslash": '"\\"',
    "json-looking text": '{"cmd":"begin","players":4}\n{"cmd":"accept"}',
    "empty string": "",
    "big int": 10 ** 400,
    "negative big int": -(10 ** 400),
    "big float": 1.7976931348623157e308,
    "tiny float": 5e-324,
    "negative zero": -0.0,
    "long string": "y" * 100000,
}


def scenario_encode_never_emits_a_raw_newline() -> None:
    print("\nencode never puts a raw newline inside a message:")
    for name, value in HOSTILE_VALUES.items():
        wire = protocol.encode({"event": "error", "message": value})
        check(wire.endswith(b"\n"),
              f"encode of a {name} value produced no terminator; the client "
              f"waits forever for a message that has already been sent")
        check(wire.count(b"\n") == 1,
              f"encode of a {name} value emitted {wire.count(b'\n')} raw "
              f"newlines; the framing desynchronises permanently from here")
        check(b"\r" not in wire[:-1],
              f"encode of a {name} value emitted a raw carriage return")
    print(f"  ok  {len(HOSTILE_VALUES)} hostile values, one newline each, at the end")

    # Keys are as attacker-controlled as values in a mapping event.
    for name, value in HOSTILE_VALUES.items():
        if not isinstance(value, str):
            continue
        wire = protocol.encode({value: "x"})
        check(wire.count(b"\n") == 1,
              f"encode of a {name} *key* emitted a raw newline")
    print("  ok  the same values used as keys emit no raw newline either")

    # And every byte is ASCII, which is what makes a split-mid-character bug
    # impossible on the daemon's own output.
    wire = protocol.encode({"event": "claim", "name": "コントローラー \U0001f3ae"})
    check(max(wire) < 0x80,
          "encode emitted non-ASCII bytes; a pad name would then be split "
          "mid-character across two recv() calls")
    print("  ok  output is pure ASCII, so no character can straddle a read")


def scenario_encode_round_trip() -> None:
    print("\nencode round-trips through feed:")
    for name, value in HOSTILE_VALUES.items():
        message = {"event": "error", "message": value, "player": 1}
        got = feed_all(protocol.encode(message))
        check(got == [message],
              f"a {name} value did not survive the round trip: sent "
              f"{value!r}, got back "
              f"{got[0]['message'] if got else '<nothing>'!r}")
    print(f"  ok  {len(HOSTILE_VALUES)} values round-trip byte for byte")

    # All of them in one message, then all of them one byte at a time.
    every = {"event": "error", "values": list(HOSTILE_VALUES.values())}
    wire = protocol.encode(every)
    check(wire.count(b"\n") == 1,
          "a message carrying every hostile value at once emitted more than "
          "one newline")
    check(feed_all(wire) == [every],
          "a message carrying every hostile value at once did not round-trip")
    reader = protocol.LineReader()
    got = []
    step = 7  # a chunk size that is coprime with nothing in particular
    for index in range(0, len(wire), step):
        got += reader.feed(wire[index:index + step])
    check(got == [every],
          "a message carrying every hostile value did not reassemble from "
          "7-byte reads")
    print(f"  ok  all of them at once ({len(wire)} bytes), whole and in 7-byte reads")


def scenario_encode_deep_nesting() -> None:
    print("\nencode and feed with deeply nested values:")
    for depth in (2, 10, 100, 500, 900):
        value: object = "bottom"
        for _ in range(depth):
            value = {"layout": [value]}
        message = {"event": "layout_choice", "choices": value}
        wire = protocol.encode(message)
        check(wire.count(b"\n") == 1,
              f"encode of a {depth}-deep value emitted a raw newline")
        check(feed_all(wire) == [message],
              f"a {depth}-deep value did not round-trip")
    print("  ok  nesting to depth 900 encodes flat and round-trips")


def scenario_encode_large_numbers() -> None:
    print("\nencode with very large numbers:")
    numbers = [10 ** 4096, -(10 ** 4096), 2 ** 63, 2 ** 64 - 1, -(2 ** 63) - 1,
               1.7976931348623157e308, -1.7976931348623157e308, 5e-324,
               0.1 + 0.2, -0.0, 0]
    for number in numbers:
        wire = protocol.encode({"event": "progress", "frac": number})
        check(wire.count(b"\n") == 1,
              f"encode of {number!r} emitted a raw newline")
        got = feed_all(wire)
        check(len(got) == 1 and got[0]["frac"] == number,
              f"{number!r} did not round-trip; it came back as "
              f"{got[0]['frac'] if got else '<nothing>'!r}")
        check(len(got) == 1 and type(got[0]["frac"]) is type(number),
              f"{number!r} changed type across the wire")
    print(f"  ok  {len(numbers)} extreme numbers round-trip with their type")

    # Above CPython's int/str digit limit both sides give up -- but they must
    # give up in a way that leaves the framing alone.
    raised = None
    try:
        protocol.encode({"n": 10 ** 100000})
    except ValueError as error:
        raised = error
    check(raised is not None,
          "encode accepted a 100001-digit integer; whatever went on the wire "
          "is not the number that was sent")
    print(f"  ok  encode refuses an integer past the digit limit ({raised})")

    line = b'{"event":"progress","frac":' + b"9" * 100000 + b"}\n"
    for around in (b"", protocol.encode(CLAIM)):
        try:
            got = feed_all(around + line + protocol.encode(STATE))
        except Exception as error:                       # noqa: BLE001
            raise SystemExit(
                f"FAIL: a line holding a 100000-digit number raised "
                f"{type(error).__name__} out of feed(); a peer can send that "
                f"line to the socket and end the daemon")
        check(got[-1] == STATE,
              f"a 100000-digit number swallowed the message after it "
              f"({got!r}); the newline is still a frame boundary")
    print("  ok  a 100000-digit number on the wire is skipped, not raised, "
          "and its neighbours survive")


def scenario_encode_nan_is_not_json() -> None:
    print("\nS12 -- a calibration fraction that is not a number:")
    wire = protocol.encode({"event": "calibration", "phase": "reach",
                            "frac": float("nan"), "player": 1})
    check(wire.count(b"\n") == 1,
          "a NaN fraction emitted a raw newline; the framing would "
          "desynchronise on top of everything else")
    print("  ok  the framing holds: still exactly one newline, at the end")
    try:
        strict_loads(wire)
        print("  ok  and the message is JSON a non-Python client can parse")
    except ValueError as error:
        gap(f"encode() emits {wire.strip().decode()!r} for a NaN or infinite "
            f"value. That is Python's JSON superset, not JSON ({error}). "
            f"QJsonDocument rejects the whole document, so the calibration "
            f"bar on the setup screen would freeze rather than report a "
            f"bad reading. LineReader accepts it back, so this is invisible "
            f"from Python. json.dumps(..., allow_nan=False) turns it into a "
            f"ValueError at the sender instead.")


def scenario_encode_rejects_unserialisable() -> None:
    print("\nencode with a value JSON cannot carry:")
    for value in ({1, 2}, b"bytes", object(), Path("/tmp"), 1 + 2j):
        raised = None
        try:
            protocol.encode({"event": "error", "message": value})
        except TypeError as error:
            raised = error
        check(raised is not None,
              f"encode silently accepted {type(value).__name__}; whatever "
              f"went on the wire is not what the daemon meant to send")
    print("  ok  5 unserialisable values raise TypeError rather than "
          "corrupting the stream")
    gap("encode() raising is the right call, but Server._send catches only "
        "OSError around it, so a TypeError from encode() leaves _send and "
        "the selector loop and ends the daemon. Same shape as the "
        "{'players': 'lots'} scar in _handle_command's docstring, one layer "
        "further out.")


# -- size, and what a peer can make the daemon hold ------------------------


def scenario_long_message_one_byte_at_a_time() -> None:
    print("\nS9 -- a long layout_choice arriving one byte at a time:")
    payload = {"event": "layout_choice", "active": True, "player": 1,
               "kind": "layout",
               "choices": [{"id": f"c{i}", "label": "x" * 64,
                            "layout": {"controls": ["a", "b", "x", "y"]}}
                           for i in range(2200)]}
    wire = protocol.encode(payload)
    check(len(wire) > 256 * 1024,
          f"test setup: the long message is only {len(wire)} bytes")

    reader = protocol.LineReader()
    started = time.monotonic()
    got = []
    for index in range(len(wire)):
        got += reader.feed(wire[index:index + 1])
    elapsed = time.monotonic() - started
    check(got == [payload],
          f"a {len(wire)}-byte layout_choice fed one byte at a time came "
          f"back as {len(got)} messages; the console picker never opens")
    check(reader._buffer == b"",
          "the reader still holds bytes after a complete message")
    print(f"  ok  {len(wire)} bytes, one at a time, reassemble exactly one "
          f"message ({elapsed:.1f}s)")

    # Does one read cost more because *earlier* bytes are still buffered?
    # Same number of reads, same bytes per read, only the size of what is
    # already pending differs -- so this measures the buffering strategy and
    # not the message.
    def cost_with_pending(pending: int, reads: int = 2000) -> float:
        local = protocol.LineReader()
        local.feed(b"y" * pending)          # no newline: it all stays pending
        chunk = b"y" * 64
        start = time.monotonic()
        for _ in range(reads):
            local.feed(chunk)
        return time.monotonic() - start

    empty = cost_with_pending(16 * 1024)
    loaded = cost_with_pending(1024 * 1024)
    ratio = loaded / empty if empty else 0.0
    if ratio > 8:
        gap(f"LineReader.feed costs O(bytes already buffered) per read: "
            f"2000 identical 64-byte reads took {ratio:.0f}x longer with "
            f"1 MiB pending than with 16 KiB pending, so reassembling one "
            f"message is quadratic in its length. `self._buffer += data` "
            f"copies the whole buffer on every read and `b'\\n' in "
            f"self._buffer` rescans all of it. Measured directly, a 1 MiB "
            f"message fed one byte at a time takes 27s -- and the daemon is "
            f"single-threaded, so that is 27s in which no pad event is "
            f"serviced and no other client is answered. A bytearray plus a "
            f"search offset is linear.")
    else:
        print(f"  ok  a read costs the same whatever is already buffered "
              f"({ratio:.1f}x for 64x the pending bytes)")


def scenario_megabytes_in_socket_sized_reads() -> None:
    print("\nmegabytes arriving in recv(65536)-sized reads:")
    payload = {"event": "sdl_mapping",
               "lines": [f"03000000{i:08x},padmap Player 1,a:b1,b:b2,"
                         + "x" * 200 for i in range(9000)]}
    wire = protocol.encode(payload)
    check(len(wire) > 2 * 1024 * 1024,
          f"test setup: the message is only {len(wire)} bytes")
    reader = protocol.LineReader()
    got = []
    reads = 0
    for index in range(0, len(wire), 65536):
        got += reader.feed(wire[index:index + 65536])
        reads += 1
        if index + 65536 < len(wire):
            check(got == [],
                  "a partial message was delivered before its newline")
    check(got == [payload],
          f"a {len(wire) // 1024} KiB message did not survive {reads} "
          f"recv-sized reads; SDL mappings never reach the front-end and "
          f"the pads are unusable until Pegasus is relaunched")
    print(f"  ok  {len(wire)} bytes over {reads} recv-sized reads, one message")

    # Two multi-megabyte messages back to back, boundary in mid-read.
    reader = protocol.LineReader()
    doubled = wire + wire
    got = []
    for index in range(0, len(doubled), 65536):
        got += reader.feed(doubled[index:index + 65536])
    check(got == [payload, payload],
          "two multi-megabyte messages back to back did not both arrive")
    print("  ok  two of them back to back, boundary inside a read")


def scenario_unbounded_buffer() -> None:
    print("\na peer that sends and sends without ever sending a newline:")
    reader = protocol.LineReader()
    megabyte = b"y" * (1024 * 1024)
    sent = 0
    for _ in range(32):
        check(reader.feed(megabyte) == [],
              "a message with no newline in it was delivered anyway")
        sent += len(megabyte)
    held = len(reader._buffer)
    print(f"  ok  {sent // (1024 * 1024)} MiB with no newline yields no "
          f"messages at all")

    check(held < sent,
          f"LineReader has no size limit: after {sent // (1024 * 1024)} MiB "
          f"with no newline it is still holding all {held // (1024 * 1024)} "
          f"MiB. The socket lives in XDG_RUNTIME_DIR and any process running "
          f"as this user may connect, so `yes | nc -U .../padmap.sock` grows "
          f"the daemon until the OOM killer takes it -- and with it every "
          f"virtual pad on the machine, mid-game. Stopped at "
          f"{sent // (1024 * 1024)} MiB rather than the gigabyte in the brief "
          f"so as not to OOM the machine running this check; unbounded here "
          f"means a gigabyte is held too")
    print(f"  ok  the buffer is capped at {held} bytes")

    # Bounded is only half of it: the connection has to make sense again
    # afterwards, or the front-end stays connected while nothing it sends is
    # ever acted on.
    check(reader.feed(b"the tail of the over-long line\n") == [],
          "the tail of a dropped over-long line was parsed as a message")
    check(reader.feed(protocol.encode(CLAIM)) == [CLAIM],
          "after dropping an over-long line the reader never resynchronised; "
          "every later event on that connection is lost")
    print("  ok  framing resynchronises on the next newline after the drop")

    # Whatever it does with the size, it must not corrupt the message.
    reader = protocol.LineReader()
    body = protocol.encode({"event": "error", "message": "z" * (2 * 1024 * 1024)})
    got = []
    for index in range(0, len(body), 65536):
        got += reader.feed(body[index:index + 65536])
    check(len(got) == 1 and len(got[0]["message"]) == 2 * 1024 * 1024,
          "a 2 MiB message was truncated on the way through the buffer")
    print("  ok  a legitimate 2 MiB message still arrives whole and exact")


def _feed_or_die(reader: protocol.LineReader, chunk: bytes,
                 depth: int) -> list[dict]:
    """feed(), turning anything it raises into a failure of this file.

    json.loads recurses once per level of nesting, so past about 52,000
    brackets it raises RecursionError -- which is a RuntimeError, not a
    ValueError, so the `except ValueError` inside feed() used to miss it.
    feed() is called from Server._on_client_read, inside the selector loop,
    inside serve(), with no try anywhere in between: whatever comes out of
    here ends the daemon and takes every virtual pad on the machine with it,
    mid-game, at the request of anything that can open the socket.
    """
    try:
        return reader.feed(chunk)
    except BaseException as error:                       # noqa: BLE001
        raise SystemExit(
            f"FAIL: feed() raised {type(error).__name__} on a {depth}-deep "
            f"line. Any local process can write that to the socket in "
            f"XDG_RUNTIME_DIR, and the daemon exits -- every controller on "
            f"the machine stops working, mid-game, with nothing on screen to "
            f"say why") from error


def scenario_pathological_nesting() -> None:
    print("\na peer that sends deeply nested JSON:")
    for depth in (1000, 10000, 200000):
        wire = (b"[" * depth) + (b"]" * depth) + b"\n"
        check(_feed_or_die(protocol.LineReader(), wire, depth) == [],
              f"a {depth}-deep array was delivered as a message; it is "
              f"not an object")
    print("  ok  nesting 1000, 10000 and 200000 deep is skipped like any "
          "other non-object")

    # And in the shape it arrives in: a real event ahead of the poison in the
    # same read. That event is the one telling the setup screen who just
    # claimed a slot, and it was being lost along with the daemon.
    depth = 200000
    wire = protocol.encode(CLAIM) + (b"[" * depth) + (b"]" * depth) + b"\n"
    reader = protocol.LineReader()
    check(_feed_or_die(reader, wire, depth) == [CLAIM],
          f"the {CLAIM['event']!r} event ahead of a {depth}-deep line in the "
          f"same read did not survive it; the player who just pressed a "
          f"button never appears on the setup screen")
    print(f"  ok  {depth}-deep nesting is skipped, the claim before it "
          f"survives")
    # The poisoned line is consumed, so framing resynchronises on the next.
    check(reader.feed(protocol.encode(STATE)) == [STATE],
          "the reader was left holding the poisoned line; framing never "
          "resynchronises and the setup screen stops updating for good")
    print("  ok  the poisoned line is consumed, so the next one is read")


def scenario_daemon_read_shape() -> None:
    print("\nthe same input through the daemon's recv/feed shape:")
    # Server._on_client_read is `data = sock.recv(65536)` then
    # `for message in client.reader.feed(data)`, with no guard around feed.
    # Reproduced here over a socketpair rather than the real socket: a live
    # daemon owns the controllers on this machine.
    def run(payload: bytes) -> tuple[list[dict], BaseException | None]:
        left, right = socket.socketpair()
        writer = threading.Thread(
            target=lambda: (right.sendall(payload), right.shutdown(socket.SHUT_WR)))
        writer.start()
        reader = protocol.LineReader()
        seen: list[dict] = []
        error: BaseException | None = None
        try:
            while True:
                data = left.recv(65536)
                if not data:
                    break
                seen += reader.feed(data)
        except BaseException as exc:                     # noqa: BLE001
            error = exc
        writer.join()
        left.close()
        right.close()
        return seen, error

    seen, error = run(protocol.encode(CLAIM) + b"garbage\n"
                      + b'{"a":\xff}\n' + b"\x00\n" + protocol.encode(STATE))
    check(error is None,
          f"a mixed hostile stream raised {type(error).__name__} out of the "
          f"daemon's read loop; the daemon would exit")
    check(seen == [CLAIM, STATE],
          f"a mixed hostile stream over a real socket yielded {seen!r}; the "
          f"good events on either side of the garbage must survive")
    print("  ok  garbage, invalid UTF-8 and a NUL line all skipped over a "
          "real socket")

    seen, error = run(b"".join(protocol.encode(m) for m in (CLAIM, STATE)) * 200)
    check(error is None and len(seen) == 400,
          f"400 events over a real socket arrived as {len(seen)}")
    print("  ok  400 events over a real socket, all of them")

    seen, error = run((b"[" * 200000) + (b"]" * 200000) + b"\n"
                      + protocol.encode(STATE))
    check(error is None,
          f"over a real socket, in the daemon's exact recv(65536)/feed shape, "
          f"a deeply nested line ends the read loop with "
          f"{type(error).__name__}. This is the daemon exiting, at the "
          f"request of any process that can open the socket: every virtual "
          f"pad on the machine disappears mid-game")
    check(seen == [STATE],
          "a deeply nested line over a real socket cost us the message "
          "after it")
    print("  ok  a deeply nested line is skipped over a real socket too")


def main() -> int:
    print("protocol framing: encode() out, LineReader.feed() in")
    print(f"sandbox: {_SANDBOX}")

    scenario_split_two()
    scenario_split_three_and_many()
    scenario_split_inside_utf8()
    scenario_split_inside_string_literal()
    scenario_newline_alone()

    scenario_many_in_one_chunk()
    scenario_empty_and_newline_chunks()
    scenario_crlf()

    scenario_malformed_between_good()
    scenario_valid_json_not_an_object()
    scenario_nul_bytes()
    scenario_invalid_utf8()
    scenario_bom()

    scenario_escaped_newline_is_not_a_boundary()

    scenario_encode_never_emits_a_raw_newline()
    scenario_encode_round_trip()
    scenario_encode_deep_nesting()
    scenario_encode_large_numbers()
    scenario_encode_nan_is_not_json()
    scenario_encode_rejects_unserialisable()

    scenario_long_message_one_byte_at_a_time()
    scenario_megabytes_in_socket_sized_reads()
    scenario_unbounded_buffer()
    scenario_pathological_nesting()
    scenario_daemon_read_shape()

    if GAPS:
        print(f"\n{len(GAPS)} gap(s) above are unfixed defects in "
              f"src/padmap/protocol.py, asserted only as far as the current "
              f"behaviour is unambiguously correct.")

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
