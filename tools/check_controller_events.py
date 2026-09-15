"""The `controller` event: when it fires, and whether it can be applied.

padmap published its configuration when a game launched and never again, so a
controller plugged in mid-game was invisible to whatever was playing -- the
only way to pick it up was to quit. This checks the event that closes that,
and the things that would make it useless:

  * **not firing during a game.** The whole point. The setup poller beside it
    is blocked during a game deliberately, and sharing that guard would have
    made this feature do nothing in the one situation it exists for.
  * **firing on something that cannot be bound.** A controller with no stored
    mapping has no clone and no binds; announcing it as `added` would have a
    consumer open a device node that is not there.
  * **an event that disagrees with itself.** The GUID is what an SDL consumer
    keys its mapping on. If it is computed from a different source than the
    vid/pid beside it the two can differ, the mapping is registered under a
    GUID SDL never looks up, and nothing reports an error.
  * **renumbering players.** A pad that drops and returns must return as the
    same player. Freeing its slot on unplug is how a four-player game becomes
    a three-player game with everyone shifted up one.
  * **announcing before the clone exists.** The event names a device node; it
    has to exist by the time anyone reads the message.

Runs against a real `server.Server` with the machine-touching parts stubbed.

    XDG_RUNTIME_DIR=$(mktemp -d) nix develop --command \
        python3 tools/check_controller_events.py
"""

import os
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import announce, controllercfg, devices, mapping  # noqa: E402
from padmap import profiles, protocol, server, virtual  # noqa: E402
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402

failures: list[str] = []


def check(name: str, condition: bool, detail: str = "") -> None:
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{(': ' + detail) if detail else ''}")
        failures.append(name)


def pad(n: int, name: str = "Test Pad", vid: int = 0x057E,
        pid: int = 0x2009) -> Pad:
    return Pad(path=f"/dev/input/event{n}", name=name, phys=f"usb-{n}",
               uniq=f"u{n}", vid=vid, pid=pid, syspath=f"/sys/dev{n}")


class FakeUi:
    def __init__(self, player: int) -> None:
        self.device = type("D", (), {"path": f"/dev/input/event{90 + player}"})()


class FakeVirtualPad:
    """Just the fields the event builder reads off a live clone."""

    def __init__(self, player: int, source_pad: Pad) -> None:
        self.player = player
        self.pad = source_pad
        self.ui = FakeUi(player)
        self.identity = virtual.Identity(
            vendor=source_pad.vid, product=source_pad.pid, bustype=3, version=1)


class FakeRepublisher:
    def __init__(self, pads) -> None:
        self.pads = pads


class Harness:
    """A Server whose scans and devices are ours to drive."""

    def __init__(self, *, present=(), assigned=(), mapped=(),
                 state=protocol.STATE_IDLE, playing=False):
        self.srv = server.Server()
        self.srv._state = state
        self.srv._assignments = [
            Assignment(player=i + 1, pad=p, button=0)
            for i, p in enumerate(assigned)
        ]
        self.srv._attached = {
            profiles.signature(p): i + 1 for i, p in enumerate(assigned)
        }
        self.events: list[dict] = []
        self.srv._broadcast = self.events.append  # type: ignore[assignment]
        self.srv._save_assignments = lambda: None  # type: ignore[assignment]
        self.srv._begin = lambda players: None  # type: ignore[assignment]

        self._scans = 0
        self._present = list(present)
        self._mapped = {profiles.signature(p) for p in mapped}
        self.republished = 0

        devices.discover = (  # type: ignore[assignment]
            lambda *a, **kw: list(self._present))
        controllercfg.has_mapping = (  # type: ignore[assignment]
            lambda p: profiles.signature(p) in self._mapped)
        protocol.game_is_running = lambda: playing  # type: ignore[assignment]

        def start() -> None:
            # Order matters: the clone has to exist before the event that
            # names it, so this stands in for the real creation and records
            # that it happened first.
            self.republished += 1
            self.srv._republisher = FakeRepublisher([  # type: ignore[assignment]
                FakeVirtualPad(a.player, a.pad) for a in self.srv._assignments
            ])

        self.srv._start_republisher = start  # type: ignore[assignment]

    def plug(self, *pads: Pad) -> "Harness":
        self._present.extend(pads)
        return self.scan()

    def unplug(self, *pads: Pad) -> "Harness":
        gone = {p.path for p in pads}
        self._present = [p for p in self._present if p.path not in gone]
        return self.scan()

    def scan(self) -> "Harness":
        # The poller returns early unless the node set changed; give it a
        # different one each time so every call is a real scan. The retry
        # throttle is wound back for the same reason -- it exists to stop a
        # failing attach spinning at tick rate, and here the ticks are ours.
        self.srv._last_attach_nodes = frozenset({str(self._scans), "x"})
        self._scans += 1
        self.srv._last_attach_scan = 0.0
        self.srv._poll_controller_changes()
        return self

    def of(self, action: str) -> list[dict]:
        return [e for e in self.events
                if e.get("event") == announce.EVENT
                and e.get("action") == action]


ONE = pad(1)
TWO = pad(2, name="Second Pad", pid=0x200A)
UNKNOWN = pad(3, name="Nobody Has Mapped This", vid=0x1234, pid=0x5678)


def check_fires_during_a_game() -> None:
    print("during a game")
    h = Harness(present=[ONE], assigned=[ONE], mapped=[ONE, TWO], playing=True)
    h.plug(TWO)
    added = h.of(announce.ACTION_ADDED)
    check("a controller plugged in mid-game is announced", len(added) == 1,
          f"{len(added)} added events")
    if added:
        check("it was given the next free player slot",
              added[0]["player"] == 2, str(added[0]["player"]))
        check("the event carries the clone's device node",
              added[0]["changed"]["virtual"]["node"] == "/dev/input/event92",
              added[0]["changed"]["virtual"]["node"])


def check_unconfigured_is_not_claimed() -> None:
    print("a controller nobody has mapped")
    h = Harness(present=[ONE], assigned=[ONE], mapped=[ONE])
    h.plug(UNKNOWN)
    check("it is announced as unconfigured",
          len(h.of(announce.ACTION_UNCONFIGURED)) == 1)
    check("it is not announced as added", not h.of(announce.ACTION_ADDED))
    check("it did not take a player slot",
          [a.player for a in h.srv._assignments] == [1])
    unconf = h.of(announce.ACTION_UNCONFIGURED)
    if unconf:
        changed = unconf[0]["changed"]
        check("it offers no virtual pad to open", "virtual" not in changed)
        check("it says it is unconfigured",
              changed["controller"]["configured"] is False)


def check_slots_are_not_renumbered() -> None:
    print("unplug and reconnect")
    h = Harness(present=[ONE, TWO], assigned=[ONE, TWO], mapped=[ONE, TWO])
    h.unplug(ONE)
    check("the departure is announced",
          len(h.of(announce.ACTION_REMOVED)) == 1)
    check("player 2 was not renumbered",
          [a.player for a in h.srv._assignments] == [1, 2])
    h.plug(ONE)
    added = h.of(announce.ACTION_ADDED)
    check("reconnecting returns as the same player",
          bool(added) and added[-1]["player"] == 1,
          str(added[-1]["player"]) if added else "no event")
    check("no third slot was invented", len(h.srv._assignments) == 2)


def check_a_gap_is_filled() -> None:
    print("a free slot in the middle")
    h = Harness(present=[TWO], mapped=[ONE, TWO])
    h.srv._assignments = [Assignment(player=2, pad=TWO, button=0)]
    h.srv._attached = {profiles.signature(TWO): 2}
    h.plug(ONE)
    added = h.of(announce.ACTION_ADDED)
    check("the empty slot is used, not a slot beyond it",
          bool(added) and added[-1]["player"] == 1,
          str(added[-1]["player"]) if added else "no event")


def check_a_session_is_not_disturbed() -> None:
    print("while the setup screen is open")
    h = Harness(present=[ONE], assigned=[ONE], mapped=[ONE, TWO],
                state=protocol.STATE_ASSIGNING)
    h.plug(TWO)
    check("nothing is announced", not h.events)
    check("no slot is claimed under the session",
          len(h.srv._assignments) == 1)


def check_event_is_self_consistent() -> None:
    print("the message itself")
    h = Harness(present=[ONE], assigned=[ONE], mapped=[ONE, TWO])
    h.plug(TWO)
    added = h.of(announce.ACTION_ADDED)
    if not added:
        check("an event was produced", False)
        return
    event = added[-1]
    virt = event["changed"]["virtual"]
    expected = mapping.sdl_guid(
        bus=int(virt["bustype"]), vendor=int(virt["vid"], 16),
        product=int(virt["pid"], 16), version=1, name=virt["name"])
    check("the GUID agrees with the ids printed beside it",
          virt["guid"] == expected, f"{virt['guid']} != {expected}")
    check("the roster holds every attached player",
          [e["player"] for e in event["roster"]] == [1, 2],
          str([e["player"] for e in event["roster"]]))
    check("the subject is named at the top level",
          event["player"] == 2)
    check("the build id is carried", bool(event.get("build")))
    check("the scope is stated", "scope" in event)
    check("retroarch binds are included",
          bool(event["changed"]["retroarch"]["binds"]))


def check_clone_exists_before_the_event() -> None:
    print("ordering")
    h = Harness(present=[ONE], mapped=[ONE])
    order: list[str] = []
    real_start = h.srv._start_republisher
    h.srv._start_republisher = lambda: (order.append("republish"),  # type: ignore[assignment]
                                        real_start())[1]
    h.srv._broadcast = lambda m: order.append("announce")  # type: ignore[assignment]
    h.plug(ONE)
    check("republish happens before the announcement",
          order[:2] == ["republish", "announce"], str(order))


def check_republish_failure_is_not_fatal() -> None:
    print("when the clone cannot be made")
    h = Harness(present=[ONE], mapped=[ONE])

    def boom() -> None:
        raise OSError("no uinput")

    h.srv._start_republisher = boom  # type: ignore[assignment]
    try:
        h.plug(ONE)
    except OSError:
        check("the daemon survives a failed republish", False, "OSError escaped")
        return
    check("the daemon survives a failed republish", True)
    check("nothing is announced as added", not h.of(announce.ACTION_ADDED))
    check("no player slot is left claimed for a pad with no clone",
          not h.srv._assignments, str(h.srv._assignments))
    check("it will be retried rather than recorded as attached",
          not h.srv._attached and h.srv._last_attach_nodes is None)


def check_a_late_permission_is_retried() -> None:
    """The race this was found by: a node exists before it is readable.

    udev applies the uaccess ACL after the node appears, so the first open of
    a freshly plugged controller can fail with EACCES and succeed a moment
    later. Seen for real against a live daemon.
    """
    print("a node that is not readable yet")
    h = Harness(present=[], mapped=[ONE])
    attempts = {"n": 0}
    real_start = h.srv._start_republisher

    def flaky() -> None:
        attempts["n"] += 1
        if attempts["n"] < 3:
            raise OSError(13, "Permission denied")
        real_start()

    h.srv._start_republisher = flaky  # type: ignore[assignment]
    h.plug(ONE)
    for _ in range(5):
        h.scan()
    added = h.of(announce.ACTION_ADDED)
    check("it is picked up once the permission lands", len(added) == 1,
          f"{len(added)} added after {attempts['n']} attempts")
    check("and lands in the first player slot",
          bool(added) and added[0]["player"] == 1)
    check("the attempt count is cleared on success",
          not h.srv._attach_attempts, str(h.srv._attach_attempts))


def check_retrying_gives_up() -> None:
    print("a node that never becomes readable")
    h = Harness(present=[], mapped=[ONE])

    def never() -> None:
        raise OSError(13, "Permission denied")

    h.srv._start_republisher = never  # type: ignore[assignment]
    h.plug(ONE)
    for _ in range(server.ATTACH_ATTEMPTS + 5):
        h.scan()
    unconf = h.of(announce.ACTION_UNCONFIGURED)
    check("it is eventually announced as unconfigured", len(unconf) == 1,
          f"{len(unconf)} events")
    if unconf:
        check("the reason distinguishes it from an unmapped pad",
              unconf[0].get("reason") == announce.REASON_UNREADABLE,
              str(unconf[0].get("reason")))
    check("it stops being retried",
          profiles.signature(ONE) in h.srv._unbindable)
    before = len(h.events)
    for _ in range(5):
        h.scan()
    check("and stays quiet afterwards", len(h.events) == before)


def check_replug_clears_the_give_up() -> None:
    print("replugging something that gave up")
    h = Harness(present=[], mapped=[ONE])
    h.srv._unbindable = {profiles.signature(ONE)}
    h.srv._attach_attempts = {profiles.signature(ONE): server.ATTACH_ATTEMPTS}
    h.plug(ONE)
    check("still skipped while it is the same appearance",
          not h.of(announce.ACTION_ADDED))
    h.unplug(ONE)
    h.plug(ONE)
    check("a fresh appearance is tried again",
          len(h.of(announce.ACTION_ADDED)) == 1,
          str(len(h.of(announce.ACTION_ADDED))))


def check_unmapped_reason() -> None:
    print("the two unconfigured reasons")
    h = Harness(present=[], mapped=[])
    h.plug(UNKNOWN)
    unconf = h.of(announce.ACTION_UNCONFIGURED)
    check("an unmapped pad says so",
          bool(unconf) and unconf[0].get("reason") == announce.REASON_UNMAPPED,
          str(unconf[0].get("reason")) if unconf else "no event")


def check_no_autoattach() -> None:
    print(f"with {server.ENV_NO_AUTOATTACH}=1")
    os.environ[server.ENV_NO_AUTOATTACH] = "1"
    try:
        h = Harness(present=[ONE], mapped=[ONE, TWO])
        h.plug(TWO)
        check("no slot is claimed", not h.srv._assignments)
        check("nothing is announced as added",
              not h.of(announce.ACTION_ADDED))
    finally:
        del os.environ[server.ENV_NO_AUTOATTACH]


def check_next_player() -> None:
    print("slot arithmetic")
    check("empty -> 1", announce.next_player([]) == 1)
    check("gap is filled", announce.next_player([1, 3]) == 2)
    check("appends past a full run", announce.next_player([1, 2, 3]) == 4)
    check("order does not matter", announce.next_player([3, 1]) == 2)
    check("duplicates do not shift it", announce.next_player([1, 1, 2]) == 3)


def check_idle_scan_is_quiet() -> None:
    print("a scan with nothing new")
    h = Harness(present=[ONE], assigned=[ONE], mapped=[ONE])
    h.scan()
    h.scan()
    check("a settled machine announces nothing", not h.events,
          str(h.events[:1]))


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        os.environ["XDG_RUNTIME_DIR"] = tmp
        for fn in (check_fires_during_a_game,
                   check_unconfigured_is_not_claimed,
                   check_slots_are_not_renumbered,
                   check_a_gap_is_filled,
                   check_a_session_is_not_disturbed,
                   check_event_is_self_consistent,
                   check_clone_exists_before_the_event,
                   check_republish_failure_is_not_fatal,
                   check_a_late_permission_is_retried,
                   check_retrying_gives_up,
                   check_replug_clears_the_give_up,
                   check_unmapped_reason,
                   check_no_autoattach,
                   check_next_player,
                   check_idle_scan_is_quiet):
            fn()
    if failures:
        print(f"\n{len(failures)} failure(s): {', '.join(failures)}")
        return 1
    print("\nall controller-event checks pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
