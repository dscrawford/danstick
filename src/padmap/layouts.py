"""Controller layouts: what to draw, where to point, and what it means.

The mapping wizard shows a picture of the controller with an arrow at the
control being mapped. Rather than shipping artwork per controller and a
separate table of where each button sits on it, the positions *are* the
artwork: the front-end draws the body from `shapes` and the controls from
their own coordinates, so a new layout is a data entry rather than an asset
hunt plus a matching set of coordinates that can drift out of step with it.

Coordinates are normalised 0..1 on a 2:1 canvas, so the front-end can size the
picture however it likes.

Three things a layout has to carry, and the third is the one that makes this
more than a drawing:

  * where each control is, so the arrow can point at it
  * what to call it to the user -- "Z", "C-up", "Select" are the words on the
    hardware, not the words SDL uses
  * which canonical control it *is*, because that is what RetroArch and SDL
    are told. An N64 pad has no X or Y and four C-buttons that behave as a
    right stick; a SNES pad has no analogue anything. Layouts differ in which
    controls exist, not merely where they sit.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True)
class Shape:
    """Part of the controller body, in normalised coordinates."""

    kind: str          # "rect" | "circle" | "polygon"
    points: tuple[float, ...]
    radius: float = 0.0


@dataclass(frozen=True)
class Control:
    """One thing the user will be asked to press."""

    # Canonical name: what SDL and RetroArch are told. Several controls can
    # share one -- a SNES 'Y' and an Xbox 'X' are the same canonical button --
    # which is the entire reason this is separate from `label`.
    canonical: str
    # What the user sees, in the hardware's own words.
    label: str
    x: float
    y: float
    kind: str = "button"   # "button" | "shoulder" | "dpad" | "stick"
    radius: float = 0.045
    # RetroArch autoconfig key, when this console does not use the one the
    # canonical name implies.
    #
    # Cores map the abstract RetroPad onto real console buttons themselves,
    # and not identically. mupen64plus-next reads N64 B from RetroPad **Y**,
    # not RetroPad A -- so binding the physical B to the key the canonical
    # name suggests produces a button that does nothing at all. The console's
    # own wiring belongs with the console's layout.
    retroarch: str = ""


@dataclass(frozen=True)
class Layout:
    id: str
    label: str
    controls: tuple[Control, ...]
    shapes: tuple[Shape, ...] = field(default_factory=tuple)
    # Optional artwork, as a filename the front-end resolves against its own
    # directory. Drawn behind the control dots in place of `shapes`.
    #
    # `shapes` is still required when an image is given, and is what gets
    # drawn if the file is missing. An image and a set of coordinates are two
    # things that can drift apart, and the drift is invisible -- an arrow
    # pointing at the wrong part of a photograph looks exactly like one
    # pointing at the right part -- so there is always something to fall back
    # to that cannot go out of step.
    image: str = ""

    def retroarch_keys(self) -> dict[str, str]:
        """Canonical control -> RetroArch key, for controls that override it."""
        return {
            c.canonical: c.retroarch for c in self.controls if c.retroarch
        }

    def order(self) -> list[str]:
        return [control.canonical for control in self.controls]

    def to_json(self) -> dict:
        return {
            "id": self.id,
            "label": self.label,
            "image": self.image,
            "shapes": [
                {"kind": s.kind, "points": list(s.points), "radius": s.radius}
                for s in self.shapes
            ],
            "controls": [
                {
                    "canonical": c.canonical, "label": c.label,
                    "x": c.x, "y": c.y, "kind": c.kind, "radius": c.radius,
                    "retroarch": c.retroarch,
                }
                for c in self.controls
            ],
        }


# Radii below are fractions of the canvas *height*; x and y are fractions of
# width and height respectively. The canvas is 2:1.

# A plain two-handled pad: rounded body, two grips.
_PAD_BODY = (
    Shape("rect", (0.18, 0.30, 0.64, 0.34), radius=0.16),
    Shape("circle", (0.28, 0.70), radius=0.13),
    Shape("circle", (0.72, 0.70), radius=0.13),
)

GENERIC = Layout(
    id="generic",
    label="Gamepad",
    shapes=_PAD_BODY,
    controls=(
        Control("a", "A (bottom face)", 0.72, 0.52),
        Control("b", "B (right face)", 0.78, 0.42),
        Control("x", "X (left face)", 0.66, 0.42),
        Control("y", "Y (top face)", 0.72, 0.32),
        Control("dpup", "D-pad up", 0.28, 0.36, kind="dpad", radius=0.045),
        Control("dpdown", "D-pad down", 0.28, 0.52, kind="dpad", radius=0.045),
        Control("dpleft", "D-pad left", 0.23, 0.44, kind="dpad", radius=0.045),
        Control("dpright", "D-pad right", 0.33, 0.44, kind="dpad", radius=0.045),
        Control("back", "Select", 0.45, 0.44, radius=0.035),
        Control("start", "Start", 0.55, 0.44, radius=0.035),
        Control("leftshoulder", "Left shoulder", 0.26, 0.20, kind="shoulder"),
        Control("rightshoulder", "Right shoulder", 0.74, 0.20, kind="shoulder"),
        Control("lefttrigger", "Left trigger", 0.34, 0.12, kind="shoulder"),
        Control("righttrigger", "Right trigger", 0.66, 0.12, kind="shoulder"),
    ),
)

# No analogue anything, no second shoulder row, and the face buttons sit in a
# diamond rotated relative to an Xbox pad -- SNES 'Y' is where an Xbox 'X' is,
# which a picture settles and a list of button names does not.
#
# No overrides, and this is the one console where that needs no argument: the
# abstract RetroPad *is* a SNES pad. snes9x's libretro.cpp maps every
# RETRO_DEVICE_ID_JOYPAD_* straight onto the SNES button of the same name
# (MAP_BUTTON(MAKE_BUTTON(PAD_1, BTN_A), "Joypad1 A") and so on), so the
# global table is already right end to end.
SNES = Layout(
    id="snes",
    label="SNES",
    shapes=(Shape("rect", (0.16, 0.32, 0.68, 0.32), radius=0.15),),
    controls=(
        Control("a", "B (bottom)", 0.72, 0.56),
        Control("b", "A (right)", 0.78, 0.46),
        Control("x", "Y (left)", 0.66, 0.46),
        Control("y", "X (top)", 0.72, 0.36),
        Control("dpup", "D-pad up", 0.28, 0.38, kind="dpad", radius=0.045),
        Control("dpdown", "D-pad down", 0.28, 0.54, kind="dpad", radius=0.045),
        Control("dpleft", "D-pad left", 0.23, 0.46, kind="dpad", radius=0.045),
        Control("dpright", "D-pad right", 0.33, 0.46, kind="dpad", radius=0.045),
        Control("back", "Select", 0.45, 0.52, radius=0.035),
        Control("start", "Start", 0.55, 0.52, radius=0.035),
        Control("leftshoulder", "L", 0.24, 0.22, kind="shoulder"),
        Control("rightshoulder", "R", 0.76, 0.22, kind="shoulder"),
    ),
)

# The trident: a top bar with three prongs. Z sits underneath the centre one,
# and the four C-buttons behave as a right stick everywhere downstream --
# which is why they carry right-stick canonical names.
#
# Verified against mupen64plus-next,
# custom/mupen64plus-core/plugin/emulate_game_controller_via_libretro.c,
# inputGetKeys_default, in the default (non-alternate_mapping) branch:
#
#   A       <- RetroPad B        L/R    <- RetroPad L/R
#   B       <- RetroPad Y        Z      <- RetroPad L2
#   C-*     <- the *right analog stick*, not any button
#
# Only B needs an override. Two things look as though they should too, and
# do not:
#
#   * The C-buttons genuinely are the right stick. The core reads
#     RETRO_DEVICE_ANALOG/INDEX_ANALOG_RIGHT and thresholds it at
#     CSTICK_DEADZONE, unconditionally and with no button fallback, so the
#     right-stick canonical names already emit the right keys. (Its
#     CSTICK_LEFT/CSTICK_RIGHT macros are swapped with respect to the
#     BUTTONS bitfield in m64p_plugin.h *and* applied to a negated sign, so
#     the two errors cancel and left really is left.)
#   * Nothing must ever bind RetroPad R2 for this layout. In the default
#     branch R2 is `cbuttons_mode`: while it is held, A and B stop being A
#     and B and become C-buttons instead. There is deliberately no
#     righttrigger control below.
N64 = Layout(
    id="n64",
    label="Nintendo 64",
    shapes=(
        # Bar first, prongs overlapping into it so they read as one body
        # rather than four separate pieces floating near each other.
        Shape("rect", (0.18, 0.26, 0.64, 0.20), radius=0.05),
        Shape("rect", (0.22, 0.42, 0.12, 0.34), radius=0.05),
        Shape("rect", (0.44, 0.42, 0.12, 0.42), radius=0.05),
        Shape("rect", (0.66, 0.42, 0.12, 0.34), radius=0.05),
    ),
    controls=(
        Control("a", "A", 0.735, 0.60),
        # Keys->B_BUTTON comes from RETRO_DEVICE_ID_JOYPAD_Y. RetroPad A is
        # unread in the default mapping, so the canonical choice
        # (input_a_btn) would leave this button dead.
        Control("b", "B", 0.685, 0.50, retroarch="input_y_btn"),
        Control("start", "Start", 0.50, 0.36, radius=0.035),
        Control("dpup", "D-pad up", 0.28, 0.52, kind="dpad", radius=0.038),
        Control("dpdown", "D-pad down", 0.28, 0.66, kind="dpad", radius=0.038),
        Control("dpleft", "D-pad left", 0.245, 0.59, kind="dpad", radius=0.038),
        Control("dpright", "D-pad right", 0.315, 0.59, kind="dpad", radius=0.038),
        Control("leftshoulder", "L (top left)", 0.235, 0.22, kind="shoulder",
                radius=0.038),
        Control("rightshoulder", "R (top right)", 0.765, 0.22, kind="shoulder",
                radius=0.038),
        Control("lefttrigger", "Z (underneath)", 0.50, 0.90, kind="shoulder",
                radius=0.04),
        Control("rightstick_up", "C-up", 0.745, 0.29, kind="stick", radius=0.032),
        Control("rightstick_down", "C-down", 0.745, 0.41, kind="stick", radius=0.032),
        Control("rightstick_left", "C-left", 0.706, 0.35, kind="stick", radius=0.032),
        Control("rightstick_right", "C-right", 0.784, 0.35, kind="stick", radius=0.032),
    ),
)

# A stick and six buttons in two rows. No shoulders, no analogue, and the
# button names are positional because arcade sticks do not agree on letters.
#
# Every face button is overridden, because mame2010 numbers MAME's buttons
# off the RetroPad in plain order rather than in RetroArch's Nintendo-crossed
# one. From src/osd/retro/retromain.c (the P1_state block and the input
# descriptors beside it):
#
#   Button 1 <- A     Button 3 <- X     Button 5 <- L     Coin  <- Select
#   Button 2 <- B     Button 4 <- Y     Button 6 <- R     Start <- Start
#
# So the global table -- built for a gamepad, where a/b and x/y cross -- puts
# the six panel buttons on MAME buttons 4, 3, 5, 2, 1, 6 reading across the
# panel: not merely shifted but scrambled, so a fighting game gets a punch
# where it expects a kick.
#
# The rows below are numbered 1-3 across the top and 4-6 across the bottom.
# That part is the cabinet convention (and MAME's own keyboard defaults),
# not something retromain.c can settle -- the core only decides which
# RetroPad control carries which MAME button *number*.
ARCADE = Layout(
    id="arcade",
    label="Arcade stick",
    shapes=(Shape("rect", (0.08, 0.24, 0.84, 0.50), radius=0.05),),
    controls=(
        Control("x", "Top-left button", 0.50, 0.40, retroarch="input_a_btn"),
        Control("y", "Top-middle button", 0.61, 0.38, retroarch="input_b_btn"),
        Control("leftshoulder", "Top-right button", 0.72, 0.40,
                retroarch="input_x_btn"),
        Control("a", "Bottom-left button", 0.50, 0.58, retroarch="input_y_btn"),
        Control("b", "Bottom-middle button", 0.61, 0.56,
                retroarch="input_l_btn"),
        # Bottom-right is MAME button 6, which is RetroPad R -- the one
        # position where the global table already agrees.
        Control("rightshoulder", "Bottom-right button", 0.72, 0.58),
        Control("dpup", "Stick up", 0.27, 0.38, kind="dpad", radius=0.045),
        Control("dpdown", "Stick down", 0.27, 0.60, kind="dpad", radius=0.045),
        Control("dpleft", "Stick left", 0.215, 0.49, kind="dpad", radius=0.045),
        Control("dpright", "Stick right", 0.325, 0.49, kind="dpad", radius=0.045),
        Control("back", "Coin / Select", 0.44, 0.20, radius=0.035),
        Control("start", "Start", 0.56, 0.20, radius=0.035),
    ),
)

# Verified against the dolphin core, Source/Core/DolphinLibretro/Input.cpp,
# retro_set_controller_port_device_gc. It clears the pad config and then sets
# every binding as a literal expression, so these are the bindings, not
# defaults something else can have overwritten:
#
#   gcButtons 0..5 are A, B, X, Y, Z, START (the order GCPadEmu.cpp adds
#   them in), bound to "A", "B", "X", "Y", "R", "Start"
#   gcTriggers 0..1 are L and R, bound to the L2/R2 analogue axis *or* the
#   L2/R2 button
#   gcDPad and the two sticks are the obvious things
#
# Which makes every button here an override, in two different ways:
#
#   * The face buttons are an identity mapping, GC A <- RetroPad A and so on.
#     That is exactly what the global table does *not* do: it crosses a/b and
#     x/y for a gamepad, so without these four the GameCube pad's A and B
#     would be swapped and its X and Y with them.
#   * L and R are the analogue triggers, and Z is a shoulder button. The
#     names line up with a gamepad's the wrong way round, so all three move.
#     Left alone, GC R would press Z and GC L would press the Triforce test
#     switch -- RetroPad L is not a GameCube button at all.
GAMECUBE = Layout(
    id="gamecube",
    label="GameCube",
    shapes=_PAD_BODY,
    controls=(
        Control("a", "A (large centre)", 0.72, 0.46, radius=0.07,
                retroarch="input_a_btn"),
        Control("b", "B (small, lower left)", 0.645, 0.57,
                retroarch="input_b_btn"),
        Control("x", "X (right)", 0.80, 0.42, retroarch="input_x_btn"),
        Control("y", "Y (top)", 0.71, 0.31, retroarch="input_y_btn"),
        Control("start", "Start", 0.50, 0.44, radius=0.035),
        Control("dpup", "D-pad up", 0.30, 0.60, kind="dpad", radius=0.04),
        Control("dpdown", "D-pad down", 0.30, 0.74, kind="dpad", radius=0.04),
        Control("dpleft", "D-pad left", 0.255, 0.67, kind="dpad", radius=0.04),
        Control("dpright", "D-pad right", 0.345, 0.67, kind="dpad", radius=0.04),
        Control("leftshoulder", "L", 0.26, 0.20, kind="shoulder",
                retroarch="input_l2_btn"),
        Control("rightshoulder", "R", 0.74, 0.20, kind="shoulder",
                retroarch="input_r2_btn"),
        Control("righttrigger", "Z", 0.80, 0.24, kind="shoulder", radius=0.04,
                retroarch="input_r_btn"),
    ),
)

ALL: dict[str, Layout] = {
    layout.id: layout
    for layout in (GENERIC, SNES, N64, ARCADE, GAMECUBE)
}

DEFAULT = GENERIC.id


def catalogue() -> list[dict]:
    """Every layout a user may pick from, in a stable order.

    Sent to the front-end rather than duplicated there. A hardcoded list in
    the theme would be a second copy of this table with nothing to notice when
    it fell behind -- a console added here would simply never appear, which
    looks exactly like the picker being broken.

    Whole layouts, not just names: the picker draws the controller it is
    offering, using the same coordinates the wizard will point its arrow at,
    so the picture cannot promise a pad the wizard does not then ask about.
    """
    return [layout.to_json() for layout in ALL.values()]


def index_of(layout_id: str) -> int:
    """Where a layout sits in the catalogue, or 0 for one that is not in it."""
    ids = list(ALL)
    return ids.index(layout_id) if layout_id in ids else 0


def get(layout_id: str) -> Layout:
    """A layout by id, falling back to the generic pad.

    Never raises: an unknown id comes from stored state or a front-end, and
    refusing to show a wizard at all is a worse answer than showing the
    ordinary one.
    """
    return ALL.get(layout_id, GENERIC)


def for_icon(icon: str) -> Layout:
    """The layout matching a controller icon the user already chose.

    The icon set and the layout set overlap by design -- someone who has said
    "this is an N64 controller" should not be asked again in different words.
    """
    return get(icon)
