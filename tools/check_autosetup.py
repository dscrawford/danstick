"""When may plugging in a controller open the setup screen by itself?

The feature is small; the ways it can go wrong are not. Opening a session
stops republishing and takes EVIOCGRAB on every pad, so firing at the wrong
moment does not merely show an unwanted screen -- it takes the controllers
away from whatever was using them. Each case below is one of those moments.

Runs against a real Server object with `devices.discover` and `_begin`
stubbed. Deliberately not a live daemon: a second daemon on this machine
would try to grab pads the real one is already holding.

    python3 tools/check_autosetup.py
"""

import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import devices, profiles, protocol, server  # noqa: E402
from padmap.assign import Assignment  # noqa: E402
from padmap.devices import Pad  # noqa: E402


def pad(name: str, vid: int = 0x1234, pid: int = 0x0001, node: str = "event90"):
    return Pad(path=f"/dev/input/{node}", name=name, phys="", uniq="",
               vid=vid, pid=pid, syspath="")


class Harness:
    """A Server with the two things that touch the machine stubbed out."""

    def __init__(self, pads, state=protocol.STATE_IDLE, clients=1,
                 client_age=60.0):
        import time as _time
        self.srv = server.Server()
        self.srv._state = state

        # Real Client objects: how long one has been connected is what
        # separates a front-end from a passing status query.
        def client(_n):
            fake = type("FakeClient", (), {})()
            fake.connected_at = _time.monotonic() - client_age
            return fake

        self.srv._clients = {n: client(n) for n in range(clients)}  # type: ignore[assignment]
        self.began: list[int] = []
        self.events: list[dict] = []

        devices.discover = lambda *a, **k: list(pads)  # type: ignore[assignment]
        self.srv._begin = lambda players: self.began.append(players)  # type: ignore[assignment]
        self.srv._broadcast = lambda message: self.events.append(message)  # type: ignore[assignment]

    def scan(self):
        # Defeat the rate limit so a check does not have to sleep.
        self.srv._last_pad_scan = 0.0
        self.srv._poll_new_controllers()
        return self


def check(label, harness, *, expect_begin, playing=False, keep_marker=False):
    # Each case owns the marker. Leaving one behind made every later negative
    # case pass for the wrong reason -- they were all "a game is running".
    marker = protocol.playing_marker()
    marker.parent.mkdir(parents=True, exist_ok=True)
    if not keep_marker:
        marker.unlink(missing_ok=True)
    if playing:
        import os
        marker.write_text(f"{os.getpid()}\n")   # a pid that certainly exists

    harness.scan()
    opened = bool(harness.began)
    if opened != expect_begin:
        want = "open setup" if expect_begin else "stay quiet"
        raise SystemExit(f"FAIL {label}: expected it to {want}, it did not")
    kinds = [e.get("event") for e in harness.events]
    if expect_begin and "newpad" not in kinds:
        raise SystemExit(f"FAIL {label}: no newpad event ({kinds})")
    if not expect_begin and "newpad" in kinds:
        raise SystemExit(f"FAIL {label}: emitted newpad while staying quiet")
    print(f"  ok  {label}")


def main() -> int:
    store = Path(tempfile.mkdtemp(prefix="padmap-autosetup-"))
    runtime = store / "run"
    (runtime / "padmap").mkdir(parents=True)
    import os
    os.environ["XDG_RUNTIME_DIR"] = str(runtime)
    os.environ["PADMAP_PROFILE_DIR"] = str(store / "devices")
    os.environ.pop(server.ENV_NO_AUTOSETUP, None)

    new = pad("Brand New Pad")

    # Negatives first, deliberately. Prompting records the model so it is not
    # offered again, so a positive case run earlier would block every later
    # case using the same pad -- and they would all "pass" for that reason
    # rather than the one under test.
    print("moments a first-time controller must NOT take the pads:")
    check("a game is running", Harness([new]), expect_begin=False,
          playing=True)
    check("no front-end is connected",
          Harness([new], clients=0), expect_begin=False)
    # `padmap ensure-daemon` and padctl connect for milliseconds to read
    # status. Counting those left the machine in `assigning` with every pad
    # grabbed and no front-end running to show anything.
    check("only a passing status query is connected",
          Harness([new], client_age=0.0), expect_begin=False)
    check("a session is already open",
          Harness([new], state=protocol.STATE_ASSIGNING), expect_begin=False)

    os.environ[server.ENV_NO_AUTOSETUP] = "1"
    check("switched off by PADMAP_NO_AUTOSETUP",
          Harness([new]), expect_begin=False)
    del os.environ[server.ENV_NO_AUTOSETUP]

    # The same pad, nothing blocking. If any case above had left a condition
    # set, this would stay quiet and give the whole block away.
    print("\nand with nothing blocking, that same pad:")
    check("opens setup", Harness([new]), expect_begin=True)

    print("\nhaving been offered once:")
    check("declining does not re-prompt a second later",
          Harness([new]), expect_begin=False)

    # ensure-daemon restarts the daemon on every front-end launch, so an
    # in-memory record would mean being asked again every single time.
    restarted = Harness([new])
    if not restarted.srv._prompted:
        raise SystemExit("FAIL: a restarted daemon forgot what it had asked")
    check("nor after a daemon restart", restarted, expect_begin=False)

    print("\na controller model already set up:")
    known = pad("Already Configured Pad", vid=0x2222)
    # With buttons: a profile alone is not "configured" any more, because
    # calibration writes one too and that says nothing about the buttons.
    from padmap.mapping import Binding
    profiles.save(profiles.Profile(
        signature=profiles.signature(known), name=known.name, icon="", axes={},
        mappings={profiles.SCOPE_UNIVERSAL: profiles.Mapping(
            buttons={"a": Binding("button", 1)})}))
    check("stays silent, it is just republished", Harness([known]),
          expect_begin=False)

    print("\none adapter, four identical nodes:")
    quad = [pad("Four Port Adapter", node=f"event9{n}") for n in range(4)]
    harness = Harness(quad).scan()
    if len(harness.began) != 1:
        raise SystemExit(f"FAIL: prompted {len(harness.began)} times, wanted 1")
    print("  ok  prompts once, not once per node")

    print("\nforgetting a controller:")
    # Two memories: the profile store, and the record of having already
    # asked. Clearing only the first is why `padmap forget` appeared to do
    # nothing -- the pad reported itself as never configured and the screen
    # still never appeared.
    from padmap import cli as padmap_cli
    forgotten = Harness([new])
    if not forgotten.srv._prompted:
        raise SystemExit("FAIL: expected a remembered offer to clear")
    removed = padmap_cli._forget_prompted(None)
    if removed < 1:
        raise SystemExit("FAIL: forget cleared no 'already asked' records")
    # The daemon holds its own copy, and must notice the file changing --
    # otherwise forgetting only works after a restart.
    check("is offered again, without restarting the daemon",
          forgotten, expect_begin=True)

    print("\nforget, for a controller that has no profile at all:")
    # The case that mattered and was missed: never configured, so nothing to
    # delete, but stuck in the 'already asked' record -- so it reported
    # itself as new and the daemon still stayed silent. Keying the clear off
    # the deleted profiles meant there was nothing to key off, and forget
    # bailed out early before even reaching it.
    import argparse as _argparse
    orphan = pad("Orphan Pad", vid=0x4444)
    protocol.prompted_path().write_text(profiles.signature(orphan) + chr(10))
    stray = Harness([orphan])
    padmap_cli.cmd_forget(_argparse.Namespace(all=False))
    if protocol.prompted_path().exists() and profiles.signature(orphan) in \
            protocol.prompted_path().read_text():
        raise SystemExit("FAIL: forget left the record for a profile-less pad")
    check("is offered again after forget", stray, expect_begin=True)

    print("\nbacking out of a session:")
    # Opening a session stops republishing. Cancelling used to leave the
    # daemon idle with no virtual pads, so declining the screen the daemon
    # had just opened by itself cost the user their controllers.
    resumed = Harness([new])
    srv = resumed.srv
    srv._assignments = [Assignment(player=1, pad=new, button=0)]
    srv._assigner = object()                       # type: ignore[assignment]
    srv._republisher = None
    calls = []
    srv._end_session = lambda release: setattr(srv, "_assigner", None)  # type: ignore[assignment]
    def fake_start():
        calls.append(True)
        srv._republisher = object()                # type: ignore[assignment]
    srv._start_republisher = fake_start            # type: ignore[assignment]
    srv._cancel()
    if not calls:
        raise SystemExit("FAIL: cancelling did not resume republishing")
    if srv._state != protocol.STATE_READY:
        raise SystemExit(f"FAIL: ended in {srv._state!r}, wanted ready")
    print("  ok  the previous assignments go back on the air")

    print("\nstale marker (padmap-play killed outright):")
    marker = protocol.playing_marker()
    marker.write_text("999999\n")   # a pid that cannot exist
    check("ignored, so a crash cannot disable setup forever",
          Harness([pad("Yet Another Pad", vid=0x3333)]), expect_begin=True,
          keep_marker=True)
    if marker.exists():
        raise SystemExit("FAIL: the stale marker was not cleaned up")
    print("  ok  and the stale marker was removed")

    print("\nresetting a controller from the keyboard:")
    # The command has to be *routed*, not merely defined. A handler that is
    # never reached is indistinguishable from a working one until someone
    # presses the key, and the front-end is the only thing that ever sends it.
    from padmap.mapping import Binding

    h = Harness([pad("Reset Me")])
    h.srv._handle_command(None, {"cmd": "forget_pad", "player": 1})
    errors = [e for e in h.events if e.get("event") == "error"]
    if not errors:
        raise SystemExit("FAIL: forget_pad produced no reply at all")
    if "unknown command" in errors[-1]["message"]:
        raise SystemExit(
            f"FAIL: forget_pad is not routed ({errors[-1]['message']!r}) -- "
            f"the key would do nothing and say nothing")
    print(f"  ok  routed; with no assignment it says {errors[-1]['message']!r}")

    print("\nnothing is thrown away when the wizard could not open:")
    # Deleting first and discovering afterwards that there is no session to
    # map in leaves the controller with no configuration and no way to make
    # one -- strictly worse than the wrong mapping it started with.
    with tempfile.TemporaryDirectory() as tmp:
        directory = Path(tmp)
        target = pad("Session-less Pad")
        profiles.save(profiles.Profile(
            signature=profiles.signature(target), name=target.name,
            mappings={"": profiles.Mapping(buttons={"a": Binding("button", 1)},
                                           layout="n64")}), directory)
        original_dir = profiles.profile_dir
        profiles.profile_dir = lambda: directory  # type: ignore[assignment]
        try:
            h = Harness([target])
            h.srv._assignments = [Assignment(player=1, pad=target, button=0)]
            h.srv._assigner = None          # no session open
            h.srv._handle_command(None, {"cmd": "forget_pad", "player": 1})
            if profiles.load(target, directory) is None:
                raise SystemExit(
                    "FAIL: the profile was deleted even though the wizard "
                    "could not open -- the pad is left with neither a "
                    "mapping nor a way to make one")
        finally:
            profiles.profile_dir = original_dir  # type: ignore[assignment]
    errors = [e for e in h.events if e.get("event") == "error"]
    if not errors or "session" not in errors[-1]["message"]:
        raise SystemExit(
            f"FAIL: no explanation given ({errors[-1]['message'] if errors else None!r})")
    print(f"  ok  kept, and says {errors[-1]['message']!r}")

    print("\nforgetting takes every scope, not just the default:")
    # "Reset this controller" meaning "reset some of this controller" leaves
    # someone re-running the wizard and still meeting old behaviour from a
    # per-console mapping they had forgotten existed.
    with tempfile.TemporaryDirectory() as tmp:
        directory = Path(tmp)
        target = pad("Multi Scope Pad")
        profile = profiles.Profile(
            signature=profiles.signature(target), name=target.name)
        for scope in ("", "console:n64", "game:n64/super-mario-64"):
            profile.record(scope, profiles.Mapping(
                buttons={"a": Binding("button", 1)}, layout="n64"))
        profiles.save(profile, directory)
        if profiles.load(target, directory) is None:
            raise SystemExit("FAIL: the fixture profile did not save")
        if not profiles.forget(target, directory):
            raise SystemExit("FAIL: forget reported nothing to remove")
        if profiles.load(target, directory) is not None:
            raise SystemExit(
                "FAIL: a scope survived the reset -- the wizard would be "
                "re-run and the old behaviour would still be there")
        if profiles.forget(target, directory):
            raise SystemExit(
                "FAIL: forgetting an absent profile claimed to remove one")
    print("  ok  all three scopes gone, and forgetting twice is honest")

    print("\nadapters the installed udev rules have fallen behind on:")
    # Real case: the rules file listed a Fightstick and a USB pad, a GameCube
    # adapter was plugged in later, and RetroArch quietly saw four extra
    # physical controllers beside the virtual ones padmap had made from them.
    from padmap import hide

    with tempfile.TemporaryDirectory() as tmp:
        rules = Path(tmp) / "99-padmap.rules"
        rules.write_text(
            '# Generated by padmap.\n'
            '# USB GamePad\n'
            'SUBSYSTEM=="input", ATTRS{idVendor}=="0079", '
            'ATTRS{idProduct}=="1879", ENV{ID_INPUT_JOYSTICK}=""\n'
        )
        original = hide.RUNTIME_RULES_PATH
        hide.RUNTIME_RULES_PATH = rules
        try:
            covered = pad("USB GamePad", vid=0x0079, pid=0x1879)
            later = pad("GameCube Adapter", vid=0x0079, pid=0x1843)
            missing = hide.unhidden([covered, later])
            if [p.pid for p in missing] != [0x1843]:
                raise SystemExit(
                    f"FAIL: reported {[hex(p.pid) for p in missing]} as "
                    f"unhidden, wanted just 0x1843")
            # Two ports of one adapter are one rule, not two warnings.
            twice = hide.unhidden([later, pad("GameCube Adapter",
                                              vid=0x0079, pid=0x1843,
                                              node="event91")])
            if len(twice) != 1:
                raise SystemExit(
                    f"FAIL: one adapter reported {len(twice)} times")
            if hide.unhidden([covered]):
                raise SystemExit(
                    "FAIL: a pad the rules already cover was reported")
        finally:
            hide.RUNTIME_RULES_PATH = original
    print("  ok  the pad added later is flagged; the covered one is not")

    print("\nwhich pads the udev rules must cover:")
    # Reported: a Fightstick configured as player 1, but "the gc autoconfigured
    # joysticks took up the first four slots". RetroArch's own log showed the
    # adapter's four ports taking player slots 2-5, because the installed
    # rules covered only the pads that had been *assigned* when they were
    # generated. Assignment changes every session; the hardware does not.
    stick = pad("Fightstick", vid=0x0079, pid=0x1830)
    usb = pad("USB GamePad", vid=0x0079, pid=0x1879, node="event91")
    cube = pad("GameCube Adapter", vid=0x0079, pid=0x1843, node="event92")

    covered = [(p.vid, p.pid) for p in hide.targets([stick, usb, cube], [stick])]
    if covered != [(0x0079, 0x1830), (0x0079, 0x1879), (0x0079, 0x1843)]:
        raise SystemExit(
            f"FAIL: rules would cover {[(hex(v), hex(d)) for v, d in covered]} "
            f"-- an unassigned adapter left visible is four extra pads taking "
            f"player slots")

    # And the destructive direction: regenerating while one pad is assigned
    # must not drop the others.
    if len(hide.targets([stick, usb, cube], [])) != 3:
        raise SystemExit("FAIL: pads went missing with nothing assigned")

    # Four ports of one adapter are one rule.
    ports = [pad("GameCube Adapter", vid=0x0079, pid=0x1843, node=f"event9{n}")
             for n in range(4)]
    if len(hide.targets(ports, [])) != 1:
        raise SystemExit("FAIL: one adapter produced more than one rule")
    print("  ok  all three adapters covered, assigned or not; ports deduped")

    print("\ninstalling the rules, rather than printing a script to paste:")
    with tempfile.TemporaryDirectory() as tmp:
        rules_file = Path(tmp) / "99-padmap.rules"
        original_path, original_udevadm = hide.RUNTIME_RULES_PATH, hide._udevadm
        hide.RUNTIME_RULES_PATH = rules_file
        hide._udevadm = lambda *a: ""      # root-only; not what is under test
        try:
            pads = hide.targets([stick, usb, cube], [])
            rules = hide.generate_rules(pads)

            changed, messages = hide.install(rules)
            if not changed or not rules_file.exists():
                raise SystemExit(f"FAIL: nothing installed ({messages})")
            if rules_file.read_text() != rules:
                raise SystemExit("FAIL: the file does not hold the rules")

            # udev keeps rules in memory: a write without a reload changes
            # nothing until reboot, and a controller still visible after
            # padmap said it hid it is the exact silent gap to avoid.
            calls = []
            hide._udevadm = lambda *a: calls.append(a) or ""
            rules_file.write_text("stale")
            hide.install(rules)
            if ("control", "--reload-rules") not in calls:
                raise SystemExit(f"FAIL: rules written without a reload ({calls})")

            # Running twice must be honest about having done nothing.
            again, messages = hide.install(rules)
            if again:
                raise SystemExit(f"FAIL: reinstalled unchanged rules ({messages})")

            # The point of all of it: after installing, nothing is left over.
            if hide.unhidden([stick, usb, cube]):
                raise SystemExit(
                    "FAIL: a pad is still unhidden after installing the rules "
                    "-- the generator and the checker disagree")
        finally:
            hide.RUNTIME_RULES_PATH = original_path
            hide._udevadm = original_udevadm
    print("  ok  written, reloaded, idempotent, and nothing left unhidden")

    print("\na player number resolves to its pad before anything is claimed:")
    # A session starts with no claims, so consulting only those made every
    # per-player command fail for the first seconds of a session -- "no
    # controller assigned to player 1", about a pad that was plainly
    # assigned and republishing. That is what made calibration appear to
    # need a button held first, with nothing saying so.
    target = pad("Assigned Pad")
    h = Harness([target])
    h.srv._assignments = [Assignment(player=1, pad=target, button=0)]
    h.srv._assigner = type("Stub", (), {"assignments": []})()
    if h.srv._pad_for_player(1) is not target:
        raise SystemExit(
            "FAIL: a session with no claims hides an assigned controller")

    # A claim still wins: mid-session, player 1 is whatever just pressed a
    # button, not whatever held the slot before.
    claimed = pad("Just Claimed", node="event95")
    h.srv._assigner = type("Stub", (), {
        "assignments": [Assignment(player=1, pad=claimed, button=0)]})()
    if h.srv._pad_for_player(1) is not claimed:
        raise SystemExit(
            "FAIL: a stored assignment outranked a live claim -- re-assigning "
            "a slot would configure the pad it replaced")
    if h.srv._pad_for_player(4) is not None:
        raise SystemExit("FAIL: an unassigned slot resolved to something")
    print("  ok  claim wins, stored assignment answers when there is none")

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
