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

The emitters, the writers and the identity change are done and tested. What
does not exist yet is the daemon calling them when assignments change, the way
it already calls `write_sdl_database` and `install_profiles`, and putting
`SDL_GAMECONTROLLERCONFIG` into the environment a game is launched with.

Until that lands, `emit::sdl_config_value` and the three writers are libraries
with no caller.
