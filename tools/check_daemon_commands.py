"""The daemon's command surface: routing, per-player resolution, guards.

Everything a front-end can ask the daemon to do arrives as one JSON object
through one `if/elif` chain. Three ways that goes wrong, all of them silent:

  * a command is defined but never *routed*, so the handler exists, reads
    correctly, and is unreachable. Nothing says so until someone presses the
    key on the setup screen -- where the pads are grabbed, so there is no
    other feedback to fall back on.
  * a per-player command cannot work out which controller "player 1" means.
    That is not hypothetical: consulting the session's claims alone made
    every one of them fail for the opening seconds of a session, about a pad
    that was assigned and republishing.
  * a destructive command validates after it destroys, leaving a controller
    with neither a mapping nor a way to make one.

Runs against a real `server.Server` with only the machine-touching parts
stubbed -- no uinput, no EVIOCGRAB, no device opened. Deliberately not a live
daemon: a real one is running on this machine and a second would fight it for
the pads.

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_daemon_commands.py
"""

import os
import re
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import (capture, icons, layouts, profiles,  # noqa: E402
                    protocol, server)
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

# Every command protocol.py documents. Spelled out as well as scraped from
# the docstring, so that deleting the documentation cannot quietly shrink
# what this file tests.
COMMANDS = (
    "begin", "reset", "accept", "cancel", "map", "choose_layout",
    "choose_scope", "map_for_game", "forget_pad", "skip_control",
    "calibrate", "configure_end", "set_icon", "status",
)

# A representative message per command: the arguments a front-end really
# sends, so a handler reached with the wrong keys shows up as a wrong answer
# rather than passing on a bare {"cmd": ...}.
SAMPLE = {
    "begin": {"players": 4},
    "map": {"player": 1, "layout": "n64", "scope": "console:n64"},
    "choose_layout": {"player": 1},
    "choose_scope": {"player": 1},
    "map_for_game": {"player": 1, "console": "n64",
                     "key": "n64/goldeneye-007-usa",
                     "title": "GoldenEye 007 (USA)"},
    "forget_pad": {"player": 1},
    "calibrate": {"player": 1},
    "set_icon": {"player": 1, "icon": "n64"},
}


def pad(name: str, vid: int = 0x1234, pid: int = 0x0001, node: str = "event90"):
    return Pad(path=f"/dev/input/{node}", name=name, phys="", uniq="",
               vid=vid, pid=pid, syspath="")


class AbsInfo:
    """Just enough of evdev's absinfo for the ranges the daemon reads."""

    def __init__(self, minimum: int, maximum: int, value: int) -> None:
        self.min, self.max, self.value = minimum, maximum, value


class FakeDevice:
    """An open pad handle that reports capabilities and answers nothing.

    Never a real device: opening one would take events away from the daemon
    actually running on this machine.
    """

    def __init__(self, keys=(0x130, 0x131), axes=((0x00, 0, 255, 128),)):
        self._keys = list(keys)
        self._axes = [(code, AbsInfo(lo, hi, rest))
                      for code, lo, hi, rest in axes]

    def capabilities(self, absinfo: bool = False):
        return {capture.EV_KEY: list(self._keys),
                capture.EV_ABS: list(self._axes)}

    def active_keys(self):
        return []


class FakeAssigner:
    """An open assignment session: its live claims and its open handles."""

    def __init__(self, claims=(), open_pads=()):
        self.assignments = list(claims)
        self._open = {p.path: FakeDevice() for p in open_pads}
        self.fds: list[int] = []
        self.grab_failures: list[Pad] = []
        self.closed = False

    def device_for(self, target):
        return self._open.get(target.path)

    def reset(self):
        self.assignments = []

    def close(self):
        self.closed = True


class Harness:
    """A Server with the machine-touching parts stubbed out."""

    def __init__(self, *, stored=(), claims=None, open_pads=None,
                 state=protocol.STATE_IDLE):
        self.srv = server.Server()
        self.srv._state = state
        self.srv._assignments = list(stored)
        if claims is None and open_pads is None:
            self.srv._assigner = None
        else:
            self.srv._assigner = FakeAssigner(  # type: ignore[assignment]
                claims or (), open_pads or ())

        self.events: list[dict] = []
        self.sent: list[tuple[object, dict]] = []
        self.began: list[int] = []
        self.client = object()

        self.srv._broadcast = self.events.append   # type: ignore[assignment]
        self.srv._send = lambda c, m: self.sent.append((c, m))  # type: ignore[assignment]
        # Grabs every pad and stops republishing: never run for real here.
        self.srv._begin = lambda players: self.began.append(players)  # type: ignore[assignment]
        # Creates uinput nodes; would publish virtual pads onto the machine.
        self.srv._start_republisher = lambda: None  # type: ignore[assignment]
        self.srv._write_controller_configs = lambda: None  # type: ignore[assignment]
        self.srv._save_assignments = lambda: None  # type: ignore[assignment]

    def send(self, command: str, **extra):
        message = {"cmd": command, **SAMPLE.get(command, {}), **extra}
        self.srv._handle_command(self.client, message)  # type: ignore[arg-type]
        return self

    @property
    def messages(self) -> list[dict]:
        """Everything that left the daemon, broadcast or replied."""
        return list(self.events) + [m for _c, m in self.sent]

    @property
    def errors(self) -> list[str]:
        return [str(m.get("message")) for m in self.messages
                if m.get("event") == "error"]

    def last_error(self) -> str:
        return self.errors[-1] if self.errors else ""


def check_routed(command: str) -> str:
    """Dispatch one command on a fresh daemon; return its complaint, if any."""
    harness = Harness()
    harness.send(command)
    for message in harness.messages:
        if message.get("event") == "error" and \
                "unknown command" in str(message.get("message")):
            raise SystemExit(
                f"FAIL: the daemon does not route {command!r} -- a front-end "
                f"sending it gets \"unknown command\" back, so the key that "
                f"sends it does nothing and says nothing")
    return harness.last_error()


def main() -> int:
    store = Path(tempfile.mkdtemp(prefix="padmap-cmds-"))
    runtime = store / "run"
    (runtime / "padmap").mkdir(parents=True)
    # Nothing here may touch the real user's state: a live daemon owns the
    # real runtime dir, and the real profile store is someone's controllers.
    os.environ["XDG_RUNTIME_DIR"] = str(runtime)
    os.environ["XDG_CONFIG_HOME"] = str(store / "config")
    os.environ["XDG_DATA_HOME"] = str(store / "data")
    os.environ["PADMAP_PROFILE_DIR"] = str(store / "devices")

    # ---------------------------------------------------------------- routing

    print("every command the protocol documents reaches a handler:")
    documented = sorted(set(re.findall(r'\{"cmd": "([a-z_]+)"',
                                       protocol.__doc__ or "")))
    missing = [c for c in COMMANDS if c not in documented]
    if missing:
        raise SystemExit(
            f"FAIL: protocol.py no longer documents {missing} -- the wire "
            f"format is the only description a front-end author has, and a "
            f"command missing from it is one nobody will send")
    for command in sorted(set(documented) | set(COMMANDS)):
        complaint = check_routed(command)
        note = f" (says {complaint!r} with nothing set up)" if complaint else ""
        print(f"  ok  {command}{note}")

    print("\nand a command the daemon does not know:")
    unknown = Harness().send("explode")
    if "unknown command" not in unknown.last_error():
        raise SystemExit(
            f"FAIL: a mistyped or newer command got {unknown.messages!r} -- a "
            f"front-end has no way to tell it was not understood")
    if "explode" not in unknown.last_error():
        raise SystemExit(
            "FAIL: the error does not name the command, so a front-end author "
            "reading the log cannot tell which one was rejected")
    print(f"  ok  refused, and says {unknown.last_error()!r}")

    blank = Harness()
    blank.srv._handle_command(blank.client, {})  # type: ignore[arg-type]
    if "unknown command" not in blank.last_error():
        raise SystemExit(
            "FAIL: a message with no cmd at all was accepted silently")
    print("  ok  so is a message carrying no cmd at all")

    print("\narguments reach the handler, not just the handler's name:")
    counted = Harness().send("begin", players=2)
    if counted.began != [2]:
        raise SystemExit(
            f"FAIL: asked for 2 slots, the session opened with "
            f"{counted.began} -- the front-end's slot count is ignored")
    if Harness().send("begin", **{}).began != [4]:
        raise SystemExit("FAIL: a begin without a count did not default to 4")
    print("  ok  begin carries its slot count, and defaults to 4")

    # ------------------------------------------------- per-player resolution

    print("\nwhich controller a player number means:")
    one = pad("Player One Pad", node="event90")
    two = pad("Player Two Pad", vid=0x2222, node="event91")

    idle = Harness(stored=[Assignment(player=1, pad=one, button=0)])
    if idle.srv._pad_for_player(1) is not one:
        raise SystemExit(
            "FAIL: with no session open, a stored assignment does not resolve "
            "-- every per-player command would refuse to act on a controller "
            "that is assigned and republishing")
    print("  ok  no session: the stored assignment answers")

    # The reported bug. A session starts with no claims, and consulting only
    # those answered "no controller assigned to player 1" about a pad that was
    # plainly assigned -- which is how calibration came to appear to need a
    # button held first, with nothing saying so.
    opening = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                      claims=[], open_pads=[one],
                      state=protocol.STATE_ASSIGNING)
    if opening.srv._pad_for_player(1) is not one:
        raise SystemExit(
            "FAIL: the first seconds of a session hide an assigned "
            "controller -- every per-player command fails until the user "
            "guesses that a button must be held first")
    print("  ok  session open, nothing claimed yet: the same pad answers")

    claimed = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                      claims=[Assignment(player=1, pad=two, button=0)],
                      open_pads=[one, two],
                      state=protocol.STATE_ASSIGNING)
    if claimed.srv._pad_for_player(1) is not two:
        raise SystemExit(
            "FAIL: a stored assignment outranked a live claim -- someone "
            "re-assigning slot 1 would configure the pad they just replaced")
    print("  ok  a live claim wins: player 1 is whatever just pressed")

    # The fallback is per slot, not all-or-nothing: claiming slot 2 must not
    # blank out slot 1, which is still assigned and still republishing.
    mixed = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                    claims=[Assignment(player=2, pad=two, button=0)],
                    open_pads=[one, two], state=protocol.STATE_ASSIGNING)
    if mixed.srv._pad_for_player(1) is not one:
        raise SystemExit(
            "FAIL: claiming slot 2 hid the controller in slot 1")
    if mixed.srv._pad_for_player(2) is not two:
        raise SystemExit("FAIL: the pad just claimed for slot 2 did not "
                         "resolve")
    print("  ok  one slot claimed, another stored: both resolve")

    if mixed.srv._pad_for_player(4) is not None:
        raise SystemExit(
            "FAIL: an empty slot resolved to a controller -- a key bound to "
            "it would configure somebody else's pad")
    if Harness().srv._pad_for_player(1) is not None:
        raise SystemExit("FAIL: a daemon with nothing assigned resolved "
                         "player 1 to something")
    print("  ok  an empty slot resolves to nothing")

    print("\nand what that means for the command that reported it:")
    fresh = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                    claims=[], open_pads=[one],
                    state=protocol.STATE_ASSIGNING)
    fresh.send("calibrate", player=1)
    if fresh.errors:
        raise SystemExit(
            f"FAIL: calibrating at the start of a session said "
            f"{fresh.last_error()!r} -- about a controller that is assigned, "
            f"open and republishing")
    if fresh.srv._calibration is None or fresh.srv._calibration.pad is not one:
        raise SystemExit("FAIL: no calibration started for the assigned pad")
    if not [m for m in fresh.messages if m.get("event") == "calibration"]:
        raise SystemExit(
            "FAIL: calibration started with nothing told to the front-end, "
            "which is a screen showing no progress at all")
    print("  ok  calibrate works from the first moment of a session")

    print("\nbut a per-player command with no session still refuses:")
    for command, word in (("calibrate", "session"), ("map", "session"),
                          ("choose_layout", "session"),
                          ("choose_scope", "session"),
                          ("map_for_game", "session")):
        lonely = Harness(stored=[Assignment(player=1, pad=one, button=0)])
        lonely.send(command, player=1)
        if word not in lonely.last_error():
            raise SystemExit(
                f"FAIL: {command} with no open session said "
                f"{lonely.last_error()!r}, which does not say which step is "
                f"missing -- the pads are grabbed on that screen, so there is "
                f"no other feedback")
        print(f"  ok  {command}: {lonely.last_error()!r}")

    print("\nand one whose controller has gone away:")
    gone = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                   claims=[], open_pads=[],   # session open, pad not in it
                   state=protocol.STATE_ASSIGNING)
    gone.send("choose_layout", player=1)
    if "no longer open" not in gone.last_error():
        raise SystemExit(
            f"FAIL: an unplugged controller gave {gone.last_error()!r} rather "
            f"than saying the pad is gone")
    print(f"  ok  says {gone.last_error()!r}")

    # ------------------------------------------------------ what is displayed

    print("\nwhat the setup screen is told, which is a different question:")
    # Resolution falls back to stored assignments; display must not. Making
    # display fall back too undoes an earlier fix: the setup screen drew
    # players from a finished session and offered to configure a controller
    # nobody had touched.
    display = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                      claims=[], open_pads=[one],
                      state=protocol.STATE_ASSIGNING)
    if display.srv._players_payload():
        raise SystemExit(
            "FAIL: a session with no claims still lists players -- the setup "
            "screen would draw a slot nobody has touched and offer to "
            "configure the controller in it")
    if display.srv._state_event()["players"]:
        raise SystemExit(
            "FAIL: the state event carries players the user has not claimed")
    print("  ok  session open, nothing claimed: no players drawn")

    display.srv._assigner.assignments = [  # type: ignore[union-attr]
        Assignment(player=2, pad=two, button=0)]
    payload = display.srv._players_payload()
    if [entry["player"] for entry in payload] != [2]:
        raise SystemExit(
            f"FAIL: with slot 2 claimed the screen shows "
            f"{[e['player'] for e in payload]} -- the stored slot 1 leaked "
            f"back into the display")
    if payload[0]["node"] != two.event:
        raise SystemExit("FAIL: the claimed pad's node is not reported")
    print("  ok  once claimed, exactly that slot is drawn")

    ready = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                    state=protocol.STATE_READY)
    if [e["player"] for e in ready.srv._players_payload()] != [1]:
        raise SystemExit(
            "FAIL: with no session open the accepted assignment is not "
            "reported -- the front-end would show no controllers at all "
            "while they are live and republishing")
    print("  ok  with no session, the accepted assignment is what is shown")

    # ------------------------------------------------------------ forget_pad

    print("\nresetting a controller validates before it destroys:")

    def stored_profile(target, scopes=("", "console:n64")):
        profile = profiles.Profile(signature=profiles.signature(target),
                                   name=target.name)
        for scope in scopes:
            profile.record(scope, profiles.Mapping(
                buttons={"a": Binding("button", 1)}, layout="n64"))
        profiles.save(profile)
        if profiles.load(target) is None:
            raise SystemExit("FAIL: the fixture profile did not save")
        return profile

    victim = pad("Reset Me", vid=0x3333, node="event92")

    stored_profile(victim)
    sessionless = Harness(stored=[Assignment(player=1, pad=victim, button=0)])
    sessionless.send("forget_pad", player=1)
    if profiles.load(victim) is None:
        raise SystemExit(
            "FAIL: the profile was deleted although there is no session to "
            "map in -- the pad is left with neither a mapping nor a way to "
            "make one, which is worse than the wrong mapping it had")
    if "session" not in sessionless.last_error():
        raise SystemExit(
            f"FAIL: nothing explained why the reset did not happen "
            f"({sessionless.last_error()!r})")
    print(f"  ok  no session: kept, and says {sessionless.last_error()!r}")

    unplugged = Harness(stored=[Assignment(player=1, pad=victim, button=0)],
                        claims=[], open_pads=[],
                        state=protocol.STATE_ASSIGNING)
    unplugged.send("forget_pad", player=1)
    if profiles.load(victim) is None:
        raise SystemExit(
            "FAIL: the profile was deleted for a controller the session "
            "cannot open -- the wizard cannot run and the mapping is gone")
    if not unplugged.errors:
        raise SystemExit("FAIL: a reset that could not run said nothing")
    print(f"  ok  pad not open: kept, and says {unplugged.last_error()!r}")

    empty_slot = Harness(claims=[], open_pads=[],
                         state=protocol.STATE_ASSIGNING)
    empty_slot.send("forget_pad", player=3)
    if "player 3" not in empty_slot.last_error():
        raise SystemExit(
            f"FAIL: resetting an empty slot said {empty_slot.last_error()!r} "
            f"-- a key that fails without naming the slot is worse than one "
            f"that does nothing")
    if profiles.load(victim) is None:
        raise SystemExit("FAIL: resetting an empty slot deleted somebody "
                         "else's profile")
    print(f"  ok  empty slot: says {empty_slot.last_error()!r}, deletes "
          f"nothing")

    print("\nand when it can run, it takes everything:")
    signature = profiles.signature(victim)
    doit = Harness(stored=[Assignment(player=1, pad=victim, button=0)],
                   claims=[], open_pads=[victim],
                   state=protocol.STATE_ASSIGNING)
    doit.srv._prompted = {signature, "other:pad"}
    doit.srv._save_prompted()
    doit.send("forget_pad", player=1)
    if doit.errors:
        raise SystemExit(f"FAIL: a reset that should work said "
                         f"{doit.last_error()!r}")
    if profiles.load(victim) is not None:
        raise SystemExit(
            "FAIL: a scope survived the reset -- the wizard is re-run and the "
            "old behaviour is still there, from a per-console mapping the "
            "user has forgotten exists")
    print("  ok  every scope is gone, not just the default one")

    if signature in doit.srv._prompted:
        raise SystemExit(
            "FAIL: the 'already asked' record survived -- padmap would never "
            "offer to set this controller up again")
    if signature in doit.srv.prompted_path.read_text():
        raise SystemExit(
            "FAIL: the record was cleared in memory only, so a daemon "
            "restart brings it back and the pad is never offered again")
    if "other:pad" not in doit.srv.prompted_path.read_text():
        raise SystemExit(
            "FAIL: resetting one controller cleared the record for another, "
            "which would re-offer setup for a pad the user already declined")
    print("  ok  the 'already asked' record goes too, and only for this pad")

    if doit.srv._choice is None:
        raise SystemExit(
            "FAIL: the reset threw the mapping away and stopped -- the user "
            "asked for 'this is wrong, fix it' and got only the first half")
    opened = [m for m in doit.messages
              if m.get("event") == "layout_choice" and m.get("active")]
    if not opened:
        raise SystemExit("FAIL: the wizard opened with nothing drawn for it")
    if opened[-1]["kind"] != capture.KIND_LAYOUT:
        raise SystemExit(
            f"FAIL: the reset opened a {opened[-1]['kind']!r} picker rather "
            f"than asking which controller this is")
    print("  ok  and it goes straight into the wizard, not back to the menu")

    # --------------------------------------------------- map_for_game scopes

    print("\nmapping for the game the library is sitting on:")
    lib = pad("Library Pad", vid=0x4444, node="event93")

    nosession = Harness(stored=[Assignment(player=1, pad=lib, button=0)])
    nosession.send("map_for_game")
    if "session" not in nosession.last_error():
        raise SystemExit(
            f"FAIL: no session gave {nosession.last_error()!r}")
    if nosession.srv._choice is not None:
        raise SystemExit("FAIL: a picker opened with no session to work it")
    print(f"  ok  no session: {nosession.last_error()!r}")

    unassigned = Harness(claims=[], open_pads=[], state=protocol.STATE_ASSIGNING)
    unassigned.send("map_for_game", player=2)
    if "player 2" not in unassigned.last_error():
        raise SystemExit(
            f"FAIL: an unassigned slot gave {unassigned.last_error()!r}")
    print(f"  ok  no controller in that slot: {unassigned.last_error()!r}")

    closed = Harness(stored=[Assignment(player=1, pad=lib, button=0)],
                     claims=[], open_pads=[], state=protocol.STATE_ASSIGNING)
    closed.send("map_for_game")
    if "no longer open" not in closed.last_error():
        raise SystemExit(
            f"FAIL: a closed device gave {closed.last_error()!r}")
    print(f"  ok  controller gone: {closed.last_error()!r}")

    nameless = Harness(stored=[Assignment(player=1, pad=lib, button=0)],
                       claims=[], open_pads=[lib],
                       state=protocol.STATE_ASSIGNING)
    nameless.send("map_for_game", console="", key="", title="Mystery Game")
    if "console" not in nameless.last_error():
        raise SystemExit(
            f"FAIL: a game with no console gave {nameless.last_error()!r} -- "
            f"a mapping filed under a console padmap cannot name is one the "
            f"launcher will never look for")
    if nameless.srv._choice is not None:
        raise SystemExit(
            "FAIL: a picker opened offering nothing, which cannot be answered")
    print(f"  ok  no console known: {nameless.last_error()!r}")

    good = Harness(stored=[Assignment(player=1, pad=lib, button=0)],
                   claims=[], open_pads=[lib], state=protocol.STATE_ASSIGNING)
    good.srv._pending_scope = "console:snes"   # left over from an earlier ask
    good.send("map_for_game")
    if good.errors:
        raise SystemExit(f"FAIL: a well-formed request said "
                         f"{good.last_error()!r}")
    choice = good.srv._choice
    if choice is None:
        raise SystemExit("FAIL: no picker opened for a known console and game")
    ids = [option.id for option in choice.options]
    if ids != ["console:n64", "game:n64/goldeneye-007-usa"]:
        raise SystemExit(
            f"FAIL: the strip offers {ids} -- the question is two entries "
            f"wide, console first, because a pad that needs remapping for one "
            f"N64 game usually needs it for all of them and the first entry "
            f"is the one a hurried user confirms")
    if choice.kind != capture.KIND_SCOPE:
        raise SystemExit(
            f"FAIL: the picker calls itself {choice.kind!r}, so the front-end "
            f"titles it as the wrong question")
    if "GoldenEye" not in choice.title:
        raise SystemExit(
            f"FAIL: the picker is titled {choice.title!r}, which does not say "
            f"which game the answer applies to")
    if choice.options[1].label != "GoldenEye 007 (USA)":
        raise SystemExit(
            f"FAIL: the per-game entry reads {choice.options[1].label!r} "
            f"rather than the game's name")
    if [o.layout for o in choice.options] != ["n64", "n64"]:
        raise SystemExit(
            "FAIL: an entry draws a pad that is not the console's -- both are "
            "captured against the N64 control set")
    if good.srv._pending_scope != "":
        raise SystemExit(
            f"FAIL: a scope left over from an abandoned ask "
            f"({good.srv._pending_scope!r}) is still set, and would file this "
            f"capture under a scope nobody chose for it")
    if not [m for m in good.messages if m.get("event") == "layout_choice"
            and m.get("active")]:
        raise SystemExit("FAIL: the picker was never sent to the front-end")
    print("  ok  exactly two options, console first, titled with the game")

    stored_profile(lib, scopes=("console:n64",))
    marked = Harness(stored=[Assignment(player=1, pad=lib, button=0)],
                     claims=[], open_pads=[lib], state=protocol.STATE_ASSIGNING)
    marked.send("map_for_game")
    flags = [option.mapped for option in marked.srv._choice.options]
    if flags != [True, False]:
        raise SystemExit(
            f"FAIL: the strip marks {flags} -- re-mapping a scope replaces "
            f"what is there, and without the mark it is a blind destructive "
            f"act")
    print("  ok  a scope already captured is marked as such")

    profiles.forget(lib)

    # ---------------------------------------------------- the rest of the surface

    print("\nthe mapping command carries both of its arguments:")
    wizard = Harness(stored=[Assignment(player=1, pad=lib, button=0)],
                     claims=[], open_pads=[lib], state=protocol.STATE_ASSIGNING)
    wizard.send("map", player=1, layout="n64", scope="console:n64")
    run = wizard.srv._mapping
    if run is None:
        raise SystemExit(f"FAIL: no wizard started ({wizard.last_error()!r})")
    if run.layout.id != "n64":
        raise SystemExit(
            f"FAIL: the wizard walks the {run.layout.id!r} layout -- the user "
            f"is asked to press controls their pad does not have")
    if run.scope != "console:n64":
        raise SystemExit(
            f"FAIL: the capture is filed under {run.scope!r}, so the mapping "
            f"lands where the launcher never looks for it")
    if run.player != 1:
        raise SystemExit("FAIL: the wizard opened for the wrong player")
    print("  ok  layout and scope both arrive, not just the player")

    print("\nleaving a modal flow without finishing it:")
    picker = Harness(stored=[Assignment(player=1, pad=lib, button=0)],
                     claims=[], open_pads=[lib], state=protocol.STATE_ASSIGNING)
    picker.send("choose_layout", player=1)
    if picker.srv._choice is None:
        raise SystemExit(f"FAIL: no layout picker opened "
                         f"({picker.last_error()!r})")
    picker.send("configure_end")
    if picker.srv._choice is not None:
        raise SystemExit(
            "FAIL: the picker is still open after being dismissed -- the "
            "overlay sits waiting for events that cannot arrive, on a screen "
            "where the pads are grabbed")
    if not [m for m in picker.messages if m.get("event") == "layout_choice"
            and not m.get("active")]:
        raise SystemExit(
            "FAIL: the front-end was never told the picker had closed")
    print("  ok  configure_end closes an open picker and says so")

    half = Harness(stored=[Assignment(player=1, pad=lib, button=0)],
                   claims=[], open_pads=[lib], state=protocol.STATE_ASSIGNING)
    half.send("map", player=1, layout="n64", scope="")
    half.srv._mapping.bindings["a"] = Binding("button", 1)   # one control in
    half.send("configure_end")
    if half.srv._mapping is not None:
        raise SystemExit("FAIL: the wizard stayed open after being dismissed")
    if profiles.load(lib) is not None:
        raise SystemExit(
            "FAIL: an abandoned wizard stored what it had -- the pad then "
            "counts as configured and is never offered again, leaving half "
            "its buttons dead with nothing to say why")
    print("  ok  and it keeps nothing from a wizard abandoned halfway")

    calibrating = Harness(stored=[Assignment(player=1, pad=lib, button=0)],
                          claims=[], open_pads=[lib],
                          state=protocol.STATE_ASSIGNING)
    calibrating.send("calibrate", player=1)
    if calibrating.srv._calibration is None:
        raise SystemExit("FAIL: no calibration to dismiss")
    calibrating.send("configure_end")
    if calibrating.srv._calibration is not None:
        raise SystemExit(
            "FAIL: calibration stayed in flight after the overlay was "
            "dismissed, so the pad's events go on being swallowed by it")
    print("  ok  it ends a calibration too")

    print("\ncommands with nothing to act on stay quiet rather than crash:")
    quiet = Harness()
    quiet.send("skip_control")
    quiet.send("configure_end")
    quiet.send("reset")
    if quiet.errors:
        raise SystemExit(
            f"FAIL: a stray key from the front-end produced "
            f"{quiet.errors!r} -- these arrive whenever a screen is open")
    print("  ok  skip_control, configure_end and reset with nothing running")

    print("\naccepting with nothing claimed:")
    nothing = Harness(claims=[], open_pads=[], state=protocol.STATE_ASSIGNING)
    nothing.send("accept")
    if "nothing assigned" not in nothing.last_error():
        raise SystemExit(
            f"FAIL: accept with no claims gave {nothing.last_error()!r} -- "
            f"confirming an empty session must not look like success")
    if nothing.srv._state != protocol.STATE_ASSIGNING:
        raise SystemExit(
            "FAIL: an empty accept ended the session anyway, taking the "
            "user's chance to claim a slot with it")
    print(f"  ok  says {nothing.last_error()!r} and keeps the session open")

    print("\nresetting the claims keeps the session:")
    live = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                   claims=[Assignment(player=1, pad=one, button=0)],
                   open_pads=[one], state=protocol.STATE_ASSIGNING)
    live.send("reset")
    if live.srv._assigner is None:
        raise SystemExit(
            "FAIL: reset closed the session -- the pads would be released "
            "mid-setup and nothing could be claimed again")
    if live.srv._assigner.assignments:  # type: ignore[union-attr]
        raise SystemExit("FAIL: the claims survived a reset")
    if not [m for m in live.messages if m.get("event") == "state"]:
        raise SystemExit(
            "FAIL: the front-end was not told the slots had emptied, so it "
            "goes on drawing controllers in them")
    print("  ok  claims dropped, session still open, front-end told")

    print("\nsetting an icon:")
    iconpad = pad("Icon Pad", vid=0x5555, node="event94")
    bad = Harness(stored=[Assignment(player=1, pad=iconpad, button=0)])
    bad.send("set_icon", player=1, icon="nintendo-64")
    if "unknown icon" not in bad.last_error():
        raise SystemExit(
            f"FAIL: an icon name padmap has no picture for gave "
            f"{bad.last_error()!r} -- the pad would be drawn with nothing")
    if profiles.load(iconpad) is not None:
        raise SystemExit("FAIL: a rejected icon was written to the profile "
                         "anyway")
    print(f"  ok  {bad.last_error()!r}, and nothing written")

    stored_profile(iconpad, scopes=("console:n64",))
    icon = Harness(stored=[Assignment(player=1, pad=iconpad, button=0)])
    icon.send("set_icon", player=1, icon="gamecube")
    saved = profiles.load(iconpad)
    if saved is None or saved.icon != "gamecube":
        raise SystemExit(
            "FAIL: the chosen icon was not recorded -- the vid/pid guess "
            "stays, and on a resold vendor id that guess is wrong")
    if "console:n64" not in saved.mappings:
        raise SystemExit(
            "FAIL: choosing a picture threw away the button mapping -- the "
            "pad still counts as configured, so it is never offered again "
            "and its console buttons quietly revert")
    if icons.for_pad(iconpad) != "gamecube":
        raise SystemExit("FAIL: the stored icon is not what the pad reports")
    profiles.forget(iconpad)
    print("  ok  recorded, and the existing mapping survives it")

    print("\nasking for status:")
    asked = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                    state=protocol.STATE_READY)
    asked.srv._sdl_lines = ["03000000,padmap Player 1,a:b1,"]
    asked.send("status")
    kinds = [m.get("event") for _c, m in asked.sent]
    if kinds[:1] != ["state"]:
        raise SystemExit(
            f"FAIL: status replied {kinds} -- a front-end that just connected "
            f"has no other way to learn what is assigned")
    if "sdl_mapping" not in kinds:
        raise SystemExit(
            "FAIL: no SDL lines were sent. Pegasus reads sdl_controllers.txt "
            "once at startup, so a front-end that reconnected is running on "
            "whatever that file said at the time")
    lines = [m for _c, m in asked.sent if m.get("event") == "sdl_mapping"][0]
    if lines["lines"] != asked.srv._sdl_lines:
        raise SystemExit("FAIL: the SDL lines sent are not the ones written")
    if any(c is not asked.client for c, _m in asked.sent):
        raise SystemExit("FAIL: status answered somebody other than the asker")
    if asked.events:
        raise SystemExit(
            f"FAIL: status broadcast {[e.get('event') for e in asked.events]} "
            f"to every client -- a passing `padmap status` would redraw a "
            f"front-end that did not ask")
    state = asked.sent[0][1]
    for field in ("state", "slots", "players", "build", "pid", "identity"):
        if field not in state:
            raise SystemExit(
                f"FAIL: the state event has no {field!r}; a client cannot "
                f"tell whether this daemon is the one it expects")
    print("  ok  state then sdl_mapping, to the asker alone")

    print("\ncancelling a session that was never opened:")
    stray = Harness(stored=[Assignment(player=1, pad=one, button=0)],
                    state=protocol.STATE_READY)
    stray.send("cancel")
    if stray.srv._state != protocol.STATE_READY:
        raise SystemExit(
            f"FAIL: cancel dropped the daemon to {stray.srv._state!r} while "
            f"the virtual pads are still running -- the front-end believes "
            f"nothing is assigned when the controllers are live")
    print("  ok  stays ready; the virtual pads are still on the air")

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
