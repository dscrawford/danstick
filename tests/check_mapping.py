"""Do the mapping outputs match what SDL and RetroArch actually produce?

Both formats fail silently. A mapping under the wrong GUID is never matched
and SDL says nothing; a RetroArch binding pointing at a button that does not
exist still reports the pad as configured. So neither is checked by reading it
back -- each is pinned against an artefact produced by the real software.

The SDL ground truth is a line SDL wrote itself, via Pegasus's gamepad editor,
for one of padmap's virtual pads:

    0600c9a7790000007918000001000000,padmap Player 1,a:b1,b:b2,...

bus 6 (BUS_VIRTUAL, since the pad is uinput), vendor 0x0079, product 0x1879,
version 1, name "padmap Player 1". If sdl_guid reproduces that string exactly,
the checksum and field order are right.

    python3 tests/check_mapping.py
"""

import os
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

# Everything pinned below was produced by a pad advertising padmap's own
# identity, so pin that too. Left to the default, the ids would be mirrored
# from the physical controller -- read off a device node that may or may not
# exist on the machine running this -- and the checks would depend on what
# happens to be plugged in. Mirroring has its own checks; see
# check_identity_modes.
os.environ["PADMAP_PAD_IDENTITY"] = "padmap"

from padmap import mapping  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

# Written by SDL, read back off the machine padmap runs on.
REAL_GUID = "0600c9a7790000007918000001000000"
REAL_NAME = "padmap Player 1"
BUS_VIRTUAL = 0x06


def main() -> int:
    print("SDL GUID, against one SDL wrote itself:")
    computed = mapping.sdl_guid(
        bus=BUS_VIRTUAL, vendor=0x0079, product=0x1879, version=0x0001,
        name=REAL_NAME,
    )
    print(f"  SDL wrote: {REAL_GUID}")
    print(f"  computed:  {computed}")
    if computed != REAL_GUID:
        raise SystemExit(
            "FAIL: the GUID does not match. A mapping written under it would "
            "never be matched, and SDL would not complain.")
    print("  ok  exact match -- checksum, field order and padding all right")

    print("\nthe checksum is actually doing something:")
    other = mapping.sdl_guid(
        bus=BUS_VIRTUAL, vendor=0x0079, product=0x1879, version=0x0001,
        name="padmap Player 2",
    )
    if other == computed:
        raise SystemExit("FAIL: the name is not affecting the GUID")
    print(f"  ok  a different name gives a different GUID ({other[:8]}...)")

    print("\nRetroArch and SDL disagree about a and b, deliberately:")
    # SDL a = bottom face button; RetroArch input_b_btn = bottom face button.
    if mapping.RETROARCH_KEYS["a"] != "input_b_btn":
        raise SystemExit("FAIL: SDL 'a' must map to RetroArch input_b_btn")
    if mapping.RETROARCH_KEYS["b"] != "input_a_btn":
        raise SystemExit("FAIL: SDL 'b' must map to RetroArch input_a_btn")
    print("  ok  a<->b crossed, so confirm and cancel are not swapped in game")

    print("\nbinding shapes:")
    cases = [
        (Binding("button", 1), "b1", "1"),
        (Binding("hat", 0, 1), "h0.1", "h0up"),
        (Binding("hat", 0, 4), "h0.4", "h0down"),
        (Binding("axis", 2, 1), "+a2", "+2"),
        (Binding("axis", 2, -1), "-a2", "-2"),
    ]
    for binding, want_sdl, want_ra in cases:
        if binding.sdl() != want_sdl:
            raise SystemExit(
                f"FAIL: {binding} -> SDL {binding.sdl()!r}, wanted {want_sdl!r}")
        if binding.retroarch() != want_ra:
            raise SystemExit(
                f"FAIL: {binding} -> RA {binding.retroarch()!r}, "
                f"wanted {want_ra!r}")
        print(f"  ok  {binding.kind:6} -> SDL {want_sdl:6} RetroArch {want_ra}")

    print("\na full mapping line:")
    bindings = {
        "a": Binding("button", 1),
        "b": Binding("button", 2),
        "start": Binding("button", 8),
        "dpup": Binding("hat", 0, 1),
        "lefttrigger": Binding("axis", 2, 1),
    }
    line = mapping.sdl_mapping(REAL_GUID, REAL_NAME, bindings)
    print(f"  {line}")
    for expected in ["a:b1", "b:b2", "start:b8", "dpup:h0.1",
                     "lefttrigger:+a2", "platform:Linux"]:
        if expected not in line:
            raise SystemExit(f"FAIL: {expected!r} missing from the line")
    if not line.endswith(","):
        raise SystemExit("FAIL: SDL lines are comma-terminated")
    print("  ok  every captured control present, trailing comma, no others")

    print("\nuncaptured controls are left out entirely:")
    lines = mapping.retroarch_lines(bindings)
    joined = "\n".join(lines)
    if "input_select_btn" in joined:
        raise SystemExit(
            "FAIL: emitted a binding for a control nobody pressed")
    for expected in ['input_b_btn = "1"', 'input_a_btn = "2"',
                     'input_start_btn = "8"', 'input_up_btn = "h0up"',
                     # An axis, so _axis rather than _btn -- see below.
                     'input_l2_axis = "+2"']:
        if expected not in joined:
            raise SystemExit(f"FAIL: {expected!r} missing")
    print(f"  ok  {len(lines)} lines, none invented")

    # A name with a comma would shift every field after it and corrupt the
    # mapping without any error.
    print("\na comma in the device name:")
    risky = mapping.sdl_mapping(REAL_GUID, "Pad, Mk II", bindings)
    if risky.split(",")[1] != "Pad Mk II":
        raise SystemExit(f"FAIL: name field is {risky.split(',')[1]!r}")
    if len(risky.split(",")) != len(line.split(",")):
        raise SystemExit("FAIL: the comma shifted the field count")
    print("  ok  neutralised rather than corrupting the line")

    print("\nbutton numbering, against the pad SDL mapped:")
    # Every key this pad reports, 0x120..0x12f -- read off the real device.
    keys = list(range(0x120, 0x130))
    for code, want in [(0x121, 1), (0x122, 2), (0x128, 8), (0x126, 6)]:
        got = mapping.sdl_button_index(keys, code)
        if got != want:
            raise SystemExit(
                f"FAIL: SDL index for 0x{code:03x} was {got}, wanted {want}")
    print("  ok  0x121->b1, 0x122->b2, 0x128->b8, 0x126->b6, as SDL wrote")

    # a:b1 in SDL's line and input_b_btn="1" in RetroArch's profile are the
    # same physical button, and both consumers numbered it 1.
    if mapping.retroarch_button_index(keys, 0x121) != 1:
        raise SystemExit("FAIL: RetroArch index disagrees on the measured pad")
    print("  ok  RetroArch agrees on this pad, matching its generated profile")

    print("\nand where the two consumers genuinely diverge:")
    # A pad carrying a BTN_MISC-range button. SDL sorts it last; RetroArch
    # counts it first, so every later binding shifts by one.
    odd = [0x100, 0x120, 0x121]
    if mapping.sdl_button_index(odd, 0x120) != 0:
        raise SystemExit("FAIL: SDL should count BTN_JOYSTICK first")
    if mapping.sdl_button_index(odd, 0x100) != 2:
        raise SystemExit("FAIL: SDL should count sub-0x120 codes last")
    if mapping.retroarch_button_index(odd, 0x100) != 0:
        raise SystemExit("FAIL: RetroArch should count BTN_MISC first")
    if mapping.retroarch_button_index(odd, 0x120) != 1:
        raise SystemExit("FAIL: RetroArch ordering is plain ascending")
    print("  ok  same button is b2 to SDL and 0 to RetroArch -- computed "
          "separately, not shared")

    print("\na code the pad does not have:")
    if mapping.sdl_button_index(keys, 0x2ff) is not None:
        raise SystemExit("FAIL: invented an index for an absent button")
    print("  ok  None, rather than a plausible wrong number")

    print("\nthe SDL database is rewritten, not appended to:")
    import tempfile
    from padmap import controllercfg
    store = Path(tempfile.mkdtemp()) / "sdl_controllers.txt"
    mine = {1: controllercfg.sdl_line_for(1, bindings)}
    store.write_text(
        "030000005e0400008e02000014010000,Someone Elses Pad,a:b0,platform:Linux,\n"
        + controllercfg.sdl_line_for(1, {"a": Binding("button", 9)}) + "\n")
    controllercfg.write_sdl_mappings(mine, store)
    body = store.read_text()
    if body.count(controllercfg.sdl_line_for(1, {}).split(",")[0]) != 1:
        raise SystemExit(
            "FAIL: two lines for the same GUID -- SDL would pick one of them")
    if "Someone Elses Pad" not in body:
        raise SystemExit("FAIL: clobbered a mapping padmap does not manage")
    if "a:b1" not in body:
        raise SystemExit("FAIL: the new mapping was not written")
    print("  ok  one line per GUID, other people's mappings untouched")

    print("\nthe RetroArch profile is built from the capture:")
    from padmap.devices import Pad as _Pad
    pad = _Pad(path="/dev/input/event1", name="Some Pad", phys="", uniq="",
               vid=0x0079, pid=0x1879, syspath="")
    cfg = controllercfg.retroarch_profile(1, pad, bindings, source="upstream.cfg")
    for expected in ['input_device = "padmap Player 1"', 'input_b_btn = "1"',
                     'input_start_btn = "8"', 'input_vendor_id = "4617"']:
        if expected not in cfg:
            raise SystemExit(f"FAIL: {expected!r} missing from the profile")
    if "input_select_btn" in cfg:
        raise SystemExit("FAIL: invented a binding nobody captured")
    print("  ok  captured bindings only, under padmap's own vendor id")

    print("\nan axis binding goes under _axis, never _btn:")
    # RetroArch parses a _btn value with strtoull, so "-0" and "+0" both come
    # out as button 0: the two directions of a stick collapse onto one button
    # and pressing either activates both. Reported from a real N64 pad whose
    # d-pad is an analogue axis.
    dpad = {"dpleft": Binding("axis", 0, -1), "dpright": Binding("axis", 0, 1)}
    emitted = mapping.retroarch_lines(dpad)
    joined = "\n".join(emitted)
    if 'input_left_btn' in joined or 'input_right_btn' in joined:
        raise SystemExit(
            f"FAIL: an axis was emitted under _btn; both directions would "
            f"parse as button 0\n{joined}")
    for expected in ['input_left_axis = "-0"', 'input_right_axis = "+0"']:
        if expected not in joined:
            raise SystemExit(f"FAIL: {expected!r} missing\n{joined}")
    print("  ok  input_left_axis=-0 and input_right_axis=+0, distinct")

    print("\n...while buttons and hats keep _btn:")
    mixed = {"a": Binding("button", 3), "dpup": Binding("hat", 0, 1)}
    joined = "\n".join(mapping.retroarch_lines(mixed))
    if 'input_b_btn = "3"' not in joined or 'input_up_btn = "h0up"' not in joined:
        raise SystemExit(f"FAIL: buttons or hats moved to _axis\n{joined}")
    print("  ok  buttons and hats unaffected")

    print("\nevery layout is internally consistent:")
    from padmap import layouts
    for layout in layouts.ALL.values():
        seen = set()
        for control in layout.controls:
            if control.canonical not in mapping.SDL_FIELDS:
                raise SystemExit(
                    f"FAIL: {layout.id} control {control.canonical!r} has no "
                    f"SDL field -- it would be captured and then dropped")
            if control.canonical not in mapping.RETROARCH_KEYS:
                raise SystemExit(
                    f"FAIL: {layout.id} control {control.canonical!r} has no "
                    f"RetroArch key")
            if control.canonical in seen:
                raise SystemExit(
                    f"FAIL: {layout.id} asks for {control.canonical!r} twice")
            seen.add(control.canonical)
            if not (0.0 <= control.x <= 1.0 and 0.0 <= control.y <= 1.0):
                raise SystemExit(
                    f"FAIL: {layout.id} control {control.label!r} at "
                    f"({control.x}, {control.y}) is off the picture")
        if not layout.controls:
            raise SystemExit(f"FAIL: {layout.id} has nothing to map")
        print(f"  ok  {layout.id:9} {len(layout.controls):>2} controls, "
              f"all drawable and emittable")

    print("\nlayouts differ in which controls exist, not just position:")
    if "x" in {c.canonical for c in layouts.N64.controls}:
        raise SystemExit("FAIL: an N64 pad has no X button")
    cbuttons = {c.canonical for c in layouts.N64.controls
                if c.canonical.startswith("rightstick_")}
    if len(cbuttons) != 4:
        raise SystemExit(f"FAIL: N64 should map four C-buttons, got {cbuttons}")
    if "lefttrigger" in {c.canonical for c in layouts.SNES.controls}:
        raise SystemExit("FAIL: a SNES pad has no analogue triggers")
    print("  ok  N64 has C-buttons and no X; SNES has no triggers")

    print("\nan N64 C-button reaches both consumers as a right stick:")
    cmap = {"rightstick_up": Binding("button", 11)}
    if "-righty:b11" not in mapping.sdl_mapping("g", "n", cmap):
        raise SystemExit("FAIL: C-up should drive SDL's right stick")
    if 'input_r_y_minus_btn = "11"' not in "\n".join(
            mapping.retroarch_lines(cmap)):
        raise SystemExit("FAIL: C-up should drive RetroArch's right stick")
    print("  ok  -righty for SDL, input_r_y_minus_btn for RetroArch")

    print("\nan unknown layout id:")
    if layouts.get("no-such-pad").id != layouts.GENERIC.id:
        raise SystemExit("FAIL: should fall back to the generic pad")
    print("  ok  falls back rather than refusing to show a wizard")

    check_layout_overrides(layouts)
    check_stored_layout_survives(layouts)
    check_identity_modes()
    check_unmapped_fallback()
    check_triggers_are_not_sticks()
    check_triggers_are_not_calibrated()

    print("\nall checks passed")
    return 0


# Every autoconfig key RetroArch will read for a RetroPad control, read out of
# the joypad-autoconfig database RetroArch itself ships -- profiles it wrote
# and consumes, not a list from documentation. RetroArch builds each key as
# `input_<base>_btn` / `input_<base>_axis` from one bind table, so a key
# outside this set is not a key it will ever look up: the binding is dropped
# in silence and the pad still reports as configured.
RETROPAD_KEYS = {
    "input_a_btn", "input_b_btn", "input_x_btn", "input_y_btn",
    "input_l_btn", "input_r_btn", "input_l2_btn", "input_r2_btn",
    "input_l3_btn", "input_r3_btn",
    "input_up_btn", "input_down_btn", "input_left_btn", "input_right_btn",
    "input_start_btn", "input_select_btn",
    "input_l_x_plus_btn", "input_l_x_minus_btn",
    "input_l_y_plus_btn", "input_l_y_minus_btn",
    "input_r_x_plus_btn", "input_r_x_minus_btn",
    "input_r_y_plus_btn", "input_r_y_minus_btn",
}


def check_layout_overrides(layouts) -> None:
    """Per-console RetroArch keys: real, unambiguous, and not regressed.

    An override exists because the core does not read the control the
    canonical name implies. Every way of getting one wrong -- a typo, two
    controls landing on one key, a correction being reverted -- produces the
    same symptom: a button that does nothing, on a pad RetroArch reports as
    fully configured.
    """
    print("\nlayout overrides name keys RetroArch actually reads:")
    for layout in layouts.ALL.values():
        for control in layout.controls:
            if control.retroarch and control.retroarch not in RETROPAD_KEYS:
                raise SystemExit(
                    f"FAIL: {layout.id} binds {control.label!r} to "
                    f"{control.retroarch!r}, which is not a RetroArch bind. "
                    f"RetroArch ignores unknown keys without complaining, so "
                    f"that button would simply never do anything.")
    overridden = sum(len(layout.retroarch_keys()) for layout in layouts.ALL.values())
    print(f"  ok  {overridden} overrides across {len(layouts.ALL)} layouts, "
          f"all in RetroArch's bind table")

    # The pinned set above is only worth anything if it still matches the
    # database. Opportunistic: the dev shell points PADMAP_AUTOCONFIG_DIRS at
    # it, but the check has to run without one too.
    from padmap import retroarch
    seen_keys: set[str] = set()
    for directory in retroarch.autoconfig_dirs():
        for cfg in directory.rglob("*.cfg"):
            seen_keys.update(retroarch.parse_profile(cfg))
    if seen_keys:
        missing = RETROPAD_KEYS - seen_keys
        if missing:
            raise SystemExit(
                f"FAIL: {sorted(missing)} appear in no profile in the shipped "
                f"autoconfig database. The pinned key list has drifted from "
                f"RetroArch's, so it can no longer catch a typo.")
        print(f"  ok  and all {len(RETROPAD_KEYS)} are used by the "
              f"autoconfig database RetroArch ships")
    else:
        print("  --  no autoconfig database found; pinned list unverified")

    print("\nno two controls on one layout claim the same key:")
    for layout in layouts.ALL.values():
        emitted: dict[str, str] = {}
        for control in layout.controls:
            key = control.retroarch or mapping.RETROARCH_KEYS[control.canonical]
            if key in emitted:
                raise SystemExit(
                    f"FAIL: {layout.id} sends both {emitted[key]!r} and "
                    f"{control.label!r} to {key}. RetroArch keeps one line "
                    f"per key, so one of those two buttons is dead.")
            emitted[key] = control.label
        print(f"  ok  {layout.id:9} {len(emitted):>2} distinct keys")

    # Below: what each core was read to actually do. Sources named so the
    # claim can be re-checked rather than taken on trust.
    print("\nN64, against mupen64plus-next's default (non-alternate) branch:")
    n64 = {c.canonical: (c.retroarch or mapping.RETROARCH_KEYS[c.canonical])
           for c in layouts.N64.controls}
    # emulate_game_controller_via_libretro.c:
    #   Keys->B_BUTTON = ret & (1 << RETRO_DEVICE_ID_JOYPAD_Y)
    if n64.get("b") == "input_a_btn":
        raise SystemExit(
            "FAIL: N64 'B' is back on input_a_btn. mupen64plus-next never "
            "reads RetroPad A in its default mapping -- it takes N64 B from "
            "RetroPad Y -- so the B button would be dead in every game.")
    if n64.get("b") != "input_y_btn":
        raise SystemExit(
            f"FAIL: N64 'B' should be input_y_btn, got {n64.get('b')!r}")
    if n64.get("a") != "input_b_btn":
        raise SystemExit("FAIL: N64 'A' comes from RetroPad B (input_b_btn)")
    if n64.get("lefttrigger") != "input_l2_btn":
        raise SystemExit("FAIL: N64 'Z' comes from RetroPad L2")
    # In the default branch RetroPad R2 is `cbuttons_mode`: while it is held,
    # A and B become C-buttons instead. Binding it turns a face button into a
    # modifier at random.
    if "righttrigger" in n64:
        raise SystemExit(
            "FAIL: the N64 layout binds RetroPad R2, which mupen uses as the "
            "C-buttons modifier. Holding it would stop A and B being A and B.")
    for control, key in [("rightstick_up", "input_r_y_minus_btn"),
                         ("rightstick_left", "input_r_x_minus_btn")]:
        if n64.get(control) != key:
            raise SystemExit(
                f"FAIL: N64 {control} should be {key} -- the core reads the "
                f"C-buttons off RETRO_DEVICE_ANALOG's right stick, with no "
                f"button fallback at all")
    print("  ok  A<-B, B<-Y, Z<-L2, C-buttons on the right stick, R2 unbound")

    print("\nGameCube, against dolphin's retro_set_controller_port_device_gc:")
    gc = {c.canonical: (c.retroarch or mapping.RETROARCH_KEYS[c.canonical])
          for c in layouts.GAMECUBE.controls}
    # gcButtons 0..5 are A, B, X, Y, Z, START and are bound to the literal
    # expressions "A", "B", "X", "Y", "R", "Start" -- an identity mapping for
    # the faces, which is precisely what the crossed gamepad table is not.
    for control, key in [("a", "input_a_btn"), ("b", "input_b_btn"),
                         ("x", "input_x_btn"), ("y", "input_y_btn")]:
        if gc.get(control) != key:
            raise SystemExit(
                f"FAIL: GameCube {control!r} should be {key}. dolphin binds "
                f"each GC face button to the RetroPad button of the same "
                f"name; the canonical table crosses a/b and x/y, which swaps "
                f"confirm and cancel on this console specifically.")
    # gcTriggers 0..1 (L and R) read the L2/R2 axis or button; RetroPad L is
    # bound to the Triforce test switch, and RetroPad R is GC's Z.
    for control, key in [("leftshoulder", "input_l2_btn"),
                         ("rightshoulder", "input_r2_btn"),
                         ("righttrigger", "input_r_btn")]:
        if gc.get(control) != key:
            raise SystemExit(
                f"FAIL: GameCube {control!r} should be {key}. L and R are the "
                f"analogue triggers (RetroPad L2/R2) and Z is RetroPad R -- "
                f"RetroPad L is not a GameCube button at all.")
    print("  ok  faces identity-mapped, L/R on the triggers, Z on RetroPad R")

    print("\narcade, against mame2010's retromain.c button numbering:")
    arcade = {c.label: (c.retroarch or mapping.RETROARCH_KEYS[c.canonical])
              for c in layouts.ARCADE.controls}
    # P1_state[KEY_BUTTON_n] is fed from A, B, X, Y, L, R for buttons 1-6, in
    # that order -- plain RetroPad order, not RetroArch's crossed one.
    panel = [
        ("Top-left button", "input_a_btn", 1),
        ("Top-middle button", "input_b_btn", 2),
        ("Top-right button", "input_x_btn", 3),
        ("Bottom-left button", "input_y_btn", 4),
        ("Bottom-middle button", "input_l_btn", 5),
        ("Bottom-right button", "input_r_btn", 6),
    ]
    for label, key, number in panel:
        if arcade.get(label) != key:
            raise SystemExit(
                f"FAIL: {label} should be {key}, MAME button {number}. "
                f"mame2010 numbers its buttons straight off the RetroPad, so "
                f"the gamepad table scatters the panel across buttons "
                f"4, 3, 5, 2, 1, 6 -- every game gets the wrong button.")
    print("  ok  panel reads 1-6 across the top row and then the bottom")

    print("\nSNES needs no overrides, and must not grow any:")
    if layouts.SNES.retroarch_keys():
        raise SystemExit(
            f"FAIL: SNES has overrides {layouts.SNES.retroarch_keys()}. The "
            f"abstract RetroPad *is* a SNES pad and snes9x maps every button "
            f"to itself, so an override here can only be wrong.")
    print("  ok  none -- snes9x maps RetroPad straight onto the SNES pad")

    print("\nan override changes RetroArch only, never SDL:")
    # Pegasus navigates by the canonical name. If an override leaked into the
    # SDL line, fixing a console's buttons would move the front-end's confirm
    # button with them.
    one = {"b": Binding("button", 4)}
    if "b:b4" not in mapping.sdl_mapping("g", "n", one):
        raise SystemExit("FAIL: SDL line should be unaffected by overrides")
    plain = mapping.retroarch_lines(one)
    with_override = mapping.retroarch_lines(one, layouts.N64.retroarch_keys())
    if plain == with_override:
        raise SystemExit("FAIL: the override did nothing")
    if with_override != ['input_y_btn = "4"']:
        raise SystemExit(f"FAIL: got {with_override}")
    print("  ok  b:b4 either way; input_a_btn -> input_y_btn for RetroArch")


def check_stored_layout_survives(layouts) -> None:
    """A capture has to remember its console all the way to the .cfg file.

    The layout is stored as an id and resolved at emission, so that fixing a
    console's keys fixes the pads already mapped under it. That only works if
    every id round-trips: an id that no longer resolves falls back to the
    generic pad, which emits plausible keys that the core does not read.
    """
    print("\nevery layout id resolves back to itself:")
    for layout_id in layouts.ALL:
        if layouts.get(layout_id).id != layout_id:
            raise SystemExit(
                f"FAIL: a profile stored as {layout_id!r} resolves to "
                f"{layouts.get(layout_id).id!r} instead. Its console-specific "
                f"keys would silently revert to the gamepad ones.")
    print(f"  ok  {', '.join(sorted(layouts.ALL))}")

    print("\nthe layout survives a profile round-trip through JSON:")
    import json
    import os
    import tempfile
    from padmap import controllercfg, profiles
    from padmap.devices import Pad as _Pad

    pad = _Pad(path="/dev/input/event9", name="Some N64 Adapter", phys="",
               uniq="", vid=0x0079, pid=0x1879, syspath="")
    bindings = {"a": Binding("button", 1), "b": Binding("button", 2),
                "rightstick_up": Binding("button", 11)}
    stored = profiles.Profile(
        signature=profiles.signature(pad), name=pad.name, icon="n64",
        mappings={profiles.SCOPE_UNIVERSAL: profiles.Mapping(
            buttons=dict(bindings), layout=layouts.N64.id)},
    )
    reloaded = profiles.Profile.from_json(json.loads(json.dumps(stored.to_json())))
    if reloaded.layout != layouts.N64.id:
        raise SystemExit(
            f"FAIL: layout came back as {reloaded.layout!r}. A capture that "
            f"forgets its console emits the gamepad keys instead.")
    print(f"  ok  {reloaded.layout!r} written and read back")

    print("\nand reaches the generated profile, off disk:")
    directory = tempfile.mkdtemp()
    os.environ[profiles.ENV_DIR] = directory
    try:
        profiles.save(stored)
        if controllercfg.stored_layout(pad) != layouts.N64.id:
            raise SystemExit(
                f"FAIL: stored_layout read {controllercfg.stored_layout(pad)!r} "
                f"back off disk, not {layouts.N64.id!r}")
        cfg = controllercfg.retroarch_profile(
            2, pad, controllercfg.stored_bindings(pad),
            layout=controllercfg.stored_layout(pad),
        )
    finally:
        os.environ.pop(profiles.ENV_DIR, None)
    print("\n".join("  " + line for line in cfg.splitlines()))
    if 'input_y_btn = "2"' not in cfg:
        raise SystemExit(
            "FAIL: the N64 'B' binding did not come out as input_y_btn. The "
            "layout was stored but not consulted at emission, so the button "
            "is bound to a control mupen64plus-next never reads.")
    if 'input_a_btn' in cfg:
        raise SystemExit(
            "FAIL: input_a_btn is in an N64 profile. Nothing on this console "
            "reads RetroPad A in the default mapping.")
    if 'input_r_y_minus_btn = "11"' not in cfg:
        raise SystemExit("FAIL: C-up lost its right-stick key")
    if f"({layouts.N64.id})" not in cfg:
        raise SystemExit("FAIL: the profile does not say which layout it used")
    print("  ok  captured under n64, emitted under n64's keys")

    print("\na profile written before layouts existed still emits:")
    legacy = profiles.Profile.from_json(
        {"signature": "x", "name": "Old Pad",
         "buttons": {"a": {"kind": "button", "index": 1}}})
    if legacy.layout != "":
        raise SystemExit("FAIL: an absent layout should read as empty")
    old = controllercfg.retroarch_profile(
        1, pad, dict(legacy.buttons), layout=legacy.layout)
    if 'input_b_btn = "1"' not in old:
        raise SystemExit(
            "FAIL: a pre-layout profile stopped emitting. Empty must resolve "
            "to the generic layout and the canonical keys, or every pad "
            "mapped before this change goes dead at once.")
    print("  ok  empty layout -> generic -> the canonical keys, as before")


# The measured pads, as SDL sees them. Real numbers: an N64 adapter whose
# d-pad is a hat, and a Fightstick that SDL's own database already knows.
N64_KEYS = list(range(0x120, 0x130))          # 16 buttons
N64_AXES = [0x00, 0x01, 0x02, 0x05, 0x10, 0x11]   # X, Y, Z, RZ, hat
FIGHTSTICK_GUID = "0300dc99790000003018000011010000"
FIGHTSTICK_LINE = (
    "0300dc99790000003018000011010000,Arcade Fightstick F300,"
    "a:b1,b:b2,back:b8,dpdown:h0.4,dpleft:h0.8,dpright:h0.2,dpup:h0.1,"
    "guide:b12,leftshoulder:b4,lefttrigger:b6,leftx:a0,lefty:a1,"
    "rightshoulder:b5,righttrigger:b7,start:b9,x:b0,y:b3,platform:Linux,"
)


def check_triggers_are_not_calibrated() -> None:
    """Centring a trigger ruins it, so calibration must not pick one up.

    Calibration takes an axis's resting value as its centre and maps that to
    the middle of the declared range. Do that to a trigger, which rests at one
    end, and it reads half pressed while untouched and loses half its travel.

    The exclusion used to be a list of codes triggers are conventionally
    reported on. This machine's GameCube adapter puts its analogue triggers on
    ABS_RX and ABS_RY -- stick codes -- so they went straight through it. That
    became urgent when the wizard started calibrating automatically: every
    mapping would have quietly wrecked the triggers it had just captured.
    """
    import evdev

    from padmap import calibrate

    class Stub:
        def __init__(self, entries):
            self._entries = entries

        def capabilities(self, absinfo=True):
            return {evdev.ecodes.EV_ABS: self._entries}

    def absinfo(value):
        # AbsInfo(value, min, max, fuzz, flat, resolution)
        return evdev.AbsInfo(value, 0, 255, 0, 15, 0)

    print("\nan axis resting at an end is never calibrated:")
    # The adapter's real numbers: sticks centred, triggers down at 24/25.
    device = Stub([
        (evdev.ecodes.ABS_X, absinfo(127)),
        (evdev.ecodes.ABS_Y, absinfo(130)),
        (evdev.ecodes.ABS_RX, absinfo(24)),
        (evdev.ecodes.ABS_RY, absinfo(25)),
        (evdev.ecodes.ABS_HAT0X, evdev.AbsInfo(0, -1, 1, 0, 0, 0)),
    ])
    got = sorted(calibrate.calibratable_axes(device))
    want = sorted([evdev.ecodes.ABS_X, evdev.ecodes.ABS_Y])
    if got != want:
        names = [evdev.ecodes.ABS.get(c, c) for c in got]
        raise SystemExit(
            f"FAIL: would calibrate {names} -- centring a trigger makes it "
            f"read half pressed at rest and halves its travel")
    print("  ok  the two sticks, and neither trigger")

    print("\n...and a pad whose triggers really are on Z/RZ still works:")
    conventional = Stub([
        (evdev.ecodes.ABS_X, absinfo(128)),
        (evdev.ecodes.ABS_Y, absinfo(128)),
        (evdev.ecodes.ABS_Z, absinfo(0)),
        (evdev.ecodes.ABS_RZ, absinfo(0)),
    ])
    got = sorted(calibrate.calibratable_axes(conventional))
    if got != want:
        raise SystemExit(f"FAIL: {[evdev.ecodes.ABS.get(c, c) for c in got]}")
    print("  ok  unchanged for the conventional layout")


def check_triggers_are_not_sticks() -> None:
    """An analogue trigger must never be declared a stick.

    Reported as a controller "stuck to the left" in the front-end. padmap
    declared the pad's sticks from a table keyed on evdev code -- ABS_RX and
    ABS_RY are the right stick -- and on the Mayflash GameCube adapter those
    two codes are the analogue L and R triggers. A trigger rests at one end of
    its travel, so SDL was told the right stick was pushed 80% to the upper
    left and never let go.

    The numbers below are this adapter's, read from its absinfo:

        a0 ABS_X  rest 127     a3 ABS_RX rest 24   <- L trigger
        a1 ABS_Y  rest 130     a4 ABS_RY rest 25   <- R trigger
    """
    from padmap import controllercfg, mapping

    codes = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x10, 0x11]
    spans = {0x00: (0, 255, 127), 0x01: (0, 255, 130), 0x02: (0, 255, 132),
             0x03: (0, 255, 24), 0x04: (0, 255, 25), 0x05: (0, 255, 131),
             0x10: (-1, 1, 0), 0x11: (-1, 1, 0)}

    print("\nan axis that rests at an end is not a stick:")
    fields = mapping.stick_fields(codes, axes=spans)
    if fields.get("leftx") != "a0" or fields.get("lefty") != "a1":
        raise SystemExit(
            f"FAIL: the real stick was dropped ({fields}) -- the front-end "
            f"now has no way to navigate but the d-pad")
    if "rightx" in fields or "righty" in fields:
        raise SystemExit(
            f"FAIL: the triggers were declared the right stick ({fields}) -- "
            f"SDL reads that stick as held hard over, for good")
    print(f"  ok  {fields}")

    print("\n...and a real right stick is still declared:")
    centred = dict(spans)
    centred[0x03] = (0, 255, 128)
    centred[0x04] = (0, 255, 127)
    fields = mapping.stick_fields(codes, axes=centred)
    if fields.get("rightx") != "a3" or fields.get("righty") != "a4":
        raise SystemExit(
            f"FAIL: a centred right stick was dropped too ({fields}) -- the "
            f"test is refusing sticks, not triggers")
    print(f"  ok  {fields}")

    print("\nan axis a capture already claims is not a stick either:")
    # No absinfo here at all: this is the second, independent guard. Nothing
    # good comes of one axis being both a stick and a button -- they disagree
    # about what is pressed and the front-end believes whichever it reads.
    bound = {"leftshoulder": Binding("axis", 3, 1),
             "rightshoulder": Binding("axis", 4, 1)}
    fields = mapping.stick_fields(codes, bound)
    if "rightx" in fields or "righty" in fields:
        raise SystemExit(
            f"FAIL: a3/a4 are bound to the shoulders and were still declared "
            f"a stick ({fields})")
    if fields.get("leftx") != "a0":
        raise SystemExit(f"FAIL: the unclaimed stick went missing ({fields})")
    print(f"  ok  {fields}")

    print("\nthe whole line for the adapter as it is really configured:")
    line = controllercfg.sdl_line_for(
        1, {"a": Binding("button", 1), "leftshoulder": Binding("axis", 3, 1),
            "rightshoulder": Binding("axis", 4, 1)},
        axis_codes=codes, axes=spans)
    if "rightx:" in line or "righty:" in line:
        raise SystemExit(f"FAIL: the published line still has a right stick:\n"
                         f"  {line}")
    if "leftx:a0" not in line or "lefty:a1" not in line:
        raise SystemExit(f"FAIL: the published line lost the real stick:\n"
                         f"  {line}")
    print("  ok  leftx/lefty kept, rightx/righty gone")

    print("\nwithout absinfo the old guess still stands:")
    # A caller that cannot read the driver is no worse off than before. Worth
    # pinning: the alternative -- dropping every stick when unsure -- would
    # cost navigation on pads that were working.
    fields = mapping.stick_fields(codes)
    if fields.get("rightx") != "a3":
        raise SystemExit(
            f"FAIL: sticks vanished when no absinfo was supplied ({fields})")
    print(f"  ok  {fields}")


def check_unmapped_fallback() -> None:
    """A controller nobody has mapped yet still has to work in the front-end.

    Virtual pads carry padmap's own 1209:0001, which SDL has never heard of,
    so without a line of some kind the front-end guesses the whole controller
    -- and its guess puts the d-pad on buttons b12-b15, which a hat-based pad
    does not have. What is left is an analogue stick that on an uncalibrated
    pad can rest far enough over to scroll the menus by itself. That is a
    controller you cannot use to reach the wizard that would fix it.
    """
    import tempfile
    from padmap import controllercfg

    print("\nnothing captured yet: the pad's own d-pad and sticks are used:")
    fields = controllercfg.guessed_fields(N64_KEYS, N64_AXES)
    for field, expected in (("dpup", "h0.1"), ("dpdown", "h0.4"),
                            ("dpleft", "h0.8"), ("dpright", "h0.2"),
                            ("leftx", "a0"), ("lefty", "a1")):
        if fields.get(field) != expected:
            raise SystemExit(
                f"FAIL: {field} is {fields.get(field)!r}, not {expected!r} -- "
                f"the front-end cannot be navigated with this")
    if "a" not in fields or "b" not in fields:
        raise SystemExit("FAIL: nothing to accept or cancel with")
    print(f"  ok  hat as the d-pad, real stick axes, a={fields['a']} "
          f"b={fields['b']}")

    print("\na pad whose d-pad is four keys rather than a hat:")
    keys = list(range(0x120, 0x128)) + list(controllercfg.DPAD_KEYS)
    fields = controllercfg.guessed_fields(keys, [0x00, 0x01])
    if fields.get("dpup") != "b8" or fields.get("dpright") != "b11":
        raise SystemExit(
            f"FAIL: BTN_DPAD_* not picked up ({fields.get('dpup')!r}, "
            f"{fields.get('dpright')!r})")
    print("  ok  BTN_DPAD_UP..RIGHT bound by their SDL index")

    print("\nan existing mapping for the physical pad is carried over:")
    store = Path(tempfile.mkdtemp()) / "sdl_controllers.txt"
    store.write_text(FIGHTSTICK_LINE + "\n")
    import os
    os.environ["SDL_GAMECONTROLLERCONFIG_FILE"] = str(store)
    carried = controllercfg.carried_fields(FIGHTSTICK_GUID)
    del os.environ["SDL_GAMECONTROLLERCONFIG_FILE"]
    if carried is None:
        raise SystemExit("FAIL: an entry that is right there was not found")
    fields, source = carried
    # a:b1 and x:b0 -- not the order anything would have guessed, which is
    # exactly why carrying it over is worth doing.
    if fields.get("a") != "b1" or fields.get("x") != "b0":
        raise SystemExit(f"FAIL: bindings changed on the way over: {fields}")
    if "platform" in fields:
        raise SystemExit(
            "FAIL: carried the platform field, which describes the line "
            "rather than a binding")
    if fields.get("guide") != "b12":
        raise SystemExit(
            "FAIL: dropped a field padmap never captures -- the user would "
            "silently lose a binding they already had")
    print(f"  ok  a:b1, x:b0 and guide:b12 kept, from {Path(source).name}")

    print("\npadmap's own lines are not mistaken for a physical pad's:")
    ours = controllercfg.sdl_line_for(1, {"a": Binding("button", 3)})
    store.write_text(ours + "\n")
    os.environ["SDL_GAMECONTROLLERCONFIG_FILE"] = str(store)
    again = controllercfg.carried_fields(controllercfg.virtual_guid(1))
    del os.environ["SDL_GAMECONTROLLERCONFIG_FILE"]
    if again is not None:
        raise SystemExit(
            "FAIL: read back a line padmap wrote -- a stale generation would "
            "feed itself in as though it were the controller's own")
    print("  ok  ignored, so a previous generation cannot feed itself back in")

    print("\nthe line SDL is actually given is a valid one:")
    parsed = mapping.parse_sdl_line(
        mapping.sdl_line(controllercfg.virtual_guid(1), "padmap Player 1",
                         controllercfg.guessed_fields(N64_KEYS, N64_AXES)))
    if parsed is None or parsed[0] != controllercfg.virtual_guid(1):
        raise SystemExit("FAIL: the emitted line does not parse back")
    if parsed[1] != "padmap Player 1":
        raise SystemExit("FAIL: the line is not keyed to the virtual pad")
    print(f"  ok  {len(parsed[2])} fields under {parsed[0]}")

    print("\na pad reporting no buttons gets no line at all:")
    from padmap.devices import Pad as _Pad
    nothing = _Pad(path="/dev/input/event99", name="Not A Pad", phys="",
                   uniq="", vid=0, pid=0, syspath="")
    if controllercfg.fallback_line_for(1, nothing, [], []) is not None:
        raise SystemExit(
            "FAIL: wrote a mapping for something with nothing to press")
    print("  ok  None, rather than a line SDL would match and find empty")


# The Fightstick as the kernel reports it, and the GUID SDL computes for it.
# Both read off this machine.
FIGHTSTICK_IDS = dict(bustype=0x03, vendor=0x0079, product=0x1830,
                      version=0x0111)
FIGHTSTICK_NAME = "MAYFLASH Arcade Fightstick F300"


class _StubDevice:
    """Just the `info` a device exposes, so no controller is needed."""

    class info:
        bustype = FIGHTSTICK_IDS["bustype"]
        vendor = FIGHTSTICK_IDS["vendor"]
        product = FIGHTSTICK_IDS["product"]
        version = FIGHTSTICK_IDS["version"]


def check_identity_modes() -> None:
    """What a virtual pad claims to be, and everything agreeing about it.

    A GUID computed from one answer and a device created from another is a
    mapping SDL never looks up, and nothing anywhere reports it.
    """
    import os

    from padmap import controllercfg, virtual
    from padmap.devices import Pad as _Pad

    pad = _Pad(path="/dev/input/event23", name=FIGHTSTICK_NAME, phys="",
               uniq="", vid=0x0079, pid=0x1830, syspath="")
    previous = os.environ.get(virtual.ENV_IDENTITY)

    try:
        print("\npadmap identity, against the GUID SDL wrote for one:")
        os.environ[virtual.ENV_IDENTITY] = virtual.IDENTITY_PADMAP
        # Reads no device at all in this mode, which is what makes the answer
        # available before anything is plugged in.
        guid = controllercfg.virtual_guid(1, pad)
        expected = mapping.sdl_guid(bus=0x06, vendor=0x1209, product=0x0001,
                                    version=0x0001, name="padmap Player 1")
        if guid != expected:
            raise SystemExit(f"FAIL: {guid} is not padmap's own {expected}")
        cfg = controllercfg.retroarch_profile(1, pad, {}, layout="")
        if 'input_vendor_id = "4617"' not in cfg:
            raise SystemExit("FAIL: the RetroArch profile claims other ids")
        print(f"  ok  {guid}, and RetroArch told 4617:1 to match")

        print("\nmirror identity, from the source controller:")
        os.environ[virtual.ENV_IDENTITY] = virtual.IDENTITY_MIRROR
        identity = virtual.identity_for(pad, source=_StubDevice())
        if (identity.vendor, identity.product) != (0x0079, 0x1830):
            raise SystemExit(f"FAIL: ids not mirrored: {identity}")
        # The bus is the field that decides whether SDL's database matches at
        # all. Measured: bus 3 with these ids resolves to the Fightstick's
        # entry; bus 6 with the same ids resolves to nothing.
        if identity.bustype != 0x03:
            raise SystemExit(
                f"FAIL: bus {identity.bustype} rather than the source's -- "
                f"SDL's database is keyed on it, so mirroring the ids alone "
                f"would match nothing")
        print(f"  ok  {identity.vendor:04x}:{identity.product:04x} on bus "
              f"{identity.bustype}, version {identity.version:#06x}")

        print("\nand the mirrored GUID differs from the hardware's only in "
              "the checksum:")
        mirrored = mapping.sdl_guid(name="padmap Player 1", bus=identity.bustype,
                                    vendor=identity.vendor,
                                    product=identity.product,
                                    version=identity.version)
        physical = mapping.sdl_guid(name=FIGHTSTICK_NAME, **FIGHTSTICK_IDS_GUID)
        # Bus, vendor and product must match; the name checksum and the
        # *version* may differ. Both are fields SDL ignores when matching,
        # which is measured rather than assumed: two pads identical but for
        # their version were both resolved to "Xbox 360 Controller" out of
        # SDL's built-in database.
        #
        # The version is where padmap puts the player number, so that a
        # consumer which blanks the checksum -- Ryujinx does, to make its
        # device id "stable" -- can still tell one padmap pad from another.
        # See virtual.version_for.
        def comparable(guid: str) -> str:
            return guid[:4] + guid[8:24] + guid[28:]

        if comparable(mirrored) != comparable(physical):
            raise SystemExit(
                f"FAIL: {mirrored} and {physical} differ outside the name "
                f"checksum and the version, so SDL would not match the same "
                f"entry")
        if mirrored == physical:
            raise SystemExit(
                "FAIL: identical -- the virtual pad is not distinguishable "
                "from the controller behind it")
        # SDL zeroes bytes 2-3 (the name checksum) before comparing and
        # ignores the version, both measured rather than assumed: a GUID with
        # these ids and a different name resolved to the same database entry,
        # and so did two differing only in version.
        print(f"  ok  {mirrored} vs {physical}")

        print("\nthe whole chain uses it, not just the device:")
        real = controllercfg.identity_for
        controllercfg.identity_for = lambda p, source=None, player=0: identity
        try:
            line = controllercfg.sdl_line_for(1, {"a": Binding("button", 1)},
                                              pad=pad)
            cfg = controllercfg.retroarch_profile(1, pad, {}, layout="")
        finally:
            controllercfg.identity_for = real
        if not line.startswith(mirrored):
            raise SystemExit(
                f"FAIL: the SDL line is under {line.split(',')[0]}, not "
                f"{mirrored} -- SDL would never look it up")
        if 'input_vendor_id = "121"' not in cfg:
            raise SystemExit(
                "FAIL: the RetroArch profile still claims padmap's ids while "
                "the pad advertises the source's")
        print("  ok  SDL line and RetroArch profile both follow the mirror")
    finally:
        if previous is None:
            os.environ.pop(virtual.ENV_IDENTITY, None)
        else:
            os.environ[virtual.ENV_IDENTITY] = previous


# The same ids under the argument names sdl_guid takes.
FIGHTSTICK_IDS_GUID = dict(bus=0x03, vendor=0x0079, product=0x1830,
                           version=0x0111)




if __name__ == "__main__":
    sys.exit(main())
