"""Does the mapping capture record what was pressed, and refuse the rest?

Reading events is easy; refusing them is the whole job. A pad streams axis
noise, an analogue trigger rests at one end of its range rather than the
middle, and the button that opened the wizard is usually still down when the
first prompt appears. Each of those otherwise fills several controls in with
one accidental input, and the user is left with a mapping that looks complete
and is wrong.

    python3 tools/check_capture.py
"""

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import layouts  # noqa: E402
from padmap.capture import (CAPTURE_GAP_SECONDS, EV_ABS, EV_KEY,  # noqa: E402
                            SKIP_HOLD_SECONDS, Chooser, MappingRun,
                            layout_options, scope_options)


class Event:
    def __init__(self, type_, code, value):
        self.type = type_
        self.code = code
        self.value = value


def press(code):
    return Event(EV_KEY, code, 1)


def release(code):
    return Event(EV_KEY, code, 0)


def axis(code, value):
    return Event(EV_ABS, code, value)


KEYS = list(range(0x120, 0x130))
# {code: (minimum, maximum, rest)}. A stick-shaped pad: everything rests in
# the middle of its range.
AXES = {0: (0, 255, 128), 1: (0, 255, 128), 2: (0, 255, 128), 5: (0, 255, 128),
        0x10: (-1, 1, 0), 0x11: (-1, 1, 0)}

# The same pad with analogue triggers on ABS_Z and ABS_RZ, which rest at their
# *minimum* rather than the middle. Measured on the GameCube adapter here.
TRIGGER_AXES = {0: (0, 255, 128), 1: (0, 255, 128), 2: (0, 255, 0),
                5: (0, 255, 0), 0x10: (-1, 1, 0), 0x11: (-1, 1, 0)}


CLOCK = {"t": 0.0}


def tick(seconds=0.05):
    CLOCK["t"] += seconds


def run(layout_id="snes", held=(), axes=None):
    CLOCK["t"] = 0.0
    return MappingRun(pad=None, player=1, layout=layouts.get(layout_id),
                      keys=KEYS, axes=dict(AXES if axes is None else axes),
                      held=set(held), now=lambda: CLOCK["t"])


def to_kind(r, kind):
    """Skip forward to the first prompt of a given kind.

    Axis input is only accepted for controls that can be an axis. Every layout
    opens with face buttons, so an axis test has to get past them first.
    """
    while r.current is not None and r.current.kind != kind:
        r.skip()
        tick(CAPTURE_GAP_SECONDS)
    return r


def to_dpad(r):
    return to_kind(r, "dpad")


def tap(r, code, seconds=0.05):
    """A press and release short enough to count as a binding.

    Waits out the settling gap first, as a person moving between prompts
    unavoidably would.
    """
    tick(CAPTURE_GAP_SECONDS)
    r.feed(press(code))
    tick(seconds)
    return r.feed(release(code))


def choice(held=(), index=0, options=None):
    CLOCK["t"] = 0.0
    return Chooser(pad=None, player=1,
                   options=options if options is not None else layout_options(),
                   index=index, axes=dict(AXES), held=set(held),
                   now=lambda: CLOCK["t"])


def check_layout_choice() -> None:
    """The console picker, which has to be workable with nothing mapped."""
    print("\nchoosing a console: pushing the d-pad right:")
    c = choice()
    c.feed(axis(0x10, 1))
    if c.chosen != list(layouts.ALL)[1]:
        raise SystemExit(f"FAIL: expected to move one along, got {c.chosen!r}")
    print(f"  ok  moved to {c.chosen}")

    print("\nholding it over does not keep moving:")
    c.feed(axis(0x10, 1))
    c.feed(axis(0x10, 1))
    if c.chosen != list(layouts.ALL)[1]:
        raise SystemExit(
            f"FAIL: a held direction walked the list to {c.chosen!r}")
    c.feed(axis(0x10, 0))
    c.feed(axis(0x10, 1))
    if c.chosen != list(layouts.ALL)[2]:
        raise SystemExit("FAIL: releasing and pushing again did not move")
    print("  ok  one move per push, and it moves again after a release")

    print("\na stick that rests off centre can still be used repeatedly:")
    # Measured on the N64 adapter here: it rests at 174 on a 0-255 axis, 36%
    # deflected. The wizard's re-arming rule wants an axis back inside 30% of
    # centre, which this pad never reaches -- so a picker sharing that rule
    # would move once and then ignore the stick forever.
    c = choice()
    for _ in range(3):
        c.feed(axis(0, 255))        # pushed fully right
        c.feed(axis(0, 174))        # let go; springs back to its resting lean
    if c.index != 3:
        raise SystemExit(
            f"FAIL: three pushes moved {c.index} place(s) -- an off-centre "
            f"stick stopped being read")
    print("  ok  three pushes, three moves, from a stick resting at 36%")

    print("\nthe button that opened the picker is still held:")
    c = choice(held={0x121})
    CLOCK["t"] = 5.0                # it has been down a long time
    c.feed(release(0x121))
    if c.confirmed:
        raise SystemExit(
            "FAIL: the press that got here chose a console by itself")
    print("  ok  ignored, as the release of something held from before")

    print("\na tap does not confirm, a hold does:")
    c = choice(index=2)
    c.feed(press(0x121))
    tick(0.1)
    c.feed(release(0x121))
    if c.confirmed:
        raise SystemExit("FAIL: a tap confirmed -- a stray press would too")
    c.feed(press(0x122))
    tick(SKIP_HOLD_SECONDS + 0.05)
    changed = c.feed(release(0x122))
    if not (c.confirmed and changed):
        raise SystemExit("FAIL: holding a button did not confirm")
    if c.chosen != list(layouts.ALL)[2]:
        raise SystemExit(f"FAIL: confirmed the wrong console: {c.chosen!r}")
    print(f"  ok  tap ignored, hold chose {c.chosen}")

    print("\nnothing moves after it is confirmed:")
    c.feed(axis(0x10, 1))
    if c.chosen != list(layouts.ALL)[2]:
        raise SystemExit("FAIL: kept moving after the choice was made")
    print("  ok  the selection is final")

    print("\nthe event carries whole layouts, in the daemon's own order:")
    payload = c.to_event()
    offered = [entry["id"] for entry in payload["choices"]]
    if offered != list(layouts.ALL):
        raise SystemExit(
            f"FAIL: the front-end would be offered {offered}, not "
            f"{list(layouts.ALL)}")
    if not payload["choices"][2]["layout"]["controls"]:
        raise SystemExit(
            "FAIL: no controls sent, so the picker has nothing to draw")
    print(f"  ok  {len(offered)} consoles, drawable, matching layouts.ALL")


def check_scope_choice() -> None:
    """The same picker asking what a mapping is *for*.

    Worth its own check rather than trusting the shared mechanism, because
    the two questions differ in the one place a shared mechanism can still go
    wrong: what the selected entry *means*. A scope option's id is a scope
    string while the thing drawn beside it is a console's layout, and getting
    those the same way round is what stops "GameCube games" being mapped as
    an N64 pad.
    """
    print("\nchoosing what a mapping is for:")
    options = scope_options(
        scopes={"console:n64"}, default_layout="gamecube",
        recent=[("n64", "n64/super-mario-64", "Super Mario 64")])
    c = choice(options=options)
    if c.chosen != "":
        raise SystemExit(
            f"FAIL: the strip does not start on the default scope "
            f"({c.chosen!r}). The commonest answer must be one hold away.")
    payload = c.to_event()
    if payload["choices"][0]["layout"]["id"] != "gamecube":
        raise SystemExit(
            "FAIL: 'any game' is not drawn as the pad's best guess, so the "
            "picker opens on a picture of a controller nobody has")

    # Walk to the N64 console entry and check the id/picture pair.
    while not c.chosen.startswith("console:n64"):
        if not c.feed(axis(0x10, 1)):
            raise SystemExit("FAIL: could not reach the N64 scope")
        c.feed(axis(0x10, 0))
    entry = c.to_event()["choices"][c.index]
    if entry["layout"]["id"] != "n64":
        raise SystemExit(
            f"FAIL: the N64 scope draws {entry['layout']['id']!r}. The picture "
            f"and the mapping the wizard walks must be the same pad.")
    if not entry["mapped"]:
        raise SystemExit(
            "FAIL: a scope that already has a capture is not marked, so "
            "re-mapping it would silently destroy what is there")
    print(f"  ok  {entry['label']!r} -> {c.chosen!r}, drawn as n64, marked")

    game = c.to_event()["choices"][-1]
    if not game["id"].startswith("game:") or game["label"] != "Super Mario 64":
        raise SystemExit(
            f"FAIL: the game just played is not offered as a scope ({game})")
    if game["layout"]["id"] != "n64":
        raise SystemExit(
            "FAIL: the per-game scope is not drawn as its console's pad")
    print(f"  ok  {game['label']!r} -> {game['id']!r}")

    print("\nasked from a game, the question is two entries wide:")
    # Reached from the library, both facts are already known -- this is an N64
    # game and it is GoldenEye -- so there is nothing to scroll past.
    from padmap.capture import game_scope_options
    pair = game_scope_options("n64", "n64/goldeneye-007-usa",
                              "GoldenEye 007 (USA)", {"console:n64"})
    if [o.id for o in pair] != ["console:n64", "game:n64/goldeneye-007-usa"]:
        raise SystemExit(f"FAIL: offered {[o.id for o in pair]}")
    if not all(o.layout == "n64" for o in pair):
        raise SystemExit(
            f"FAIL: {[(o.id, o.layout) for o in pair]} -- a mapping for one "
            f"N64 game is still a capture of the N64 control set")
    if not pair[0].mapped or pair[1].mapped:
        raise SystemExit("FAIL: the existing console mapping is unmarked")

    # No console means no scope the launcher would ever look up.
    if game_scope_options("", "", "Mystery", set()):
        raise SystemExit("FAIL: offered a scope for a game with no console")
    print(f"  ok  {[o.label for o in pair]}, both drawn as n64")

    print("\nseveral recent games are all offered, each once:")
    options = scope_options(
        scopes={"game:n64/smash"}, default_layout="gamecube",
        recent=[("n64", "n64/goldeneye", "GoldenEye"),
                ("n64", "n64/smash", "Smash"),
                ("n64", "n64/goldeneye", "GoldenEye")])
    games = [o for o in options if o.id.startswith("game:")]
    if [o.id for o in games] != ["game:n64/goldeneye", "game:n64/smash"]:
        raise SystemExit(
            f"FAIL: offered {[o.id for o in games]} -- a game replayed twice "
            f"must not take two slots on a strip worked from the pad")
    if not next(o for o in games if o.id == "game:n64/smash").mapped:
        raise SystemExit(
            "FAIL: an existing per-game mapping is unmarked, so re-mapping "
            "would destroy it with no warning")
    print(f"  ok  {[o.label for o in games]}, Smash marked as already mapped")

    print("\nno recent game, no per-game scope:")
    plain = scope_options(scopes=set(), default_layout="")
    if any(o.id.startswith("game:") for o in plain):
        raise SystemExit(
            "FAIL: a per-game scope was offered with no game to attach it to")
    if any(o.id == "console:generic" for o in plain):
        raise SystemExit(
            "FAIL: 'generic' was offered as a console. No core ever reports "
            "it, so that scope could never resolve.")
    print(f"  ok  {[o.id for o in plain]}")


def check_analogue_triggers() -> None:
    """A trigger that rests at one end of its travel, not in the middle.

    Reported verbatim: "when I registered a gamecube controller, pressing R
    causes it to stay stuck in the interface". Deflection used to be measured
    from the middle of the declared range, so an untouched trigger read as
    fully deflected. Three things followed, and all three are checked here: it
    could answer a prompt nobody had touched it for, it recorded the direction
    it was travelling away *from*, and it could never come back near enough to
    the middle to be re-armed -- so after one press the trigger was dead and
    the wizard stopped responding to it.
    """
    print("\nan untouched analogue trigger is not a press:")
    r = to_kind(run("gamecube", axes=TRIGGER_AXES), "shoulder")
    at = r.index
    for code in (2, 5, 2, 5):
        r.feed(axis(code, 0))       # resting reports, as drivers emit them
    if r.index != at:
        raise SystemExit(
            f"FAIL: a trigger sitting at rest answered a prompt "
            f"({r.bindings})")
    print("  ok  a trigger resting at its minimum reads as untouched")

    print("\npressing one records the direction it was pushed:")
    for value in (20, 90, 180, 255):
        r.feed(axis(2, value))
    if r.index != at + 1:
        raise SystemExit("FAIL: pressing the trigger recorded nothing")
    binding = r.bindings[r.layout.controls[at].canonical]
    if binding.sdl() != "+a2":
        raise SystemExit(
            f"FAIL: recorded {binding.sdl()!r}, wanted '+a2' -- a trigger "
            f"pushed towards its maximum is a positive deflection, and the "
            f"first event of the press is the lowest, not the highest")
    print("  ok  +a2, from a trigger travelling up from zero")

    print("\n...and the other one still works afterwards:")
    for value in (180, 60, 0):
        r.feed(axis(2, value))      # let go
    tick(CAPTURE_GAP_SECONDS)
    for value in (20, 120, 255):
        r.feed(axis(5, value))
    if r.index != at + 2:
        raise SystemExit(
            "FAIL: the second trigger was ignored -- this is the pad going "
            "dead partway through the wizard")
    second = r.bindings[r.layout.controls[at + 1].canonical]
    if second.sdl() != "+a3":
        raise SystemExit(f"FAIL: recorded {second.sdl()!r}, wanted '+a3'")
    print("  ok  +a3, distinct from the first")


def check_stale_rest() -> None:
    """An axis whose reported resting value is not where it actually rests.

    Rest is read from the driver as the wizard opens, and an adapter can hold
    a stale power-on default until the stick is physically moved: the N64
    adapter here claims 174 on a 0-255 axis that really centres at 128, 36%
    out. Every threshold has to tolerate that much error, or the axis re-arms
    only sometimes -- which presents as a wizard that ignores every other
    press. This is what AXIS_RELEASE's headroom above the real rest is for;
    tightening it back to 0.30 fails here.
    """
    print("\na stick whose driver reports the wrong resting value:")
    stale = dict(AXES)
    stale[0] = (0, 255, 174)        # driver says 174; it really centres at 128
    r = to_dpad(run("snes", axes=stale))
    at = r.index
    for value in (200, 255):
        r.feed(axis(0, value))      # pushed hard right
    if r.index != at + 1:
        raise SystemExit("FAIL: a full deflection was not recorded")
    for value in (200, 150, 128):
        # Let go. A release takes far less time than the capture gap, so all
        # of it lands inside one -- which is the only chance there is to
        # notice, since the stick then sits still and stops reporting.
        r.feed(axis(0, value))
    tick(CAPTURE_GAP_SECONDS)
    for value in (100, 40, 0):
        r.feed(axis(0, value))      # a deliberate push the other way
    if r.index != at + 2:
        raise SystemExit(
            "FAIL: the axis never re-armed, so the stick answered one prompt "
            "and then went dead")
    first = r.bindings[r.layout.controls[at].canonical]
    second = r.bindings[r.layout.controls[at + 1].canonical]
    if first.sdl() == second.sdl():
        raise SystemExit(f"FAIL: both directions recorded as {first.sdl()!r}")
    print(f"  ok  {first.sdl()} then {second.sdl()}, from a rest 36% out")


def main() -> int:
    print("the button that opened the wizard is still held:")
    r = run(held={0x121})
    r.feed(release(0x121))          # let go of it
    if r.index != 0:
        raise SystemExit("FAIL: releasing the opening press answered a prompt")
    tap(r, 0x121)
    if r.index != 1:
        raise SystemExit("FAIL: the first real press was ignored")
    print("  ok  its release is swallowed, the next press counts")

    print("\nnothing held at the start costs no press:")
    r = run()
    tap(r, 0x121)
    if r.index != 1:
        raise SystemExit(
            "FAIL: the very first press was eaten even though nothing was held")
    print("  ok  the first press binds straight away")

    print("\nholding any button skips the control:")
    r = run()
    r.feed(press(0x121))
    tick(1.0)                        # longer than SKIP_HOLD_SECONDS
    r.feed(release(0x121))
    if r.index != 1:
        raise SystemExit("FAIL: a long hold did not skip")
    if r.bindings:
        raise SystemExit(f"FAIL: the hold was bound anyway ({r.bindings})")
    print("  ok  skipped, and nothing bound")

    print("\n...and the same button still binds when tapped:")
    tap(r, 0x121)
    if len(r.bindings) != 1:
        raise SystemExit("FAIL: a tap after a skip did not bind")
    print("  ok  a tap is a binding, a hold is a skip")

    print("\nholding a button does not walk the wizard:")
    r = run()
    r.feed(press(0x121))
    for _ in range(5):
        r.feed(Event(EV_KEY, 0x121, 2))     # autorepeat
    if r.index != 0:
        raise SystemExit(f"FAIL: autorepeat advanced to {r.index}")
    tick(0.05)
    r.feed(release(0x121))
    if r.index != 1:
        raise SystemExit("FAIL: the release did not bind")
    print("  ok  autorepeat ignored; the release is what binds")

    print("\none button cannot answer two prompts:")
    r = run()
    tap(r, 0x121)
    tap(r, 0x121)
    if r.index != 1:
        raise SystemExit(
            "FAIL: the same button was accepted for a second control")
    print("  ok  refused, so a stuck button cannot fill the whole layout")

    print("\nresting axis noise is not a press:")
    r = run()
    r = to_dpad(run())
    at = r.index
    for value in (128, 130, 126, 131, 127):
        r.feed(axis(0, value))
    if r.index != at:
        raise SystemExit("FAIL: idle stick jitter answered a prompt")
    print("  ok  nothing within the deadband counts")

    print("\na face button prompt refuses a stick:")
    # Nudging the stick while being asked for a face button used to bind that
    # button to an axis -- and since the axis is the stick, every later stick
    # movement then pressed it. One mapping ended up with cancel on `-a3`.
    r = run()
    r.feed(axis(0, 255))
    if r.index != 0 or r.bindings:
        raise SystemExit(
            f"FAIL: a stick answered a face-button prompt ({r.bindings})")
    print("  ok  ignored -- a face button cannot be a stick")

    print("\n...but a d-pad prompt accepts one:")
    r = to_dpad(run())
    at = r.index
    r.feed(axis(0, 255))
    if r.index != at + 1:
        raise SystemExit("FAIL: a full deflection was ignored on a d-pad")
    binding = r.bindings[r.layout.controls[at].canonical]
    if binding.kind != "axis" or binding.sdl() != "+a0":
        raise SystemExit(f"FAIL: recorded {binding.sdl()!r}, wanted '+a0'")
    print("  ok  recorded as +a0")

    print("\nan axis is recorded by index, not evdev code:")
    r = to_dpad(run())
    at = r.index
    r.feed(axis(5, 255))            # ABS_RZ: code 5, but the fourth axis
    binding = r.bindings[r.layout.controls[at].canonical]
    if binding.sdl() != "+a3":
        raise SystemExit(
            f"FAIL: ABS_RZ recorded as {binding.sdl()!r}; code 5 is axis 3")
    print("  ok  ABS_RZ is axis 3, not axis 5")

    print("\nholding one input across the advance:")
    # Reported: "there is no delay between buttons being set -- if I hold the
    # d-pad too long it registers as two". Whatever a held input does next --
    # an analogue oscillation, a hat bounce, a repeat on another code -- it
    # must not land on the control that just became current.
    r = to_dpad(run())
    r.feed(axis(0, 0))                  # d-pad left, captured
    after_first = r.index
    tick(0.05)
    r.feed(axis(0x11, -1))              # a different code, still mid-hold
    if r.index != after_first:
        raise SystemExit(
            "FAIL: a second input answered the next prompt immediately")
    tick(0.05)
    r.feed(axis(1, 255))                # and another
    if r.index != after_first:
        raise SystemExit("FAIL: input during the settling gap was accepted")
    print("  ok  nothing accepted while the gap is open")

    print("\n...and the next control works once the gap passes:")
    tick(CAPTURE_GAP_SECONDS)
    r.feed(axis(0x11, -1))
    if r.index != after_first + 1:
        raise SystemExit("FAIL: the gap never closed, so the wizard is stuck")
    print("  ok  accepted again after the gap")

    print("\na button held across the gap is not still 'down' after it:")
    r = run()
    r.feed(press(0x121))
    tick(0.05)
    r.feed(release(0x121))              # binds control 1, opens the gap
    first = r.index
    r.feed(press(0x122))                # pressed during the gap
    tick(CAPTURE_GAP_SECONDS + 0.05)
    r.feed(release(0x122))              # released after it
    if r.index != first:
        raise SystemExit(
            "FAIL: a press that began inside the gap was bound on release")
    tap(r, 0x122)
    if r.index != first + 1:
        raise SystemExit("FAIL: the button is now permanently ignored")
    print("  ok  ignored, and the button still works next time")

    print("\na d-pad wired to an analogue axis, pressed once:")
    # Reported from a real N64 adapter: pressing left filled in both left and
    # right. The stick springs back through centre on release and overshoots
    # far enough to read as a deliberate push the other way.
    r = to_dpad(run())
    r.feed(axis(0, 0))          # pushed left
    first = r.index
    tick(CAPTURE_GAP_SECONDS)   # the gap is not what is under test here
    r.feed(axis(0, 255))        # spring-back overshoot, never touched
    if r.index != first:
        raise SystemExit(
            "FAIL: the spring-back answered the next prompt -- one press "
            "filled in two controls")
    print("  ok  one press, one control")

    print("\n...and it works again once it has settled:")
    r.feed(axis(0, 128))        # back at rest
    tick(CAPTURE_GAP_SECONDS)
    r.feed(axis(0, 255))        # now a real push the other way
    if r.index != first + 1:
        raise SystemExit("FAIL: the axis never re-armed, so it is now dead")
    left = r.bindings[r.layout.controls[first - 1].canonical]
    right = r.bindings[r.layout.controls[first].canonical]
    if left.sdl() == right.sdl():
        raise SystemExit(
            f"FAIL: both directions recorded as {left.sdl()!r}")
    print(f"  ok  {left.sdl()} and {right.sdl()}, distinct")

    print("\na hat springs back too:")
    r = to_dpad(run())
    r.feed(axis(0x10, -1))      # hat left
    before = r.index
    tick(CAPTURE_GAP_SECONDS)
    r.feed(axis(0x10, 1))       # bounce the other way without releasing
    if r.index != before:
        raise SystemExit("FAIL: a hat bounce answered the next prompt")
    r.feed(axis(0x10, 0))       # released
    tick(CAPTURE_GAP_SECONDS)
    r.feed(axis(0x10, 1))       # deliberate press right
    if r.index != before + 1:
        raise SystemExit("FAIL: the hat never re-armed")
    print("  ok  bounce ignored, deliberate press after release accepted")

    print("\nthe d-pad is a hat:")
    r = to_dpad(run())
    at = r.index
    r.feed(axis(0x11, -1))
    binding = r.bindings[r.layout.controls[at].canonical]
    if binding.sdl() != "h0.1" or binding.retroarch() != "h0up":
        raise SystemExit(f"FAIL: hat up recorded as {binding.sdl()!r}")
    print("  ok  h0.1 for SDL, h0up for RetroArch")

    print("\nboth button numberings are kept:")
    odd = [0x100, 0x120, 0x121]
    r = MappingRun(pad=None, player=1, layout=layouts.get("snes"),
                   keys=odd, axes={})
    r.now = lambda: CLOCK["t"]
    CLOCK["t"] = 0.0
    tap(r, 0x100)
    binding = r.bindings["a"]
    if binding.sdl() != "b2":
        raise SystemExit(f"FAIL: SDL index {binding.sdl()!r}, wanted b2")
    if binding.retroarch() != "0":
        raise SystemExit(
            f"FAIL: RetroArch index {binding.retroarch()!r}, wanted 0 -- "
            f"storing one number for both silently shifts every binding")
    print("  ok  b2 to SDL and 0 to RetroArch, from one press")

    print("\nskipping a control the pad does not have:")
    r = run()
    r.skip()
    tap(r, 0x121)
    if "a" in r.bindings:
        raise SystemExit("FAIL: the skipped control was bound anyway")
    if r.bindings.get("b") is None:
        raise SystemExit("FAIL: the next control was not captured")
    print("  ok  skipped control left unbound, next one captured")

    print("\nwalking a whole layout:")
    r = run("snes")
    total = len(layouts.get("snes").controls)
    for n in range(total):
        tap(r, 0x120 + n)
    if not r.finished:
        raise SystemExit(f"FAIL: stopped at {r.index} of {total}")
    if len(r.bindings) != total:
        raise SystemExit(
            f"FAIL: {len(r.bindings)} bindings for {total} controls")
    payload = r.to_event()
    if not payload["done"] or payload["layout"]["id"] != "snes":
        raise SystemExit("FAIL: the finished event is wrong")
    print(f"  ok  {total} controls, {total} bindings, done reported")

    print("\nnothing is recorded once it is finished:")
    before = dict(r.bindings)
    tap(r, 0x12f)
    if r.bindings != before:
        raise SystemExit("FAIL: kept recording past the end of the layout")
    print("  ok  further presses ignored")

    check_analogue_triggers()
    check_stale_rest()
    check_layout_choice()
    check_scope_choice()

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
