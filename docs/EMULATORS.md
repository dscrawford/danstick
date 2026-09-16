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

**Ryujinx cannot tell padmap's pads apart.** It builds its device id from the
SDL GUID and then blanks the name CRC — its own comment says "Remove the first
4 char of the guid (CRC part) to make it stable". That CRC is the only thing
distinguishing padmap's pads:

```
0600c9a7091200000100000001000000  ->  0-00000006-1209-0000-0100-000001000000
060089a6091200000100000001000000  ->  0-00000006-1209-0000-0100-000001000000
06004866091200000100000001000000  ->  0-00000006-1209-0000-0100-000001000000
060009a4091200000100000001000000  ->  0-00000006-1209-0000-0100-000001000000
```

Four GUIDs, one id. What separates them is a `n-` prefix that is SDL
*connection order*, so the binding is only as stable as the order padmap
creates its clones in. `ryujinx::device_id` takes that ordinal as an argument
rather than pretending the id is a property of the device.

Varying the product id per player would fix it, and is deliberately not done:
in mirror mode the product is the source controller's, which is what makes
SDL's own database match the pad, and changing it would move every stored
mapping to a GUID nothing looks up.

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

## What these files are checked against

Not documentation. For Cemu, a `controller0.xml` Cemu itself wrote on the
development machine — all twenty-four mapping pairs must match it. For ares, a
`settings.bml` ares itself wrote, trimmed to the `VirtualPad` ports; the control
names come from there because ares keeps its own unbound entry beside any name
it does not recognise, so a misspelling leaves the control dead and silent.

## Not wired up yet

The emitters and the file writers exist and are tested. What does not exist yet
is the daemon calling them when assignments change, the way it already calls
`write_sdl_database` and `install_profiles`, and setting
`SDL_GAMECONTROLLERCONFIG` in the launch environment. Until that lands these are
libraries with no caller.
