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
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
