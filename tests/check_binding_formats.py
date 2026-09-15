"""The two formats one capture is written out in, and what neither will say.

mapping.py turns one press into an SDL database field and a RetroArch
autoconfig value. Neither consumer reports a mapping it cannot read: SDL drops
a malformed line, RetroArch binds a button that does not exist and still calls
the pad configured. So everything that cannot be expressed has to be caught
here, on the way out, or it is caught by the user in a game.

Three failures, all reproduced before they were fixed:

  * a button whose evdev code is below BTN_MISC (KEY_A on a combo adapter, an
    arcade encoder's keyboard codes). RetroArch's udev driver never enumerates
    those, but retroarch_button_index answered None for that as well as for
    "the two consumers agree", and Binding read the None as agreement -- so
    SDL's number was written into the autoconfig, naming a button that does
    not exist or a real but different one.
  * a device name carrying a newline. sdl_line stripped commas, because the
    name sits in a comma-separated field, but not line breaks -- and the
    database is read a line at a time, so one pad wrote two lines and SDL read
    the first as a device with no bindings at all.
  * a hat value that is not one direction bit. .sdl() spelled it, .retroarch()
    raised KeyError from the middle of writing a launch profile.

Stories: S7 (the wizard walks the layout), S14 (a game launches with the
player's mapping), S15 (only padmap's virtual pads reach RetroArch).

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tests/check_binding_formats.py
"""

import os
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

# Before importing padmap: a LIVE daemon owns the real controllers and the
# real config. Nothing here may read or write either.
_SANDBOX = Path(tempfile.mkdtemp(prefix="padmap-formats-"))
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    _dir = _SANDBOX / _var.lower()
    _dir.mkdir(parents=True, exist_ok=True)
    os.environ[_var] = str(_dir)
os.environ["RETROARCH_CONFIG_DIR"] = str(_SANDBOX / "retroarch")
os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "profiles")
os.environ["PADMAP_PAD_IDENTITY"] = "padmap"

from padmap import controllercfg, mapping  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

GUID = "0" * 32


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


# -- S7/S14: a button RetroArch's driver cannot see --------------------------


def check_three_answers_for_a_button_code() -> None:
    print("S7: retroarch_button_index tells the three cases apart")
    # A combo adapter: two keyboard codes and twelve real buttons. The server
    # hands the wizard every EV_KEY code the device reports, unfiltered.
    keys = [30, 48] + list(range(0x120, 0x12C))
    if mapping.retroarch_button_index(keys, 0x121) != 1:
        fail("a button RetroArch does number came back wrong; every binding "
             "on the pad would shift")
    if mapping.retroarch_button_index(keys, 48) != mapping.RA_INVISIBLE:
        fail("KEY_B (48) is not reported as a code RetroArch cannot see. "
             "Answering None here means 'the two consumers agree', and the "
             "SDL number gets written into the autoconfig.")
    if mapping.retroarch_button_index(keys, 0x1FF) is not None:
        fail("a code the pad does not report should answer None, not an "
             "index -- it is not a button at all")
    print("  ok  an index, RA_INVISIBLE and None are three distinct answers")


def check_invisible_button_is_never_written() -> None:
    print("\nS14: a keyboard code is written to SDL and to no autoconfig line")
    keys = [30, 48] + list(range(0x120, 0x12C))
    binding = Binding("button", mapping.sdl_button_index(keys, 48) or 0,
                      ra_index=mapping.retroarch_button_index(keys, 48))
    if binding.sdl() != "b13" or not binding.sdl_visible():
        fail(f"SDL's own number for KEY_B is b13, not {binding.sdl()!r} -- "
             "SDL can see the code and the front-end binding is real")
    if binding.retroarch_visible():
        fail("the binding claims RetroArch can name a code below BTN_MISC. "
             "It counts only the twelve real buttons, so the number written "
             "presses nothing at all and RetroArch still reports the pad as "
             "configured: the control works in Pegasus and is dead in game.")
    try:
        lines = mapping.retroarch_lines({"a": binding})
    except Exception as error:                             # noqa: BLE001
        fail(f"writing the autoconfig for this pad raised "
             f"{type(error).__name__}: {error}. It is written while a game is "
             "launching, so the player would get no controller config at all "
             "-- the binding has to be left out, not thrown at the writer.")
    if lines:
        fail(f"the autoconfig carries {lines} for a button RetroArch cannot "
             "see; the user gets a pad that reads as configured with a dead "
             "confirm button")
    if "a:b13" not in mapping.sdl_mapping(GUID, "Combo Adapter", {"a": binding}):
        fail("the SDL line lost a binding SDL can honour perfectly well; "
             "Pegasus would lose the button too, for nothing")
    print("  ok  b13 to SDL, nothing to RetroArch, no invented number")


def check_invisible_button_can_name_a_real_one() -> None:
    print("\nS14: the worse shape -- the invented number names a real button")
    # One keyboard code, one BTN_MISC code, one joystick button. SDL orders
    # the joystick range first, so KEY_A is b1; RetroArch's driver sees only
    # the two codes at or above BTN_MISC, and *does* have a button 1 -- 0x120.
    keys = [30, 0x100, 0x120]
    sdl_index = mapping.sdl_button_index(keys, 30)
    if sdl_index != 1:
        fail(f"SDL numbers KEY_A b{sdl_index} here; this scenario is built on "
             "it being b1, the same number RetroArch gives a different button")
    binding = Binding("button", sdl_index,
                      ra_index=mapping.retroarch_button_index(keys, 30))
    if binding.retroarch_visible():
        fail("KEY_A is offered to RetroArch as button 1, which on this pad is "
             "0x120 -- a real, different button. The user's A press fires "
             "whatever 0x120 is bound to, in every game.")
    print("  ok  the press is refused rather than aimed at somebody else's "
          "button")


def check_invisible_survives_a_profile_round_trip() -> None:
    print("\nS14: the marking survives being stored and read back")
    keys = [30, 48] + list(range(0x120, 0x12C))
    binding = Binding("button", 13,
                      ra_index=mapping.retroarch_button_index(keys, 48))
    restored = Binding.from_json(binding.to_json())
    if restored != binding or restored.retroarch_visible():
        fail(f"a stored binding came back as {restored!r}; a profile off disk "
             "is written out on every launch, so losing the marking here puts "
             "the dead binding back into the autoconfig")
    # Whatever else a hand-edited profile holds, a negative index is not a
    # button anyone can press.
    if Binding("button", 3, ra_index=-7).retroarch_visible():
        fail("a negative ra_index off disk is offered to RetroArch")
    print("  ok  RA_INVISIBLE round-trips, and any negative index is refused")


def check_ordinary_buttons_are_untouched() -> None:
    print("\nS7: the pads this does not concern are unaffected")
    keys = list(range(0x130, 0x13A))
    for offset, code in enumerate(keys):
        index = mapping.retroarch_button_index(keys, code)
        if index != offset:
            fail(f"button {code:#x} numbered {index}, not {offset} -- every "
                 "binding on an ordinary pad would shift")
    plain = Binding("button", 2, ra_index=2)
    agreed = Binding("button", 2)
    for binding in (plain, agreed):
        if not binding.retroarch_visible() or binding.retroarch() != "2":
            fail(f"{binding!r} no longer writes itself out as button 2")
    if mapping.retroarch_lines({"a": agreed}) != ['input_b_btn = "2"']:
        fail("an ordinary button stopped reaching the autoconfig")
    print("  ok  a pad whose codes all sit at 0x120+ writes exactly as before")


# -- S15: nothing in a name may split a database line -------------------------


def check_no_name_can_split_a_line() -> None:
    print("\nS15: a device name cannot break the SDL entry into two lines")
    names = [
        ("a newline", "Line\nBreak Pad", "LineBreak Pad"),
        ("a carriage return", "Retro\rPad", "RetroPad"),
        ("CRLF", "Windows\r\nPad", "WindowsPad"),
        ("a vertical tab", "Vert\x0bPad", "VertPad"),
        ("a form feed", "Form\x0cPad", "FormPad"),
        ("U+2028", "Uni\u2028Pad", "UniPad"),
        ("a comma and a newline", "Some,Pad\nMk II", "SomePadMk II"),
    ]
    for label, hostile, expected in names:
        line = mapping.sdl_line(GUID, hostile, {"a": "b0", "dpup": "h0.1"})
        # str.splitlines() is the strict test: SDL splits on \n, and padmap's
        # own rewriter splits on all of these while deciding which lines it
        # owns.
        if len(line.splitlines()) != 1:
            fail(f"{label} in a device name still writes "
                 f"{len(line.splitlines())} physical lines. SDL reads the "
                 "first as a device with no bindings and drops the rest, so "
                 "the pad has no mapping at all and nothing says so.")
        parsed = mapping.parse_sdl_line(line)
        if parsed is None:
            fail(f"the line written for {label} cannot be read back")
        _, name, fields = parsed
        if name != expected:
            fail(f"{label} came back as {name!r}, not {expected!r}")
        if fields.get("a") != "b0" or fields.get("dpup") != "h0.1":
            fail(f"{label} corrupted the fields after the name: {fields}")
    print(f"  ok  {len(names)} line-breaking names, each one field, each one "
          "line, bindings intact")


def check_a_broken_name_cannot_poison_the_database() -> None:
    print("\nS15: and the database keeps its other lines")
    target = _SANDBOX / "sdl" / "sdl_controllers.txt"
    target.parent.mkdir(parents=True, exist_ok=True)
    stranger = "ffffffffffffffffffffffffffffffff,Someone Else Pad,a:b0,"
    target.write_text(stranger + "\n")

    ours = mapping.sdl_line(GUID, "Line\nBreak Pad", {"a": "b0"})
    controllercfg.write_sdl_mappings({1: ours}, target)
    written = target.read_text()
    if stranger not in written:
        fail("rewriting the database dropped a line padmap does not own; "
             "the user's own mappings are in this file")
    entries = [
        mapping.parse_sdl_line(line) for line in written.splitlines()
    ]
    if any(entry is None and line.strip() and not line.startswith("#")
           for entry, line in zip(entries, written.splitlines())):
        # An unguarded newline leaves half a mapping behind as a line padmap
        # does not recognise -- and write_sdl_mappings keeps every line it
        # does not recognise, so the junk would come back on every rewrite.
        fail("the database now holds a line that is not a mapping and not a "
             "comment; padmap keeps such lines forever, so a broken name "
             "would accumulate on every controller assignment")
    if len([e for e in entries if e and e[0] == GUID]) != 1:
        fail("padmap's own pad is not written exactly once")
    print("  ok  one line for our pad, the stranger's line kept, no junk left "
          "for the next rewrite to inherit")


# -- S14: a hat value that is not a direction ---------------------------------


def check_stray_hat_values_agree() -> None:
    print("\nS14: a hat value that is not one direction bit")
    for value, label in ((3, "a diagonal"), (0, "centred"), (255, "junk")):
        stray = Binding("hat", 0, value)
        if stray.sdl_visible() or stray.retroarch_visible():
            fail(f"{label} (value {value}) is still offered to a consumer. "
                 "RetroArch's config has one direction word per key and an "
                 "SDL mask of two bits only matches while both are held, so "
                 "accepting it in one and not the other is a d-pad direction "
                 "that works in the front-end and not in game.")
        for render, who in ((stray.sdl, "SDL"), (stray.retroarch, "RetroArch")):
            try:
                render()
            except ValueError:
                pass
            except Exception as error:                     # noqa: BLE001
                fail(f"the {who} spelling of {label} raised "
                     f"{type(error).__name__}. A profile off disk can hold "
                     "this, and it is rendered while the launch profiles are "
                     "written -- the game then starts with no controller "
                     "config at all.")
            else:
                fail(f"the {who} spelling of {label} is {render()!r}; nobody "
                     "can press that")
        try:
            ra = mapping.retroarch_lines({"dpup": stray})
            sdl = mapping.sdl_mapping(GUID, "Stray", {"dpup": stray})
        except Exception as error:                         # noqa: BLE001
            fail(f"writing out a profile holding {label} raised "
                 f"{type(error).__name__}: {error}. The writers run while a "
                 "game is being launched, so the whole controller config is "
                 "lost over one field nobody can press.")
        if ra:
            fail(f"{label} reached the autoconfig as {ra}")
        if "dpup" in sdl:
            fail(f"{label} reached the SDL line as {sdl!r}")
    print("  ok  diagonal, centred and junk hat values are refused by both, "
          "and neither writer dies on one")


def check_a_stray_hat_does_not_cost_the_other_controls() -> None:
    print("\nS14: one bad value does not take the rest of the mapping with it")
    bindings = {
        "a": Binding("button", 1),
        "dpup": Binding("hat", 0, 3),        # the corrupt one
        "dpdown": Binding("hat", 0, 4),
        "lefttrigger": Binding("axis", 2, 1),
    }
    try:
        line = mapping.sdl_mapping(GUID, "Half Broken Pad", bindings)
        ra = mapping.retroarch_lines(bindings)
    except Exception as error:                             # noqa: BLE001
        fail(f"writing a mapping with one corrupt binding raised "
             f"{type(error).__name__}: {error}. That happens while the launch "
             "profiles are written, so the game starts with no controller "
             "config at all -- every control lost over one bad field.")
    for expected in ("a:b1", "dpdown:h0.4", "lefttrigger:+a2"):
        if expected not in line:
            fail(f"{expected} was lost from the SDL line because another "
                 "control held a corrupt value; the whole pad would be "
                 "unusable over one bad field")
    for expected in ('input_b_btn = "1"', 'input_down_btn = "h0down"',
                     'input_l2_axis = "+2"'):
        if expected not in ra:
            fail(f"{expected!r} missing from the autoconfig; one corrupt "
                 "binding must not cost the game every other control")
    if any("up" in entry for entry in ra):
        fail(f"the corrupt direction was written anyway: {ra}")
    print("  ok  three good controls survive, the corrupt direction is the "
          "only thing missing")


def main() -> int:
    print(f"sandbox: {_SANDBOX}\n")

    check_three_answers_for_a_button_code()
    check_invisible_button_is_never_written()
    check_invisible_button_can_name_a_real_one()
    check_invisible_survives_a_profile_round_trip()
    check_ordinary_buttons_are_untouched()

    check_no_name_can_split_a_line()
    check_a_broken_name_cannot_poison_the_database()

    check_stray_hat_values_agree()
    check_a_stray_hat_does_not_cost_the_other_controls()

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
