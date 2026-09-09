"""What happens when the controller itself is malformed?

Every consumer padmap writes for -- SDL's database, RetroArch's autoconfig,
the profile store on disk -- is fed strings and numbers that came from a USB
descriptor written by somebody else. None of them validate anything. A comma
in a device name silently shifts every field of an SDL line; a slash in one
puts a profile somewhere other than the profile directory; an axis whose
declared range is empty divides by zero on the way to deciding whether it is
a stick.

The shapes here are not invented. Measured on this machine:

  * a USB adapter whose name begins with a 0x18 control character
  * a GameCube adapter presenting FOUR ports with identical name, vendor and
    product, distinguishable by nothing static at all
  * that adapter's analogue triggers sitting on the right stick's axis codes
    and resting at 24 of 0-255, which told SDL the stick was jammed

The rest -- no axes, no buttons, twenty axes, a 500-character name, an empty
one, vendor 0 -- are the neighbouring cases, fed through the same functions.

Stories: S1 (setup opens itself for an unknown model), S3 (a held button
claims a slot), S7 (the wizard walks the layout), S15 (only padmap's virtual
pads reach RetroArch), S17 (two players, two pads, two profiles).

    QT_QPA_PLATFORM=offscreen XDG_RUNTIME_DIR=$(mktemp -d) \
        nix develop --command python3 tools/check_hostile_devices.py
"""

import os
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

# Before importing padmap: several modules read these at import time, and a
# LIVE daemon owns the real ones. Nothing below may touch the user's state.
_SANDBOX = Path(tempfile.mkdtemp(prefix="padmap-hostile-"))
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    _dir = _SANDBOX / _var.lower()
    _dir.mkdir(parents=True, exist_ok=True)
    os.environ[_var] = str(_dir)
os.environ["RETROARCH_CONFIG_DIR"] = str(_SANDBOX / "retroarch")
os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "profiles")
# padmap's own identity, so nothing here reads a device node to find out what
# a virtual pad would mirror. Mirroring has its own checks; see check_mapping.
os.environ["PADMAP_PAD_IDENTITY"] = "padmap"

from padmap import (assign, capture, controllercfg, devices,  # noqa: E402
                    icons, layouts, mapping, profiles, retroarch, virtual)
from padmap.capture import EV_ABS, EV_KEY, Chooser, MappingRun  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402

PROFILE_DIR = Path(os.environ["PADMAP_PROFILE_DIR"])

# A path no device can be behind. Every function that takes a Pad and opens it
# must survive that, and it guarantees this check cannot grab a real
# controller out from under the running daemon.
DEAD_NODE = "/dev/input/padmap-check-hostile-nonexistent"

GAPS: list[str] = []


def gap(text: str) -> None:
    """Something the source does not do yet. Reported, not asserted."""
    GAPS.append(text)
    print(f"  gap: {text}")


def pad(name: str, vid: int = 0x0079, pid: int = 0x1879,
        phys: str = "usb-0000:00:14.0-2/input0") -> Pad:
    return Pad(path=DEAD_NODE, name=name, phys=phys, uniq="",
               vid=vid, pid=pid, syspath="/sys/devices/fake/input/input9")


class Event:
    """An evdev event, which is all capture ever wanted from one."""

    def __init__(self, type_: int, code: int, value: int) -> None:
        self.type = type_
        self.code = code
        self.value = value


class AbsInfo:
    def __init__(self, minimum: int, maximum: int, value: int) -> None:
        self.min = minimum
        self.max = maximum
        self.value = value


class StubDevice:
    """Enough of an evdev device for the capability readers, and no more."""

    def __init__(self, caps: dict) -> None:
        self._caps = caps
        self.closed = False

    def capabilities(self):
        return self._caps

    def close(self) -> None:
        self.closed = True


CLOCK = {"t": 0.0}


def now() -> float:
    return CLOCK["t"]


def tick(seconds: float = 0.05) -> None:
    CLOCK["t"] += seconds


# Names no one would design on purpose. All but the last are single-line, so a
# single-line SDL entry is the whole promise for them.
HOSTILE_NAMES: list[tuple[str, str]] = [
    ("an empty name", ""),
    ("a name that is only whitespace", "   "),
    ("a name containing a comma", "Mayflash,Arcade Fightstick F300"),
    ("the 0x18 prefix this machine really reports", "\x18USB Gamepad"),
    ("control characters throughout", "\x01\x02USB\x7fPad\x1b"),
    ("non-ASCII", "コントローラー ünïcøde"),
    ("a 500-character name", "L" * 500),
    ("a path traversal", "../../../etc/passwd"),
    ("a slash", "8Bitdo/SN30 Pro"),
    ("SDL field syntax inside the name", "a:b0,platform:Linux,"),
    ("a comment marker and quotes", '# "quoted" pad'),
]

NEWLINE_NAME = "Line\nBreak Pad"

GUID = "0" * 32


def read_back(line: str, expected_name: str, expected: dict, where: str):
    """One physical line, and parse_sdl_line gets back what was written."""
    if "\n" in line or "\r" in line:
        raise SystemExit(
            f"FAIL: the SDL entry for {where} spans more than one line. "
            "sdl_controllers.txt is line-oriented, so SDL would read the "
            "remainder as a second, broken device and the pad would have no "
            "mapping at all.")
    parsed = mapping.parse_sdl_line(line)
    if parsed is None:
        raise SystemExit(
            f"FAIL: the line written for {where} cannot be read back by "
            "parse_sdl_line -- padmap would not recognise its own output, so "
            "the entry would be duplicated on every regeneration.")
    guid, name, fields = parsed
    if name != expected_name:
        raise SystemExit(
            f"FAIL: {where} came back as {name!r}, not {expected_name!r}")
    for field, target in expected.items():
        if fields.get(field) != target:
            raise SystemExit(
                f"FAIL: {where} lost or corrupted {field} -- wrote {target!r}, "
                f"read back {fields.get(field)!r}. SDL would report the "
                "control as absent and say nothing about it.")
    if fields.get("platform") != "Linux":
        raise SystemExit(
            f"FAIL: {where} has no platform field; SDL skips such a line")
    return guid, name, fields


# -- S15: the SDL database line ----------------------------------------------


def check_comma_name() -> None:
    print("S15: a comma in a device name cannot corrupt the fields after it")
    name = "Mayflash,Arcade Fightstick F300 rev:2"
    bindings = {"a": Binding("button", 1), "b": Binding("button", 2)}
    line = mapping.sdl_mapping(GUID, name, bindings)
    _, written, fields = read_back(
        line, "MayflashArcade Fightstick F300 rev:2", {"a": "b1", "b": "b2"},
        "a comma in the name")
    if "," in written:
        raise SystemExit("FAIL: the comma survived into the name field")
    if len(fields) != 3:      # a, b, platform -- nothing invented
        raise SystemExit(
            f"FAIL: the name leaked extra fields into the line: {fields}")
    print("  ok  the comma is removed, a:b1 b:b2 survive, nothing is invented")

    # What the guard is worth: the same line with the comma left in.
    naive = f"{GUID},{name},a:b1,b:b2,platform:Linux,"
    _, hurt_name, hurt_fields = mapping.parse_sdl_line(naive)
    if hurt_name == name:
        raise SystemExit(
            "FAIL: parse_sdl_line no longer splits on commas, so the comma "
            "guard in sdl_line is protecting nothing")
    if "Arcade Fightstick F300 rev" not in hurt_fields:
        raise SystemExit(
            "FAIL: the unguarded line did not turn the rest of the name into "
            "a binding field, so this demonstration has stopped demonstrating")
    print(f"  ok  unguarded, the name reads {hurt_name!r} and the rest of it "
          "becomes a bogus field -- the failure being prevented")


def check_hostile_names_round_trip() -> None:
    print("\nS15: every hostile name still writes one readable line")
    bindings = {"a": Binding("button", 0), "dpup": Binding("hat", 0, 1),
                "lefttrigger": Binding("axis", 2, 1)}
    expected = {"a": "b0", "dpup": "h0.1", "lefttrigger": "+a2"}
    for description, name in HOSTILE_NAMES:
        line = mapping.sdl_mapping(GUID, name, bindings)
        read_back(line, name.replace(",", ""), expected, description)
        print(f"  ok  {description} ({len(name)} chars) round-trips intact")


def check_newline_name() -> None:
    print("\nS15: a newline in a device name")
    line = mapping.sdl_line(GUID, NEWLINE_NAME, {"a": "b0"})
    if "," in line.split(",")[1]:
        raise SystemExit("FAIL: the name field is not comma-free")
    print("  ok  the comma guard still applies to a multi-line name")
    if "\n" in line:
        first = mapping.parse_sdl_line(line.splitlines()[0])
        lost = first is not None and not [
            f for f in first[2] if f != "platform"
        ]
        gap("mapping.sdl_line strips commas from the name but not newlines, "
            "so a pad whose name contains one writes two physical lines into "
            "sdl_controllers.txt. Repro: "
            "mapping.sdl_line('0'*32, 'Line\\nBreak Pad', {'a': 'b0'}) -- "
            "SDL reads line one as a device with "
            f"{'no bindings at all' if lost else 'the wrong bindings'} and "
            "line two as junk. Not reachable from a physical pad today "
            "(every caller passes virtual_name), but the comma guard beside "
            "it is one-sided.")
    else:
        raise SystemExit(
            "FAIL: this is now guarded -- turn the gap into an assertion")


def check_guid_hostile_identity() -> None:
    print("\nS15: the GUID SDL will look the pad up under")
    zero = mapping.sdl_guid(bus=3, vendor=0, product=0, version=0, name="")
    if len(zero) != 32 or any(c not in "0123456789abcdef" for c in zero):
        raise SystemExit(
            f"FAIL: vendor/product 0 gave {zero!r}, which is not a GUID. SDL "
            "would never match the line padmap wrote for that pad.")
    print("  ok  vendor 0, product 0 and an empty name still give 32 hex")

    huge = mapping.sdl_guid(bus=0x1FFFF, vendor=0x1FFFF, product=0x1FFFF,
                            version=0x1FFFF, name="x")
    masked = mapping.sdl_guid(bus=0xFFFF, vendor=0xFFFF, product=0xFFFF,
                              version=0xFFFF, name="x")
    if huge != masked or len(huge) != 32:
        raise SystemExit(
            "FAIL: an out-of-range id is not masked to 16 bits, so the GUID "
            "grows extra digits and matches nothing")
    print("  ok  ids wider than 16 bits are masked, not spilled into the GUID")

    # physical_guid deliberately checksums the *raw* name. If cleaning it made
    # no difference the docstring's warning would be empty.
    raw = mapping.sdl_guid(bus=3, vendor=0x79, product=0x1879, version=0x111,
                           name="\x18USB Gamepad")
    cleaned = mapping.sdl_guid(bus=3, vendor=0x79, product=0x1879,
                               version=0x111, name="USB Gamepad")
    if raw == cleaned:
        raise SystemExit(
            "FAIL: the 0x18 prefix does not change the checksum, so nothing "
            "distinguishes the raw name from the cleaned one")
    print(f"  ok  the 0x18 prefix changes the checksum ({raw[4:8]} vs "
          f"{cleaned[4:8]}) -- stripping it would look up nothing")

    japanese = mapping.sdl_guid(bus=3, vendor=1, product=1, version=1,
                                name="コン")
    other = mapping.sdl_guid(bus=3, vendor=1, product=1, version=1,
                             name="コント")
    if len(japanese) != 32 or japanese == other:
        raise SystemExit(
            "FAIL: non-ASCII names do not checksum distinctly, so two "
            "different controllers would share one GUID")
    print("  ok  a non-ASCII name is checksummed as UTF-8 and stays distinct")


# -- S1/S17: identity, when nothing static tells two pads apart --------------


def check_four_identical_ports() -> None:
    print("\nS1: four identical adapter ports are one model, offered once")
    ports = [pad("Mayflash GameCube Adapter", 0x0079, 0x1843) for _ in range(4)]
    signatures = {profiles.signature(p) for p in ports}
    if len(signatures) != 1:
        raise SystemExit(
            f"FAIL: the four ports gave {len(signatures)} signatures, so "
            "plugging the adapter in would open controller setup once per "
            "port instead of once")
    print("  ok  one signature, so S1 prompts once for the whole adapter")

    groups = devices.ambiguous_groups(ports)
    if len(groups) != 1 or len(groups[0]) != 4:
        raise SystemExit(
            "FAIL: the four ports are not reported as indistinguishable, so "
            "padmap cannot explain why press-to-claim is required")
    print("  ok  all four are reported ambiguous, which is why S3 exists")


def check_phys_only_difference() -> None:
    print("\nS1: two pads differing only in phys")
    left = pad("USB Gamepad", phys="usb-0000:00:14.0-1/input0")
    right = pad("USB Gamepad", phys="usb-0000:00:14.0-2/input0")
    if devices.ambiguous_groups([left, right]):
        raise SystemExit(
            "FAIL: pads with different phys are called indistinguishable")
    print("  ok  phys tells them apart, so they are not an ambiguous group")
    if profiles.signature(left) != profiles.signature(right):
        raise SystemExit(
            "FAIL: the two ports get different profiles, so mapping one "
            "would leave the other unmapped -- and phys changes between "
            "plugs, so the mapping would go stale anyway")
    print("  ok  they still share one profile: a signature is a model, not a "
          "port")


def check_control_characters_are_one_model() -> None:
    print("\nS1: names differing only in unprintable characters")
    base = profiles.signature(pad("USB Gamepad"))
    for description, name in [
        ("a 0x18 prefix", "\x18USB Gamepad"),
        ("a trailing newline", "USB Gamepad\n"),
        ("surrounding whitespace", "  USB Gamepad  "),
        ("an embedded NUL", "USB Gamepad\x00"),
    ]:
        if profiles.signature(pad(name)) != base:
            raise SystemExit(
                f"FAIL: {description} makes a different signature, so the "
                "same controller would be offered setup again every time the "
                "adapter renumbered")
        print(f"  ok  {description} collapses onto the same model")


def check_filename_is_safe() -> None:
    print("\nS1: a profile filename derived from a hostile name")
    for description, name in HOSTILE_NAMES + [("a newline", NEWLINE_NAME)]:
        filename = profiles._filename(profiles.signature(pad(name)))
        for bad in ("/", "\\", "\x00", "\n"):
            if bad in filename:
                raise SystemExit(
                    f"FAIL: {description} put {bad!r} in the profile "
                    f"filename ({filename!r}) -- the profile would be written "
                    "outside the profile directory, or not at all")
        if Path(filename).name != filename:
            raise SystemExit(
                f"FAIL: {description} gave {filename!r}, which is a path "
                "rather than a filename")
        landing = (PROFILE_DIR / filename).resolve()
        if landing.parent != PROFILE_DIR.resolve():
            raise SystemExit(
                f"FAIL: {description} would put the profile in "
                f"{landing.parent} -- the device name walked out of the "
                "profile directory")
        if not filename.endswith(".json") or filename == ".json":
            raise SystemExit(
                f"FAIL: {description} gave the unusable filename {filename!r}")
        if len(filename) > 128:
            raise SystemExit(
                f"FAIL: {description} gave a {len(filename)}-character "
                "filename; most filesystems stop at 255 bytes and a UTF-8 "
                "name is several bytes a character")
        if not filename.startswith("0079_1879_"):
            raise SystemExit(
                f"FAIL: {description} lost the vid:pid prefix ({filename!r}), "
                "so a name that cleans down to nothing would collide with "
                "every other such pad")
    print(f"  ok  {len(HOSTILE_NAMES) + 1} hostile names all give a plain, "
          "prefixed, bounded .json filename")


def check_profile_round_trip_on_disk() -> None:
    print("\nS1: a hostile-named controller's profile really reaches the disk")
    hostile = pad("\x18Wei,rd/Name\né" + "z" * 400)
    profile = profiles.Profile(signature=profiles.signature(hostile),
                               icon=icons.N64)
    profile.record(profiles.SCOPE_UNIVERSAL,
                   profiles.Mapping(buttons={"a": Binding("button", 1)},
                                    layout="n64"))
    written = profiles.save(profile, PROFILE_DIR)
    if written.parent != PROFILE_DIR:
        raise SystemExit(
            f"FAIL: the profile landed in {written.parent}, not the profile "
            "directory -- the name escaped the filename")
    loaded = profiles.load(hostile, PROFILE_DIR)
    if loaded is None or not loaded.has_bindings():
        raise SystemExit(
            "FAIL: the profile cannot be read back, so this controller would "
            "be offered the wizard again on every session")
    if loaded.buttons["a"].sdl() != "b1":
        raise SystemExit("FAIL: the stored binding did not survive the trip")
    if not profiles.is_known(hostile, PROFILE_DIR):
        raise SystemExit(
            "FAIL: is_known says no, so S1 would re-open setup by itself")
    print(f"  ok  saved as {written.name[:40]}... and loaded back with its "
          "bindings")


def check_icon_for_hostile_name() -> None:
    print("\nS1: the setup screen always has an icon to draw")
    for description, name in HOSTILE_NAMES + [("a newline", NEWLINE_NAME)]:
        icon = icons.for_pad(pad(name, 0, 0), {})
        if icon not in icons.ICON_NAMES:
            raise SystemExit(
                f"FAIL: {description} gave the icon {icon!r}, which has no "
                "SVG in the theme -- the setup screen would draw a hole")
    print(f"  ok  {len(HOSTILE_NAMES) + 1} hostile names, every one "
          "renderable")
    if icons.for_pad(pad("", 0, 0), {}) != icons.GAMEPAD:
        raise SystemExit(
            "FAIL: an empty name and vendor/product 0 do not fall back to the "
            "generic pad")
    print("  ok  an empty name with vendor 0 and product 0 falls back to "
          "'gamepad'")
    # The learned profile outranks every guess, including for a name the
    # built-in table would have matched.
    known = pad("Mayflash GameCube Adapter", 0x0079, 0x1843)
    profiles.save(profiles.Profile(signature=profiles.signature(known),
                                   icon=icons.ARCADE), PROFILE_DIR)
    previous = os.environ["PADMAP_PROFILE_DIR"]
    os.environ["PADMAP_PROFILE_DIR"] = str(PROFILE_DIR)
    try:
        chosen = icons.for_pad(known, {})
    finally:
        os.environ["PADMAP_PROFILE_DIR"] = previous
    if chosen != icons.ARCADE:
        raise SystemExit(
            f"FAIL: the learned profile said {icons.ARCADE!r} and for_pad "
            f"answered {chosen!r} -- a user's correction is being overruled "
            "by a vid/pid guess")
    print("  ok  a stored profile beats the built-in table for the same ids")


# -- S15: pads with shapes nobody would design -------------------------------


def check_buttons_but_no_axes() -> None:
    print("\nS15: a pad with buttons and no axes at all")
    keys = list(range(0x120, 0x12A))
    fields = controllercfg.guessed_fields(keys, [])
    if not fields:
        raise SystemExit(
            "FAIL: a button-only pad gets no fields, so a front-end would "
            "see a controller with nothing on it")
    if any(target.startswith("a") for target in fields.values()):
        raise SystemExit(
            f"FAIL: axes were declared on a pad with none: {fields}")
    result = controllercfg.fallback_line_for(1, pad("Buttons Only"), keys, [])
    if result is None:
        raise SystemExit(
            "FAIL: a pad with ten buttons got no SDL line, so the front-end "
            "cannot be navigated with it")
    read_back(result[0], virtual.virtual_name(1), {"a": "b0"},
              "a button-only pad")
    print(f"  ok  {len(fields)} fields, no sticks invented, one usable line")


def check_axes_but_no_buttons() -> None:
    print("\nS15: a pad with axes and no buttons")
    axis_codes = [0x00, 0x01, 0x02, 0x05]
    result = controllercfg.fallback_line_for(
        1, pad("Axes Only"), [], axis_codes)
    if result is not None:
        raise SystemExit(
            "FAIL: a line was written for a pad with no buttons. Nothing can "
            "be confirmed with it, and the line would claim a slot's GUID "
            "that a real controller then could not use.")
    print("  ok  no buttons means no line -- there is nothing to confirm with")
    fields = controllercfg.guessed_fields([], axis_codes)
    if any(field in fields for field in ("a", "b", "x", "y")):
        raise SystemExit(
            f"FAIL: face buttons were guessed onto a pad with none: {fields}")
    print("  ok  guessed_fields invents no face buttons either")


def check_single_axis() -> None:
    print("\nS15: a pad with exactly one axis")
    fields = controllercfg.guessed_fields([0x120], [0x00])
    if fields.get("leftx") != "a0":
        raise SystemExit(
            f"FAIL: the one axis was not declared as leftx: {fields}")
    if "lefty" in fields or "rightx" in fields or "righty" in fields:
        raise SystemExit(
            f"FAIL: axes the pad does not have were declared: {fields} -- SDL "
            "would report readings from an axis that never sends events")
    line = mapping.sdl_line(GUID, "One Axis", fields)
    read_back(line, "One Axis", {"leftx": "a0"}, "a single-axis pad")
    print("  ok  leftx:a0 and nothing else")


def check_hat_only() -> None:
    print("\nS15: a pad whose only absolute axes are a hat")
    axis_codes = [0x10, 0x11]
    fields = controllercfg.guessed_fields([0x120, 0x121], axis_codes)
    expected = {"dpup": "h0.1", "dpright": "h0.2",
                "dpdown": "h0.4", "dpleft": "h0.8"}
    for field, target in expected.items():
        if fields.get(field) != target:
            raise SystemExit(
                f"FAIL: {field} came out as {fields.get(field)!r}, not "
                f"{target!r} -- the d-pad is the only way to navigate a pad "
                "with no sticks")
    if any(target.startswith("a") for target in fields.values()):
        raise SystemExit(
            f"FAIL: a hat was counted as a stick axis: {fields} -- SDL and "
            "RetroArch both number hats separately")
    result = controllercfg.fallback_line_for(
        3, pad("Hat Only"), [0x120, 0x121], axis_codes)
    read_back(result[0], virtual.virtual_name(3), expected, "a hat-only pad")
    print("  ok  four hat directions, no phantom sticks, one line")


def check_twenty_axes() -> None:
    print("\nS15: a pad reporting twenty absolute axes")
    axis_codes = list(range(0, 20))
    spans = {code: (0, 255, 128) for code in axis_codes}
    fields = mapping.stick_fields(axis_codes, None, spans)
    if fields != {"leftx": "a0", "lefty": "a1",
                  "rightx": "a3", "righty": "a4"}:
        raise SystemExit(
            f"FAIL: twenty axes gave the sticks {fields} -- SDL numbers axes "
            "by position among the non-hat codes, so a wrong index points the "
            "stick at some other axis entirely")
    # 0x10-0x17 are hat codes; both consumers count them as hats, not axes.
    if mapping.axis_index(axis_codes, 0x0F) != 15:
        raise SystemExit("FAIL: the sixteenth real axis is not index 15")
    for hat_code in (0x10, 0x11, 0x12, 0x13):
        if mapping.axis_index(axis_codes, hat_code) is not None:
            raise SystemExit(
                f"FAIL: hat code {hat_code:#x} was given an axis index, so "
                "every axis after it would be numbered one too high")
    line = controllercfg.sdl_line_for(
        1, {"a": Binding("button", 0)}, axis_codes=axis_codes, axes=spans)
    read_back(line, virtual.virtual_name(1),
              {"a": "b0", "leftx": "a0", "righty": "a4"}, "a twenty-axis pad")
    print("  ok  sticks by index, hat codes skipped, one line for 20 axes")


def check_degenerate_axis_range() -> None:
    print("\nS15: an axis reporting max == min")
    if mapping.rests_centred((128, 128, 128)):
        raise SystemExit(
            "FAIL: an axis with no travel was called a centred stick. SDL "
            "would be told the pad has a stick that never moves, and on the "
            "menu side that reads as a stick held at whatever it reports.")
    print("  ok  a zero-width axis is not a stick (and does not divide by 0)")
    if mapping.rests_centred((255, 0, 128)):
        raise SystemExit("FAIL: an inverted range (max < min) passed as a stick")
    print("  ok  an inverted range is refused too")
    if capture.deflection((128, 128, 128), 200) != 0.0:
        raise SystemExit(
            "FAIL: deflection on a zero-width axis is not 0, so wizard "
            "prompts would be answered by an axis that cannot move")
    print("  ok  deflection reads 0 rather than raising ZeroDivisionError")
    fields = mapping.stick_fields([0, 1], None, {0: (5, 5, 5), 1: (5, 5, 5)})
    if fields:
        raise SystemExit(f"FAIL: a broken axis was declared a stick: {fields}")
    print("  ok  no stick fields are written for it")


def check_gamecube_triggers() -> None:
    print("\nS15: the GameCube adapter's triggers on the right stick's codes")
    # Measured here: ABS_RX/ABS_RY carry the analogue triggers and rest at 24
    # of 0-255. Declaring them the right stick jammed the menus to the left.
    spans = {0x00: (0, 255, 128), 0x01: (0, 255, 128),
             0x03: (0, 255, 24), 0x04: (0, 255, 24)}
    fields = mapping.stick_fields([0x00, 0x01, 0x03, 0x04], None, spans)
    if "rightx" in fields or "righty" in fields:
        raise SystemExit(
            "FAIL: an axis resting at 24 of 0-255 was declared the right "
            "stick. That is the reported 'pad stuck to the left' bug: SDL "
            "sees a stick pushed 80% over and held there forever.")
    if fields.get("leftx") != "a0" or fields.get("lefty") != "a1":
        raise SystemExit(
            f"FAIL: the real sticks were dropped too: {fields} -- refusing a "
            "trigger must not cost the pad its stick")
    print("  ok  triggers refused, the two centred axes kept as the left stick")
    if capture.deflection((0, 255, 24), 24) != 0.0:
        raise SystemExit(
            "FAIL: an untouched trigger does not read 0, so the wizard would "
            "record the direction it was travelling from")
    if capture.deflection((0, 255, 24), 255) < capture.AXIS_THRESHOLD:
        raise SystemExit(
            "FAIL: a fully pressed trigger does not pass the wizard's "
            "threshold, so L and R could never be mapped")
    print("  ok  measured from rest, the trigger reads 0 idle and 1.8 pressed")


def check_absinfo_outside_range() -> None:
    print("\nS15: absinfo reporting a resting value outside min..max")
    caps = {
        controllercfg.EV_ABS: [
            (0x00, AbsInfo(0, 255, 900)),      # rest above max
            (0x01, AbsInfo(0, 255, -900)),     # rest below min
            (0x02, AbsInfo(255, 0, 3)),        # inverted range
            (0x03, AbsInfo(0, 0, 0)),          # no travel at all
            0x99,                              # not a (code, info) pair
        ],
        controllercfg.EV_KEY: [0x120, 30],
    }
    real_open = devices.open_device
    devices.open_device = lambda _pad: StubDevice(caps)
    try:
        spans = controllercfg.pad_axis_spans(pad("Lying Adapter"))
        keys, axis_codes = controllercfg.pad_capabilities(pad("Lying Adapter"))
    finally:
        devices.open_device = real_open

    for code, (minimum, maximum, rest) in spans.items():
        if minimum <= maximum and not minimum <= rest <= maximum:
            raise SystemExit(
                f"FAIL: axis {code:#x} kept a resting value of {rest} outside "
                f"{minimum}..{maximum}. deflection measures from rest, so "
                "every reading would look like a huge deliberate push and the "
                "first prompt would be answered by the pad sitting still.")
    if spans.get(0x00) != (0, 255, 127) or spans.get(0x01) != (0, 255, 127):
        raise SystemExit(
            f"FAIL: an impossible rest was not replaced by the midpoint: "
            f"{spans}")
    print("  ok  a rest of 900 and one of -900 both become the midpoint")
    if 0x99 in spans or 0x99 in axis_codes:
        raise SystemExit(
            "FAIL: a bare code with no absinfo was read as an axis")
    print("  ok  an EV_ABS entry that is not a (code, absinfo) pair is skipped")
    sticks = mapping.stick_fields(axis_codes, None, spans)
    if "rightx" in sticks:
        raise SystemExit(
            "FAIL: ABS_RX, whose declared range is 0..0, was made the right "
            "stick -- SDL would read a stick that never moves")
    if sticks.get("leftx") != "a0" or sticks.get("lefty") != "a1":
        raise SystemExit(
            f"FAIL: the two axes whose declared travel is fine were dropped "
            f"because their resting value was nonsense: {sticks}. A lying "
            "absinfo value must cost the pad its calibration, not its stick.")
    print("  ok  the repaired axes are still sticks; the zero-width one is not")
    if keys != [30, 0x120]:
        raise SystemExit(f"FAIL: the key list came back as {keys}")
    print("  ok  the key list survives a keyboard-range code (see S7 below)")


def check_nothing_at_all() -> None:
    print("\nS15: a device reporting neither buttons nor axes")
    fields = controllercfg.guessed_fields([], [])
    if fields:
        raise SystemExit(f"FAIL: something was guessed from nothing: {fields}")
    line = mapping.sdl_line(GUID, "Empty Device", fields)
    read_back(line, "Empty Device", {}, "a device with no controls")
    print("  ok  an empty field set still writes a parseable line")


# -- S17: two players, two pads ----------------------------------------------


def check_two_identical_pads_get_their_own_slots() -> None:
    print("\nS17: two identical controllers still get a slot each")
    bindings = {"a": Binding("button", 0)}
    first = controllercfg.sdl_line_for(1, bindings, axis_codes=[0, 1])
    second = controllercfg.sdl_line_for(2, bindings, axis_codes=[0, 1])
    guid_one, name_one, _ = read_back(
        first, virtual.virtual_name(1), {"a": "b0"}, "player 1")
    guid_two, name_two, _ = read_back(
        second, virtual.virtual_name(2), {"a": "b0"}, "player 2")
    if guid_one == guid_two:
        raise SystemExit(
            "FAIL: both slots share a GUID, so SDL matches one line for both "
            "pads and the second player inherits the first one's mapping")
    if name_one == name_two:
        raise SystemExit("FAIL: both virtual pads carry the same name")
    print(f"  ok  distinct GUIDs ({guid_one[:8]}../{guid_two[:8]}..) and names")
    shared = {profiles.signature(pad("Mayflash GameCube Adapter", 0x79, 0x1843))
              for _ in range(2)}
    if len(shared) != 1:
        raise SystemExit("FAIL: identical pads no longer share a profile")
    print("  ok  and they still share one learned profile, as a model should")


# -- S3/S7/S8: the wizard, on hardware that does not cooperate ---------------


def new_run(keys, axes, layout_id="", index=0):
    run = MappingRun(pad=None, player=1, layout=layouts.get(layout_id),
                     keys=list(keys), axes=dict(axes), now=now)
    # Start partway through when the control under test is not a face button;
    # the wizard refuses an axis for a face button on purpose.
    run.index = index
    return run


def tap(run, code):
    run.feed(Event(EV_KEY, code, 1))
    tick(0.10)
    result = run.feed(Event(EV_KEY, code, 0))
    tick(capture.CAPTURE_GAP_SECONDS + 0.05)
    return result


def hold(run, code):
    run.feed(Event(EV_KEY, code, 1))
    tick(capture.SKIP_HOLD_SECONDS + 0.10)
    result = run.feed(Event(EV_KEY, code, 0))
    tick(capture.CAPTURE_GAP_SECONDS + 0.05)
    return result


def check_wizard_without_axes() -> None:
    print("\nS7: the wizard on a pad with buttons and no axes")
    run = new_run(range(0x120, 0x128), {})
    for code in (0x00, 0x01, 0x10, 0x11):
        if run.feed(Event(EV_ABS, code, 255)):
            raise SystemExit(
                f"FAIL: an axis event on code {code:#x} answered a prompt on "
                "a pad that reports no axes at all")
    if run.index != 0:
        raise SystemExit("FAIL: the wizard advanced on phantom axis events")
    print("  ok  axis events from a pad with no axes answer nothing")
    if not tap(run, 0x121):
        raise SystemExit("FAIL: a real button press was not recorded")
    binding = run.bindings["a"]
    if binding.sdl() != "b1" or binding.retroarch() != "1":
        raise SystemExit(
            f"FAIL: recorded {binding.sdl()}/{binding.retroarch()} for the "
            "second button of a straightforward pad")
    print("  ok  a button still records as b1 / 1")


def check_wizard_hat_only() -> None:
    print("\nS7: the wizard on a hat-only pad")
    # index 4 is the first d-pad control of the generic layout; a face button
    # deliberately refuses axis input.
    run = new_run([0x120], {0x10: (-1, 1, 0), 0x11: (-1, 1, 0)}, index=4)
    if not run.feed(Event(EV_ABS, 0x11, -1)):
        raise SystemExit(
            "FAIL: a hat push did not answer the d-pad prompt, so a pad whose "
            "only directions are a hat cannot be mapped")
    binding = run.bindings["dpup"]
    if binding.sdl() != "h0.1" or binding.retroarch() != "h0up":
        raise SystemExit(
            f"FAIL: the hat recorded as {binding.sdl()}/{binding.retroarch()}, "
            "and a wrong spelling is a direction that silently does nothing")
    print("  ok  hat up records as h0.1 for SDL and h0up for RetroArch")
    tick(capture.CAPTURE_GAP_SECONDS + 0.05)
    run.feed(Event(EV_ABS, 0x11, 0))
    if not run.feed(Event(EV_ABS, 0x11, 1)):
        raise SystemExit("FAIL: the opposite hat direction was refused")
    if run.bindings["dpdown"].sdl() != "h0.4":
        raise SystemExit("FAIL: hat down did not record as h0.4")
    print("  ok  the opposite direction records separately, as h0.4")


def check_wizard_twenty_axes() -> None:
    print("\nS7: the wizard on a twenty-axis pad records the index, not the "
          "code")
    spans = {code: (0, 255, 128) for code in range(20)}
    run = new_run([0x120], spans, index=4)
    if not run.feed(Event(EV_ABS, 0x0F, 255)):
        raise SystemExit("FAIL: axis 0x0f did not answer the prompt")
    binding = run.bindings["dpup"]
    if binding.index != 15 or binding.sdl() != "+a15":
        raise SystemExit(
            f"FAIL: axis code 0x0f recorded as {binding.sdl()}. Storing the "
            "evdev code instead of the index points the binding at whichever "
            "axis happens to sit at that position.")
    print("  ok  evdev code 0x0f becomes axis index 15 (+a15 / +15)")
    if binding.retroarch() != "+15":
        raise SystemExit("FAIL: the RetroArch spelling lost its sign")
    print("  ok  the sign survives, so the two stick directions stay apart")


def check_wizard_low_button_codes() -> None:
    print("\nS8/S7: a pad carrying BTN_MISC-range codes")
    keys = [0x100, 0x101] + list(range(0x120, 0x124))
    if mapping.sdl_button_index(keys, 0x100) == \
            mapping.retroarch_button_index(keys, 0x100):
        raise SystemExit(
            "FAIL: the two consumers now agree about a sub-0x120 code, which "
            "makes carrying both indices pointless -- check the numbering")
    run = new_run(keys, {})
    if not tap(run, 0x100):
        raise SystemExit("FAIL: a BTN_MISC-range button was not recorded")
    binding = run.bindings["a"]
    if binding.sdl() != "b4":
        raise SystemExit(
            f"FAIL: SDL index came out as {binding.sdl()}; SDL walks "
            "0x120..KEY_MAX before 0..0x120, so this button is b4")
    if binding.retroarch() != "0":
        raise SystemExit(
            f"FAIL: RetroArch index came out as {binding.retroarch()}; its "
            "udev driver counts from BTN_MISC, so this button is 0. Getting "
            "it wrong shifts every binding on this pad silently.")
    print("  ok  the same press is b4 to SDL and 0 to RetroArch, both stored")
    if not hold(run, 0x121):
        raise SystemExit(
            "FAIL: holding a button did not skip a control the pad lacks")
    if "b" in run.bindings or run.index != 2:
        raise SystemExit(
            "FAIL: the skip recorded a binding instead of moving past")
    print("  ok  S8: holding any button still skips, no named button needed")


def check_keyboard_range_code() -> None:
    print("\nS3/S7: an adapter that also reports keyboard key codes")
    keys = [30, 48] + list(range(0x120, 0x12C))   # KEY_A, KEY_B, 12 buttons
    if 48 >= assign.BTN_FIRST:
        raise SystemExit("FAIL: KEY_B is not below the assigner's cutoff")
    print(f"  ok  S3: assign ignores everything below {assign.BTN_FIRST:#x}, "
          "so a stuck keyboard key cannot claim a player slot")
    if mapping.retroarch_button_index(keys, 48) is not None:
        raise SystemExit(
            "FAIL: RetroArch's numbering now includes a sub-BTN_MISC code")
    print("  ok  RetroArch's udev driver cannot see KEY_B at all (index None)")
    visible = len([code for code in keys if code >= mapping.BTN_MISC])
    if mapping.sdl_button_index(keys, 48) != 13:
        raise SystemExit("FAIL: SDL's numbering for KEY_B moved")
    print(f"  ok  SDL numbers it b13, while RetroArch sees only {visible} "
          "buttons")

    run = new_run(keys, {})
    if not tap(run, 48):
        raise SystemExit(
            "FAIL: the wizard refused the key outright -- if that is the new "
            "behaviour, turn the gap below into an assertion")
    binding = run.bindings["a"]
    if binding.sdl() != "b13":
        raise SystemExit(f"FAIL: SDL binding is {binding.sdl()}, not b13")
    if binding.ra_index is None and int(binding.retroarch()) >= visible:
        gap("capture.MappingRun records a button whose evdev code is below "
            "BTN_MISC, and mapping.Binding treats ra_index=None as 'both "
            "consumers agree' -- but retroarch_button_index returned None "
            "because RetroArch cannot see that code at all. Repro: "
            "keys=[30, 48]+list(range(0x120,0x12c)); tap KEY_B (48) in a "
            "MappingRun -> Binding(kind='button', index=13, ra_index=None), "
            f"whose .retroarch() is '13' on a pad where RetroArch counts only "
            f"{visible} buttons (0..{visible - 1}). The autoconfig line binds "
            "a button that does not exist and RetroArch still reports the pad "
            "as configured. assign.BTN_FIRST already refuses this range for "
            "slot claims; the wizard does not.")
    else:
        raise SystemExit(
            "FAIL: this is now handled -- turn the gap into an assertion")


def check_chooser_hostile() -> None:
    print("\nS3/S7: the chooser, driven from unhelpful hardware")
    empty = Chooser(pad=None, player=1, options=[], now=now)
    if empty.chosen != "" or empty.chosen_layout != "":
        raise SystemExit(
            "FAIL: a chooser with no options claims something is selected")
    if empty.move(1) or empty.feed(Event(EV_ABS, 0x10, 1)):
        raise SystemExit("FAIL: an empty chooser reported a change")
    event = empty.to_event()
    if event["choices"] or event["chosen"] != "":
        raise SystemExit("FAIL: an empty chooser sends a bogus selection")
    print("  ok  an empty option strip cannot be moved or mis-selected")

    # A pad whose ABS_X range is broken: the hat still has to work, or the
    # picker is unusable and S1's automatic setup screen is a dead end.
    stuck = Chooser(pad=None, player=1, options=capture.layout_options(),
                    axes={0x00: (128, 128, 128)}, now=now)
    if stuck.feed(Event(EV_ABS, 0x00, 255)):
        raise SystemExit(
            "FAIL: an axis with no declared travel moved the selection")
    if stuck.index != 0:
        raise SystemExit("FAIL: the selection moved on a dead axis")
    if not stuck.feed(Event(EV_ABS, 0x10, 1)):
        raise SystemExit(
            "FAIL: the hat cannot move the picker, so a pad with a broken "
            "ABS_X could never choose a layout")
    print("  ok  a zero-travel ABS_X is ignored and the hat still works")

    # An adapter that puts a trigger-shaped axis on ABS_X: it rests at one end
    # of its range, so measuring from the midpoint would call it pushed.
    trigger = Chooser(pad=None, player=1, options=capture.layout_options(),
                      axes={0x00: (0, 255, 24)}, now=now)
    if trigger.feed(Event(EV_ABS, 0x00, 24)):
        raise SystemExit(
            "FAIL: an axis sitting at rest moved the selection, so the picker "
            "would scroll on its own")
    if not trigger.feed(Event(EV_ABS, 0x00, 255)):
        raise SystemExit("FAIL: a full push did not move the selection")
    if trigger.feed(Event(EV_ABS, 0x00, 250)):
        raise SystemExit(
            "FAIL: a held push moved the selection twice; a stick held over "
            "would spin the strip")
    print("  ok  an off-centre ABS_X rests still, moves once, and stops")


def check_chooser_hold_is_the_only_confirm() -> None:
    print("\nS3: nothing but a deliberate hold confirms")
    chooser = Chooser(pad=None, player=1, options=capture.layout_options(),
                      axes={}, held={0x130}, now=now)
    chooser.feed(Event(EV_KEY, 0x130, 0))     # the press that opened it
    if chooser.confirmed:
        raise SystemExit(
            "FAIL: the press that opened the picker confirmed it, so the user "
            "never sees the strip")
    print("  ok  the press that opened the picker does not confirm it")
    chooser.feed(Event(EV_KEY, 0x131, 1))
    tick(0.10)
    chooser.feed(Event(EV_KEY, 0x131, 0))
    if chooser.confirmed:
        raise SystemExit("FAIL: a tap confirmed the picker")
    print("  ok  a tap does not confirm either")
    chooser.feed(Event(EV_KEY, 0x131, 1))
    tick(capture.SKIP_HOLD_SECONDS + 0.10)
    chooser.feed(Event(EV_KEY, 0x131, 0))
    if not chooser.confirmed:
        raise SystemExit(
            "FAIL: a held button did not confirm, and no other gesture can -- "
            "the pads are grabbed, so the front-end hears nothing")
    print("  ok  a hold confirms, which is the one gesture needing no mapping")


# -- S15: the RetroArch side -------------------------------------------------


def check_derive_profile_shape() -> None:
    print("\nS15: derive_profile writes one setting per line")
    text = retroarch.derive_profile(None, 4, vid=0x0079, pid=0x1879)
    body = [line for line in text.splitlines() if not line.startswith("#")]
    for line in body:
        if line.count("=") != 1 or not line.endswith('"'):
            raise SystemExit(
                f"FAIL: {line!r} is not a single quoted RetroArch setting; "
                "RetroArch stops reading a profile it cannot parse")
    if f'input_device = "{virtual.virtual_name(4)}"' not in body:
        raise SystemExit(
            "FAIL: the profile does not name the virtual pad, so RetroArch "
            "would never match it")
    if 'input_vendor_id = "121"' not in body:
        raise SystemExit(
            f"FAIL: vendor 0x0079 was not written as decimal 121: {body}")
    print("  ok  named after the virtual pad, ids in decimal, one per line")
    if "<none found>" not in text.splitlines()[1]:
        raise SystemExit("FAIL: a missing source profile is not reported")
    print("  ok  a pad with no upstream profile says so in the header")


def check_vendor_zero() -> None:
    print("\nS15: a pad that reports vendor 0 and product 0")
    zero = virtual.Identity(0, 0, 0x03, 0x0111)
    real_identity = controllercfg.identity_for
    controllercfg.identity_for = lambda _pad: zero
    try:
        captured = controllercfg.retroarch_profile(
            1, pad("Zero Ids", 0, 0), {"a": Binding("button", 1)}, layout="")
    finally:
        controllercfg.identity_for = real_identity
    if 'input_vendor_id = "0"' not in captured:
        raise SystemExit(
            "FAIL: retroarch_profile does not write the ids the virtual pad "
            "actually advertises; a disagreeing vid/pid scores against the "
            f"profile in RetroArch's autoconfig match. Got: "
            f"{[l for l in captured.splitlines() if 'vendor' in l]}")
    print("  ok  a captured profile writes vendor 0, matching what the pad "
          "advertises")

    derived = retroarch.derive_profile(None, 1, vid=0, pid=0)
    if f'input_vendor_id = "{virtual.PADMAP_VID}"' in derived:
        gap("retroarch.derive_profile writes `vid or PADMAP_VID`, so a pad "
            "whose descriptor reports vendor 0 (or product 0) gets padmap's "
            f"own {virtual.PADMAP_VID}:{virtual.PADMAP_PID} in its autoconfig "
            "profile while the virtual pad advertises 0:0 -- exactly the "
            "disagreement the docstring says scores against the profile. "
            "Repro: retroarch.derive_profile(None, 1, vid=0, pid=0) -> "
            f'input_vendor_id = "{virtual.PADMAP_VID}". Reached from '
            "install_profiles for any unmapped pad in mirror mode whose "
            "vendor is 0, including one whose /sys id/vendor is unreadable "
            "(devices._read_hex returns 0). controllercfg.retroarch_profile, "
            "the captured-bindings path just above, writes 0 correctly -- so "
            "the same controller gets two different answers depending on "
            "whether it has been through the wizard.")
    else:
        raise SystemExit(
            "FAIL: this is now handled -- turn the gap into an assertion")


def check_diagonal_hat_binding() -> None:
    print("\nS15: a hat binding holding a diagonal value")
    for value, spelling in ((1, "h0up"), (2, "h0right"),
                            (4, "h0down"), (8, "h0left")):
        if Binding("hat", 0, value).retroarch() != spelling:
            raise SystemExit(
                f"FAIL: hat bit {value} is not {spelling}; a wrong direction "
                "word is a d-pad that silently does nothing")
    print("  ok  the four single-bit directions spell out correctly")
    if Binding("hat", 0, 3).sdl() != "h0.3":
        raise SystemExit("FAIL: SDL's hat spelling changed")
    try:
        mapping.retroarch_lines({"dpup": Binding("hat", 0, 3)})
    except KeyError:
        gap("mapping.Binding.retroarch() raises KeyError on a hat value that "
            "is not a single direction bit, while .sdl() accepts it (h0.3 is "
            "a legal diagonal). Binding.from_json takes any int, so a "
            "hand-edited or partially written profile carrying "
            '{\"kind\": \"hat\", \"value\": 3} crashes retroarch_lines while '
            "the launch profiles are being written -- the game then starts "
            "with no controller config at all. Repro: "
            "mapping.retroarch_lines({'dpup': mapping.Binding('hat', 0, 3)}).")
    else:
        raise SystemExit(
            "FAIL: this is now handled -- turn the gap into an assertion")


def main() -> int:
    print(f"sandbox: {_SANDBOX}\n")

    check_comma_name()
    check_hostile_names_round_trip()
    check_newline_name()
    check_guid_hostile_identity()

    check_four_identical_ports()
    check_phys_only_difference()
    check_control_characters_are_one_model()
    check_filename_is_safe()
    check_profile_round_trip_on_disk()
    check_icon_for_hostile_name()

    check_buttons_but_no_axes()
    check_axes_but_no_buttons()
    check_single_axis()
    check_hat_only()
    check_twenty_axes()
    check_degenerate_axis_range()
    check_gamecube_triggers()
    check_absinfo_outside_range()
    check_nothing_at_all()

    check_two_identical_pads_get_their_own_slots()

    check_wizard_without_axes()
    check_wizard_hat_only()
    check_wizard_twenty_axes()
    check_wizard_low_button_codes()
    check_keyboard_range_code()
    check_chooser_hostile()
    check_chooser_hold_is_the_only_confirm()

    check_derive_profile_shape()
    check_vendor_zero()
    check_diagonal_hat_binding()

    if GAPS:
        print(f"\n{len(GAPS)} gap(s) reported above -- these are defects in "
              "the source, left unasserted so this check stays green. Fix the "
              "source and turn each one into an assertion.")
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
