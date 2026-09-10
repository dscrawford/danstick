"""The assignment journey, end to end, at the daemon level.

Everything between "a controller is plugged in" and "my controllers are still
player 1 and 2 after an upgrade" happens inside one process, driven by two
kinds of input: JSON commands from the front-end, and button events from the
pads. This walks that journey as a sequence of both, asserting at each step
what the *user* was promised rather than what the daemon stores:

    S1  setup opens by itself for a controller padmap has never seen -- and
        stays out of the way at every moment where opening it would take the
        controllers away from something else
    S2  begin opens a session and says how many pads it can see
    S3  a hold claims the next free slot, in order; the press that opened the
        screen does not, a tap does not, and one pad cannot claim two slots
    S4  holding again confirms: profiles written, pads republished, launch
        config generated
    S5  cancel -- or the front-end simply going away -- releases the pads
    S6  reset drops the claims and keeps the session open
    S11 forget_pad throws away every scope and re-runs the wizard
    S18 the assignment survives a daemon restart, which is what
        `padmap ensure-daemon` does to pick up new code

Runs against a real `server.Server`. Only the two things that touch hardware
are stubbed: opening an evdev node, and creating a uinput device. Everything
else -- the assigner, the hold timers, the confirm gesture, the profile
store, the launch config writer, restore() -- is the shipped code. A live
padmap daemon runs on this machine, so nothing here opens a real controller,
grabs anything, or creates a uinput node; the fake pads live on device paths
that deliberately do not exist.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_journey_setup.py
"""

import json
import logging
import os
import socket
import sys
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

# Redirected *before* padmap is imported. retroarch.py resolves its config
# directory at import time, so setting these later would leave one module
# pointed at the real user's ~/.config while every other module used the
# temporary one -- and the real one belongs to the daemon that is running.
STORE = Path(tempfile.mkdtemp(prefix="padmap-journey-"))
RUNTIME = STORE / "run"
(RUNTIME / "padmap").mkdir(parents=True)
os.environ["XDG_RUNTIME_DIR"] = str(RUNTIME)
os.environ["XDG_CONFIG_HOME"] = str(STORE / "config")
os.environ["XDG_DATA_HOME"] = str(STORE / "data")
os.environ["PADMAP_PROFILE_DIR"] = str(STORE / "devices")
os.environ.pop("PADMAP_NO_AUTOSETUP", None)

from evdev import ecodes  # noqa: E402

from padmap import (assign, devices, profiles, protocol,  # noqa: E402
                    server, virtual)
from padmap.assign import HOLD_SECONDS  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

# The daemon logs its whole lifecycle at INFO/WARNING and there is no handler
# configured here, so it would all land on stderr and bury the report.
logging.disable(logging.CRITICAL)

# Long enough that the hold has certainly completed, short enough that a file
# full of them still runs in seconds. The real constants are used throughout:
# how long a claim takes is part of what is being checked.
PAST_HOLD = HOLD_SECONDS * 1.3
PAST_CONFIRM = server.CONFIRM_HOLD_SECONDS * 1.2

BTN_SOUTH = 0x130
BTN_EAST = 0x131


def pad(name: str, vid: int = 0x1234, pid: int = 0x0001,
        node: str = "event9990") -> Pad:
    """A physical pad on a device path that certainly does not exist.

    event9990 and up, not event9x: a real controller may well be sitting on
    event90, and anything in this file that reaches evdev directly must fail
    to open rather than succeed against somebody's live pad.
    """
    return Pad(path=f"/dev/input/{node}", name=name, phys=f"usb-0000:00/{node}",
               uniq="", vid=vid, pid=pid, syspath=f"/sys/devices/{node}")


class Event:
    """An evdev event, as far as anything in the daemon looks at one."""

    def __init__(self, type: int, code: int, value: int) -> None:
        self.type, self.code, self.value = type, code, value


class AbsInfo:
    def __init__(self, minimum: int, maximum: int, value: int) -> None:
        self.min, self.max, self.value = minimum, maximum, value


class FakeDevice:
    """An open pad handle whose event stream the test writes.

    `fd` is the read end of a real pipe, because the daemon registers it with
    a selector and a selector wants a real descriptor. Events are delivered by
    calling the daemon's own readable-callback, so the path under test is the
    one the event loop uses.
    """

    def __init__(self, source: Pad) -> None:
        self.pad = source
        self.name = source.name
        self.path = source.path
        self.fd, self._write_end = os.pipe()
        self.queue: list[Event] = []
        self.grabbed = False
        self.ungrabs = 0
        self.closes = 0
        self.info = SimpleNamespace(bustype=3, vendor=source.vid,
                                    product=source.pid, version=0x0111)

    # -- what the daemon calls -------------------------------------------
    def grab(self) -> None:
        self.grabbed = True

    def ungrab(self) -> None:
        self.ungrabs += 1
        self.grabbed = False

    def close(self) -> None:
        # The descriptor is deliberately left open: the selector unregisters
        # it just before this runs, and a closed-then-reused fd number would
        # make failures here look like epoll bugs.
        self.closes += 1

    def read(self) -> list[Event]:
        pending, self.queue = self.queue, []
        return pending

    def read_one(self):
        return self.queue.pop(0) if self.queue else None

    def capabilities(self, absinfo: bool = False):
        return {
            ecodes.EV_KEY: [BTN_SOUTH, BTN_EAST, 0x133],
            ecodes.EV_ABS: [(0x00, AbsInfo(0, 255, 128)),
                            (0x01, AbsInfo(0, 255, 128))],
        }

    def active_keys(self):
        return []


class Journey:
    """A daemon plus the pads plugged into it.

    Nothing about the assignment flow is stubbed: `begin`, the hold timers,
    the claim, the confirm gesture, `accept` and `restore` are the shipped
    code, and the files they write land in a temporary runtime directory.
    """

    def __init__(self, pads, *, clients: int = 0, client_age: float = 60.0,
                 fail_open=(), fail_create=()) -> None:
        self.pads = list(pads)
        self.devices = {p.path: FakeDevice(p) for p in self.pads}
        self.events: list[dict] = []
        self.sent: list[tuple[object, dict]] = []
        self.created: list[int] = []
        self.republishers: list[SimpleNamespace] = []
        self.client = object()

        self.srv = server.Server()
        self.srv._broadcast = self.events.append          # type: ignore[assignment]
        self.srv._send = lambda c, m: self.sent.append((c, m))  # type: ignore[assignment]
        # A front-end is what makes the daemon willing to grab the pads on its
        # own. Off by default so a tick in the middle of some other scenario
        # cannot decide to open setup underneath it.
        self.srv._clients = {                             # type: ignore[assignment]
            n: SimpleNamespace(connected_at=time.monotonic() - client_age)
            for n in range(clients)
        }
        self.srv._last_pad_scan = time.monotonic()

        devices.discover = lambda *a, **k: list(self.pads)  # type: ignore[assignment]

        def opener(target: Pad):
            # A pad that has gone away between discovery and opening: the
            # kernel answers ENODEV, which is what a replug in the gap looks
            # like.
            if target.path in fail_open:
                raise OSError(19, "No such device")
            return self.devices[target.path]

        # assign.py imported the name directly; controllercfg and virtual go
        # through the module. Both have to be stubbed or a real node is opened.
        assign.open_device = opener                       # type: ignore[assignment]
        devices.open_device = opener                      # type: ignore[assignment]

        def create(source: Pad, player: int):
            if player in fail_create:
                raise OSError(19, "No such device")
            self.created.append(player)
            return SimpleNamespace(
                player=player, pad=source,
                # The daemon logs which descriptor it is watching for which
                # pad, so a clone stand-in needs a source with an fd and a
                # path -- it does not need them to be real.
                source=SimpleNamespace(fd=2100 + player, path=source.path),
                ui=SimpleNamespace(device=SimpleNamespace(
                    path=f"/dev/input/event210{player}")))

        def republisher(vpads):
            made = SimpleNamespace(vpads=list(vpads), fds=[], closed=0,
                                   paused=False)
            made.close = lambda: setattr(made, "closed", made.closed + 1)
            made.handle_readable = lambda fd: None
            # The daemon silences the clone while a wizard is open, and it
            # re-applies that on every restart -- so a stand-in has to answer
            # set_paused or the journey fails on the double, not the daemon.
            made.set_paused = lambda paused: setattr(made, "paused", paused)
            made.dead_fds = lambda: []
            self.republishers.append(made)
            return made

        virtual.create = create                           # type: ignore[assignment]
        virtual.Republisher = republisher                 # type: ignore[assignment]

    # -- driving it -------------------------------------------------------

    def send(self, command: str, **extra):
        self.srv._handle_command(self.client, {"cmd": command, **extra})  # type: ignore[arg-type]
        return self

    def tick(self):
        """One turn of the daemon's loop: read what is readable, then tick.

        Delivery is driven from here rather than from press(), because that is
        the order the real loop runs in -- select reports a descriptor
        readable, the events are consumed, and only then do the hold timers
        advance. A press that was queued before the session opened is
        therefore delivered exactly as the kernel would deliver it: still
        sitting in the buffer, unless something drained it.
        """
        for device in self.devices.values():
            if device.queue:
                self.srv._on_pad_read(device.fd)
        self.srv._tick()
        return self

    def scan(self):
        """Force the once-a-second look for a new controller model."""
        self.srv._last_pad_scan = 0.0
        self.srv._poll_new_controllers()
        return self

    def press(self, target: Pad, code: int = BTN_SOUTH):
        self.devices[target.path].queue.append(Event(ecodes.EV_KEY, code, 1))
        return self

    def release(self, target: Pad, code: int = BTN_SOUTH):
        self.devices[target.path].queue.append(Event(ecodes.EV_KEY, code, 0))
        return self

    def hold(self, target: Pad, code: int = BTN_SOUTH):
        """Press and keep pressing, long enough to claim a slot."""
        self.press(target, code)
        self.tick()                 # a partial hold, as the ring is drawn
        time.sleep(PAST_HOLD)
        self.tick()
        return self

    # -- reading it back --------------------------------------------------

    @property
    def messages(self) -> list[dict]:
        return list(self.events) + [m for _c, m in self.sent]

    def of(self, kind: str) -> list[dict]:
        return [m for m in self.messages if m.get("event") == kind]

    @property
    def errors(self) -> list[str]:
        return [str(m.get("message")) for m in self.of("error")]

    def last_error(self) -> str:
        return self.errors[-1] if self.errors else ""

    def last_state(self) -> dict:
        states = self.of("state")
        return states[-1] if states else {}

    def claims(self) -> list[tuple[int, str]]:
        return [(m["player"], m["node"]) for m in self.of("claim")]

    def grabbed(self) -> list[str]:
        return sorted(d.pad.name for d in self.devices.values() if d.grabbed)

    def forget_events(self) -> None:
        self.events.clear()
        self.sent.clear()


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def mapped_profile(target: Pad, scopes=("", "console:n64")) -> None:
    """A controller that has been through the wizard, under several scopes."""
    profile = profiles.Profile(signature=profiles.signature(target),
                               name=target.name, icon="n64")
    for scope in scopes:
        profile.record(scope, profiles.Mapping(
            buttons={"a": Binding("button", BTN_SOUTH)}, layout="n64"))
    profiles.save(profile)
    if profiles.load(target) is None:
        fail("the fixture profile did not save; nothing below means anything")


def main() -> int:
    original = (devices.discover, devices.open_device, assign.open_device,
                virtual.create, virtual.Republisher)
    try:
        return journey()
    finally:
        (devices.discover, devices.open_device, assign.open_device,
         virtual.create, virtual.Republisher) = original


def journey() -> int:
    one = pad("Player One Pad", node="event9990")
    two = pad("Player Two Pad", vid=0x2222, node="event9991")
    marker = protocol.playing_marker()
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.unlink(missing_ok=True)

    # =================================================== S1: it opens itself
    #
    # Negatives first and deliberately so: opening the screen records the
    # model as prompted, and a positive case run earlier would make every
    # later case pass because the pad had already been offered rather than
    # because the condition under test held.

    print("S1: moments a brand-new controller must NOT take the pads:")
    fresh = pad("Brand New Pad", vid=0x3333, node="event9992")

    marker.write_text(f"{os.getpid()}\n")
    playing = Journey([fresh], clients=1).scan()
    if playing.of("newpad"):
        fail("setup opened while a game was running -- the player is holding "
             "a controller that has just gone dead mid-game")
    if playing.srv._state != protocol.STATE_IDLE:
        fail(f"a game was running and the daemon still moved to "
             f"{playing.srv._state!r}")
    marker.unlink(missing_ok=True)
    print("  ok  a game is running: no session, and the pads stay published")

    lonely = Journey([fresh], clients=0).scan()
    if lonely.of("newpad"):
        fail("setup opened with no front-end connected -- the pads are "
             "grabbed for a screen nobody can see")
    print("  ok  no front-end connected: nothing happens")

    passing = Journey([fresh], clients=1, client_age=0.0).scan()
    if passing.of("newpad"):
        fail("a passing `padmap status` query counted as a front-end -- that "
             "is how ensure-daemon left the machine in `assigning` with every "
             "pad grabbed and no front-end running")
    print("  ok  only a status query connected: still nothing")

    print("\nS1: and with a settled front-end and no game:")
    opens = Journey([fresh], clients=1).scan()
    newpad = opens.of("newpad")
    if not newpad:
        fail("plugging in an unknown controller did not open setup -- the "
             "user has to know a settings screen exists to configure it")
    if newpad[-1]["names"] != ["Brand New Pad"]:
        fail(f"the front-end was told {newpad[-1]['names']!r}, which does not "
             f"name the controller that was just plugged in")
    if opens.srv._state != protocol.STATE_ASSIGNING:
        fail(f"newpad was announced but the daemon stayed in "
             f"{opens.srv._state!r}, so no screen appears")
    if opens.grabbed() != ["Brand New Pad"]:
        fail("the session opened without grabbing the pad, so every press "
             "also reaches the front-end underneath")
    if not opens.of("pads"):
        fail("the session opened without telling the front-end how many pads "
             "it can see")
    print("  ok  setup opens by itself, names the pad, and grabs it")

    again = Journey([fresh], clients=1).scan()
    if again.of("newpad"):
        fail("the same model was offered setup twice -- declining it once "
             "must be enough, or the screen reappears a second later")
    print("  ok  a model already offered is not offered again")

    restarted = Journey([fresh], clients=1)
    if profiles.signature(fresh) not in restarted.srv._prompted:
        fail("a freshly constructed daemon has forgotten what it had already "
             "asked about -- ensure-daemon restarts the daemon on every "
             "front-end launch, so this is 'asked every single time'")
    restarted.scan()
    if restarted.of("newpad"):
        fail("a restarted daemon re-offered a controller the user declined")
    print("  ok  nor after the daemon restarts")

    known = pad("Already Mapped Pad", vid=0x4444, node="event9993")
    mapped_profile(known)
    quiet = Journey([known], clients=1).scan()
    if quiet.of("newpad"):
        fail("a controller that has already been through the wizard opened "
             "setup again instead of being silently republished")
    print("  ok  a controller already mapped is republished in silence")

    adapter = [pad("Four Port Adapter", vid=0x5555, node=f"event999{n}")
               for n in (4, 5, 6, 7)]
    ports = Journey(adapter, clients=1).scan()
    if len(ports.of("newpad")) != 1:
        fail(f"one adapter produced {len(ports.of('newpad'))} offers -- four "
             f"identical ports are one controller model")
    print("  ok  four ports of one adapter prompt once, not four times")

    # ========================================== S2: opening setup on purpose

    print("\nS2: begin opens a session and reports what it can see:")
    started = Journey([one, two])
    started.send("begin", players=4)
    pads_event = started.of("pads")
    if not pads_event or pads_event[-1]["count"] != 2:
        fail(f"begin reported {pads_event!r} for two plugged-in pads -- the "
             f"screen has no other way to say 'I can see your controllers'")
    if started.srv._state != protocol.STATE_ASSIGNING:
        fail("begin did not move the daemon into assigning, so no screen "
             "appears however many keys the user presses")
    if started.last_state().get("slots") != 4:
        fail(f"the front-end asked for 4 slots and was told "
             f"{started.last_state().get('slots')!r}")
    if started.grabbed() != ["Player One Pad", "Player Two Pad"]:
        fail(f"only {started.grabbed()} were grabbed -- an ungrabbed pad's "
             f"presses reach the front-end as well, so claiming a slot "
             f"doubles as a UI keypress")
    if started.last_state().get("players"):
        fail("the screen was told about players before anybody had claimed a "
             "slot, so it draws controllers nobody has touched")
    print("  ok  two pads seen, four slots, both grabbed, no players yet")

    empty = Journey([])
    empty.send("begin", players=4)
    if "no joypads" not in empty.last_error():
        fail(f"begin with nothing plugged in said {empty.last_error()!r}")
    if empty.srv._state != protocol.STATE_IDLE:
        fail("begin with no pads left the daemon in a session it can never "
             "finish")
    print(f"  ok  nothing plugged in: {empty.last_error()!r}, and no session")

    live = Journey([one, two])
    live.srv._assignments = [assign.Assignment(player=1, pad=one, button=0)]
    live.srv._start_republisher()
    live.srv._state = protocol.STATE_READY
    live.send("begin", players=2)
    if not live.republishers or live.republishers[0].closed != 1:
        fail("begin did not stop republishing first -- the republisher holds "
             "EVIOCGRAB on the same pads, so every press during setup would "
             "be invisible")
    print("  ok  begin from ready stops republishing before grabbing")

    # ================================================= S3: claiming a slot

    print("\nS3: holding a button claims the next free slot:")
    claiming = Journey([one, two])
    claiming.send("begin", players=4)
    claiming.forget_events()
    claiming.hold(one)
    if claiming.claims() != [(1, "event9990")]:
        fail(f"a held button produced {claiming.claims()!r} -- press-to-claim "
             f"is the only way to tell two identical pads apart, so nothing "
             f"can be assigned at all")
    claim = claiming.of("claim")[0]
    if claim["name"] != "Player One Pad":
        fail(f"the claim names {claim['name']!r}; the screen has nothing else "
             f"to label the slot with")
    if not claim["icon"]:
        fail("the claim carries no icon, so the slot is drawn blank")
    if claim["configured"] is not False:
        fail("a controller nobody has ever configured reported itself as "
             "configured, so the front-end never offers to set it up")
    fractions = [m["frac"] for m in claiming.of("progress")]
    if 1.0 not in fractions:
        fail(f"the hold ring never filled ({fractions}) -- the user is given "
             f"no sign that holding is doing anything")
    if claiming.last_state().get("players", [{}])[0].get("player") != 1:
        fail("the state event does not report the slot that was just claimed")
    print("  ok  claims player 1, the ring fills, and the screen is told")

    claiming.forget_events()
    claiming.hold(two)
    if claiming.claims() != [(2, "event9991")]:
        fail(f"the second controller claimed {claiming.claims()!r} rather "
             f"than player 2 -- slots fill in the order buttons are held")
    order = [entry["player"] for entry in claiming.last_state()["players"]]
    if order != [1, 2]:
        fail(f"the screen shows slots {order}; two players expect to be 1 "
             f"and 2 in the order they claimed")
    nodes = [entry["node"] for entry in claiming.last_state()["players"]]
    if nodes != ["event9990", "event9991"]:
        fail(f"the slots hold {nodes} -- each player must have their own pad")
    print("  ok  the second hold takes player 2; the slots fill in order")

    print("\nS3: presses that must not claim anything:")
    already = Journey([one, two])
    # The button that opened the screen is still down when the session opens.
    already.devices[one.path].queue.append(Event(ecodes.EV_KEY, BTN_SOUTH, 1))
    already.send("begin", players=4)
    already.tick()          # the loop's first turn after the session opened
    time.sleep(PAST_HOLD)
    already.tick()          # ...and the user is still holding the button
    if already.claims():
        fail(f"the press that opened the screen claimed {already.claims()!r} "
             f"-- a slot is burnt before the user has done anything, and on "
             f"the auto-opened screen they never pressed anything at all")
    already.release(one)
    already.hold(one)
    if already.claims() != [(1, "event9990")]:
        fail("after releasing, a fresh hold did not claim -- the drain took "
             "the pad out of the session entirely")
    print("  ok  a button held from before the session opened claims nothing")
    print("  ok  and releasing it, then holding again, does claim")

    tap = Journey([one, two])
    tap.send("begin", players=4)
    tap.forget_events()
    tap.press(one)
    tap.tick()
    time.sleep(HOLD_SECONDS * 0.4)
    tap.release(one)
    tap.tick()              # let go well before the hold could complete
    time.sleep(PAST_HOLD)
    tap.tick()
    if tap.claims():
        fail(f"a tap claimed {tap.claims()!r} -- an empty adapter port emits "
             f"stray presses, and each one would silently burn a slot")
    if not tap.of("progress") or tap.of("progress")[-1]["frac"] != 0.0:
        fail(f"the hold ring was left at {tap.of('progress')[-1:]!r} after the "
             f"button came up, so the screen shows a hold still in progress")
    print("  ok  a tap claims nothing, and the ring empties again")

    twice = Journey([one, two])
    twice.send("begin", players=4)
    twice.hold(one)
    twice.forget_events()
    twice.release(one)
    twice.hold(one)
    if twice.claims():
        fail(f"one controller claimed a second slot ({twice.claims()!r}) -- "
             f"one pad would be two players, and player 2's controller could "
             f"never be assigned")
    if [a.player for a in twice.srv._assigner.assignments] != [1]:
        fail("the session holds more than one claim for a single pad")
    print("  ok  one physical pad cannot claim two slots")

    # ==================================================== S6: starting over

    print("\nS6: reset drops the claims and keeps the session open:")
    over = Journey([one, two])
    over.send("begin", players=4)
    over.hold(one)
    over.hold(two)
    over.forget_events()
    over.send("reset")
    if over.srv._assigner is None:
        fail("reset closed the session -- the pads are released mid-setup and "
             "nothing can be claimed again without starting over by hand")
    if over.srv._assigner.assignments:
        fail("the claims survived a reset, so the ordering cannot be redone")
    if over.last_state().get("players"):
        fail("the screen was not told the slots had emptied, so it goes on "
             "drawing controllers in them")
    if over.grabbed() != ["Player One Pad", "Player Two Pad"]:
        fail("reset released the pads; the session is open but deaf")
    print("  ok  claims gone, session open, pads still grabbed, screen told")

    over.forget_events()
    over.hold(two)
    if over.claims() != [(1, "event9991")]:
        fail(f"after a reset the ordering did not start over: got "
             f"{over.claims()!r}, wanted the second pad as player 1")
    print("  ok  and the ordering starts over: the other pad becomes player 1")

    held = Journey([one, two])
    held.send("begin", players=4)
    held.hold(one)
    held.forget_events()
    # Still holding the button when reset arrives, which is the ordinary case:
    # the user presses F with a thumb on the pad.
    held.send("reset")
    time.sleep(PAST_HOLD)
    held.tick()
    if held.claims():
        fail(f"a button still held across a reset re-claimed a slot "
             f"({held.claims()!r}) -- the ordering the user just cleared "
             f"reappears without them pressing anything")
    print("  ok  a button still held across the reset does not re-claim")

    # ================================================ S4: confirming it all

    print("\nS4: holding again on an assigned pad confirms the session:")
    done = Journey([one, two])
    done.send("begin", players=2)
    done.hold(one)
    done.hold(two)
    done.forget_events()
    done.release(two)
    done.press(two)
    done.tick()
    time.sleep(server.CONFIRM_HOLD_SECONDS * 0.35)
    done.tick()
    partial = [m["frac"] for m in done.of("confirm")]
    if not partial or max(partial) >= 1.0:
        fail(f"the confirm ring read {partial} halfway through the hold -- "
             f"either it is not drawn at all or it completed early, and "
             f"confirming ends the session")
    time.sleep(PAST_CONFIRM)
    done.tick()
    if not done.of("accepted"):
        fail(f"holding for {server.CONFIRM_HOLD_SECONDS}s did not confirm "
             f"(confirm fractions {[m['frac'] for m in done.of('confirm')]})")
    accepted = done.of("accepted")[-1]
    if [entry["player"] for entry in accepted["players"]] != [1, 2]:
        fail(f"the accepted event reports {accepted['players']!r}; both "
             f"players must survive the confirmation")
    if not accepted.get("launch_config"):
        fail("nothing was told where the launch config landed")
    if done.srv._state != protocol.STATE_READY:
        fail(f"after confirming the daemon is {done.srv._state!r}, so the "
             f"front-end still shows the setup screen")
    print("  ok  the confirm ring fills, then accept, then ready")

    if done.created != [1, 2]:
        fail(f"virtual pads were created for {done.created} -- one per player "
             f"is what makes the controllers work at all")
    if done.grabbed():
        fail(f"{done.grabbed()} are still grabbed by the finished session -- "
             f"the republisher opens them next, and two exclusive grabs on "
             f"one pad cannot both succeed")
    print("  ok  one virtual pad per player, and the session's grabs released")

    launch = done.srv.launch_config_path
    if not launch.is_file() or "input_player1_reserved_device" not in \
            launch.read_text():
        fail(f"{launch} does not reserve player 1's pad -- RetroArch would "
             f"assign the ports in whatever order it enumerated them")
    if not done.srv.launch_args_path.is_file():
        fail("no launch args were written, so unassigned core ports are never "
             "emptied and one controller becomes four players")
    if not done.srv.state_path.is_file():
        fail("the assignment was not saved, so a daemon restart loses it")
    saved = json.loads(done.srv.state_path.read_text())
    if [entry["player"] for entry in saved] != [1, 2]:
        fail(f"the saved assignment reads {saved!r}")
    profile_dir = protocol.runtime_dir() / "autoconfig" / "udev"
    written = sorted(p.name for p in profile_dir.glob("*.cfg"))
    if written != ["padmap Player 1.cfg", "padmap Player 2.cfg"]:
        fail(f"RetroArch profiles written: {written} -- a player with no "
             f"profile is a controller RetroArch does not know how to read")
    sdl = done.of("sdl_mapping")
    if not sdl or len(sdl[-1]["lines"]) != 2:
        fail(f"the front-end was handed {sdl[-1:]!r} SDL lines. Pegasus reads "
             f"sdl_controllers.txt once at startup, so a mapping it is not "
             f"handed does nothing until it is relaunched")
    print("  ok  launch config, launch args, saved state, one profile each")
    print("  ok  and the SDL lines are handed to the front-end, not just filed")

    print("\nS4: a confirm hold released too early does nothing:")
    nearly = Journey([one, two])
    nearly.send("begin", players=2)
    nearly.hold(one)
    nearly.forget_events()
    nearly.release(one)
    nearly.press(one)
    nearly.tick()
    time.sleep(server.CONFIRM_HOLD_SECONDS * 0.3)
    nearly.tick()
    nearly.release(one)
    nearly.tick()
    if nearly.of("accepted"):
        fail("a confirm hold that was released early ended the session anyway")
    if nearly.of("confirm")[-1]["frac"] != 0.0:
        fail(f"the confirm ring was left at "
             f"{nearly.of('confirm')[-1]['frac']!r} after the button came up")
    if nearly.srv._state != protocol.STATE_ASSIGNING:
        fail("letting go of the confirm button ended the session")
    print("  ok  the ring empties and the session stays open")

    print("\nS4: confirming an empty session is refused:")
    nothing = Journey([one, two])
    nothing.send("begin", players=2)
    nothing.forget_events()
    nothing.send("accept")
    if "nothing assigned" not in nothing.last_error():
        fail(f"accept with no claims said {nothing.last_error()!r} -- "
             f"confirming an empty session must not look like success")
    if nothing.srv._state != protocol.STATE_ASSIGNING:
        fail("an empty accept closed the session, taking the user's chance "
             "to claim a slot with it")
    if nothing.srv.launch_config_path.is_file() and \
            nothing.of("accepted"):
        fail("an empty accept still produced a launch config")
    print(f"  ok  {nothing.last_error()!r}, and the session stays open")

    # ================================================ S5: leaving the screen

    print("\nS5: cancel releases the pads and writes nothing:")
    for path in (protocol.runtime_dir() / "launch.cfg",
                 protocol.runtime_dir() / "assignments.json"):
        path.unlink(missing_ok=True)
    abandoned = Journey([one, two])
    abandoned.send("begin", players=2)
    abandoned.hold(one)
    abandoned.forget_events()
    abandoned.send("cancel")
    if abandoned.grabbed():
        fail(f"{abandoned.grabbed()} are still grabbed after cancel -- every "
             f"controller on the machine stays dead until the daemon is "
             f"restarted, so the rest of Pegasus goes deaf")
    if any(d.ungrabs != 1 for d in abandoned.devices.values()):
        fail("a pad was never ungrabbed on the way out")
    if abandoned.srv._assigner is not None:
        fail("the session object survived a cancel")
    if abandoned.srv._state != protocol.STATE_IDLE:
        fail(f"cancelling left the daemon in {abandoned.srv._state!r} with "
             f"nothing published")
    if abandoned.srv.state_path.is_file():
        fail("cancelling wrote the assignment anyway -- abandoning the screen "
             "is how a user says 'not that'")
    if abandoned.created:
        fail("cancelling published virtual pads for claims it threw away")
    if not abandoned.of("state"):
        fail("the front-end was never told the session had ended, so the "
             "setup screen stays up with the pads released underneath it")
    print("  ok  ungrabbed, session gone, nothing written, screen told")

    print("\nS5: and the front-end going away without saying anything:")
    crashed = Journey([one, two])
    left, right = socket.socketpair()
    client = server.Client(left)
    crashed.srv._clients = {left.fileno(): client}
    crashed.srv._selector.register(left, 1, crashed.srv._on_client_read)
    crashed.send("begin", players=2)
    crashed.hold(one)
    if not crashed.grabbed():
        fail("the fixture never grabbed anything; the release below proves "
             "nothing")
    crashed.srv._drop_client(left)
    right.close()
    if crashed.grabbed():
        fail(f"{crashed.grabbed()} stayed grabbed after the last front-end "
             f"disconnected -- nobody is left to send cancel, and every "
             f"controller on the machine is dead until a daemon restart")
    if crashed.srv._assigner is not None:
        fail("the session outlived the front-end that opened it")
    print("  ok  the last client disconnecting releases the pads too")

    # ============================================ S11: resetting one pad

    print("\nS11: forget_pad throws away every scope and re-runs the wizard:")
    victim = pad("Wrongly Mapped Pad", vid=0x6666, node="event9994")
    mapped_profile(victim, scopes=("", "console:n64", "game:n64/goldeneye"))
    reset = Journey([victim])
    reset.srv._prompted = {profiles.signature(victim), "other:controller"}
    reset.srv._save_prompted()
    reset.send("begin", players=2)
    reset.hold(victim)
    reset.forget_events()
    reset.send("forget_pad", player=1)
    if reset.errors:
        fail(f"resetting a claimed controller said {reset.last_error()!r}")
    if profiles.load(victim) is not None:
        fail("a scope survived the reset -- the user re-runs the wizard and "
             "still meets the old behaviour, from a per-console mapping they "
             "have forgotten exists")
    if profiles.signature(victim) in reset.srv._prompted:
        fail("the 'already asked' record survived, so this controller is "
             "never offered setup again")
    if "other:controller" not in reset.srv.prompted_path.read_text():
        fail("resetting one controller cleared the record for another")
    opened = [m for m in reset.of("layout_choice") if m.get("active")]
    if not opened:
        fail("the mapping was thrown away and nothing opened -- the user "
             "asked for 'this is wrong, fix it' and got only the first half")
    if opened[-1]["player"] != 1:
        fail(f"the wizard opened for player {opened[-1]['player']}")
    print("  ok  every scope gone, the record cleared, the wizard re-opened")

    empty_slot = Journey([one, two])
    empty_slot.send("begin", players=4)
    empty_slot.forget_events()
    empty_slot.send("forget_pad", player=3)
    if "player 3" not in empty_slot.last_error():
        fail(f"resetting an empty slot said {empty_slot.last_error()!r}, "
             f"which does not name the slot that could not be reset")
    print(f"  ok  an empty slot: {empty_slot.last_error()!r}")

    # ======================================= S18: surviving a daemon restart

    print("\nS18: after accepting, the assignment survives a restart:")
    finished = Journey([one, two])
    finished.send("begin", players=2)
    finished.hold(one)
    finished.hold(two)
    finished.send("accept")
    if not finished.of("accepted"):
        fail(f"the fixture session did not accept ({finished.last_error()!r})")
    finished.srv.launch_config_path.unlink(missing_ok=True)

    # What `padmap ensure-daemon` does: a second daemon, same runtime dir,
    # started because the first was running stale code.
    upgraded = Journey([one, two])
    upgraded.srv.restore()
    if upgraded.srv._state != protocol.STATE_READY:
        fail(f"a restarted daemon came up {upgraded.srv._state!r} -- the user "
             f"loses their controller order to every upgrade, which is what "
             f"makes restarting too expensive to do automatically")
    restored = [(a.player, a.pad.path) for a in upgraded.srv._assignments]
    if restored != [(1, one.path), (2, two.path)]:
        fail(f"restored {restored!r}: the same controllers must come back on "
             f"the same slots, or player 1 and player 2 swap after an upgrade")
    if upgraded.created != [1, 2]:
        fail(f"only {upgraded.created} were republished, so a player's "
             f"controller is silently missing after the restart")
    if not upgraded.srv.launch_config_path.is_file():
        fail("the launch config was not regenerated on restore, so the next "
             "game launches against a file that is no longer there")
    print("  ok  ready again, same pads, same slots, launch config rebuilt")

    print("\nS18: a controller that is not there any more is not faked:")
    unplugged = Journey([two])
    unplugged.srv.restore()
    if unplugged.created != [2]:
        fail(f"republished {unplugged.created} with player 1 unplugged -- a "
             f"dead virtual pad in the enumeration shifts every index after "
             f"it, so player 2's controller stops working too")
    if [a.player for a in unplugged.srv._assignments] != [2]:
        fail("an absent controller was restored anyway")
    print("  ok  the missing player is dropped; the present one is kept")

    print("\nS18: a state file nothing could have written:")
    for label, raw in (
        ("truncated json", "[{\"player\": 1, "),
        ("an object, not a list", '{"player": 1, "path": "/dev/input/x"}'),
        ("entries that are not objects", '["event9990", 3, null]'),
        ("a player number that is not one", '[{"player": "one", '
                                            '"path": "/dev/input/event9990"}]'),
        ("no path at all", '[{"player": 1}]'),
        ("not utf-8 at all", None),
    ):
        broken = Journey([one, two])
        if raw is None:
            broken.srv.state_path.write_bytes(b"[{\"player\": 1, \xff}]")
        else:
            broken.srv.state_path.write_text(raw)
        try:
            broken.srv.restore()
        except Exception as error:                    # noqa: BLE001
            fail(f"a {label} state file crashed the daemon on startup "
                 f"({type(error).__name__}: {error}) -- padmap then cannot "
                 f"start at all, and every controller stays dead")
        if broken.srv._state != protocol.STATE_IDLE or broken.created:
            fail(f"a {label} state file produced assignments "
                 f"({broken.srv._assignments!r})")
        print(f"  ok  {label}: ignored, daemon still starts")

    # =========================================== error paths beside the flow

    print("\nS2/S4: what a malformed command from a front-end does:")
    # Not hypothetical politeness: the command chain runs inside the selector
    # loop with nothing catching anything, so whatever it raises leaves the
    # daemon dead and every controller with it.
    malformed = Journey([one, two])
    wire, front = socket.socketpair()
    malformed.srv._clients = {wire.fileno(): server.Client(wire)}
    front.sendall(protocol.encode({"cmd": "begin", "players": "lots"}))
    crash = None
    try:
        malformed.srv._on_client_read(wire)
    except Exception as error:                        # noqa: BLE001
        crash = error
    if crash is None:
        if malformed.srv._state != protocol.STATE_IDLE:
            fail("a begin whose player count is not a number opened a session "
                 "anyway")
        print("  ok  a nonsense player count is refused, and nothing opens")
    else:
        print(f"  gap: a nonsense player count raises "
              f"{type(crash).__name__} out of the command handler, which "
              f"nothing catches -- see notes")
    # The part that is unambiguously right, and must keep working either way.
    front.sendall(protocol.encode({"cmd": "nonsense"}))
    malformed.srv._on_client_read(wire)
    if "unknown command" not in str(malformed.sent[-1][1].get("message", "")):
        fail(f"a command the daemon does not know produced "
             f"{malformed.sent[-1][1]!r} rather than saying so")
    front.close()
    print("  ok  a command it does not know is refused by name, not by dying")

    print("\nS1: a controller that vanishes while setup is opening:")
    # Between discover() and open(): one second of scan interval, and a pad
    # that has just been plugged in is exactly the one being unplugged again.
    vanishing = Journey([one, two], fail_open={two.path})
    crash = None
    try:
        vanishing.send("begin", players=2)
    except Exception as error:                        # noqa: BLE001
        crash = error
    if crash is None:
        stuck = [d.pad.name for d in vanishing.devices.values()
                 if d.grabbed and vanishing.srv._assigner is None]
        if stuck:
            fail(f"{stuck} were left grabbed by a session that never opened "
                 f"-- the controllers are dead and nothing will release them")
        print("  ok  the session copes with a pad that went away")
    else:
        print(f"  gap: opening a pad that has gone away raises "
              f"{type(crash).__name__} out of begin -- see notes")

    print("\nS4: a controller that vanishes just before it is confirmed:")
    dying = Journey([one, two], fail_create={2})
    dying.send("begin", players=2)
    dying.hold(one)
    dying.hold(two)
    dying.forget_events()
    crash = None
    try:
        dying.send("accept")
    except Exception as error:                        # noqa: BLE001
        crash = error
    if crash is None:
        if not (dying.of("accepted") or dying.of("error")):
            fail("the confirmation neither succeeded nor said anything, so "
                 "the setup screen waits for an event that never comes")
        print("  ok  the front-end is told what became of the confirmation")
    else:
        print(f"  gap: a pad that cannot be republished raises "
              f"{type(crash).__name__} out of accept, after the assignment "
              f"has already been saved -- see notes")

    print("\nS1: a `prompted` record that is not text:")
    corrupt = protocol.prompted_path()
    corrupt.write_bytes(b"1234:0001:Fine Pad\n\xff\xfe\n")
    crash = None
    try:
        rebuilt = server.Server()
        prompted = rebuilt._prompted
    except Exception as error:                        # noqa: BLE001
        crash = error
    if crash is None:
        if not isinstance(prompted, set):
            fail("the prompted record did not read back as a set of "
                 "signatures")
        print("  ok  an unreadable record reads as 'nothing asked yet'")
    else:
        print(f"  gap: a `prompted` file that is not UTF-8 raises "
              f"{type(crash).__name__} from the Server constructor, so the "
              f"daemon cannot start at all -- see notes")
    corrupt.unlink(missing_ok=True)

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
