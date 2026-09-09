"""What the once-a-second controller poll is allowed to cost.

`Server._poll_new_controllers` runs from `_tick`, on the single selector
thread that forwards every physical controller event to its virtual pad.
Anything slow in there is felt as input lag, and it was. Measured on the real
machine by reading the virtual pad for ten seconds while the stick moved:

    1695 events over 10.0s -> 169/s
    gap ms: median 7.94  mean 5.91  max 176.0
      gaps > 50ms: 11
      gaps >100ms: 5      (104, 128, 128, 160, 176)

A healthy 7.94ms cadence punctuated by a stall of 80-176ms about once a
second. The stall was the poll: `devices.discover()` spawns `udevadm info`
**as a subprocess per input device** to read ID_INPUT_JOYSTICK, and that
machine has 32 input devices.

    devices.discover()      : 257.3 ms   (7 pads)
    signature + has_mapping :   1.0 ms
    repeat: 276.6 / 274.0 / 293.9 ms

So the fix is a cheap signal first -- the set of /dev/input/event* node names
plus the prompted file's mtime -- and the full scan only when that changed.
This file is therefore a PERFORMANCE test as much as a behavioural one: it
counts calls to `discover` rather than trusting that the poll "looks fast",
and it checks the two ordering rules that make skipping safe.

The one that is easy to get wrong: the signature must be recorded *after*
the blocked test, never before. Recording it while blocked means a
controller plugged in during a game is never noticed once the game ends --
the next look sees nothing changed and skips it for good.

Runs against a real Server with `devices.discover`, `controllercfg.has_mapping`
and `server._event_nodes` stubbed, so nothing here enumerates or grabs the
real controllers a live daemon on this machine is holding.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_poll_cost.py
"""

import os
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

# Every scrap of state this touches must be redirected before padmap is
# imported and before any Server is built: a real daemon is running on this
# machine and its runtime dir holds the socket, the prompted record and the
# playing marker.
_STORE = Path(tempfile.mkdtemp(prefix="padmap-pollcost-"))
os.environ["XDG_RUNTIME_DIR"] = str(_STORE / "run")
os.environ["XDG_CONFIG_HOME"] = str(_STORE / "config")
os.environ["XDG_DATA_HOME"] = str(_STORE / "data")
os.environ["PADMAP_PROFILE_DIR"] = str(_STORE / "devices")
for _sub in ("run/padmap", "config", "data", "devices"):
    (_STORE / _sub).mkdir(parents=True, exist_ok=True)

from padmap import controllercfg, devices, profiles, protocol, server  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.protocol import STATE_ASSIGNING, STATE_IDLE  # noqa: E402

# What the real thing cost, from the capture quoted above. Used only as the
# yardstick the cheap path has to fit inside many times over.
DISCOVER_MS = 257.0

COUNTS = {"discover": 0, "has_mapping": 0, "signature": 0}
PADS: list[Pad] = []
CONFIGURED: set[str] = set()

_real_signature = profiles.signature
_real_discover = devices.discover
_real_has_mapping = controllercfg.has_mapping


def pad(name: str, vid: int = 0x1234, pid: int = 0x0001, node: str = "event90"):
    return Pad(path=f"/dev/input/{node}", name=name, phys="", uniq="",
               vid=vid, pid=pid, syspath="")


def _counting_discover(*_a, **_k):
    COUNTS["discover"] += 1
    return list(PADS)


def set_pads(pads, configured=()):
    """Change what a scan would find, mid-check."""
    global PADS
    PADS = list(pads)
    CONFIGURED.clear()
    CONFIGURED.update(_real_signature(p) for p in configured)


def _counting_has_mapping(target):
    COUNTS["has_mapping"] += 1
    return _real_signature(target) in CONFIGURED


def _counting_signature(target):
    COUNTS["signature"] += 1
    return _real_signature(target)


class Harness:
    """A Server whose two expensive calls are counted instead of made.

    `nodes` stands in for /dev/input: assigning to it is how a check says "a
    controller was plugged in" without touching any hardware.
    """

    def __init__(self, pads=(), *, state=STATE_IDLE, clients=1,
                 client_age=60.0, nodes=("event0", "event1"),
                 configured=False, keep_prompted=False):
        # A pad that has been through the wizard is silently republished, so
        # a scan finds nothing fresh and writes nothing. That keeps a check
        # about *when* a scan happens from also being a check about
        # prompting, which rewrites the very file the signal watches.
        set_pads(pads, pads if configured else ())
        # The prompted record outlives a Server -- deliberately, it is a file
        # in XDG_RUNTIME_DIR -- so a scenario that prompted earlier would
        # otherwise silence every later one and they would all pass for the
        # wrong reason.
        if not keep_prompted:
            protocol.prompted_path().unlink(missing_ok=True)
        self.nodes = set(nodes)
        self.srv = server.Server()
        self.srv._state = state
        self.srv._clients = {                       # type: ignore[assignment]
            n: type("FakeClient", (), {
                "connected_at": time.monotonic() - client_age})()
            for n in range(clients)
        }
        self.began: list[int] = []
        self.events: list[dict] = []
        self.srv._begin = lambda players: self.began.append(players)  # type: ignore[assignment]
        self.srv._broadcast = lambda message: self.events.append(message)  # type: ignore[assignment]
        server._event_nodes = lambda: frozenset(self.nodes)  # type: ignore[assignment]
        self.reset_counts()

    def reset_counts(self):
        for key in COUNTS:
            COUNTS[key] = 0
        # What a *priming* poll did is not what the check is asking about.
        self.began.clear()
        self.events.clear()
        return self

    def poll(self, times=1, rate_limited=False):
        """Poll, defeating the rate limit unless the check is about it."""
        for _ in range(times):
            if not rate_limited:
                self.srv._last_pad_scan = 0.0
            self.srv._poll_new_controllers()
        return self

    def plug(self, node="event42"):
        self.nodes.add(node)
        return self

    def unplug(self, node):
        self.nodes.discard(node)
        return self


def start_game():
    """Pretend padmap-play is holding the marker for a live game."""
    marker = protocol.playing_marker()
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.write_text(f"{os.getpid()}\n")   # a pid that certainly exists
    return marker


def end_game():
    protocol.playing_marker().unlink(missing_ok=True)


def touch_prompted(harness, content="somebody:else\n", when=None):
    """Rewrite the prompted file with a deliberately distinct mtime.

    `padmap forget` clears this file so a controller is offered setup again,
    which changes nothing about what is plugged in. Setting the timestamp by
    hand rather than trusting the clock keeps the check off the filesystem's
    mtime granularity.
    """
    path = harness.srv.prompted_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    stamp = when if when is not None else int(time.time_ns()) + 10**9
    os.utime(path, ns=(stamp, stamp))
    return stamp


def expect(label, *, discovers, harness=None, began=None, signature=None):
    """Assert the poll cost, and optionally what it decided."""
    if COUNTS["discover"] != discovers:
        raise SystemExit(
            f"FAIL {label}: devices.discover ran {COUNTS['discover']} time(s), "
            f"expected {discovers}. Each one spawns udevadm per input device "
            f"(~{DISCOVER_MS:.0f}ms on the real machine) on the single thread "
            f"that forwards controller events -- the user feels it as the "
            f"stick going unresponsive for a fifth of a second.")
    if discovers == 0 and (COUNTS["has_mapping"] or COUNTS["signature"]):
        raise SystemExit(
            f"FAIL {label}: no scan ran, yet {COUNTS['signature']} profile "
            f"signature(s) and {COUNTS['has_mapping']} profile parse(s) "
            f"happened -- a skipped poll must read nothing off disk at all.")
    if began is not None and bool(harness.began) != began:
        want = "open setup" if began else "stay quiet"
        raise SystemExit(
            f"FAIL {label}: expected the poll to {want}, it did not")
    if signature is not None:
        recorded = harness.srv._last_scan_signature is not None
        if recorded != signature:
            what = ("recorded the signature it must not have"
                    if recorded else "failed to record the signature")
            raise SystemExit(
                f"FAIL {label}: the poll {what} -- see the ordering note in "
                f"_poll_new_controllers")
    print(f"  ok  {label}")


def main() -> int:
    os.environ.pop(server.ENV_NO_AUTOSETUP, None)
    end_game()
    devices.discover = _counting_discover           # type: ignore[assignment]
    controllercfg.has_mapping = _counting_has_mapping  # type: ignore[assignment]
    profiles.signature = _counting_signature        # type: ignore[assignment]
    original_nodes = server._event_nodes

    try:
        run_checks(original_nodes)
    finally:
        devices.discover = _real_discover           # type: ignore[assignment]
        controllercfg.has_mapping = _real_has_mapping  # type: ignore[assignment]
        server._event_nodes = original_nodes        # type: ignore[assignment]
        profiles.signature = _real_signature        # type: ignore[assignment]
        end_game()

    print("\nall checks passed")
    return 0


def run_checks(real_event_nodes) -> None:
    new = pad("Brand New Pad")

    # -- the cheap signal, on its own ------------------------------------
    print("the signal the poll decides on is a directory listing:")
    fake_listing = ["event0", "event3", "mice", "mouse0", "js0", "by-id"]
    real_listdir = os.listdir

    def listing(path, *a, **k):
        if str(path) == "/dev/input":
            return list(fake_listing)
        return real_listdir(path, *a, **k)

    server.os.listdir = listing                     # type: ignore[assignment]
    try:
        seen = real_event_nodes()
        if seen != frozenset({"event0", "event3"}):
            raise SystemExit(
                f"FAIL: _event_nodes returned {sorted(seen)} -- mice, js and "
                f"by-id entries are not evdev nodes and counting them would "
                f"make the signal answer a different question")
        print("  ok  only event* nodes count")

        # The signature is compared as a tuple, so a listing that came back in
        # a different order must not read as somebody plugging a pad in.
        fake_listing.reverse()
        if real_event_nodes() != seen:
            raise SystemExit(
                "FAIL: reordering the directory listing changed the signal -- "
                "the poll would do a quarter-second scan at random")
        print("  ok  order of the listing is irrelevant")

        def broken(path, *a, **k):
            if str(path) == "/dev/input":
                raise OSError("no /dev/input here")
            return real_listdir(path, *a, **k)

        server.os.listdir = broken                  # type: ignore[assignment]
        try:
            empty = real_event_nodes()
        except OSError as error:
            raise SystemExit(
                f"FAIL: an unreadable /dev/input threw {error!r} out of the "
                f"poll -- that is raised inside _tick, on the selector loop, "
                f"so the daemon dies and every controller stops working")
        if empty != frozenset():
            raise SystemExit(
                f"FAIL: an unreadable /dev/input answered {sorted(empty)} "
                f"instead of nothing")
        print("  ok  an unreadable /dev/input answers empty, it does not "
              "throw and kill the daemon")
    finally:
        server.os.listdir = real_listdir             # type: ignore[assignment]

    # -- the first look ---------------------------------------------------
    print("\nthe first poll after the daemon starts:")
    h = Harness([new])
    if h.srv._last_scan_signature is not None:
        raise SystemExit(
            "FAIL: a fresh Server claims to have already scanned -- the very "
            "first poll would skip and a controller already plugged in at "
            "boot would never be offered setup")
    h.poll()
    expect("scans, because nothing is known yet", discovers=1, harness=h,
           began=True, signature=True)

    # -- the whole point: an idle machine costs nothing -------------------
    print("\nnothing plugged, unplugged, or forgotten since:")
    h = Harness([new], configured=True).poll().reset_counts()
    h.poll()
    expect("a second poll does not scan", discovers=0, harness=h)

    h.reset_counts().poll(times=60)
    expect("nor do sixty more (a minute of idling)", discovers=0)

    stamp_before = h.srv._read_prompted_stamp()
    h.reset_counts().poll(times=5)
    if h.srv._read_prompted_stamp() != stamp_before:
        raise SystemExit(
            "FAIL: a poll that scanned nothing still rewrote the prompted "
            "file -- that write changes the mtime and makes the next poll "
            "scan, so the skip would never hold")
    expect("and they write nothing either", discovers=0)

    if h.srv._last_scan_signature != (frozenset(h.nodes),
                                      h.srv._read_prompted_stamp()):
        raise SystemExit(
            "FAIL: the recorded signature is not the one the next poll will "
            "compute, so the two can never compare equal and the scan is "
            "back to once a second")
    print("  ok  the recorded signature is exactly what the next poll builds")

    # -- but a real change must still get through -------------------------
    print("\na controller appearing (a new /dev/input/event node):")
    h = Harness([new], configured=True).poll().reset_counts()
    h.plug("event42").poll()
    expect("scans", discovers=1)

    h.reset_counts().poll(times=10)
    expect("once, then settles again", discovers=0)

    print("\na controller going away (a node disappearing):")
    h = Harness([new], nodes=("event0", "event1", "event7"),
                configured=True).poll().reset_counts()
    h.unplug("event7").poll()
    expect("scans too -- the signal is 'changed', not 'grew'", discovers=1)
    h.reset_counts().poll(times=10)
    expect("and settles", discovers=0)

    print("\nthe prompted file, which `padmap forget` clears:")
    # forget exists precisely to have a controller offered again while
    # nothing at all changes about what is plugged in, so the node set alone
    # would never notice it.
    h = Harness([new], configured=True).poll().reset_counts()
    touch_prompted(h)
    h.poll()
    expect("a changed mtime scans, with nothing plugged or unplugged",
           discovers=1)

    h.reset_counts()
    h.srv.prompted_path.unlink(missing_ok=True)
    h.poll()
    expect("deleting it outright scans as well", discovers=1)

    h.reset_counts().poll(times=10)
    expect("and then it is quiet again", discovers=0)

    print("\nboth halves of the signal are load-bearing:")
    h = Harness([new], configured=True).poll().reset_counts()
    touch_prompted(h)
    h.plug("event42")
    h.poll()
    expect("both changing at once is still one scan", discovers=1)

    # -- the ordering rule that a plausible-looking fix gets wrong ---------
    print("\nblocked polls must not record the signature:")
    # This is the bug the ordering exists to prevent. Recording before the
    # blocked test means the next unblocked look sees nothing changed, skips,
    # and the controller is never noticed at all.
    h = Harness([new], clients=0)          # no front-end connected
    h.poll()
    expect("no front-end: no scan", discovers=0, harness=h, began=False,
           signature=False)

    h.reset_counts().plug("event42").poll()
    expect("a pad plugged in while blocked: still no scan", discovers=0,
           harness=h, began=False, signature=False)

    h.srv._clients = {0: type("FakeClient", (), {  # a front-end settles
        "connected_at": time.monotonic() - 60.0})()}   # type: ignore[assignment]
    h.reset_counts().poll()
    expect("once a front-end connects, that pad IS found", discovers=1,
           harness=h, began=True, signature=True)

    print("\nthe same sequence with the reason that actually happens:")
    # A game running is the common case: someone plugs a second pad in
    # mid-game and expects setup when they quit back to the front-end.
    known = pad("Already Configured Pad", vid=0x2222)
    h = Harness([known], configured=True).poll().reset_counts()
    before = h.srv._last_scan_signature
    start_game()
    try:
        set_pads([known, new], configured=[known])   # the pad is plugged in
        h.plug("event42").poll()
        expect("a pad plugged in mid-game: no scan", discovers=0, harness=h,
               began=False)
        if h.srv._last_scan_signature != before:
            raise SystemExit(
                "FAIL: the mid-game poll recorded the new signature -- when "
                "the game ends the next poll sees nothing changed and skips "
                "for good, so a controller plugged in during a game is never "
                "noticed at all")
        print("  ok  and the signature is left alone")
    finally:
        end_game()
    h.reset_counts().poll()
    expect("quitting the game finds it", discovers=1, harness=h, began=True)

    print("\nand for a session that is already open:")
    h = Harness([new], state=STATE_ASSIGNING)
    h.plug("event42").poll()
    expect("no scan, no signature", discovers=0, harness=h, began=False,
           signature=False)
    h.srv._state = STATE_IDLE
    h.reset_counts().poll()
    expect("closing it finds the pad", discovers=1, harness=h, began=True)

    print("\nand for the escape hatch:")
    os.environ[server.ENV_NO_AUTOSETUP] = "1"
    try:
        h = Harness([new])
        h.plug("event42").poll(times=20)
        expect("PADMAP_NO_AUTOSETUP costs nothing per poll", discovers=0,
               harness=h, began=False, signature=False)
    finally:
        del os.environ[server.ENV_NO_AUTOSETUP]
    h.reset_counts().poll()
    expect("and turning it off again resumes scanning", discovers=1)

    # -- a whole game, polled the way _tick polls -------------------------
    print("\nacross a full game, on the thread carrying the input:")
    h = Harness([known], configured=True).poll().reset_counts()
    set_pads([known, new], configured=[known])
    h.plug("event42")            # something changed, so only the block stops it
    start_game()
    try:
        h.poll(times=600)        # ten minutes at one poll a second
        expect("600 polls mid-game, not one scan", discovers=0, harness=h,
               began=False)
    finally:
        end_game()
    h.reset_counts().poll()
    expect("and the pad is waiting when the game ends", discovers=1,
           harness=h, began=True)

    # -- the rate limit still holds ---------------------------------------
    print("\nthe rate limit, which is what keeps a real change to once a "
          "second:")
    if server.PAD_SCAN_SECONDS < 1.0:
        raise SystemExit(
            f"FAIL: PAD_SCAN_SECONDS is {server.PAD_SCAN_SECONDS}, so a scan "
            f"could run more often than once a second on the input thread")
    if server.TICK_SECONDS >= server.PAD_SCAN_SECONDS:
        raise SystemExit(
            "FAIL: the tick rate is not below the scan rate, so the limit "
            "cannot limit anything")
    print(f"  ok  PAD_SCAN_SECONDS={server.PAD_SCAN_SECONDS} above a "
          f"{server.TICK_SECONDS}s tick")

    h = Harness([new], configured=True).poll().reset_counts()
    h.plug("event42")
    h.poll(times=50, rate_limited=True)   # a second of ticks, straight after
    expect("a change arriving just after a look waits its turn", discovers=0)

    # Now let the second elapse and tick fifty more times.
    h.srv._last_pad_scan = time.monotonic() - (server.PAD_SCAN_SECONDS + 0.05)
    h.reset_counts().poll(times=50, rate_limited=True)
    expect("50 ticks spanning the limit: exactly one scan", discovers=1)

    h = Harness([new], configured=True).poll().reset_counts()
    h.plug("event42")
    h.srv._last_pad_scan = time.monotonic() - (server.PAD_SCAN_SECONDS * 0.5)
    h.srv._poll_new_controllers()
    expect("half a second after the last look: too soon, no scan",
           discovers=0)

    h.srv._last_pad_scan = time.monotonic() - (server.PAD_SCAN_SECONDS + 0.05)
    h.srv._poll_new_controllers()
    expect("a second after it: scans", discovers=1)

    # -- _tick is the caller, and must stay the caller --------------------
    print("\nthe poll is reached from the tick, not from anywhere else:")
    h = Harness([new], configured=True)
    h.srv._assigner = None
    h.srv._last_pad_scan = 0.0
    h.srv._tick()
    expect("_tick polls", discovers=1, harness=h)

    h.reset_counts()
    h.srv._last_pad_scan = 0.0
    h.srv._tick()
    expect("and the second tick costs nothing", discovers=0)

    # -- prompting settles, rather than rescanning for ever ---------------
    print("\nafter setup has been offered:")
    # _save_prompted rewrites the file, which is itself a change to the
    # signal. That may cost one more scan; it must not cost one a second.
    h = Harness([new]).poll()
    if not h.began:
        raise SystemExit("FAIL: the fixture pad was never offered setup")
    h.reset_counts().poll(times=10)
    if COUNTS["discover"] > 1:
        raise SystemExit(
            f"FAIL: {COUNTS['discover']} scans in ten polls after prompting "
            f"-- writing the prompted record re-triggers the signal on every "
            f"single poll, which is the once-a-second stall back again")
    print(f"  ok  settles after at most one more scan "
          f"({COUNTS['discover']} in ten polls)")

    print("\na controller that is already configured:")
    h = Harness([new], configured=True)
    h.poll()
    expect("scanned once and silently republished", discovers=1, harness=h,
           began=False, signature=True)
    h.reset_counts().poll(times=20)
    expect("and never scanned for again", discovers=0, harness=h, began=False)

    # -- the timing claim itself ------------------------------------------
    print("\nwhat the cheap path actually costs:")
    # Absolute bounds, sized against the 257ms a single discover() took, and
    # generous by an order of magnitude so a loaded machine cannot fail this.
    reps = 500
    start = time.perf_counter()
    for _ in range(reps):
        real_event_nodes()
    nodes_ms = (time.perf_counter() - start) * 1000.0
    if nodes_ms > DISCOVER_MS:
        raise SystemExit(
            f"FAIL: {reps} calls to _event_nodes took {nodes_ms:.1f}ms, more "
            f"than the ~{DISCOVER_MS:.0f}ms of the single discover() it "
            f"replaces -- the cheap signal is not cheap and the controller "
            f"lag is back")
    print(f"  ok  {reps} x _event_nodes in {nodes_ms:.1f}ms "
          f"({nodes_ms / reps:.3f}ms each) vs ~{DISCOVER_MS:.0f}ms for one "
          f"discover()")

    h = Harness([new])
    start = time.perf_counter()
    for _ in range(reps):
        h.srv._read_prompted_stamp()
    stamp_ms = (time.perf_counter() - start) * 1000.0
    if stamp_ms > DISCOVER_MS:
        raise SystemExit(
            f"FAIL: {reps} prompted-mtime reads took {stamp_ms:.1f}ms -- the "
            f"other half of the signal costs what the scan it avoids costs")
    print(f"  ok  {reps} x _read_prompted_stamp in {stamp_ms:.1f}ms")

    print("\nand the whole poll, with a discover() that costs what the real "
          "one costs:")
    # The end-to-end version of the claim. If the skip is ever lost, these
    # polls pick up a quarter-second each and the bound is missed by 5x --
    # which is exactly the stall the user reported feeling.
    slow_reps = 20

    def slow_discover(*_a, **_k):
        COUNTS["discover"] += 1
        time.sleep(DISCOVER_MS / 1000.0)
        return list(PADS)

    original = devices.discover
    devices.discover = slow_discover                # type: ignore[assignment]
    try:
        # Primed outside the clock: the first look always scans, and the pad
        # is a configured one so nothing is prompted and nothing is written.
        h = Harness([new], configured=True).poll().reset_counts()
        start = time.perf_counter()
        h.poll(times=slow_reps)
        idle_ms = (time.perf_counter() - start) * 1000.0
    finally:
        devices.discover = original                 # type: ignore[assignment]
    # Half of one discover, so a single scan sneaking in fails this outright
    # while leaving two orders of magnitude of headroom for a busy machine.
    budget = DISCOVER_MS / 2
    if COUNTS["discover"] or idle_ms > budget:
        raise SystemExit(
            f"FAIL: {slow_reps} idle polls took {idle_ms:.1f}ms over a "
            f"{budget:.0f}ms budget, with {COUNTS['discover']} scan(s) -- at "
            f"one poll a second that is a {idle_ms / slow_reps:.0f}ms freeze "
            f"of every controller, every second, exactly as reported")
    print(f"  ok  {slow_reps} idle polls in {idle_ms:.1f}ms "
          f"({idle_ms / slow_reps:.2f}ms each), against {DISCOVER_MS:.0f}ms "
          f"if even one had scanned")


if __name__ == "__main__":
    sys.exit(main())
