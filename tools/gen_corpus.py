"""Record what the Python answers, so the Rust port can be held to it.

    python3 tools/gen_corpus.py

Writes JSON under rust/crates/padmap-core/tests/corpus/. Each file is a list
of {"in": ..., "out": ...} recorded by calling the real Python function, and
the Rust tests replay them. Not a pile of hand-written expectations: a
hand-written one encodes what the porter *believed* the Python did, which is
exactly the thing that goes wrong.

The cases are chosen for the edges, not for coverage of the happy path. Every
section below is either a shape that reached a real file, a boundary the
Python treats specially, or a difference between the two languages that is
silent in both -- Python's round() is ties-to-even where Rust's rounds away
from zero, and `//` floors where `/` truncates, and both of those run on every
analogue event.

Regenerate after any deliberate change to the Python, and read the diff: a
line moving here is a behaviour change, which is the point.
"""

from __future__ import annotations

import os
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

OUT = REPO / "rust" / "crates" / "padmap-core" / "tests" / "corpus"

from padmap import layouts, mapping, profiles  # noqa: E402
from padmap.mapping import Binding  # noqa: E402


def write(name: str, cases: list[dict]) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / f"{name}.json"
    path.write_text(json.dumps(cases, indent=1, sort_keys=True) + "\n")
    print(f"  {path.relative_to(REPO)}  {len(cases)} cases")


def _try(call) -> dict:
    """Call something that may raise, recording which it did.

    Both outcomes are the contract. `Binding.retroarch` raising for a button
    below BTN_MISC is not an accident to be smoothed over in the port -- it is
    what stops a keyboard code being written into an autoconfig as a plausible
    button number.
    """
    try:
        return {"ok": True, "value": call()}
    except ValueError as error:
        return {"ok": False, "error": str(error)}


# -- bindings ----------------------------------------------------------------
def bindings() -> None:
    cases = []
    shapes = []
    for index in (0, 1, 13, 127):
        shapes.append(Binding("button", index))
        for ra in (None, 0, 11, mapping.RA_INVISIBLE, -2):
            shapes.append(Binding("button", index, ra_index=ra))
    # Every hat value that is a direction, several that are not.
    for value in (0, 1, 2, 3, 4, 5, 6, 8, 9, 12, 15, -1, 16):
        shapes.append(Binding("hat", 0, value))
        shapes.append(Binding("hat", 1, value, ra_index=0))
    for index in (0, 2, 5):
        for value in (-1, 0, 1):
            shapes.append(Binding("axis", index, value))
            shapes.append(Binding("axis", index, value, ra_index=3))

    for binding in shapes:
        cases.append({
            "in": binding.to_json(),
            "out": {
                "sdl_visible": binding.sdl_visible(),
                "retroarch_visible": binding.retroarch_visible(),
                "sdl": _try(binding.sdl),
                "retroarch": _try(binding.retroarch),
            },
        })
    write("bindings", cases)


# -- the two numberings ------------------------------------------------------
def indices() -> None:
    keysets = [
        [],
        list(range(0x120, 0x12c)),                  # an ordinary pad
        [0x100, 0x120, 0x121],                      # BTN_0 alongside joystick
        [0x1e, 0x30, 0x120, 0x121, 0x122],          # a combo adapter's KEY_*
        [0x122, 0x120, 0x121],                      # unsorted input
        list(range(0x130, 0x13a)) + [0x2c0, 0x2c1],  # BTN_TRIGGER_HAPPY
    ]
    cases = []
    for keys in keysets:
        probes = sorted(set(keys) | {0x99, 0x120, 0x100, 0x1e, 0x2c0})
        for code in probes:
            cases.append({
                "in": {"keys": keys, "code": code},
                "out": {
                    "sdl": mapping.sdl_button_index(keys, code),
                    "retroarch": mapping.retroarch_button_index(keys, code),
                },
            })

    axissets = [
        [],
        [0x00, 0x01],
        [0x00, 0x01, 0x03, 0x04],
        [0x00, 0x01, 0x02, 0x05],                    # ABS_Z and ABS_RZ
        [0x00, 0x01, 0x10, 0x11],                    # a hat among the axes
        [0x11, 0x10, 0x05, 0x00],                    # unsorted
        list(range(0x00, 0x18)),                     # every hat code present
    ]
    axis_cases = []
    for codes in axissets:
        for code in sorted(set(codes) | {0x00, 0x05, 0x10, 0x3f}):
            axis_cases.append({
                "in": {"codes": codes, "code": code},
                "out": mapping.axis_index(codes, code),
            })
    write("button_indices", cases)
    write("axis_indices", axis_cases)


# -- SDL ---------------------------------------------------------------------
def guids() -> None:
    names = [
        "padmap Player 1", "padmap Player 2", "",
        "Arcade Fightstick F300",
        "Nintendo Co., Ltd. Pro Controller",
        "\x18 an adapter whose name starts with a control character",
        "Pokémon pad",                       # multi-byte, so the CRC sees more
        "x" * 200,
    ]
    cases = []
    for bus in (0x03, 0x05, 0x06):
        for vendor, product in ((0x0079, 0x1879), (0x057e, 0x2009), (0x1209, 0x0001)):
            for version in (0x0001, 0x8001, 0x0110):
                for name in names:
                    cases.append({
                        "in": {"bus": bus, "vendor": vendor, "product": product,
                               "version": version, "name": name},
                        "out": mapping.sdl_guid(bus=bus, vendor=vendor,
                                                product=product,
                                                version=version, name=name),
                    })
    write("guids", cases)


def sdl_lines() -> None:
    hostile_names = [
        "Ordinary Pad",
        "Evil, Pad",
        "Pad\nTwo",
        "Pad\rTwo",
        "Pad\x0bTwo",
        "Pad\x0cTwo",
        "Pad\x1cTwo",
        "Pad\x85Two",
        "Pad Two",
        "Pad Two",
        ",,,",
        "",
    ]
    cases = []
    for name in hostile_names:
        fields = {"a": "b1", "b": "b2", "leftx": "a0"}
        cases.append({
            "in": {"guid": "0" * 32, "name": name, "fields": fields,
                   "platform": "Linux"},
            "out": mapping.sdl_line("0" * 32, name, fields, "Linux"),
        })

    captures = [
        {},
        {"a": Binding("button", 1)},
        {"a": Binding("button", 1), "b": Binding("button", 2),
         "start": Binding("button", 9), "dpup": Binding("hat", 0, 1)},
        # A half-written profile: a hat naming two directions. SDL must still
        # get a line for everything else.
        {"a": Binding("button", 1), "dpup": Binding("hat", 0, 3)},
        # The N64 C cluster, as right-stick halves.
        {"rightstick_up": Binding("button", 11),
         "rightstick_down": Binding("button", 12),
         "rightstick_left": Binding("button", 13),
         "rightstick_right": Binding("button", 14)},
        # Triggers on axes.
        {"lefttrigger": Binding("axis", 2, -1),
         "righttrigger": Binding("axis", 5, 1)},
        # A button RetroArch cannot see, which SDL still can.
        {"a": Binding("button", 13, ra_index=mapping.RA_INVISIBLE)},
    ]
    mapping_cases = []
    for bindings in captures:
        for sticks in (None, {"leftx": "a0", "lefty": "a1"}):
            mapping_cases.append({
                "in": {
                    "guid": "0600c9a7790000007918000001000000",
                    "name": "padmap Player 1",
                    "bindings": {k: v.to_json() for k, v in bindings.items()},
                    "platform": "Linux",
                    "sticks": sticks,
                },
                "out": mapping.sdl_mapping(
                    "0600c9a7790000007918000001000000", "padmap Player 1",
                    bindings, "Linux", sticks),
            })
    write("sdl_lines", cases)
    write("sdl_mappings", mapping_cases)


def parsed_lines() -> None:
    raw = [
        "",
        "   ",
        "\t\n",
        "# a comment",
        "  # indented",
        "not,a,guid",
        "0" * 32,
        "0" * 32 + ",Pad,",
        "0" * 32 + ",Pad,a:b0,b:b1,platform:Linux,",
        ("0" * 31) + "A,Pad,a:b0,",
        "0" * 32 + ",Pad,a:b0,broken,b:,:c,platform:Linux,",
        "0" * 32 + ",Pad, a : b0 ,platform:Linux,",
        "0" * 32 + ",Pad with spaces,dpup:h0.1,-righty:b11,",
        "0" * 33 + ",Pad,a:b0,",
    ]
    cases = []
    for line in raw:
        parsed = mapping.parse_sdl_line(line)
        cases.append({
            "in": line,
            "out": None if parsed is None else {
                "guid": parsed[0], "name": parsed[1],
                "fields": list(parsed[2].items()),
            },
        })
    write("parsed_sdl_lines", cases)


def sticks() -> None:
    cases = []
    axissets = [[], [0x00, 0x01], [0x00, 0x01, 0x03, 0x04],
                [0x00, 0x01, 0x02, 0x03, 0x04, 0x05]]
    spansets: list[dict | None] = [
        None,
        {0x00: (0, 255, 128), 0x01: (0, 255, 127),
         0x03: (0, 255, 20), 0x04: (0, 255, 235)},     # triggers on RX/RY
        {0x00: (0, 255, 128), 0x01: (0, 255, 128),
         0x03: (0, 255, 128), 0x04: (0, 255, 128)},
        {0x00: (0, 0, 0)},                              # degenerate
    ]
    bindsets = [
        {},
        {"lefttrigger": Binding("axis", 2, 1)},
        {"a": Binding("button", 1)},
    ]
    for codes in axissets:
        for spans in spansets:
            for binds in bindsets:
                cases.append({
                    "in": {
                        "axis_codes": codes,
                        "axes": None if spans is None else
                        [[c, list(s)] for c, s in spans.items()],
                        "bindings": {k: v.to_json() for k, v in binds.items()},
                    },
                    "out": list(mapping.stick_fields(codes, binds, spans).items()),
                })
    write("stick_fields", cases)


# -- RetroArch ---------------------------------------------------------------
def retroarch_lines() -> None:
    captures = [
        {},
        {"a": Binding("button", 1)},
        {"a": Binding("button", 1), "b": Binding("button", 2),
         "x": Binding("button", 3), "y": Binding("button", 4)},
        {"dpup": Binding("hat", 0, 1), "dpdown": Binding("hat", 0, 4),
         "dpleft": Binding("hat", 0, 8), "dpright": Binding("hat", 0, 2)},
        {"dpup": Binding("hat", 0, 3)},                   # not one direction
        {"a": Binding("button", 13, ra_index=mapping.RA_INVISIBLE)},
        {"a": Binding("button", 13, ra_index=11)},
        {"lefttrigger": Binding("axis", 2, -1)},
        # The shadowed-axis case: a button on one half, an axis on the other.
        {"rightstick_up": Binding("button", 11),
         "rightstick_down": Binding("axis", 3, 1)},
        {"rightstick_left": Binding("axis", 2, -1),
         "rightstick_right": Binding("button", 14)},
        # Both halves as buttons: nothing to drop.
        {"rightstick_up": Binding("button", 11),
         "rightstick_down": Binding("button", 12)},
        {"a": Binding("axis", 0, -1)},                    # axis 0, sign matters
    ]
    overridesets: list[dict] = [
        {},
        {"b": "input_y_btn"},                             # what N64 needs
        {"b": ""},                                        # falsy: falls back
        {"a": "input_a_btn", "b": "input_b_btn"},
    ]
    cases = []
    for bindings in captures:
        for overrides in overridesets:
            cases.append({
                "in": {
                    "bindings": {k: v.to_json() for k, v in bindings.items()},
                    "overrides": overrides,
                },
                "out": mapping.retroarch_lines(bindings, overrides),
            })

    shadow_cases = []
    linesets = [
        [],
        ["# a comment"],
        ['input_r_y_minus_btn = "11"', 'input_r_y_plus_axis = "+3"'],
        ['input_r_y_plus_axis = "+3"', 'input_r_y_minus_btn = "11"'],
        ['input_r_x_plus_btn = "12"', 'input_r_x_minus_axis = "-2"'],
        ['input_r_y_minus_axis = "-3"', 'input_r_y_plus_axis = "+3"'],
        ['input_r_y_minus_btn = "11"', 'input_r_y_plus_btn = "12"'],
        ['input_l_x_minus_btn = "1"', 'input_l_x_plus_axis = "+0"',
         'input_l_y_plus_btn = "2"', 'input_l_y_minus_axis = "-1"'],
        ['input_b_btn = "1"', 'no separator here'],
    ]
    for lines in linesets:
        shadow_cases.append({
            "in": lines,
            "out": mapping.drop_shadowed_axis_halves(list(lines)),
        })
    write("retroarch_lines", cases)
    write("shadowed_axis_halves", shadow_cases)


# -- calibration -------------------------------------------------------------
def calibration() -> None:
    """Every case is swept across its whole input range.

    This is the one function on the per-event path, and two of its details
    differ between the languages without either saying so: Python's round() is
    ties-to-even and its `//` floors. Sweeping rather than sampling because a
    disagreement of one count at one value is exactly what would survive a
    spot check and then read as a stick that drifts.
    """
    setups = [
        profiles.AxisCalibration(center=128, minimum=0, maximum=255),
        profiles.AxisCalibration(center=174, minimum=0, maximum=255),
        profiles.AxisCalibration(center=128, minimum=0, maximum=255, flat=10),
        profiles.AxisCalibration(center=128, minimum=0, maximum=255, flat=20),
        profiles.AxisCalibration(center=200, minimum=0, maximum=255,
                                 reach_min=160, reach_max=255),
        profiles.AxisCalibration(center=128, minimum=0, maximum=255,
                                 reach_min=40, reach_max=200),
        profiles.AxisCalibration(center=128, minimum=0, maximum=255,
                                 reach_min=200, reach_max=240),
        # A range that straddles zero, where `//` and `/` disagree.
        profiles.AxisCalibration(center=0, minimum=-32768, maximum=32767),
        profiles.AxisCalibration(center=-1, minimum=-1, maximum=0),
        profiles.AxisCalibration(center=17, minimum=-128, maximum=127, flat=3,
                                 reach_min=-100, reach_max=90),
        # Degenerate: nothing may divide by zero.
        profiles.AxisCalibration(center=0, minimum=0, maximum=0),
        profiles.AxisCalibration(center=128, minimum=128, maximum=128),
        profiles.AxisCalibration(center=128, minimum=0, maximum=255, flat=1000),
    ]
    cases = []
    for cal in setups:
        low = min(cal.minimum, cal.maximum) - 5
        high = max(cal.minimum, cal.maximum) + 5
        if high - low > 4096:
            # A 16-bit axis: sweep the interesting neighbourhoods rather than
            # 65k rows of a straight line.
            probes = sorted({
                *range(low, low + 40),
                *range(high - 40, high + 1),
                *range(cal.center - 40, cal.center + 40),
                *range(-40, 40),
                cal.minimum, cal.maximum, cal.center, 0,
            })
        else:
            probes = list(range(low, high + 1))
        cases.append({
            # The key names a profile actually uses on disk, so a rename on
            # either side shows up here rather than in a user's file.
            "in": {
                "center": cal.center, "min": cal.minimum,
                "max": cal.maximum, "flat": cal.flat,
                "reach_min": cal.reach_min, "reach_max": cal.reach_max,
            },
            "out": {
                "low": cal.low,
                "high": cal.high,
                "fits_evdev": cal.fits_evdev(),
                "midpoint": (cal.minimum + cal.maximum) // 2,
                "applied": [[value, cal.apply(value)] for value in probes],
            },
        })
    write("calibration", cases)


# -- scopes and layouts ------------------------------------------------------
def scopes() -> None:
    roms = [
        "/roms/n64/Super Smash Bros. (U) [!].z64",
        "Legend of Zelda, The (v1.2).z64",
        "/roms/mame/10yard",
        "/mnt/old/roms/Mario 64.z64",
        "  ...Hello___World!!!  .rom",
        "--x--.rom",
        "...z64",
        "!!!.rom",
        "",
        "日本語.rom",
        "MARIO.z64",
        "mario.z64",
        "a.b.c.d.rom",
        "/trailing/slash/",
        ".hidden",
    ]
    cases = []
    for console in ("n64", "arcade", ""):
        for rom in roms:
            cases.append({
                "in": {"console": console, "rom": rom},
                "out": profiles.game_key(console, rom),
            })
    order_cases = []
    for console in ("", "n64"):
        for game in ("", "n64/mario-64"):
            order_cases.append({
                "in": {"console": console, "game": game},
                "out": profiles.scope_order(console, game),
            })
    write("game_keys", cases)
    write("scope_order", order_cases)


def cores() -> None:
    probes = sorted({
        *layouts.CORE_LAYOUTS,
        *(f"{name}_libretro.so" for name in layouts.CORE_LAYOUTS),
        *(f"{name}-libretro.dll" for name in layouts.CORE_LAYOUTS),
        "/nix/store/abc/mupen64plus_next_libretro.so",
        "Mupen64plus_Next_libretro.so",
        "MUPEN64PLUS_NEXT_LIBRETRO.SO",
        "mupen64plus_next_libretro.SO",
        "unknown_core_libretro.so",
        "snes9x_libretro.dylib",
        "",
        "libretro",
        "_libretro.so",
    })
    cases = [{"in": core, "out": layouts.for_core(core)} for core in probes]
    write("cores", cases)

    # Every layout, whole, so a coordinate that moves in one and not the other
    # is a test failure rather than an arrow pointing somewhere wrong.
    write("layouts", [
        {"in": layout_id, "out": layouts.get(layout_id).to_json()}
        for layout_id in layouts.ALL
    ])


# -- profiles ----------------------------------------------------------------
def printable() -> None:
    """Every codepoint the port has to agree with Python about.

    `str.isprintable` decides which characters survive into a signature, and a
    signature is a profile's *filename*. Disagreeing by one codepoint orphans
    every profile whose controller's name contains it -- silently, because a
    profile that cannot be found reads as "never configured" and the wizard
    simply opens again.

    Surrogates are skipped: Python will hold one in a str and Rust's `char`
    cannot exist as one, so there is nothing to compare.
    """
    points = set(range(0x0, 0x600))
    points |= set(range(0x2000, 0x2100))      # the separator and format blocks
    points |= set(range(0xE000, 0xE010))      # private use
    points |= set(range(0xFE00, 0xFF10))      # variation selectors, halfwidth
    points |= {0x1F3AE, 0x1F600, 0xE0001, 0x10FFFF, 0x10FFFE}
    points |= set(range(0x0870, 0x0890))      # recently assigned, and not
    cases = []
    for point in sorted(points):
        if 0xD800 <= point <= 0xDFFF:
            continue
        cases.append({"in": point, "out": chr(point).isprintable()})
    write("printable", cases)


def signatures() -> None:
    names = [
        "N64 Adapter",
        "Pro Controller",
        "  padded  ",
        "\x18 an adapter whose name starts with a control character",
        "\x18\x18\x18",
        "Pok\u00e9mon pad",
        "\u65e5\u672c\u8a9e",
        "tab\there",
        "new\nline",
        "zero\u200bwidth",
        "nbsp\u00a0space",
        "",
        " ",
        "a" * 400,
        "MAYFLASH GameCube Adapter",
        "Sony Interactive Entertainment Wireless Controller",
        "slash/and\\backslash",
        "colon:in:name",
        "....",
        "--_--",
    ]
    cases = []
    for vid, pid in ((0x0079, 0x1879), (0x057e, 0x2009), (0x0000, 0x0000), (0xffff, 0xffff)):
        for name in names:
            sig = profiles.signature(_FakePad(vid, pid, name))
            cases.append({
                "in": {"vid": vid, "pid": pid, "name": name},
                "out": {"signature": sig, "filename": profiles._filename(sig)},
            })
    write("signatures", cases)


class _FakePad:
    """Only the three fields `profiles.signature` reads.

    A real Pad needs a device node; the signature is a pure function of these.
    """

    def __init__(self, vid: int, pid: int, name: str) -> None:
        self.vid = vid
        self.pid = pid
        self.name = name


def stored_profiles() -> None:
    """Whole profiles, in and out, including the shapes a damaged file has."""
    button = Binding("button", 1).to_json()
    raws: list = [
        None,
        [1, 2],
        "text",
        {},
        {"signature": "0079:1879:Pad", "name": "Pad", "icon": "n64"},
        # Written before scopes: flat buttons/layout, no mappings.
        {"signature": "s", "layout": "n64", "buttons": {"a": button}},
        # Older still: buttons with no layout at all.
        {"signature": "s", "buttons": {"a": button}},
        # Legacy keys present *and* scoped mappings -- the scoped ones win.
        {"signature": "s", "layout": "snes", "buttons": {"a": button},
         "mappings": {"": {"layout": "n64", "buttons": {"b": button}}}},
        # Junk in one axis slot, and one axis that is fine.
        {"signature": "s", "axes": {
            "0": {"center": 128, "min": 0, "max": 255},
            "1": "not an object",
            "2": {"center": 1},
            "notanumber": {"center": 128, "min": 0, "max": 255},
        }},
        # Every scope populated.
        {"signature": "s", "mappings": {
            "": {"layout": "gamecube", "buttons": {"a": button}},
            "console:n64": {"layout": "n64", "buttons": {"b": button}},
            "game:n64/mario": {"layout": "n64", "buttons": {"x": button}},
        }},
        # A scope with an empty capture, which must not shadow a general one.
        {"signature": "s", "mappings": {
            "": {"layout": "gamecube", "buttons": {"a": button}},
            "console:n64": {"layout": "n64", "buttons": {}},
        }},
        # A control name padmap does not know.
        {"signature": "s", "mappings": {
            "": {"layout": "", "buttons": {"a": button, "guide": button}}}},
        # A calibration no evdev value can carry.
        {"signature": "s", "axes": {
            "0": {"center": 0, "min": -1099511627776, "max": 1099511627776}}},
        # Deliberately NOT here: a profile whose signature/name/icon are not
        # strings. Python's `str(raw.get(...))` stringifies whatever it finds,
        # so `None` is written back as the literal "None" and `["x"]` as
        # "['x']". The port answers "" instead, and that divergence is pinned
        # in profile.rs rather than frozen here -- writing a lie that looks
        # like data back to a user's file is not behaviour worth reproducing.
    ]
    cases = []
    for raw in raws:
        profile = profiles.Profile.from_json(raw)  # type: ignore[arg-type]
        resolved = {
            f"{console}|{game}": profile.resolve(console, game)[0]
            for console, game in (("", ""), ("n64", ""), ("n64", "n64/mario"),
                                  ("snes", ""), ("", "n64/mario"))
        }
        cases.append({
            "in": raw,
            "out": {
                "json": profile.to_json(),
                "has_bindings": profile.has_bindings(),
                "layout": profile.layout,
                "resolved": resolved,
            },
        })
    write("stored_profiles", cases)



def icon_choices() -> None:
    """Which picture every plausible controller name gets.

    An ordered list of regexes, and the order is the whole rule: an arcade
    stick that mentions Steam is an arcade stick because the arcade pattern is
    first. Reimplementing that by hand is how the two answers drift apart, and
    the drift is invisible -- a wrong icon looks like a design choice.
    """
    from padmap import icons

    class _Pad:
        def __init__(self, vid: int, pid: int, name: str) -> None:
            self.vid, self.pid, self.name = vid, pid, name

    names = [
        "", "Unknown Device", "USB Gamepad",
        "Microsoft X-Box 360 pad 0", "Microsoft X-Box One pad",
        "Xbox Wireless Controller", "X Box pad", "xinput device",
        "MAYFLASH Arcade Fightstick F300", "Qanba Drone", "HORI Fighting Stick",
        "Street Fighter IV Arcade", "Generic Joystick",
        "Nintendo Co., Ltd. N64 Controller", "N64 Adapter", "Nintendo 64 pad",
        "retrolink n64 usb", "Mayflash GameCube Adapter", "Wii U GC adapter",
        "my gc pad", "SNES Controller", "Super Nintendo pad", "SFC gamepad",
        "Nintendo Switch Pro Controller", "Joy-Con (L)", "joycon right",
        "NSO controller", "Sega Genesis pad", "Mega Drive 6B",
        "8BitDo M30", "retro-bit saturn", "Sony DualShock 4",
        "DualSense Wireless Controller", "PlayStation 3 Controller", "PS5 pad",
        "Steam Controller", "Steam Deck", "Valve Software Steam Controller Puck",
        "Steampunk Arcade Fightstick", "Logitech G29 Driving Force",
        "racing wheel", "G27 Racing Wheel",
        # Ordering traps: each of these matches more than one pattern.
        "Nintendo Switch Online SNES Controller",
        "GameCube style Switch Pro Controller",
        "Valve xinput emulator",
        "N64 arcade stick",
    ]
    ids = [(0x1234, 0x5678), (0x0079, 0x1843), (0x045E, 0x028E),
           (0x057E, 0x2019), icons.STEAM_VIRTUAL_ID, (0x28DE, 0x1304)]
    cases = []
    for vid, pid in ids:
        for name in names:
            cases.append({
                "in": {"vid": vid, "pid": pid, "name": name},
                "out": icons.for_pad(_Pad(vid, pid, name), overrides={}),
            })
    write("icons", cases)



def hide_rules() -> None:
    """The udev rules text, byte for byte.

    udev matches these literally: a stray space, a lowercase hex digit where
    the kernel writes uppercase, and the rule covers nothing at all -- with no
    error anywhere, because a rule that matches no device is a legal rule.
    """
    from padmap import hide

    class _Pad:
        def __init__(self, name: str, vid: int, pid: int) -> None:
            self.name, self.vid, self.pid = name, vid, pid

    sets = [
        [],
        [_Pad("Switch Pro", 0x057E, 0x2009)],
        [_Pad("Mayflash GameCube Adapter", 0x0079, 0x1843)] * 4,
        [_Pad("Nameless", 0, 0)],
        [_Pad("Half", 0x0079, 0)],
        [_Pad("A", 0x0079, 0x1830), _Pad("B", 0x057E, 0x2009),
         _Pad("C", 0x28DE, 0x11FF)],
        [_Pad('Quoted "Pad"', 0x1234, 0x5678)],
        [_Pad("ff ff", 0xFFFF, 0xFFFF)],
    ]
    cases = []
    for pads in sets:
        shape = [{"name": p.name, "vid": p.vid, "pid": p.pid} for p in pads]
        cases.append({
            "in": shape,
            "out": {
                "rules": hide.generate_rules(pads),
                "nix": hide.nix_module_snippet(pads),
                "targets": [{"name": p.name, "vid": p.vid, "pid": p.pid}
                            for p in hide.targets(pads)],
            },
        })
    write("hide_rules", cases)



def emitted_files() -> None:
    """The two artefacts other programs read, and the guess behind them.

    Neither consumer complains when it reads one wrong. SDL silently never
    matches a mapping under a GUID it did not compute; RetroArch binds a button
    that does not exist and still calls the pad configured. So both are pinned
    whole rather than sampled.
    """
    import os
    os.environ["PADMAP_PAD_IDENTITY"] = "padmap"
    from padmap import controllercfg

    captures = [
        {},
        {"a": Binding("button", 1), "b": Binding("button", 2)},
        {"a": Binding("button", 1), "b": Binding("button", 2),
         "x": Binding("button", 3), "y": Binding("button", 4),
         "dpup": Binding("hat", 0, 1), "start": Binding("button", 9)},
        {"rightstick_up": Binding("button", 11),
         "rightstick_down": Binding("axis", 3, 1)},
        {"lefttrigger": Binding("axis", 2, -1)},
    ]
    profiles_cases = []
    for layout_id in ("", "n64", "snes", "gamecube", "arcade"):
        for index, bindings in enumerate(captures):
            for scope, context in (("", ""), ("console:n64", ""),
                                   ("game:n64/mario", "Super Mario 64")):
                profiles_cases.append({
                    "in": {"player": 1, "bindings": {k: v.to_json()
                                                     for k, v in bindings.items()},
                           "layout": layout_id, "scope": scope,
                           "context": context, "source": "" if index else "upstream"},
                    "out": controllercfg.retroarch_profile(
                        1, _ProfilePad(), bindings, "" if index else "upstream",
                        layout_id, scope, context),
                })
    write("retroarch_profiles", profiles_cases)

    guess_cases = []
    keysets = [
        [],
        list(range(0x130, 0x13c)),
        list(range(0x130, 0x133)),
        list(range(0x130, 0x150)),
        [0x130, 0x220, 0x221, 0x222, 0x223],
        [0x1e, 0x130, 0x131],
    ]
    axissets = [[], [0x00, 0x01], [0x00, 0x01, 0x10, 0x11],
                [0x00, 0x01, 0x03, 0x04], [0x10, 0x11]]
    for keys in keysets:
        for codes in axissets:
            guess_cases.append({
                "in": {"keys": keys, "axis_codes": codes},
                "out": list(controllercfg.guessed_fields(keys, codes).items()),
            })
    write("guessed_fields", guess_cases)

    guid_cases = []
    for player in (1, 2, 4, 16):
        guid_cases.append({
            "in": {"player": player},
            "out": controllercfg.virtual_guid(player),
        })
    write("virtual_guids", guid_cases)


class _ProfilePad:
    """A pad whose identity is padmap's own, so the corpus needs no hardware."""

    path = "/dev/input/event0"
    name = "Fixture Pad"
    phys = ""
    uniq = ""
    vid = 0x0079
    pid = 0x1843
    syspath = "/sys"
    retroarch_visible = True


# -- runtime state ----------------------------------------------------------
def runtime_paths() -> None:
    """Where padmap keeps per-session state, and how it compares two of them.

    `_same_runtime` is the interesting one. Compared as raw strings,
    `/run/user/1000` and `/run/user/1000/` are two different daemons -- and
    they are not, they bind the same socket, because every path here is built
    with `/ "padmap"` and a trailing separator disappears the moment it is. A
    daemon started from a shell where the variable carried a slash was
    invisible to every caller, so `ensure-daemon` started a second one on the
    socket the first was already listening on.
    """
    from padmap import protocol

    cases = []
    for one, other in (
        ("/run/user/1000", "/run/user/1000"),
        ("/run/user/1000", "/run/user/1000/"),
        ("/run/user/1000/", "/run/user/1000"),
        ("/run/user/1000//", "/run/user/1000"),
        ("/run/user/1000/.", "/run/user/1000"),
        ("/run/user/1000/../1000", "/run/user/1000"),
        ("/run/user/1000", "/run/user/1001"),
        ("/tmp", "/tmp/"),
        ("", ""),
        ("", "/tmp"),
        ("relative", "relative/"),
    ):
        cases.append({"one": one, "other": other,
                      "same": protocol._same_runtime(one, other)})
    write("same_runtime", cases)

    # The paths themselves, against a known runtime dir -- recorded as the
    # trailing part, because the prefix is whatever XDG_RUNTIME_DIR says.
    previous = os.environ.get("XDG_RUNTIME_DIR")
    os.environ["XDG_RUNTIME_DIR"] = "/run/user/1000"
    try:
        paths = {
            "runtime_dir": str(protocol.runtime_dir()),
            "socket": str(protocol.socket_path()),
            "log": str(protocol.daemon_log_path()),
            "prompted": str(protocol.prompted_path()),
            "playing": str(protocol.playing_marker()),
            "last_game": str(protocol.last_game_path()),
        }
        del os.environ["XDG_RUNTIME_DIR"]
        unset = {"runtime_dir": str(protocol.runtime_dir())}
    finally:
        if previous is None:
            os.environ.pop("XDG_RUNTIME_DIR", None)
        else:
            os.environ["XDG_RUNTIME_DIR"] = previous
    write("runtime_paths", [{"env": "/run/user/1000", "paths": paths},
                            {"env": None, "paths": unset}])


def recent_games() -> None:
    """What `lastgame.json` is read back as, over the shapes it arrives in.

    Never raises: this decorates a picker, and a missing or malformed file
    means fewer options rather than a failure. The pre-list format -- a bare
    object rather than {"games": [...]} -- is in here because a user upgrading
    mid-session would otherwise lose the per-game scope for the game they are
    playing right now, which is exactly when they want it.
    """
    import tempfile

    from padmap import protocol

    shapes = [
        None,
        "",
        "not json at all",
        "[]",
        "{}",
        '{"games": []}',
        '{"key": "a", "console": "n64", "title": "T"}',
        '{"key": "a"}',
        '{"console": "n64", "title": "T"}',
        '{"games": [{"key": "a", "console": "n64", "title": "A"}]}',
        '{"games": [{"key": "a"}, {"key": "b"}, {"key": "c"}, '
        '{"key": "d"}, {"key": "e"}, {"key": "f"}, {"key": "g"}]}',
        '{"games": [{"key": ""}, {"key": "b"}]}',
        '{"games": [null, 3, "x", {"key": "b"}]}',
        '{"games": {"key": "a"}}',
        '{"games": [{"key": "a", "console": 7, "title": null}]}',
        '"a string"',
        "42",
    ]
    cases = []
    previous = os.environ.get("XDG_RUNTIME_DIR")
    with tempfile.TemporaryDirectory() as tmp:
        os.environ["XDG_RUNTIME_DIR"] = tmp
        try:
            for raw in shapes:
                path = protocol.last_game_path()
                path.parent.mkdir(parents=True, exist_ok=True)
                if raw is None:
                    path.unlink(missing_ok=True)
                else:
                    path.write_text(raw)
                cases.append({
                    "file": raw,
                    "recent": protocol.read_recent_games(),
                    "last": protocol.read_last_game(),
                })

            # And what writing does: newest first, deduplicated by key.
            # Replaying a game moves it to the front rather than filling the
            # list with copies of itself.
            protocol.last_game_path().unlink(missing_ok=True)
            writes = []
            for console, key, title in (
                ("n64", "mario64", "Mario 64"),
                ("snes", "smw", "Super Mario World"),
                ("n64", "mario64", "Mario 64"),
                ("gc", "melee", "Melee"),
                ("ps2", "ico", "Ico"),
                ("gba", "metroid", "Metroid"),
                ("nes", "smb", "SMB"),
            ):
                protocol.write_last_game(console, key, title)
                writes.append({
                    "wrote": {"console": console, "key": key, "title": title},
                    "recent": protocol.read_recent_games(),
                })
        finally:
            if previous is None:
                os.environ.pop("XDG_RUNTIME_DIR", None)
            else:
                os.environ["XDG_RUNTIME_DIR"] = previous
    write("recent_games", cases)
    write("recent_games_writes", writes)


# -- MAME titles ------------------------------------------------------------
def mame_titles() -> None:
    """Turning a ROM set name into something a person would recognise.

    Three separate things, and the middle one is the reason this is corpused
    rather than unit-tested: `load_json` reads a *build product*, so a
    truncated write, a half-copied file or a hand-edited one all arrive at it.
    A row that is a plain string used to load as Title(title="P", year="a")
    because a string subscripts exactly like a list -- every one-character
    arcade name, silently, with nothing saying so.
    """
    from padmap import titles

    # _flatten: a description the dump wrapped across two lines arrives with
    # the newline still in it, and a title with a newline writes a line of its
    # own in every line-oriented file downstream.
    flatten = [
        "Pac-Man",
        "Puck\n            Man",
        "  padded  ",
        "Double  Dragon",
        "tab\tseparated",
        "line\r\nbreak",
        "unicode\u2028separator",
        "unicode\u2029paragraph",
        "trailing\n",
        "\n\nleading",
        "",
        "\n",
        "a\n\n\nb",
        "  \t \n  spaced  \n \t ",
    ]
    write("flatten", [{"raw": raw, "flat": titles._flatten(raw)}
                      for raw in flatten])

    # _column: one cell of a row, or "" for something that is not one.
    columns = ["Pac-Man", "", "  x  ", True, False, 0, 1, -3, 2.5,
               None, [], {}, ["a"], "wrapped\nname"]
    write("title_column", [{"raw": raw, "column": titles._column(raw)}
                           for raw in columns])

    # load_json, over the shapes a damaged build product arrives in.
    import tempfile

    shapes = [
        '{"pacman": ["Pac-Man", "1980", "Namco", "good"]}',
        '{"pacman": ["Pac-Man", "1980", "Namco"]}',
        '{"pacman": ["Pac-Man"]}',
        '{"pacman": []}',
        '{"pacman": "Pac-Man"}',
        '{"pacman": ["Pac-Man", "1980", "Namco", "good", "extra"]}',
        '{"pacman": ["", "1980", "Namco", "good"]}',
        '{"pacman": [null, "1980"]}',
        '{"pacman": 7}',
        '{"pacman": null}',
        '{"pacman": {"title": "Pac-Man"}}',
        '{"pacman": [true, false, 1, 2.5]}',
        '{"a": ["A"], "b": 7, "c": ["C", "1990"]}',
        '{"pacman": ["Puck\\n   Man", "1980", "Namco", "preliminary"]}',
        "{}",
    ]
    cases = []
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "titles.json"
        for raw in shapes:
            path.write_text(raw)
            table = titles.load_json(path)
            cases.append({
                "file": raw,
                "table": {
                    name: {"title": t.title, "year": t.year,
                           "manufacturer": t.manufacturer,
                           "status": t.status, "working": t.working}
                    for name, t in sorted(table.items())
                },
            })
        # Not an object at all: this one raises rather than answering, and
        # that is the contract -- a table that is a list is not a table.
        path.write_text("[1, 2, 3]")
        cases.append({"file": "[1, 2, 3]",
                      "raises": _try(lambda: titles.load_json(path))})
    write("title_tables", cases)

    # parse_mame_xml, against a dump with every awkwardness in it.
    xml = """<?xml version="1.0"?>
<mame>
 <game name="pacman">
  <description>Pac-Man (Midway)</description>
  <year>1980</year>
  <manufacturer>Namco</manufacturer>
  <driver status="good"/>
 </game>
 <game name="puckman">
  <description>Puck
            Man</description>
  <year>1980</year>
  <manufacturer>Namco</manufacturer>
  <driver status="imperfect"/>
 </game>
 <game name="broken">
  <description>Does Not Run</description>
  <driver status="preliminary"/>
 </game>
 <game name="nodescription">
  <year>1984</year>
 </game>
 <game name="blankdescription">
  <description></description>
 </game>
 <game name="unclosed">
  <description>Never Ends</description>
 <game name="afterunclosed">
  <description>After</description>
  <driver status="good"/>
 </game>
 <game name="baddump">
  <description>Bad Dump</description>
  <rom name="x" status="baddump"/>
  <driver status="good"/>
 </game>
</mame>
"""
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "mame.xml"
        path.write_text(xml)
        parsed = titles.parse_mame_xml(path)
    write("mame_xml", [{
        "xml": xml,
        "table": {
            name: [t.title, t.year, t.manufacturer, t.status]
            for name, t in sorted(parsed.items())
        },
    }])

    # resolve: keyed on the ROM basename, not the playlist label, because the
    # label is only incidentally the same string and users can edit it.
    table = {"pacman": titles.Title("Pac-Man", "1980", "Namco", "good")}
    lookups = [
        ("pacman", "/roms/pacman.zip"),
        ("whatever the user renamed it", "/roms/pacman.zip"),
        ("pacman", "/roms/pacman.7z"),
        ("pacman", "pacman"),
        ("pacman", "/roms/PACMAN.zip"),
        ("Unknown", "/roms/missing.zip"),
        ("", "/roms/missing.zip"),
        ("", ""),
        ("label", "/roms/sub.dir/pacman.zip"),
        ("label", "/roms/pacman.tar.gz"),
    ]
    write("title_resolve", [
        {"label": label, "path": path,
         "title": titles.resolve(label, path, table).title}
        for label, path in lookups
    ])


# -- capability bitmaps ------------------------------------------------------
def capabilities() -> None:
    """What makes a device a joypad, read from sysfs rather than by opening it.

    The distinction between `None` and `0` is the load-bearing part. A device
    with no absolute axes has an `abs` file containing "0", and reading that
    as "could not be read" sends it down the fallback that *opens* the
    device -- which is the expensive path this exists to avoid. Releasing a
    USB HID descriptor takes about 11ms, and that was 390ms of a 400ms scan.
    Twelve of this machine's input devices are exactly that shape.
    """
    from padmap import devices

    raw_cases = [
        None,
        "",
        "   ",
        "0",
        "1",
        "ffffffffffffffff",
        "1 0",
        "0 0",
        "3 ffffffffffffffff",
        # A real key bitmap: several 64-bit words, most significant first.
        "7 0 0 0 0 0 0 ffffffff",
        "10000 0 0 0 0",
        "notahexnumber",
        "1 notahex",
        "ffffffffffffffffff",
        # Leading and trailing whitespace, as a sysfs read can carry.
        " 1 2 ",
    ]
    cases = []
    for raw in raw_cases:
        if raw is None:
            value = None
        else:
            import tempfile
            with tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp) / "bits"
                path.write_text(raw)
                value = devices._capability_mask(str(path))
        cases.append({
            "raw": raw,
            "none": value is None,
            "zero": value == 0 if value is not None else None,
            # Bit indices, not the integer: JSON cannot hold a 768-bit number
            # and a decimal string would only move the parsing problem.
            "bits": (
                sorted(i for i in range(1024) if (value >> i) & 1)
                if value else []
            ),
        })
    write("capability_masks", cases)

    # The decision, over the shapes the two bitmaps arrive in.
    BTN_SOUTH, BTN_JOYSTICK, BTN_DPAD_UP = 0x130, 0x120, 0x220
    KEY_A = 0x1E
    decisions = [
        (None, None), (None, 1), (1, None),
        (0, 0), (0, 1 << BTN_SOUTH),
        (1, 0), (1, 1 << KEY_A),
        (1, 1 << BTN_SOUTH), (1, 1 << BTN_JOYSTICK),
        (1, 1 << 0x13F), (1, 1 << 0x140), (1, 1 << 0x11F),
        (1, 1 << BTN_DPAD_UP),
        (1, (1 << KEY_A) | (1 << BTN_SOUTH)),
        (7, 1 << BTN_SOUTH),
        ((1 << 200), 1 << BTN_SOUTH),
    ]
    write("joypad_by_capability", [
        {"abs": None if a is None else str(a),
         "keys": None if k is None else str(k),
         "verdict": devices._joypad_by_capability(a, k)}
        for a, k in decisions
    ])

    # And every input device actually on this machine, which is the set of
    # shapes nobody would think to write down.
    import glob as _glob
    live = []
    for input_dir in sorted(_glob.glob("/sys/class/input/input*")):
        events = sorted(Path(p).name for p in _glob.glob(f"{input_dir}/event*"))
        if not events:
            continue
        caps = Path(input_dir) / "capabilities"
        live.append({
            "abs_raw": devices._read(str(caps / "abs")),
            "key_raw": devices._read(str(caps / "key")),
            "joypad": devices._looks_like_joypad(f"/dev/input/{events[0]}"),
        })
    write("live_capabilities", live)


# -- the calibration machine -------------------------------------------------
def calibration_machine() -> None:
    """Rest samples and reach sweeps, turned into a calibration.

    Two rules in here, and both were wrong once in a way nothing reported.
    `calibratable_axes` decides by *resting position* rather than by axis
    code, because this machine's GameCube adapter puts its analogue triggers
    on ABS_RX/ABS_RY -- stick codes -- resting at 24 of 0-255, and centring a
    trigger costs it half its travel. `merge_reach` ignores a direction that
    never left the dead band, because recording one makes `apply` compute a
    span of zero and that whole direction reads dead centre.
    """
    from evdev import AbsInfo

    from padmap import calibrate

    def info(minimum, maximum, value, flat=0):
        return AbsInfo(value=value, min=minimum, max=maximum,
                       fuzz=0, flat=flat, resolution=0)

    # rest_from_samples: observed wobble -> centre and dead band.
    rest_cases = [
        # (min, max, value, flat, samples)
        (-32768, 32767, 0, 128, [0, 0]),
        (-32768, 32767, 0, 128, [-40, 60]),
        (-32768, 32767, 0, 0, [-40, 60]),
        (0, 255, 128, 0, [126, 131]),
        (0, 255, 128, 0, []),
        (0, 255, 24, 0, [24, 24]),
        (0, 4095, 2048, 128, [2000, 2100]),
        (-32768, 32767, 5, 0, [-32768, 32767]),
        (0, 1, 0, 0, [0, 1]),
        (-1, 1, 0, 0, [-1, 1]),
    ]
    cases = []
    for minimum, maximum, value, flat, samples in rest_cases:
        axes = {0: info(minimum, maximum, value, flat)}
        seen = {0: [min(samples), max(samples)]} if samples else {}
        out = calibrate.rest_from_samples(axes, seen)[0]
        cases.append({
            "min": minimum, "max": maximum, "value": value, "flat": flat,
            "samples": samples,
            "center": out.center, "cal_flat": out.flat,
            "cal_min": out.minimum, "cal_max": out.maximum,
        })
    write("calibration_rest", cases)

    # merge_reach: which sweeps count as having been measured.
    from padmap.profiles import AxisCalibration

    merge_cases = [
        # (center, min, max, flat, reach_low, reach_high)
        (0, -32768, 32767, 128, -30000, 30000),
        (0, -32768, 32767, 128, -100, 100),
        (0, -32768, 32767, 128, -129, 129),
        (0, -32768, 32767, 128, -128, 128),
        (0, -32768, 32767, 128, -1, 30000),
        (0, -32768, 32767, 0, -1, 1),
        (0, -32768, 32767, 0, 0, 0),
        (128, 0, 255, 10, 5, 250),
        (128, 0, 255, 10, 120, 136),
        (128, 0, 255, 10, 117, 139),
        (2048, 0, 4095, 128, 0, 4095),
    ]
    merged = []
    for center, minimum, maximum, flat, low, high in merge_cases:
        cal = AxisCalibration(center=center, minimum=minimum,
                              maximum=maximum, flat=flat)
        out = calibrate.merge_reach({0: cal}, {0: (low, high)})[0]
        merged.append({
            "center": center, "min": minimum, "max": maximum, "flat": flat,
            "reach": [low, high],
            "reach_min": out.reach_min, "reach_max": out.reach_max,
        })
    # And the no-sweep-at-all case, which must not pin travel to zero.
    cal = AxisCalibration(center=0, minimum=-32768, maximum=32767, flat=128)
    out = calibrate.merge_reach({0: cal}, {})[0]
    merged.append({
        "center": 0, "min": -32768, "max": 32767, "flat": 128, "reach": None,
        "reach_min": out.reach_min, "reach_max": out.reach_max,
    })
    write("calibration_reach", merged)

    # calibratable_axes: a stick centres and a trigger does not, and the axis
    # code cannot carry that difference.
    class FakeDevice:
        def __init__(self, entries):
            self._entries = entries

        def capabilities(self, absinfo=True):
            from evdev import ecodes as e
            return {e.EV_ABS: self._entries}

    from evdev import ecodes as e
    axis_sets = [
        ("a centred stick", [(e.ABS_X, info(-32768, 32767, 0))]),
        ("a trigger by code", [(e.ABS_Z, info(0, 255, 0))]),
        ("a hat", [(e.ABS_HAT0X, info(-1, 1, 0))]),
        ("a zero-width axis", [(e.ABS_X, info(0, 0, 0))]),
        ("an inverted range", [(e.ABS_X, info(10, 5, 7))]),
        # The GameCube adapter: triggers on stick codes, resting at 24/255.
        ("gamecube triggers on stick codes",
         [(e.ABS_RX, info(0, 255, 24)), (e.ABS_RY, info(0, 255, 25)),
          (e.ABS_X, info(0, 255, 128)), (e.ABS_Y, info(0, 255, 128))]),
        ("an off-centre stick", [(e.ABS_X, info(0, 255, 200))]),
        ("a just-off-centre stick", [(e.ABS_X, info(0, 255, 130))]),
    ]
    write("calibratable_axes", [
        {"what": what,
         "axes": [[code, i.min, i.max, i.value] for code, i in entries],
         "calibratable": sorted(
             calibrate.calibratable_axes(FakeDevice(entries)))}
        for what, entries in axis_sets
    ])


# -- the Switch Pro decode ---------------------------------------------------
def switch_reports() -> None:
    """Report 0x30 turned into evdev events, and the overrides that select it.

    The pad powers up sending 0x3f -- a cut-down report with no analogue data
    at all -- and has to be *asked* for 0x30. Every 0x3f is discarded by the
    report-id filter, so a pad stuck in simple mode delivers no input while
    looking perfectly healthy: node present, descriptor live, reports
    flowing, nothing raised and nothing logged. The only visible symptom is
    that no button does anything.
    """
    from padmap import hidraw

    def fresh():
        source = object.__new__(hidraw.Source)
        source._buttons = {}
        source._axes = {}
        source._hat = (0, 0)
        return source

    def report(right=0, shared=0, left=0, lx=2048, ly=2048, rx=2048, ry=2048):
        data = bytearray(64)
        data[0] = 0x30
        data[3], data[4], data[5] = right, shared, left
        # The controller reports Y increasing upwards; the decode flips it.
        for at, x, y in ((6, lx, 4095 - ly), (9, rx, 4095 - ry)):
            data[at] = x & 0xFF
            data[at + 1] = ((x >> 8) & 0x0F) | ((y & 0x0F) << 4)
            data[at + 2] = (y >> 4) & 0xFF
        return bytes(data)

    # One control at a time, from a settled source, so each case is the
    # change and not the opening frame.
    cases = []
    singles = [("right", bit) for bit in (0x01, 0x02, 0x04, 0x08, 0x40, 0x80)]
    singles += [("shared", bit) for bit in (0x01, 0x02, 0x04, 0x08, 0x10, 0x20)]
    singles += [("left", bit) for bit in (0x01, 0x02, 0x04, 0x08, 0x40, 0x80)]
    for byte, bit in singles:
        source = fresh()
        source._decode(report())
        events = source._decode(report(**{byte: bit}))
        cases.append({
            "byte": byte, "bit": bit,
            "events": [[e.type, e.code, e.value] for e in events],
        })
    # Combinations, and the d-pad's opposite-cancels rule.
    for label, kwargs in (
        ("a and b", {"right": 0x04 | 0x08}),
        ("up", {"left": 0x02}),
        ("up and down", {"left": 0x01 | 0x02}),
        ("left and right", {"left": 0x04 | 0x08}),
        ("up and right", {"left": 0x02 | 0x04}),
        ("all four", {"left": 0x0F}),
        ("sticks pushed", {"lx": 4095, "ly": 4095, "rx": 0, "ry": 0}),
        ("sticks at zero", {"lx": 0, "ly": 0, "rx": 0, "ry": 0}),
        ("a nudge inside the fuzz", {"lx": 2050}),
        ("a move past the fuzz", {"lx": 2100}),
    ):
        source = fresh()
        source._decode(report())
        events = source._decode(report(**kwargs))
        cases.append({
            "byte": label, "bit": None,
            "events": [[e.type, e.code, e.value] for e in events],
        })
    write("switch_decode", cases)

    # The opening frame: what a freshly opened source emits for a neutral pad.
    source = fresh()
    write("switch_first_frame", [{
        "events": [[e.type, e.code, e.value] for e in source._decode(report())],
    }])

    # load_overrides, over the shapes the file arrives in.
    import tempfile
    override_shapes = [
        None, "", "not json", "[]", "{}",
        '{"057e:2009": true}',
        '{"057E:2009": true}',
        '{"057e:2009": false}',
        '{"057e:2009": 1}',
        '{"057e:2009": "yes"}',
        '{"057e:2009": null}',
        '{"057e:2009": true, "0079:1843": false}',
        '"a string"',
        '{"": true}',
    ]
    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "hidraw.json"
        for raw in override_shapes:
            if raw is None:
                path.unlink(missing_ok=True)
            else:
                path.write_text(raw)
            rows.append({"file": raw,
                         "overrides": hidraw.load_overrides(path)})
    write("hidraw_overrides", rows)


# -- rewriting the user's retroarch.cfg --------------------------------------
def clean_config() -> None:
    """Stripping padmap's leaked settings back out, byte for byte.

    The only code in padmap that writes to the user's RetroArch config, and
    the contract is narrow: the lines it reports are the only lines that may
    differ. Everything else -- a CRLF ending, a latin-1 ROM path in
    system_directory, an unquoted value, a file with no trailing newline --
    has to come back out exactly as it went in.

    Recorded as hex because the interesting cases are not valid UTF-8, which
    is the whole reason the Python reads bytes and decodes with
    surrogateescape rather than using read_text.
    """
    import tempfile

    from padmap import retroarch

    VIRT = "padmap Player 1"
    configs = [
        "",
        "\n",
        "# just a comment\n",
        f'input_player1_reserved_device = "{VIRT}"\n',
        f'input_player1_reserved_device = "{VIRT}"\ninput_player1_device_reservation_type = "2"\n',
        'input_player1_reserved_device = "Real Controller"\n',
        'input_player1_device_reservation_type = "2"\n',
        f'input_player1_reserved_device="{VIRT}"\n',
        f'input_player1_reserved_device = {VIRT}\n',
        f'  input_player1_reserved_device  =  "{VIRT}"   \n',
        f'input_player1_reserved_device = "{VIRT}"\r\n',
        f'input_player1_reserved_device = "{VIRT}"',
        f'input_player3_reserved_device = "{VIRT}"\ninput_player3_device_reservation_type = "2"\n',
        'video_fullscreen = "true"\n',
        f'video_fullscreen = "true"\ninput_player1_reserved_device = "{VIRT}"\nvideo_vsync = "true"\n',
        f'input_player1_reserved_device = "{VIRT}"\ninput_player1_reserved_device = "{VIRT}"\n',
        'input_libretro_device_p1 = "1"\n',
        f'input_player1_reserved_device = "{VIRT}"\n\n\n',
        'not a setting line at all\n',
        '=leading equals\n',
    ]
    cases = []
    with tempfile.TemporaryDirectory() as tmp:
        for index, text in enumerate(configs):
            path = Path(tmp) / f"cfg{index}"
            raw = text.encode("utf-8")
            path.write_bytes(raw)
            changes, _ = retroarch.clean_user_config(path, dry_run=True)
            # Run it for real to capture the bytes written.
            path.write_bytes(raw)
            retroarch.clean_user_config(path)
            cases.append({
                "in": raw.hex(), "changes": changes,
                "out": path.read_bytes().hex(),
            })

        # Bytes that are not UTF-8: a latin-1 path in a setting the cleaner
        # must not touch, beside one it must.
        hostile = (b'system_directory = "/roms/caf\xe9"\n'
                   b'input_player1_reserved_device = "padmap Player 1"\n')
        path = Path(tmp) / "hostile"
        path.write_bytes(hostile)
        changes, _ = retroarch.clean_user_config(path, dry_run=True)
        path.write_bytes(hostile)
        retroarch.clean_user_config(path)
        cases.append({"in": hostile.hex(), "changes": changes,
                      "out": path.read_bytes().hex()})
    write("clean_user_config", cases)

    # parse_profile_text: an autoconfig read back as settings.
    profile_texts = [
        "",
        'input_device = "Pad"\n',
        'input_device="Pad"\n',
        "input_device = Pad\n",
        "  input_device = Pad  \n",
        "# comment\ninput_device = Pad\n",
        'input_a_btn = "0"\ninput_b_btn = "1"\n',
        "input_device = Pad\r\n",
        "not a line\ninput_device = Pad\n",
        'input_device = "has spaces"\n',
        'input_device = ""\n',
        "input_device =\n",
        'input_device = "Pad"\ninput_device = "Other"\n',
    ]
    write("parse_profile_text", [
        {"text": text, "settings": retroarch.parse_profile_text(text)}
        for text in profile_texts
    ])


# -- the launch override -----------------------------------------------------
def launch_override() -> None:
    """The file handed to RetroArch with --appendconfig, exactly.

    Every one of the sixteen player slots is written, assigned or not: an
    unmanaged slot gets a vacant pad index, a cleared reservation and
    RETRO_DEVICE_NONE, which together are the difference between "padmap said
    nothing about player 3" and "player 3 has no controller".

    The index arithmetic is the part that decides which physical controller a
    game sees as player 1, and it has been wrong in ways that end the daemon:
    a player number outside 1..MAX_PLAYERS was counted as managed without
    consuming a spare index, so the iterator ran dry and launch_config raised
    StopIteration during startup, after the pads were grabbed.
    """
    from padmap import retroarch, virtual
    from padmap.assign import Assignment
    from padmap.devices import Pad

    def pad(n):
        return Pad(path=f"/dev/input/event{n}", name=f"Pad {n}", phys=f"usb-{n}",
                   uniq="", vid=0x045E, pid=0x028E, syspath=f"/sys/{n}")

    # _empty_indices: distinct vacant indices for the unmanaged slots.
    write("empty_indices", [
        {"pads": pads, "wanted": wanted,
         "indices": retroarch._empty_indices(pads, wanted)}
        for pads, wanted in [
            (0, 0), (0, 1), (0, 16), (2, 3), (2, 16), (15, 4),
            (16, 4), (20, 4), (100, 2),
        ]
    ])

    # compute_pad_indices / managed_players, against a fixed enumeration.
    order = {0: "/dev/input/event90", 1: "/dev/input/event91",
             2: "/dev/input/event92"}
    scenarios = [
        ("all three visible", [1, 2, 3],
         {1: "/dev/input/event90", 2: "/dev/input/event91",
          3: "/dev/input/event92"}),
        ("one clone missing from the enumeration", [1, 2],
         {1: "/dev/input/event90", 2: "/dev/input/event99"}),
        ("no clones at all", [1], {}),
        ("out of order", [1, 2],
         {1: "/dev/input/event92", 2: "/dev/input/event90"}),
        ("a player number beyond the slots", [99],
         {99: "/dev/input/event90"}),
        ("player zero", [0], {0: "/dev/input/event90"}),
    ]
    rows = []
    for what, players, paths in scenarios:
        assignments = [Assignment(player=p, pad=pad(p), button=0)
                       for p in players]
        rows.append({
            "what": what, "order": order, "paths": paths,
            "indices": retroarch.compute_pad_indices(paths, order),
            "managed": retroarch.managed_players(assignments, paths, order),
        })
    write("pad_indices", rows)

    # _reservation_lines and reservation_config, which are pure text.
    write("reservation_lines", [
        {"managed": managed, "text": retroarch._reservation_lines(managed)}
        for managed in ([], [1], [1, 2], [3], [1, 16], list(range(1, 17)))
    ])
    write("reservation_config", [
        {"players": players,
         "text": retroarch.reservation_config(
             [Assignment(player=p, pad=pad(p), button=0) for p in players])}
        for players in ([], [1], [2, 1], [1, 2, 3, 4])
    ])
    _ = virtual


def launch_flags() -> None:
    """--nodevice flags for the core ports nobody is assigned to.

    The only working way to empty a port. input_libretro_device_pN reads like
    the setting for it and RetroArch ignores it from a config file entirely --
    it lives only in .rmp remap files -- so setting it in the launch override
    changed nothing at all.
    """
    from padmap import retroarch
    from padmap.assign import Assignment
    from padmap.devices import Pad

    def pad(n):
        return Pad(path=f"/dev/input/event{n}", name=f"Pad {n}", phys="",
                   uniq="", vid=1, pid=1, syspath="")

    order = {0: "/dev/input/event90", 1: "/dev/input/event91"}
    rows = []
    for what, players, paths in [
        ("nobody assigned", [], {}),
        ("one player", [1], {1: "/dev/input/event90"}),
        ("two players", [1, 2],
         {1: "/dev/input/event90", 2: "/dev/input/event91"}),
        ("player two only", [2], {2: "/dev/input/event91"}),
        ("a clone not enumerated", [1], {1: "/dev/input/event99"}),
    ]:
        assignments = [Assignment(player=p, pad=pad(p), button=0)
                       for p in players]
        rows.append({"what": what, "paths": paths, "order": order,
                     "args": retroarch.launch_args(assignments, paths, order)})
    write("launch_args", rows)


def _try_any(call) -> dict:
    """Like `_try`, but records the exception type as well.

    `int(None)` is a TypeError and `int("x")` a ValueError, and both reach a
    client as the same error reply -- but the port has to refuse both, and
    recording which is which is what says so.
    """
    try:
        return {"ok": True, "value": call()}
    except Exception as error:                          # noqa: BLE001
        return {"ok": False, "error": type(error).__name__}


# -- the daemon's command surface -------------------------------------------
def daemon_commands() -> None:
    """What a client asked for, as the arguments the handler takes.

    The process boundary. The socket lives in XDG_RUNTIME_DIR and any process
    running as this user may write to it, so every field arrives from outside
    and is coerced rather than trusted -- and where the coercion *raises*, the
    error reply is part of the contract too, because a front-end reads it.
    """
    from padmap.server import COMMANDS, Server

    messages = [
        {}, {"cmd": None}, {"cmd": ""}, {"cmd": "nope"}, {"cmd": 7},
        {"cmd": "status"}, {"cmd": "reset"}, {"cmd": "accept"},
        {"cmd": "cancel"}, {"cmd": "skip_control"}, {"cmd": "configure_end"},
        {"cmd": "begin"},
        {"cmd": "begin", "players": 2},
        {"cmd": "begin", "players": "3"},
        {"cmd": "begin", "players": 2.9},
        {"cmd": "begin", "players": True},
        {"cmd": "begin", "players": -1},
        {"cmd": "begin", "players": "three"},
        {"cmd": "begin", "players": None},
        {"cmd": "map"},
        {"cmd": "map", "player": 1, "layout": "n64", "scope": "console"},
        {"cmd": "map", "player": "2"},
        {"cmd": "map", "player": 1, "layout": 7},
        {"cmd": "map_for_game", "player": 1, "console": "n64",
         "key": "mario64", "title": "Mario 64"},
        {"cmd": "map_for_game"},
        {"cmd": "set_icon", "player": 1, "icon": "xbox"},
        {"cmd": "set_icon", "player": 1, "icon": None},
        {"cmd": "choose_layout", "player": 3},
        {"cmd": "choose_scope"},
        {"cmd": "forget_pad", "player": 2},
        {"cmd": "calibrate", "player": 4},
        {"cmd": "calibrate", "player": "x"},
        {"cmd": "status", "extra": "ignored"},
    ]
    cases = []
    for message in messages:
        cases.append({"message": message,
                      **_try_any(lambda m=message: Server.parse_command(m))})
    write("daemon_commands", cases)
    write("daemon_command_names", [{"commands": list(COMMANDS)}])


def main() -> int:
    print(f"recording the Python's answers into {OUT.relative_to(REPO)}:")
    bindings()
    indices()
    guids()
    sdl_lines()
    parsed_lines()
    sticks()
    retroarch_lines()
    calibration()
    scopes()
    cores()
    printable()
    signatures()
    stored_profiles()
    icon_choices()
    hide_rules()
    emitted_files()
    runtime_paths()
    recent_games()
    mame_titles()
    capabilities()
    calibration_machine()
    switch_reports()
    clean_config()
    launch_override()
    launch_flags()
    daemon_commands()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
