# Emulators padmap configures

padmap publishes stable virtual gamepads — `padmap Player 1`..`N`, each with a
GUID that does not change. Anything that reads a controller will see them. What
this file is about is the extra step: writing each emulator's *own*
configuration so player N is already bound to `padmap Player N`, and the user
never opens its input settings.

| | config | binds by | needs padmap's SDL mapping? |
|---|---|---|---|
| RetroArch | autoconfig + `--appendconfig` | name and pad index | yes, via the autoconfig |
| Cemu | `controllerProfiles/controllerN.xml` | **SDL GUID** | **yes, and only via the environment** |
| Ryujinx | `Config.json` → `input_config` | a GUID with the name CRC blanked | yes |
| ares | `settings.bml` → `VirtualPadN` | SDL GUID, raw joystick indices | **no** |

## The three surprises

**ares does not use SDL's gamepad layer at all.** It reads raw joystick state —
`SDL_OpenJoystick`, `SDL_GetNumJoystickAxes|Hats|Buttons` — so padmap's mapping
line does nothing for it, and every number in a binding is a *position on the
device*. Those positions are the ordinal in ascending evdev code order, with
`ABS_HAT0X`/`Y` pulled out as a hat. Measured, not assumed: a uinput device
declaring eleven keys and six non-hat axes came back from SDL 3 as
`axes=6 hats=1 buttons=11`.

**Cemu reads no controller database.** It only lists devices SDL already
recognises as gamepads, and it never loads a `gamecontrollerdb.txt`. So the
profile alone is not enough — padmap's mapping has to reach it through
`SDL_GAMECONTROLLERCONFIG` in the environment Cemu is launched with. Writing
the profile and not the variable produces a profile naming a controller that
does not appear in the list.

**Ryujinx could not tell padmap's pads apart, so padmap changed.** It builds
its device id from the SDL GUID and blanks the name CRC — its own comment says
"Remove the first 4 char of the guid (CRC part) to make it stable". That CRC
was the only thing distinguishing padmap's pads, so four players collapsed to
one id and the binding fell back to SDL connection order.

padmap is the abstraction layer, so this is padmap's problem and not a caveat
to hand the user. Each clone now advertises **the player number as its GUID
version** — a field Ryujinx preserves and SDL's own matching ignores:

```
player 1  0-00000006-1209-0000-0100-000001000000
player 2  0-00000006-1209-0000-0100-000002000000
player 3  0-00000006-1209-0000-0100-000003000000
player 4  0-00000006-1209-0000-0100-000004000000
```

Measured, not assumed: two pads identical but for their version got distinct
GUIDs and *both* were still matched to "Xbox 360 Controller" out of SDL's
built-in database. So mirror mode keeps working — a controller nobody has
mapped still behaves as it did before padmap existed — and nothing else keys
on the field. RetroArch matches on name and vid/pid; a stored capture is filed
under the *physical* pad's signature.

This is the one field mirroring deliberately does not carry, and
`virtual.version_for` says so.

## Dolphin

The easiest of the targets, and worth saying why, because it looks like it
should be the hardest: **Dolphin's SDL backend names inputs by standard gamepad
element** -- `Button S`, `Left Y+`, `Pad N` -- and does the per-model lookup
itself. There is no capture to translate and no table per controller, which is
the opposite of ares.

Two things have to be right, and neither is the bindings:

* **The device line**, `SDL/<n>/<name>`, where `n` counts devices already
  sharing that *name*. Every padmap pad is named for its player, so `n` is
  always 0 -- simpler than counting devices that share a GUID, which is what
  binding physical pads requires. A binding naming a device Dolphin cannot see
  is silently inert.
* **The port's device type.** `SIDevice0..3` in `Dolphin.ini`, and a port with
  no controller declared in it is ignored however well its pad is bound. Note
  `SIDeviceN` is zero-based where `[GCPadN]` is one-based.

Unmanaged ports are set to `SIDEVICE_NONE` rather than left alone, for the
same reason padmap clears an unused RetroArch reservation: a port still
declared from a session with more players is a phantom controller in the next
game.

Both files are edited rather than rewritten -- `Dolphin.ini` key by key,
`GCPadNew.ini` keeping every section that is not a GameCube port. Unlike ares
and Ryujinx, padmap *will* create them: neither is the whole of Dolphin's
settings, and Dolphin reads them at startup whether or not it has run before,
so bindings are worth having on the first run too.

## The keyboard

padmap binds pads, and the keyboard is not one -- but a person with no pad
still has it, and every emulator above either binds it to port 1 by default
(RetroArch, Dolphin, Ryujinx) or not at all (ares, Cemu). Seating a pad on
port 1 used to take the keyboard's port with it, silently.

Now **the keyboard takes the first port no pad holds**, in every emulator, in
that emulator's own keys where it has them:

| | where it goes | which keys |
|---|---|---|
| RetroArch | `input_player{N}_*` suffix-less binds; player 1's are nulled when a pad sits there, and `input_all_users_control_menu` is set so the keyboard can still drive the menu | RetroArch's own: arrows, Z/X/A/S, Q/W, Enter, right Shift |
| Dolphin | `[GCPad{N}]` on `XInput2/0/Virtual core pointer`, `SIDevice{N-1} = 6` | Dolphin's own: X/Z/C/S/D, Q/W, arrows + IJKL sticks, TGFH d-pad |
| Ryujinx | the existing `WindowKeyboard` entry moved to `Player{N}`, or Ryujinx's default seeded there | the user's own if there is one, else Ryujinx's: WASD/IJKL, Z/X/C/V, E/U, Q/O |
| ares | `VirtualPad{N}` as `0x1/0/<key index>` | padmap's layout (below) |
| Cemu | `controller{N-1}.xml` with `<api>Keyboard</api>`, marked as padmap's | padmap's layout (below) |

padmap's layout, for the two that have none: arrows for direction -- the d-pad
and the left stick both, since which one is "the direction" depends on the
game -- Z/X/A/S for south/east/west/north, Q/W bumpers, E/R triggers, Enter
start, right Shift select, I/J/K/L right stick, B/N stick clicks.

Two things are Linux-specific and say so in the code: ares' key numbers are
positions in its xlib key table, and Cemu's are GDK keysyms. With every port
seated the keyboard drives nobody rather than doubling a pad, and a keyboard
block padmap left at another port last time is removed, so the keyboard is
never two players at once. A Cemu keyboard profile the user made themselves is
not touched. Dolphin's Wii Remote 1, which padmap does not write, stays on the
mouse and keyboard as Dolphin ships it.

A front-end can also seat the keyboard on purpose with `seat_keyboard`
(`docs/EVENTS.md`), which pins it to a seat of its own ahead of pads seated
later. The research behind the tables is `docs/KEYBOARD.md`.

## Motion

padmap serves every seated player's gyro and accelerometer over **DSU**, the
cemuhook UDP protocol, on `127.0.0.1:26760`. Slot N-1 is player N. Every config
padmap writes points at it:

| | how |
|---|---|
| Cemu | a second `<controller>` with `<api>DSUController</api>` and `<motion>true</motion>` on each profile |
| Ryujinx | `motion_backend: CemuHook`, `slot` = player − 1 |
| Dolphin | a `padmap` entry in `DSUClient.ini`; nothing is bound, because a GameCube pad has no gyro |

**Why not pass the sensor through the clone.** SDL pairs a joystick with its
motion sensor by comparing `EVIOCGUNIQ`, and uinput cannot set it. Every clone
reports an empty uniq, so with two players SDL hands out the gyros in
enumeration order. DSU addresses controllers by slot, which is exactly padmap's
player number.

**Where the samples come from.** A pad whose kernel driver publishes an IMU
node beside it (DualShock, DualSense, Switch Pro) is read through that node,
scaled by the resolution the driver declares. The Steam Controller has no such
node, so padmap decodes the IMU block from the reports it already reads.
Buttons and sticks ride along in each packet, taken from the calibrated stream
the clone sees.

**Player one is a Wii U GamePad in Cemu.** Cemu's Pro Controller has no motion
code, and a Wii U game that uses a gyro reads the GamePad. Cemu emulates one,
so players two and up stay Pro Controllers. The GamePad numbers its controls
differently from the d-pad on, and padmap writes the matching table.

**The frame.** DSU's accelerometer axes point left, down and away from the
player, and its gyro reports pitch up, yaw right and roll right. SDL's frame is
the mirror image. The conversion comes from Dolphin, which names both sets of
axes with the same English words in two neighbouring files.

**Turning it off.** `PADMAP_DSU_PORT=0`. A port already taken by another DSU
server is not fatal: padmap logs it, and the controllers work without motion.

## Why the button tables are constant

They look like they should be per-controller and they are not. For Cemu and
Ryujinx the table maps an emulated button to an **SDL gamepad id**, and padmap's
clone already reaches SDL's gamepad layer as a standard pad — so by the time
either sees it, A is SDL button 0 whatever the user physically pressed during
capture. Translating the capture again would translate it twice.

ares is the exception, because it is below that layer.

## Nintendo's labels are mirrored

Both Cemu and Ryujinx map by label, and Nintendo's A is where everyone else's B
is. So Wii U **A** is SDL *East* and Switch **A** is SDL's `"B"`. Writing the
obvious pairing swaps A and B in every game, which feels like the emulator's
fault rather than padmap's.

## The Wii U layout

The Wii U Pro Controller is the Switch Pro's control set under a different
name — A right, B bottom, X top, Y left, L/R, ZL/ZR, Plus/Minus — so the
`wiiu` layout is the `switch` layout's controls under its own id. It exists so
"my pad, when playing Wii U games" is a scope a user can map to, and so the
wizard says Wii U when that is what is being set up. Cemu's profile does not
depend on which of the two was used: its table maps by SDL position. Player
one is written as a Wii U GamePad rather than a Pro Controller; see Motion.

## What these files are checked against

Not documentation. For Cemu, a `controller0.xml` Cemu itself wrote on the
development machine — all twenty-four mapping pairs must match it. For ares, a
`settings.bml` ares itself wrote, trimmed to the `VirtualPad` ports; the control
names come from there because ares keeps its own unbound entry beside any name
it does not recognise, so a misspelling leaves the control dead and silent.

## When they are written

On every republish, beside the SDL database and the RetroArch autoconfig — the
same moment, from the same input. `padmap_input::emulators::publish` does the
writing; the Rust daemon calls it directly from `publish_artefacts`, and the
Python daemon, which is still the one that runs, calls it through
`padmap_daemon::publish`, which calls it directly.

The subprocess exists so there is one implementation of three file formats
rather than two. Ryujinx's device id and ares' raw joystick indices are exactly
the kind of thing that drifts when written twice, and only one of the two
copies would be the one a user's emulator reads. The cost is a process per
republish — a few an hour, off the forwarding path. When the daemon finishes
moving to Rust the subprocess goes with it.

Every write is best-effort and reported rather than propagated. ares and
Ryujinx each keep all of their settings in one file, so padmap refuses to
invent one for an emulator that has never run; on most machines at least one of
the three is absent, and that has to read as an ordinary skip rather than as
the mapping files having failed.

## Writing somewhere other than the user's home

`emit` takes a destination per target, and an absent flag keeps the default:

```sh
padmap emit \
  --cemu-dir      "$STATE/config/Cemu/controllerProfiles" \
  --dolphin-dir   "$STATE/config/dolphin-emu" \
  --ares-settings "$STATE/config/ares/settings.bml" \
  --ryujinx-config "$STATE/config/Ryujinx/Config.json" \
  --env-file      "$STATE/padmap-env.sh"   < pads.json
```

For a launcher that runs each game in an environment of its own. Two variants
of one game -- the plain launch and the 120fps one -- are two configurations
that must never see each other, and neither of them is the one in
`~/.config`. Overriding one destination leaves the others where they were.

The paths written are printed on stdout, one per line, so a caller can act on
what happened rather than assuming its flags landed. A flag given without a
value is refused rather than ignored: `--ryujinx-config` with the path
forgotten would otherwise mean "the user's own Ryujinx config", which is the
one file this exists to avoid.

## Reaching Cemu at all

Cemu reads no mapping database, so a pad SDL does not already recognise as a
gamepad never appears in its device list — the profile alone reaches nothing.
The mapping has to arrive in the environment, so padmap writes
`$XDG_RUNTIME_DIR/padmap/env.sh` on every republish:

```sh
padmap-rs exec -- Cemu       # or: . "$XDG_RUNTIME_DIR/padmap/env.sh"; Cemu
```

`exec` reads the file rather than recomputing the value — it has no pads open,
and opening them would take them from the daemon that does.

## Appending a port ares has never written

`rewrite_ares_settings` replaces the `VirtualPadN` blocks padmap manages and
appends the ones that are not in the file. Only replacing them looked right —
ares writes all five ports itself — but a settings.bml that has never had a pad
bound has none, and a binding that is simply absent is indistinguishable from
one that failed: ares starts, the pad is listed, nothing is bound.
