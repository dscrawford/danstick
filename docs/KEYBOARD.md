# Keyboard and mouse: what each emulator binds by default, and what danstick does to it

*Research report, 2026-09-21. Sources: 54, read from upstream source where it
exists. Confidence: high for all five emulators; a few file-format details
are marked UNVERIFIED inline.*

## Executive summary

danstick binds gamepads and nothing else. The keyboard and mouse pass through
the `exec` sandbox untouched, and the assigner ignores keyboard key codes so
a combo device cannot take a seat by typing. Whether a keyboard-only person
can play therefore depends on each emulator's own defaults, and danstick's
config writers change those defaults in three different ways without meaning
to: RetroArch's keyboard survives beside pad 1, Ryujinx's and Dolphin's are
replaced the moment danstick seats player 1, and ares and Cemu have none to
begin with.

There is no cross-emulator standard layout, but RetroArch and Dolphin agree
on the core (arrows, Z/X/A/S faces, Q/W shoulders, Enter start) and Ryujinx
adds WASD/IJKL sticks. No launcher surveyed synthesises a keyboard gamepad;
those that support keyboard players write each emulator's own keys. The
cheapest thing danstick can do that is actually better than today: put the
keyboard on the **first free port** in every emulator, using that emulator's
own default table, so seating a pad never silently unbinds the keyboard.

## 1. What danstick does today

| emulator | keyboard default exists? | after danstick seats player 1 |
|---|---|---|
| RetroArch | yes, player 1 only | **kept**: danstick nulls only `_btn`/`_axis`; the suffix-less keyboard keys stay, so keyboard and pad both drive port 1 |
| Dolphin (GameCube) | yes, `[GCPad1]` is keyboard by design | **lost**: `[GCPad1..4]` are rewritten and unmanaged ports set to `SIDEVICE_NONE` |
| Ryujinx | yes, Player1 keyboard | **lost**: `merge` replaces the entry sharing `player_index`; a keyboard on another player index would survive |
| ares | none | nothing to lose; `VirtualPadN` written only for seated players |
| Cemu | none: with no profile there is no controller at all | nothing to lose; `controller0.xml` is written for player 1 |

Code: `retroarch.rs` (nul loop), `dolphin.rs` (`replace_pad_sections`, `SIDevice`), `ryujinx.rs` (`merge`), `ares.rs` (`virtual_pad`), `cemu.rs` (`profile_filename`). Keyboard handling elsewhere: `assign.rs` `BTN_FIRST`, `isolate.rs` (keyboard and mouse kept in the sandbox).

## 2. Defaults per emulator

### RetroArch

Compiled in (`config.def.keybinds.h`), player 1 only; players 2–16 are all
`RETROK_UNKNOWN`. Keyboard binds have **no suffix**; joypad binds use
`_btn`/`_axis`; `"nul"` disables any bind.

| RetroPad | key | cfg |
|---|---|---|
| B / A / Y / X | z / x / a / s | `input_player1_b = "z"` |
| L / R | q / w | |
| Start / Select | enter / rshift | |
| D-pad | up down left right | |
| L2 R2 L3 R3, sticks | unbound | |

Hotkeys are live bare keys because `input_enable_hotkey` is unbound: F1 menu,
Esc exit, F fullscreen, F2/F4 save/load, F8 screenshot, Space fast-forward,
P pause, R rewind. Batocera writes `input_enable_hotkey = "shift"`; RetroArch's
own answer for cores that read the raw keyboard is Game Focus (Scroll Lock).
Key names are the `input_config_key_map[]` table (`rshift`, `num1`,
`keypad1`, `kp_enter`, ...). Mouse: `input_playerN_mouse_index` (0-based) for
lightgun and pointer cores only.

Sources: [config.def.keybinds.h](https://github.com/libretro/RetroArch/blob/master/config.def.keybinds.h), [retroarch.cfg](https://github.com/libretro/RetroArch/blob/master/retroarch.cfg), [input_keymaps.c](https://github.com/libretro/RetroArch/blob/master/input/input_keymaps.c), [docs: input and controls](https://docs.libretro.com/guides/input-and-controls/), [issue #15658](https://github.com/libretro/RetroArch/issues/15658).

### Dolphin (GameCube pad)

`GCPad::LoadDefaults` binds the keyboard on purpose; its own comment reads
"Because our defaults use keyboard input, set calibration shapes to squares."

| GC | key |
|---|---|
| A / B / X / Y / Z | X / Z / C / S / D |
| L / R | Q / W |
| Start | Return |
| Main stick | arrows (Shift = modifier) |
| C-stick | I K J L (Ctrl = modifier) |
| D-pad | T G F H |

Config: `~/.config/dolphin-emu/GCPadNew.ini`, section `[GCPad1]`, keys as
`` Buttons/A = `X` ``, `` Main Stick/Up = `Up` `` (backticks around control
names). The default device is the highest-priority one, on X11
`XInput2/0/Virtual core pointer`; its key names are `XKeysymToString` output
with letters uppercased (`Return`, `Up`, `KP_1`), and `Ctrl`/`Shift`/`Alt`
are combined aliases Dolphin registers itself. With no ini, only **controller
0** gets the full defaults; ports 2–4 get the device line and no bindings.
`Dolphin.ini` `[Core]` defaults `SIDevice0 = 6`, `SIDevice1..3 = 0`.

Wii Remote 1 is emulated by default (`WiimoteNew.ini`, `Source = 1`) on
mouse and keyboard: A/B = left/right click, 1/2 = `1`/`2`, −/+ = Q/E, Home =
Return, IR = cursor, shake = middle click, Nunchuk stick = WASD, C/Z =
Control_L/Shift_L. Remotes 2–4 are `Source = 0`. danstick writes this file
too, moving that section to whichever seat holds the keyboard and mouse and
giving the other remotes their pads (`dolphin::wiimote_sections`).

Hotkeys (`Hotkeys.ini`): F1–F8 load state, Shift+F1–F8 save, F12 undo load,
F9 screenshot, F10 pause, Esc stop, Alt+Return fullscreen, Tab unlimit speed.

Sources: [GCPadEmu.cpp](https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Source/Core/Core/HW/GCPadEmu.cpp), [WiimoteEmu.cpp](https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Source/Core/Core/HW/WiimoteEmu/WiimoteEmu.cpp), [Nunchuk.cpp](https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Source/Core/Core/HW/WiimoteEmu/Extension/Nunchuk.cpp), [HotkeyManager.cpp](https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Source/Core/Core/HotkeyManager.cpp), [ControllerEmu.cpp](https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Source/Core/InputCommon/ControllerEmu/ControllerEmu.cpp), [CoreDevice.cpp](https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Source/Core/InputCommon/ControllerInterface/CoreDevice.cpp), [XInput2.cpp](https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Source/Core/InputCommon/ControllerInterface/Xlib/XInput2.cpp), [InputConfig.cpp](https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Source/Core/InputCommon/InputConfig.cpp), [MainSettings.cpp](https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Source/Core/Core/Config/MainSettings.cpp), [WiimoteSettings.cpp](https://raw.githubusercontent.com/dolphin-emu/dolphin/master/Source/Core/Core/Config/WiimoteSettings.cpp), [wiki: Input Syntax](https://wiki.dolphin-emu.org/index.php?title=Input_Syntax), [EmuDeck GCPadNew.ini](https://raw.githubusercontent.com/dragoonDorise/EmuDeck/main/configs/org.DolphinEmu.dolphin-emu/config/dolphin-emu/GCPadNew.ini).

### Ryujinx

`ConfigurationState.LoadDefault()` seeds one `StandardKeyboardInputConfig`
for Player1, backend `WindowKeyboard`, id `"0"`, controller type
**`JoyconPair`** (not ProController). Verified on two source mirrors.

| Switch | key | Switch | key |
|---|---|---|---|
| Left stick | W A S D, click F | Right stick | I J K L, click H |
| D-pad | arrows | A / B / X / Y | Z / X / C / V |
| L / ZL | E / Q | R / ZR | U / O |
| Minus / Plus | Minus / Plus | SL / SR | Unbound |

Hotkeys: F1 vsync, F2 mute, F4 UI, F5 pause, F8 screenshot. Key names are the
`Key` enum (`ShiftLeft`, `Number1`, `Keypad1`, `Enter`, `Space`, `Unbound`).
Full `input_config` JSON block: see source 4.

Sources: [ConfigurationState.cs](https://codeberg.org/smj2k/Ryujinx/raw/branch/master/src/Ryujinx.UI.Common/Configuration/ConfigurationState.cs), [Key.cs](https://codeberg.org/smj2k/Ryujinx/raw/branch/master/src/Ryujinx.Common/Configuration/Hid/Key.cs), [shipped Config.json](https://git.axenov.dev/Museum/ryujinx/raw/commit/117e32a6fffc30cdb895aa98483af7df353a8dd1/Ryujinx/Config.json), [Ryubing FAQ](https://docs.ryujinx.app/info/faq-&-troubleshooting/).

### ares

**No default bindings at all**, for pads or hotkeys: `input.cpp` and
`hotkeys.cpp` define the inputs, nothing assigns them. A launcher must write
everything. Keyboard assignments are `0x1/0/<index>`, where the index is the
key's position in the platform driver's table. Linux (xlib): Escape 0,
F1–F12 1–12, Num0–9 16–25, Backspace 28, A–Z 35–60, keypad after, then Enter,
Up Down Left Right, Tab, Return, Spacebar, modifiers. The index differs on
Windows and macOS. RetroBat writes exactly this for a keyboard fallback and
ports RetroArch's hotkey set onto ares (F fullscreen, F2/F4 states, Esc quit).

Sources: [input.cpp](https://github.com/ares-emulator/ares/blob/master/desktop-ui/input/input.cpp), [hotkeys.cpp](https://github.com/ares-emulator/ares/blob/master/desktop-ui/input/hotkeys.cpp), [settings.cpp](https://github.com/ares-emulator/ares/blob/master/desktop-ui/settings/settings.cpp), [hid.hpp](https://github.com/ares-emulator/ares/blob/master/nall/nall/hid.hpp), [xlib.cpp](https://github.com/ares-emulator/ares/blob/master/ruby/input/keyboard/xlib.cpp), [RetroBat Ares.Controllers.cs](https://github.com/RetroBat-Official/emulatorlauncher/blob/master/emulatorLauncher/Generators/Ares.Controllers.cs).

### Cemu

**No default keyboard mapping, and no default controller at all.**
`VPADController::set_default_mapping()` has SDL and XInput branches only, and
`InputManager::load` returns false when `controllerProfiles/controllerN.xml`
is missing. Choosing "Keyboard" in the UI gives an empty mapping the user
fills in by hand.

A keyboard profile is a `<controller>` with `<api>Keyboard</api>`,
`<uuid>keyboard</uuid>`, `<display_name>Keyboard</display_name>` and
`<mappings><entry><mapping>ID</mapping><button>CODE</button>`. `ID` is the
`VPADController::ButtonId` ordinal (A=1, B=2, X=3, Y=4, L=5, R=6, ZL=7, ZR=8,
Plus=9, Minus=10, Up..Right=11..14, StickL=15, StickR=16, StickL
Up/Down/Left/Right=17..20, StickR=21..24, Home=27). **`CODE` is platform
dependent**: the raw wx keycode, which is a GDK keysym on Linux (`x`=120,
`Return`=65293, `Up`=65362, `Escape`=65307) and a Win32 VK code on Windows
(`X`=88, `Return`=13). Keysyms follow the key's *output*, so a Linux writer
should use the unshifted lowercase keysym for letters (inference from the GDK
path, UNVERIFIED by test). A profile may hold several `<controller>` blocks
(keyboard and pad together), but each emulated button binds to one physical
control. Whether the `rumble`/`axis`/`rotation`/`trigger` nodes may be
omitted is UNVERIFIED; the agent's example keeps them.

Sources: [VPADController.cpp](https://raw.githubusercontent.com/cemu-project/Cemu/main/src/input/emulated/VPADController.cpp), [VPADController.h](https://raw.githubusercontent.com/cemu-project/Cemu/main/src/input/emulated/VPADController.h), [InputManager.cpp](https://raw.githubusercontent.com/cemu-project/Cemu/main/src/input/InputManager.cpp), [KeyboardController.cpp](https://raw.githubusercontent.com/cemu-project/Cemu/main/src/input/api/Keyboard/KeyboardController.cpp), [EmulatedController.h](https://raw.githubusercontent.com/cemu-project/Cemu/main/src/input/emulated/EmulatedController.h), [CemuApp.cpp](https://raw.githubusercontent.com/cemu-project/Cemu/main/src/gui/wxgui/CemuApp.cpp), [wxHelpers.cpp](https://raw.githubusercontent.com/cemu-project/Cemu/main/src/gui/wxgui/helpers/wxHelpers.cpp), [wxWindowSystem.cpp](https://raw.githubusercontent.com/cemu-project/Cemu/main/src/gui/wxgui/wxWindowSystem.cpp), [Cemu issue #459](https://github.com/cemu-project/Cemu/issues/459), [wxKeyEvent docs](https://docs.wxwidgets.org/3.2/classwx_key_event.html), [gdkkeysyms.h](https://gitlab.gnome.org/GNOME/gtk/-/raw/main/gdk/gdkkeysyms.h), [cemu.cfw.guide](https://cemu.cfw.guide/controller-configuration), [EmuDeck controller0.xml](https://raw.githubusercontent.com/dragoonDorise/EmuDeck/main/configs/info.cemu.Cemu/data/cemu/controllerProfiles/controller0.xml).

## 3. Where the defaults agree

| input | RetroArch | Dolphin GC | Ryujinx |
|---|---|---|---|
| primary direction | arrows (d-pad) | arrows (main stick) | arrows (d-pad), WASD (stick) |
| two main faces | x / z | X / Z | Z / X (Nintendo position) |
| other faces | s / a | C / S | C / V |
| shoulders | q / w | Q / W | E / U, triggers Q / O |
| start | enter | Return | Plus |
| select | rshift | – | Minus |
| screenshot | F8 | – | F8 |

Agreement: arrows, Z/X, Q/W, Enter. Disagreement: Ryujinx swaps Z/X (maps by
physical position, so Nintendo's A lands on Z) and uses WASD.

## 4. Prior art

- **Batocera**: keyboard is not a controller; configgen never writes keyboard
  binds for it ("you will have to rebind it within the emulator"), and its
  Dolphin and Cemu generators write pads only. The one bridge,
  `keyboardToPads.py`, turns keyboard-encoder arcade boards into virtual
  joysticks so configgen can treat them as pads.
- **EmuDeck**: Dolphin and Cemu configs bind the Steam Deck controller only;
  Dolphin's keyboard hotkeys are left at their defaults.
- **RetroPie**: writes RetroArch's defaults verbatim into `retroarch.cfg` and
  lets a keyboard player be configured as suffix-less keys.
- **RetroBat**: writes a keyboard fallback into every ares VirtualPad.
- **ES-DE, Pegasus, Lakka**: keyboard for navigation only.
- **Keyboard → uinput gamepad tools**: `xboxdrv --evdev`, ControllerEmulator,
  evsieve, input-remapper. SDL 3 has `SDL_AttachVirtualJoystick`, but it is
  in-process: the app must feed it.

Sources: [Batocera libretroControllers.py](https://github.com/batocera-linux/batocera.linux/blob/master/package/batocera/core/batocera-configgen/configgen/configgen/generators/libretro/libretroControllers.py), [Batocera supported_controllers](https://wiki.batocera.org/supported_controllers), [RetroPie retroarch.sh](https://github.com/RetroPie/RetroPie-Setup/blob/master/scriptmodules/emulators/retroarch.sh), [RetroPie keyboard controllers](https://raw.githubusercontent.com/RetroPie/RetroPie-Docs/master/docs/Keyboard-Controllers.md), [ES-DE FAQ](https://gitlab.com/es-de/emulationstation-de/-/raw/master/FAQ.md), [Pegasus controls](https://pegasus-frontend.org/docs/user-guide/controls/), [SDL_AttachVirtualJoystick](https://wiki.libsdl.org/SDL3/SDL_AttachVirtualJoystick), [ControllerEmulator](https://github.com/WebFreak001/ControllerEmulator), [evsieve](https://github.com/KarsMulder/evsieve), [input-remapper](https://github.com/sezanzeb/input-remapper), [xboxdrv man page](https://manpages.ubuntu.com/manpages/xenial/man1/xboxdrv.1.html).

## 5. Options for danstick

**A. Keyboard is the first free port, in each emulator's own keys.** Extend
each writer: after seating N pads, write that emulator's default keyboard
table to port N+1 (RetroArch: `input_player{N+1}_*` suffix-less keys; Dolphin:
`[GCPad{N+1}]` on `XInput2/0/Virtual core pointer` with `SIDevice{N} = 6`;
Ryujinx: move the keyboard entry to `Player{N+1}` instead of dropping it;
ares: `0x1/0/<xlib index>` on `VirtualPad{N+1}`; Cemu: a `Keyboard`
controller in `controller{N}.xml` with GDK keysyms). No new device, no
grabbing, no new state. Fixes the two silent regressions (Dolphin, Ryujinx)
and gives ares and Cemu a keyboard they never had. Cost: five small tables
and an ordinal rule; the ares and Cemu tables are Linux-specific, which
danstick is anyway.

**B. A keyboard seat.** A keyboard republished through uinput as `danstick
Player N`, one danstick-wide layout, taken by holding a key like any other seat.
Every writer already handles it. Cost: the keyboard must be grabbed and its
unbound keys re-emitted through a virtual keyboard (what evsieve does), or
every bound key leaks into the emulator as a hotkey. That is real work, and
it changes what "the keyboard" means for menus while a game runs. Only worth
it if GOTG wants a keyboard person seated by the same hold gesture.

**C. Leave it**, and document the table in section 1.

**Recommendation: A**, as its own request from GOTG if they want it. It is
the smallest change that makes the keyboard behave the same way in every
emulator danstick already writes, and B can be added on top later without
undoing it.

## What was built

Option A, on 2026-09-21: `danstick_core::keyboard::first_free` is the rule, and
each writer carries its table -- `retroarch::keyboard_config`,
`dolphin::keyboard_section`, `ryujinx::keyboard_entry` (inside `merge`),
`ares::keyboard_pad`, `cemu::keyboard_profile`. ares' key indices were
re-read from `xlib.cpp` and differ from RetroBat's: those are Windows numbers
(Space is 90 on Linux, not 92). Consumer-facing summary in `docs/EMULATORS.md`.

## Gaps

- Cemu: lowercase-keysym rule and optional nodes need one live test.
- Dolphin under Wayland: the default device is whatever sorts first; the
  X11 name above assumes XInput2.
- Ryubing fork hotkey additions (source hosts blocked or down).
- RetroDECK configs (mirror unreadable).

## Methodology

Three parallel research agents, about 30 queries, 54 unique sources read,
plus a read of danstick's five config writers. Sub-questions: RetroArch
defaults and cfg syntax; ares defaults and settings.bml; Dolphin and Cemu
defaults and file formats; Ryujinx defaults and Config.json; cross-emulator
conventions, uinput keyboard-gamepad tools, and launcher prior art. Emulator
defaults were read from upstream source (Dolphin `LoadDefaults`, Ryujinx
`LoadDefault`, RetroArch `config.def.keybinds.h`, Cemu
`set_default_mapping`, ares `input.cpp`) rather than from guides; Dolphin's
GameCube table was confirmed independently by two agents.
