"""Button mapping: what the user pressed, and what each consumer needs to hear.

One capture, two very different outputs. RetroArch reads its own autoconfig
files and refers to buttons by evdev code; Pegasus reads SDL's game controller
database and refers to them by SDL's own button numbering, under a GUID it
computes from the device. Neither will tell you it has read a mapping wrong --
it simply behaves as though the button does not exist -- so both formats are
verified against artefacts produced by the real thing rather than by
inspection.

The capture itself is deliberately in terms of a *canonical* controller. padmap
creates the virtual pad, so it decides what shape that pad has; recording
"which physical button is A" once means every consumer downstream can be told
about one standard layout instead of a different arrangement per controller.
"""

from __future__ import annotations

from dataclasses import dataclass

# Canonical control -> how SDL spells it in a mapping line.
#
# Most are the same word. The exceptions are the stick halves: SDL has no
# concept of "C-up", it has a right stick, and a button that pushes that stick
# one way is written as a half-axis target (`-righty:b11`). An N64 pad's four
# C-buttons are exactly that, which is why layouts give them right-stick
# canonical names rather than inventing face buttons for them.
SDL_FIELDS: dict[str, str] = {
    "a": "a",
    "b": "b",
    "x": "x",
    "y": "y",
    "back": "back",
    "start": "start",
    "leftshoulder": "leftshoulder",
    "rightshoulder": "rightshoulder",
    "lefttrigger": "lefttrigger",
    "righttrigger": "righttrigger",
    "dpup": "dpup",
    "dpdown": "dpdown",
    "dpleft": "dpleft",
    "dpright": "dpright",
    "rightstick_up": "-righty",
    "rightstick_down": "+righty",
    "rightstick_left": "-rightx",
    "rightstick_right": "+rightx",
}

# Stable output order, so regenerating a mapping does not reshuffle the file
# and make a diff look like a change.
CANONICAL_ORDER: list[str] = list(SDL_FIELDS)

# Canonical control -> the RetroArch autoconfig key for the same thing.
#
# The names differ in more than spelling. RetroArch's `a`/`b` are the
# *Nintendo* positions -- b is the bottom face button, a is the right one --
# while SDL's a/b are bottom and right respectively, so a and b do not
# cross-map the way the letters suggest. Getting this backwards swaps confirm
# and cancel in every game, which is exactly the sort of thing that reads as
# "the mapping did not take".
RETROARCH_KEYS: dict[str, str] = {
    "a": "input_b_btn",
    "b": "input_a_btn",
    "x": "input_y_btn",
    "y": "input_x_btn",
    "start": "input_start_btn",
    "back": "input_select_btn",
    "leftshoulder": "input_l_btn",
    "rightshoulder": "input_r_btn",
    "lefttrigger": "input_l2_btn",
    "righttrigger": "input_r2_btn",
    "dpup": "input_up_btn",
    "dpdown": "input_down_btn",
    "dpleft": "input_left_btn",
    "dpright": "input_right_btn",
    # RetroArch can drive a stick axis from a button, which is how C-buttons
    # reach an N64 core. Y is positive downwards.
    "rightstick_up": "input_r_y_minus_btn",
    "rightstick_down": "input_r_y_plus_btn",
    "rightstick_left": "input_r_x_minus_btn",
    "rightstick_right": "input_r_x_plus_btn",
}


@dataclass(frozen=True)
class Binding:
    """Where one control lives on a pad.

    `kind` is "button", "hat" or "axis". A d-pad is a hat on most pads and
    four ordinary buttons on some, and triggers are an axis on anything with
    analogue ones -- so the capture has to be able to say which, rather than
    assuming everything is a button.
    """

    kind: str
    index: int
    value: int = 0
    # RetroArch numbers buttons from a lower base than SDL, so the same
    # physical button can be b2 to one and 0 to the other. Carrying both is
    # the only way a stored binding stays right for both consumers; None
    # means they agree, which is the case on most pads, and RA_INVISIBLE
    # means RetroArch has no number for this button at all.
    ra_index: int | None = None

    def sdl_visible(self) -> bool:
        """Whether SDL can be told about this binding at all."""
        if self.kind == "hat":
            return self.value in HAT_DIRECTIONS
        return self.kind in ("button", "axis")

    def retroarch_visible(self) -> bool:
        """Whether RetroArch can be told about this binding at all.

        False for a button whose evdev code sits below BTN_MISC: RetroArch's
        udev driver never enumerates those, so there is no number that names
        it. See RA_INVISIBLE for what happens when one is invented anyway.
        """
        if not self.sdl_visible():
            return False
        if self.kind == "button" and self.ra_index is not None:
            # Any negative index, not just RA_INVISIBLE itself: a profile off
            # disk can hold whatever it likes, and no real button is negative.
            return self.ra_index >= 0
        return True

    def sdl(self) -> str:
        if self.kind == "button":
            return f"b{self.index}"
        if self.kind == "hat":
            if self.value not in HAT_DIRECTIONS:
                # A hat mask naming two directions or none is not a control
                # anyone can press. See retroarch() below: refusing it here
                # too is what stops the two consumers disagreeing about
                # whether the d-pad direction exists.
                raise ValueError(
                    f"hat value {self.value} is not one direction bit")
            return f"h{self.index}.{self.value}"
        if self.kind == "axis":
            sign = "+" if self.value >= 0 else "-"
            return f"{sign}a{self.index}"
        raise ValueError(f"unknown binding kind {self.kind!r}")

    def retroarch(self) -> str:
        """RetroArch's spelling of the same thing.

        Hats are `hN<direction>`, e.g. `h0up`; axes carry a sign; buttons are
        a bare number.

        Raises ValueError for a binding RetroArch cannot be told about --
        `retroarch_visible` is the question to ask first. Both the shapes that
        raise came out of real files: a hat value that is not a single
        direction (a hand-edited or half-written profile) used to raise
        KeyError from the middle of writing the launch profiles, and a button
        below BTN_MISC used to come out as a plausible-looking number that
        named a different button, or none.
        """
        index = self.index if self.ra_index is None else self.ra_index
        if self.kind == "button":
            if index < 0:
                raise ValueError(
                    "RetroArch's udev driver has no number for this button "
                    "(its evdev code is below BTN_MISC)")
            return str(index)
        if self.kind == "hat":
            if self.value not in HAT_DIRECTIONS:
                raise ValueError(
                    f"hat value {self.value} is not one direction bit")
            return f"h{index}{HAT_DIRECTIONS[self.value]}"
        if self.kind == "axis":
            sign = "+" if self.value >= 0 else "-"
            return f"{sign}{index}"
        raise ValueError(f"unknown binding kind {self.kind!r}")

    def to_json(self) -> dict[str, int | str | None]:
        return {
            "kind": self.kind, "index": self.index, "value": self.value,
            "ra_index": self.ra_index,
        }

    @classmethod
    def from_json(cls, raw: dict) -> "Binding":
        ra = raw.get("ra_index")
        return cls(
            kind=str(raw.get("kind", "button")),
            index=int(raw.get("index", 0)),
            value=int(raw.get("value", 0)),
            ra_index=None if ra is None else int(ra),
        )


# SDL hat bit -> RetroArch's direction word.
#
# Only the four single bits. A hat reads 3 ("up and right") on a diagonal and
# 0 at rest, and neither is a control: RetroArch's config has one direction
# word per key, and an SDL mask of two bits only matches while both are held.
# Binding refuses both rather than letting one consumer render what the other
# cannot -- see Binding.sdl.
HAT_DIRECTIONS = {1: "up", 2: "right", 4: "down", 8: "left"}

# Where each consumer starts counting buttons. They are not the same, and the
# difference is invisible on most pads.
BTN_MISC = 0x100
BTN_JOYSTICK = 0x120

# ra_index for a button RetroArch cannot see at all.
#
# retroarch_button_index used to answer None for two different reasons -- the
# two consumers agree (the usual case), or the code is below BTN_MISC and
# RetroArch's udev driver never enumerates it -- and Binding read that None as
# "they agree". A combo adapter reporting KEY_A/KEY_B alongside its twelve
# buttons therefore stored Binding(kind="button", index=13, ra_index=None),
# whose .retroarch() was "13" on a pad RetroArch numbers 0..11. RetroArch
# binds a button that does not exist -- or, if the pad has enough BTN_MISC
# codes, a real but entirely different one -- without complaining, and still
# reports the pad as configured: the control works in Pegasus and is dead in
# every game. A negative index keeps the two answers apart; no real button
# index is negative.
RA_INVISIBLE = -1


def sdl_button_index(keys: list[int], code: int) -> int | None:
    """Which button number SDL will give an evdev key code.

    SDL walks BTN_JOYSTICK..KEY_MAX first and only then 0..BTN_JOYSTICK, so
    the numbering is *not* simply ascending for a pad carrying any button
    below 0x120 -- some arcade sticks report BTN_MISC-range codes.

    Pinned in check_mapping.py against a mapping SDL wrote: on the measured
    pad, key 0x121 is b1 and 0x128 is b8.
    """
    ordered = ([c for c in sorted(keys) if c >= BTN_JOYSTICK]
               + [c for c in sorted(keys) if c < BTN_JOYSTICK])
    return ordered.index(code) if code in ordered else None


def retroarch_button_index(keys: list[int], code: int) -> int | None:
    """Which button number RetroArch's udev driver will give an evdev key code.

    Plain ascending order from BTN_MISC, which is a lower starting point than
    SDL's. On a pad whose buttons all sit at 0x120 or above -- most of them --
    the two agree exactly, which is why this difference can go unnoticed until
    it silently shifts every binding on the one pad that does not.

    Three answers, not two, because a caller storing this in a Binding has to
    be able to tell them apart:

    * an index, when RetroArch numbers the code;
    * RA_INVISIBLE, when the pad reports the code but RetroArch's driver
      cannot see it -- anything below BTN_MISC, which is every KEY_* code a
      combo adapter or an arcade encoder throws in alongside its buttons;
    * None, when the code is not one of this pad's at all.

    Answering None for the middle case is what let a keyboard key be written
    into an autoconfig as a button number: see RA_INVISIBLE.
    """
    if code not in keys:
        return None
    if code < BTN_MISC:
        return RA_INVISIBLE
    ordered = [c for c in sorted(keys) if c >= BTN_MISC]
    return ordered.index(code)


# Hats are absolute axes too, but neither consumer counts them as axes.
HAT_CODES = set(range(0x10, 0x18))


def axis_index(codes: list[int], code: int) -> int | None:
    """Which axis number an evdev ABS code will be given.

    Both SDL and RetroArch number axes by ascending code among the real axes,
    skipping the hat codes, which they treat as hats instead. Storing the raw
    evdev code and hoping it is the index works right up to the first pad
    whose axes are not 0,1,2,... -- ABS_RZ is 5.
    """
    ordered = [c for c in sorted(codes) if c not in HAT_CODES]
    return ordered.index(code) if code in ordered else None


def _crc16(data: bytes) -> int:
    """SDL's CRC-16, needed to reproduce a joystick GUID.

    SDL 2.26 onwards stores a checksum of the device name in bytes 2-3 of the
    GUID, so a mapping written for the wrong checksum is simply never matched.
    This is CRC-16/ARC -- reflected, polynomial 0xA001, zero initial value --
    which `check_mapping.py` pins against a GUID SDL itself wrote.
    """
    crc = 0
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc & 0xFFFF


def sdl_guid(bus: int, vendor: int, product: int, version: int, name: str) -> str:
    """The GUID SDL will compute for a device, as it appears in its database.

    Sixteen little-endian 16-bit fields with padding between them; the name
    checksum sits second. padmap creates the virtual pads, so it knows every
    input here and can write a mapping before SDL has ever seen the device.
    """
    fields = [
        bus & 0xFFFF,
        _crc16(name.encode("utf-8")),
        vendor & 0xFFFF,
        0,
        product & 0xFFFF,
        0,
        version & 0xFFFF,
        0,
    ]
    return "".join(f"{value & 0xFF:02x}{(value >> 8) & 0xFF:02x}"
                   for value in fields)


# evdev ABS code -> the SDL stick field it feeds.
#
# Sticks are not captured: the wizard asks about buttons, and asking someone
# to "press left stick X" is both awkward and unnecessary, since a stick is
# already unambiguous from the device's own axes. They still have to be in the
# mapping -- without leftx/lefty SDL reports no stick at all and a front-end
# loses every form of navigation except the d-pad.
STICK_AXES: dict[int, str] = {
    0x00: "leftx",    # ABS_X
    0x01: "lefty",    # ABS_Y
    0x03: "rightx",   # ABS_RX
    0x04: "righty",   # ABS_RY
}


# One axis's declared travel plus where it sits untouched: minimum, maximum,
# rest. Read from the driver's absinfo -- see Server._absolute_ranges.
AxisSpan = tuple[int, int, int]


# How far from the middle of its range an axis may rest and still be called a
# stick, as a fraction of half that range. A real stick centres within a few
# percent; the pads measured here sit inside 4%. Anything near an end is a
# trigger, and this is a wide berth around that distinction.
STICK_REST_TOLERANCE = 0.5


def rests_centred(span: AxisSpan) -> bool:
    """Whether an axis sits near the middle of its travel when untouched."""
    minimum, maximum, rest = span
    if maximum <= minimum:
        return False
    offset = (rest - (minimum + maximum) / 2) / ((maximum - minimum) / 2)
    return abs(offset) <= STICK_REST_TOLERANCE


def stick_fields(
    axis_codes: list[int],
    bindings: dict[str, Binding] | None = None,
    axes: dict[int, AxisSpan] | None = None,
) -> dict[str, str]:
    """SDL stick entries for the axes a pad actually reports.

    The map above is a guess by evdev code, and on the Mayflash GameCube
    adapter that guess is wrong in the worst possible way: its analogue
    triggers are ABS_RX and ABS_RY, so declaring them the right stick told SDL
    the stick was jammed 80% to the upper-left and held there forever. The
    reported symptom was the pad being "stuck to the left" in the front-end.

    So two axes are refused:

    * one that does not rest near the middle of its range. A stick centres and
      a trigger does not, which is the difference the code number cannot
      express. This needs `axes`; without it the old guess stands, since a
      caller with no absinfo is no worse off than before.
    * one a capture already claims. Nothing good comes of an axis being both a
      stick and a button: at best the two disagree about what is pressed, and
      the front-end believes whichever it reads first.

    Deliberately not the same test as capture.deflection, which asks how far
    an axis has moved *from* rest. This asks where rest is.
    """
    taken = {
        binding.index for binding in (bindings or {}).values()
        if binding.kind == "axis"
    }
    fields = {}
    for code, field in STICK_AXES.items():
        if code not in axis_codes:
            continue
        index = axis_index(axis_codes, code)
        if index is None or index in taken:
            continue
        span = (axes or {}).get(code)
        if span is not None and not rests_centred(span):
            continue
        fields[field] = f"a{index}"
    return fields


# Characters a device name may not carry into a database line.
#
# The comma is the field separator. The rest all end a line for one reader or
# the other: SDL splits the file on newline, and padmap's own rewriter --
# controllercfg.write_sdl_mappings, which must preserve the lines it does not
# own -- uses str.splitlines(), which additionally breaks on \v, \f, \x1c-\x1e,
# NEL and the Unicode line/paragraph separators. A name is a USB string
# descriptor written by somebody else (this machine reports one
# beginning 0x18), so none of these is hypothetical.
_NAME_FORBIDDEN = {
    ord(char): None
    for char in ",\n\r\v\f\x1c\x1d\x1e\x85\u2028\u2029"
}


def sdl_line(guid: str, name: str, fields: dict[str, str],
             platform: str = "Linux") -> str:
    """One line for SDL's controller database, from plain field:target pairs.

    Separate from sdl_mapping because not every line padmap writes comes from
    a capture: one carried over from another database has fields padmap never
    asks about (`guide`, `leftstick`), and dropping them on the way through
    would quietly cost the user bindings they already had.

    Commas separate the fields and the name sits in one of them, so a name
    containing a comma would silently corrupt every field after it. A newline
    is worse and was not guarded: the database is read a line at a time, so a
    name carrying one writes *two* physical lines -- SDL reads the first as a
    device with no bindings and drops the second as junk, and the pad ends up
    with no mapping at all rather than a damaged one. Carriage return goes the
    same way, since SDL trims CR when it splits lines and a stray one would
    otherwise ride along inside the name it matches on.
    """
    parts = [guid, name.translate(_NAME_FORBIDDEN)]
    parts += [f"{field}:{target}" for field, target in fields.items()]
    parts.append(f"platform:{platform}")
    return ",".join(parts) + ","


def parse_sdl_line(line: str) -> tuple[str, str, dict[str, str]] | None:
    """Split a database line into (guid, name, fields), or None if it is not one.

    Tolerant on purpose: this reads files the user and other programs write,
    where a comment, a blank line or a trailing comma are all normal.
    """
    stripped = line.strip()
    if not stripped or stripped.startswith("#"):
        return None
    parts = stripped.split(",")
    if len(parts) < 2 or len(parts[0]) != 32:
        return None
    fields: dict[str, str] = {}
    for part in parts[2:]:
        field, sep, target = part.partition(":")
        if sep and field and target:
            fields[field.strip()] = target.strip()
    return parts[0].lower(), parts[1], fields


def sdl_mapping(guid: str, name: str, bindings: dict[str, Binding],
                platform: str = "Linux",
                sticks: dict[str, str] | None = None) -> str:
    """One line for SDL's controller database, from a capture.

    A binding SDL cannot express is left out rather than raising: the shape
    that arrives here is a hat value that is not a single direction, off a
    profile somebody edited or a write that was cut short, and dying halfway
    through means the pad gets no line at all -- every control lost to save
    one. Left out, the direction reads as unmapped and the wizard can be run
    again.
    """
    fields = {}
    for control in CANONICAL_ORDER:
        binding = bindings.get(control)
        field = SDL_FIELDS.get(control)
        if binding is not None and field is not None and binding.sdl_visible():
            fields[field] = binding.sdl()
    fields.update(sticks or {})
    return sdl_line(guid, name, fields, platform)


# The four analog stick half-axis pairs, by the stem of their RetroArch keys.
# Each stem gets both a `_minus` and a `_plus` bind, and RetroArch reads the
# two together -- see drop_shadowed_axis_halves.
ANALOG_STEMS: tuple[str, ...] = (
    "input_l_x", "input_l_y", "input_r_x", "input_r_y",
)


def drop_shadowed_axis_halves(lines: list[str]) -> list[str]:
    """Remove an `_axis` bind that would stop the other half's `_btn` working.

    RetroArch reads a stick axis in `input_joypad_analog_axis`, and it reads
    both halves before it will look at a button:

        res  = abs(input_joypad_axis(..., axis_plus,  ...));
        res -= abs(input_joypad_axis(..., axis_minus, ...));

        if (res == 0)
        {
            ... consult bind_minus->joykey / bind_plus->joykey ...
        }

    The button fallback is gated behind `res == 0`. So a mapping that puts a
    button on one half of an axis and leaves an axis on the other half only
    works while that axis reads *exactly* zero, and it never does:

    * `udev_compute_axis` is `(value - min) * 0xffff / range - 0x7fff`. With
      the 0..255 range these adapters report, the value that would normalise
      to zero is 127.5 -- there isn't one. The nearest, 127, comes out at -128
      and 128 at +129, so one half or the other is always slightly live.
    * an uncalibrated stick is worse. This adapter's C-stick rests at 131,
      which is +900. Under 3% of full scale, so it sits inside the core's own
      deadzone and the stick looks perfectly normal -- while `res` is 900, the
      fallback never runs, and the button bound to the other half is dead.

    That was the reported bug: Y mapped to C-up did nothing in Smash Bros,
    with no error anywhere and a C-stick that behaved.

    The captured button is the deliberate instruction, so it wins. Dropping the
    opposing axis makes both halves AXIS_NONE, `res` is then always 0, and the
    button is read every time. It costs the stick's other direction, which is
    the part of this that is not padmap's to fix: RetroArch has no way to
    express "this button, and also that axis" on one analog axis.
    """
    keys = {line.split(" = ", 1)[0] for line in lines if " = " in line}
    doomed = set()
    for stem in ANALOG_STEMS:
        for half, other in (("minus", "plus"), ("plus", "minus")):
            if f"{stem}_{half}_btn" in keys and f"{stem}_{other}_axis" in keys:
                doomed.add(f"{stem}_{other}_axis")
    if not doomed:
        return lines
    return [
        line for line in lines
        if line.split(" = ", 1)[0] not in doomed
    ]


def retroarch_lines(
    bindings: dict[str, Binding], overrides: dict[str, str] | None = None
) -> list[str]:
    """Autoconfig entries for the controls that were captured.

    Only what the user actually pressed: a key bound to nothing is worse than
    an absent one, because RetroArch will happily bind a button that does not
    exist and report the pad as configured.

    Which is also why a binding RetroArch cannot name is dropped here instead
    of guessed at. Two shapes reach this:

    * a button whose evdev code is below BTN_MISC -- a combo adapter's KEY_A,
      an arcade encoder's keyboard codes. RetroArch's udev driver never
      enumerates them, so writing SDL's number for one names a button that
      does not exist, or a real but different one. See RA_INVISIBLE.
    * a hat value that is not one direction bit, which used to raise KeyError
      out of the middle of writing a launch profile, leaving the game to start
      with no controller config at all.
    """
    overrides = overrides or {}
    lines = []
    for control in CANONICAL_ORDER:
        binding = bindings.get(control)
        # A console-specific key wins: cores wire the RetroPad onto real
        # buttons differently, and the canonical name only describes where the
        # control sits on the pad.
        key = overrides.get(control) or RETROARCH_KEYS.get(control)
        if binding is None or key is None:
            continue
        if not binding.retroarch_visible():
            continue
        if binding.kind == "axis":
            # An axis has to go under the _axis key, not _btn.
            #
            # RetroArch parses a _btn value with strtoull, so "-0" and "+0"
            # both come out as button 0 -- the two directions of one stick
            # collapse onto the same button and pressing either activates
            # both. It is a silent misparse: nothing warns, and the pad looks
            # configured. Only the _axis parser understands the sign.
            key = key.replace("_btn", "_axis")
        lines.append(f'{key} = "{binding.retroarch()}"')
    return drop_shadowed_axis_halves(lines)
