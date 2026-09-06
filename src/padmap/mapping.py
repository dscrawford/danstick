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
    # means they agree, which is the case on most pads.
    ra_index: int | None = None

    def sdl(self) -> str:
        if self.kind == "button":
            return f"b{self.index}"
        if self.kind == "hat":
            return f"h{self.index}.{self.value}"
        if self.kind == "axis":
            sign = "+" if self.value >= 0 else "-"
            return f"{sign}a{self.index}"
        raise ValueError(f"unknown binding kind {self.kind!r}")

    def retroarch(self) -> str:
        """RetroArch's spelling of the same thing.

        Hats are `hN<direction>`, e.g. `h0up`; axes carry a sign; buttons are
        a bare number.
        """
        index = self.index if self.ra_index is None else self.ra_index
        if self.kind == "button":
            return str(index)
        if self.kind == "hat":
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
HAT_DIRECTIONS = {1: "up", 2: "right", 4: "down", 8: "left"}

# Where each consumer starts counting buttons. They are not the same, and the
# difference is invisible on most pads.
BTN_MISC = 0x100
BTN_JOYSTICK = 0x120


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
    """
    ordered = [c for c in sorted(keys) if c >= BTN_MISC]
    return ordered.index(code) if code in ordered else None


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


def stick_fields(axis_codes: list[int]) -> dict[str, str]:
    """SDL stick entries for the axes a pad actually reports."""
    fields = {}
    for code, field in STICK_AXES.items():
        if code in axis_codes:
            index = axis_index(axis_codes, code)
            if index is not None:
                fields[field] = f"a{index}"
    return fields


def sdl_line(guid: str, name: str, fields: dict[str, str],
             platform: str = "Linux") -> str:
    """One line for SDL's controller database, from plain field:target pairs.

    Separate from sdl_mapping because not every line padmap writes comes from
    a capture: one carried over from another database has fields padmap never
    asks about (`guide`, `leftstick`), and dropping them on the way through
    would quietly cost the user bindings they already had.

    Commas separate the fields and the name sits in one of them, so a name
    containing a comma would silently corrupt every field after it.
    """
    parts = [guid, name.replace(",", "")]
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
    """One line for SDL's controller database, from a capture."""
    fields = {}
    for control in CANONICAL_ORDER:
        binding = bindings.get(control)
        field = SDL_FIELDS.get(control)
        if binding is not None and field is not None:
            fields[field] = binding.sdl()
    fields.update(sticks or {})
    return sdl_line(guid, name, fields, platform)


def retroarch_lines(
    bindings: dict[str, Binding], overrides: dict[str, str] | None = None
) -> list[str]:
    """Autoconfig entries for the controls that were captured.

    Only what the user actually pressed: a key bound to nothing is worse than
    an absent one, because RetroArch will happily bind a button that does not
    exist and report the pad as configured.
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
    return lines
