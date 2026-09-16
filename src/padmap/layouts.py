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
from pathlib import Path


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
    # How to name this console inside a sentence, when `label` does not read
    # as one. "Arcade stick games" describes the controller rather than the
    # games; "Arcade games" is what the scope actually covers. Data rather
    # than a rule, because the exceptions are per-console and there is no
    # rule that produces them.
    console_label: str = ""
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
    console_label="Arcade",
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
        # The C-stick, captured rather than guessed.
        #
        # It has to be asked for, because the automatic path cannot find it on
        # this hardware. stick_fields assumes a right stick lives on ABS_RX and
        # ABS_RY; on the MAYFLASH adapter those are the analogue *triggers*,
        # resting at one end of their travel, and the C-stick is on ABS_Z and
        # ABS_RZ instead. stick_fields correctly refuses the triggers -- that
        # is what stopped the pad reading as jammed to the left -- but nothing
        # then claims the real C-stick, so it was simply absent. Dolphin reads
        # the GameCube C-stick from the RetroPad right analog stick, so these
        # are the right canonical names; capturing them is the only part that
        # was missing.
        Control("rightstick_up", "C-stick up", 0.50, 0.60, kind="stick",
                radius=0.032),
        Control("rightstick_down", "C-stick down", 0.50, 0.72, kind="stick",
                radius=0.032),
        Control("rightstick_left", "C-stick left", 0.46, 0.66, kind="stick",
                radius=0.032),
        Control("rightstick_right", "C-stick right", 0.54, 0.66, kind="stick",
                radius=0.032),
    ),
)

# The one console where "no overrides" needs no argument beyond the hardware
# itself: the abstract RetroPad was modelled on a DualShock, so the PS2 pad is
# the shape every other layout here is a deviation from.
#
# Verified against pcsx2/PAD/PAD.cpp, the table that maps each PS2 button to
# the RetroPad id it is read from:
#
#   RETRO_DEVICE_ID_JOYPAD_X,   // PAD_TRIANGLE    ..._L,  // PAD_L1
#   RETRO_DEVICE_ID_JOYPAD_A,   // PAD_CIRCLE      ..._R,  // PAD_R1
#   RETRO_DEVICE_ID_JOYPAD_B,   // PAD_CROSS       ..._L2, // PAD_L2
#   RETRO_DEVICE_ID_JOYPAD_Y,   // PAD_SQUARE      ..._R2, // PAD_R2
#
# Against the global table that is an exact match on all eight, because both
# describe the same pad: SDL's `a` is the bottom face button and RetroPad's B
# is the bottom face button, and the bottom face button is Cross.
#
# L3 and R3 are deliberately absent. The core reads them (PAD_L3/PAD_R3 above),
# but padmap has no canonical name for a stick click -- SDL_FIELDS stops at the
# triggers -- so there is nothing to bind them to yet. Adding two names to that
# table would be the whole change, and it is worth doing when a pad that has
# them is being mapped. Neither controller on this machine does: the GameCube
# adapter has no stick clicks at all.
PS2 = Layout(
    id="ps2",
    label="PlayStation 2",
    shapes=_PAD_BODY,
    controls=(
        Control("a", "Cross (bottom)", 0.72, 0.52),
        Control("b", "Circle (right)", 0.78, 0.42),
        Control("x", "Square (left)", 0.66, 0.42),
        Control("y", "Triangle (top)", 0.72, 0.32),
        Control("dpup", "D-pad up", 0.28, 0.36, kind="dpad", radius=0.045),
        Control("dpdown", "D-pad down", 0.28, 0.52, kind="dpad", radius=0.045),
        Control("dpleft", "D-pad left", 0.23, 0.44, kind="dpad", radius=0.045),
        Control("dpright", "D-pad right", 0.33, 0.44, kind="dpad", radius=0.045),
        Control("back", "Select", 0.45, 0.44, radius=0.035),
        Control("start", "Start", 0.55, 0.44, radius=0.035),
        Control("leftshoulder", "L1", 0.26, 0.20, kind="shoulder"),
        Control("rightshoulder", "R1", 0.74, 0.20, kind="shoulder"),
        Control("lefttrigger", "L2", 0.34, 0.12, kind="shoulder"),
        Control("righttrigger", "R2", 0.66, 0.12, kind="shoulder"),
    ),
)

# The Switch Pro pad. Nintendo puts its face buttons where nobody else does:
# A is the *right* button and B the *bottom* one, X the top and Y the left --
# the mirror image of an Xbox pad, which is why a Switch controller feels like
# it has A and B swapped everywhere else.
#
# So the labels and the canonical names deliberately disagree, and that is the
# entire reason this layout exists rather than reusing GENERIC. `canonical` is
# a *position* -- SDL's `a` is the bottom face button whatever is printed on
# it -- while `label` is the letter under the user's thumb. Getting this
# backwards is not cosmetic: the wizard would say "press A", the user presses
# the button marked A, and the binding lands on the bottom button, so confirm
# and cancel come out swapped in every game.
#
# No RetroArch overrides, and for the same reason as SNES: the abstract
# RetroPad already uses Nintendo positions, so the global table maps these
# straight through. The work here is telling the user which button to press,
# not rewiring where it goes.
#
# Home and Capture are left out -- they have no canonical name, and Home is
# claimed by the front-end anyway. The stick clicks are out for the reason
# given on PS2: padmap has no canonical name for one yet.
SWITCH = Layout(
    id="switch",
    label="Switch Pro",
    console_label="Switch",
    shapes=_PAD_BODY,
    controls=(
        Control("a", "B (bottom)", 0.72, 0.52),
        Control("b", "A (right)", 0.78, 0.42),
        Control("x", "Y (left)", 0.66, 0.42),
        Control("y", "X (top)", 0.72, 0.32),
        Control("dpup", "D-pad up", 0.28, 0.36, kind="dpad", radius=0.045),
        Control("dpdown", "D-pad down", 0.28, 0.52, kind="dpad", radius=0.045),
        Control("dpleft", "D-pad left", 0.23, 0.44, kind="dpad", radius=0.045),
        Control("dpright", "D-pad right", 0.33, 0.44, kind="dpad", radius=0.045),
        Control("back", "Minus", 0.45, 0.44, radius=0.035),
        Control("start", "Plus", 0.55, 0.44, radius=0.035),
        Control("leftshoulder", "L", 0.26, 0.20, kind="shoulder"),
        Control("rightshoulder", "R", 0.74, 0.20, kind="shoulder"),
        Control("lefttrigger", "ZL", 0.34, 0.12, kind="shoulder"),
        Control("righttrigger", "ZR", 0.66, 0.12, kind="shoulder"),
    ),
)

# The Wii U Pro Controller is the Switch Pro's control set under a different
# name: A right, B bottom, X top, Y left, L/R, ZL/ZR, Plus/Minus. Same
# positions, same Nintendo labels, same absence of overrides. It is its own
# layout rather than an alias so that "my pad, when playing Wii U games" is
# a scope a user can map to and Cemu's profile can name -- which it could
# not while the only Nintendo layout was called Switch.
WIIU = Layout(
    id="wiiu",
    label="Wii U Pro",
    console_label="Wii U",
    shapes=_PAD_BODY,
    controls=SWITCH.controls,
)

# Six face buttons in two rows, no shoulders, and a Mode button nobody uses.
#
# Verified against genesis-plus-gx, libretro/libretro.c, the DEVICE_PAD6B case:
#
#   JOYPAD_L      -> INPUT_X       JOYPAD_Y     -> INPUT_A
#   JOYPAD_X      -> INPUT_Y       JOYPAD_B     -> INPUT_B
#   JOYPAD_R      -> INPUT_Z       JOYPAD_A     -> INPUT_C
#   JOYPAD_SELECT -> INPUT_MODE    JOYPAD_START -> INPUT_START
#
# Note where X and Z come from: RetroPad **L and R**, the shoulders. A Genesis
# pad has no shoulders, so the core borrowed them for the top row -- and that
# is why this layout needs no `retroarch=` overrides despite looking like it
# should. Naming the top-left button `leftshoulder` and the top-right one
# `rightshoulder` makes the global table emit input_l_btn and input_r_btn,
# which is exactly what the core reads them from.
#
# The alternative -- canonical face names plus overrides -- was rejected: it
# would put two buttons on input_l_btn/input_r_btn by a different route and
# leave the layout lying about which control is which.
#
# A and C are the crossed pair, as everywhere: SDL's `x` is the left face
# button and Genesis A is the left of the bottom row, so they coincide.
GENESIS = Layout(
    id="genesis",
    label="Genesis",
    console_label="Genesis",
    shapes=_PAD_BODY,
    controls=(
        Control("x", "A (bottom left)", 0.62, 0.52),
        Control("a", "B (bottom middle)", 0.72, 0.52),
        Control("b", "C (bottom right)", 0.82, 0.52),
        Control("leftshoulder", "X (top left)", 0.62, 0.38),
        Control("y", "Y (top middle)", 0.72, 0.38),
        Control("rightshoulder", "Z (top right)", 0.82, 0.38),
        Control("dpup", "D-pad up", 0.28, 0.36, kind="dpad", radius=0.045),
        Control("dpdown", "D-pad down", 0.28, 0.52, kind="dpad", radius=0.045),
        Control("dpleft", "D-pad left", 0.23, 0.44, kind="dpad", radius=0.045),
        Control("dpright", "D-pad right", 0.33, 0.44, kind="dpad", radius=0.045),
        Control("start", "Start", 0.47, 0.46, radius=0.035),
        Control("back", "Mode", 0.47, 0.56, radius=0.035),
    ),
)

ALL: dict[str, Layout] = {
    layout.id: layout
    for layout in (GENERIC, SNES, N64, ARCADE, GAMECUBE, PS2, SWITCH, WIIU,
                   GENESIS)
}

DEFAULT = GENERIC.id

# The layouts that name an actual console, in catalogue order.
#
# `generic` is a layout but not a console: "my pad, when playing generic
# games" is not a thing anyone can mean, and offering it as a mapping scope
# would produce a scope that never resolves because no core ever reports it.
CONSOLES: list[str] = [
    layout_id for layout_id in ALL if layout_id != GENERIC.id
]


# libretro core name -> the layout whose key table that core reads.
#
# This is the launch-time half of the console-specific mapping: padmap-play is
# handed `-L <core.so>`, and the core is the only thing at that moment that
# says which console is about to run. Matched on the core's *file* name with
# the `_libretro` suffix stripped, which is stable across store paths and
# platforms.
#
# The four entries carrying a comment are the ones already verified against
# the core's own source while building the layouts above -- the same reading
# that established the RetroArch key overrides. The rest are near neighbours
# of those, taken from the core names libretro ships; a wrong entry here
# resolves a mapping the user did not intend rather than corrupting anything,
# and an *absent* one simply falls through to the universal mapping, which is
# the behaviour before any of this existed.
CORE_LAYOUTS: dict[str, str] = {
    # emulate_game_controller_via_libretro.c, inputGetKeys_default -- the
    # reading that produced N64's input_y_btn override.
    "mupen64plus_next": N64.id,
    "mupen64plus": N64.id,
    "parallel_n64": N64.id,
    # libretro.cpp, MAP_BUTTON(MAKE_BUTTON(PAD_1, BTN_A), "Joypad1 A") -- the
    # reading that established SNES needs no overrides at all.
    "snes9x": SNES.id,
    "snes9x2010": SNES.id,
    "snes9x2005": SNES.id,
    "snes9x2002": SNES.id,
    "bsnes": SNES.id,
    "bsnes_mercury_accuracy": SNES.id,
    "bsnes_mercury_balanced": SNES.id,
    "bsnes_mercury_performance": SNES.id,
    "mesen_s": SNES.id,
    # src/osd/retro/retromain.c, the P1_state block -- the reading that
    # produced the arcade layout's five overrides.
    "mame2010": ARCADE.id,
    "mame2003": ARCADE.id,
    "mame2003_plus": ARCADE.id,
    "mame2000": ARCADE.id,
    "mame": ARCADE.id,
    "fbalpha": ARCADE.id,
    "fbalpha2012": ARCADE.id,
    "fbneo": ARCADE.id,
    # Source/Core/DolphinLibretro/Input.cpp,
    # retro_set_controller_port_device_gc -- the reading that produced the
    # GameCube layout's identity face-button mapping.
    "dolphin": GAMECUBE.id,
    # pcsx2/PAD/PAD.cpp, the PAD_* -> RETRO_DEVICE_ID_JOYPAD_* table -- the
    # reading that established PS2 needs no overrides at all.
    "pcsx2": PS2.id,
    # Play! is the other PS2 core libretro ships. Not read, and listed on the
    # same terms as the other near neighbours above: a wrong entry resolves a
    # mapping the user did not intend, an absent one falls through.
    "play": PS2.id,
    # libretro/libretro.c, the DEVICE_PAD6B case -- the reading that showed the
    # Genesis top row comes from RetroPad L/R, and so needs no overrides.
    "genesis_plus_gx": GENESIS.id,
    "picodrive": GENESIS.id,
    "blastem": GENESIS.id,
}


def for_core(core: str) -> str:
    """The console a libretro core plays, as a layout id, or "".

    Empty rather than `generic` for a core nothing is known about. The two
    are not the same answer: `generic` is a layout somebody could deliberately
    map to, while "" means "no console context", which resolution has to treat
    as "skip the console scope" rather than "look for a mapping filed under
    the generic pad".
    """
    if not core:
        return ""
    name = Path(core).name
    # Strip the platform's library suffix, however it is spelled, then the
    # libretro marker: `mupen64plus_next_libretro.so` -> `mupen64plus_next`.
    for suffix in (".so", ".dll", ".dylib"):
        if name.endswith(suffix):
            name = name[: -len(suffix)]
            break
    for marker in ("_libretro", "-libretro"):
        if name.endswith(marker):
            name = name[: -len(marker)]
            break
    return CORE_LAYOUTS.get(name.lower(), "")


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
