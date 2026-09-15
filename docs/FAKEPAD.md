# Virtual controllers, for testing without the hardware

padmap's bugs are mostly not in padmap. They are in the gap between what a
controller actually reports and what the code assumed it would — a trigger
resting at 81% of its range, a d-pad that is four buttons here and a hat
there, a stick counting upwards where evdev counts down, a controller with no
evdev node at all. Every one of those was found by plugging something in and
being surprised.

`src/padmap/fakepad.py` is those surprises, written down and executable.

    from padmap import fakepad

    pad = fakepad.get("xbox360")
    pad.press("a")            # the frame a real 360 would send
    ui = pad.spawn()          # a real device node padmap will discover

## The interface

`Controller` declares what a controller *is*; two transports say how padmap
reads it.

| | | |
|---|---|---|
| `EvdevController` | `spawn()` → a real uinput device | Xbox360, XboxSeriesX, MayflashGameCube |
| `HidrawController` | `report()` → raw HID bytes | SteamControllerPuck, SwitchPro |

Both speak one vocabulary — `a`, `b`, `x`, `y`, `l`, `r`, `l2`, `r2`, `l3`,
`r3`, `start`, `select`, `home`, `up`/`down`/`left`/`right`, `lx`/`ly`/`rx`/`ry`,
`lt`/`rt` — so one test body runs against every controller. That is the whole
point of the interface: these pads differ in every detail *except* what the
user pressed.

## Nothing here is from memory

Each class carries a `source` naming where its numbers came from, and the test
suite asserts that it does. A fixture nobody can check is a guess with a class
around it.

| controller | source |
|---|---|
| Xbox360, XboxSeriesX | `drivers/input/joystick/xpad.c` — device table, `xpad_common_btn`, `xpad_set_up_abs` |
| MayflashGameCube | `FINDINGS.md`, measured on this machine; MSC_SCAN per `hid-input.c:1787` |
| SteamControllerPuck | SDL `SDL_hidapi_steam_triton.c` + `steam/controller_structs.h` |
| SwitchPro | padmap's own `hidraw.py` tables, verified against hardware with `tools/switchprobe.py` |

## Why these five

Not five variations on an Xbox pad. Each is in here because it breaks a
different assumption:

- **Xbox360** — the unremarkable one. Sticks centred at zero, triggers from
  zero, a hat. Anything that works *only* here is assuming this shape.
- **XboxSeriesX** — same driver, triggers `0..1023` instead of `0..255`.
  Catches anything that hard-codes 255 because the 360 does.
- **MayflashGameCube** — the pathological one. Its triggers are on `ABS_RX`
  and `ABS_RY`, not the `ABS_Z`/`ABS_RZ` the names suggest, and they *rest at
  24 and 25 of 0-255* — 81% deflected, untouched. That broke padmap three ways
  at once. It is also hid-generic driven, so it emits `MSC_SCAN` before every
  key, which xpad does not.
- **SteamControllerPuck** — has no evdev node at all. There is nothing to
  spawn, so it emits vendor reports and `triton.py` decodes them.
- **SwitchPro** — 12-bit sticks packed three-bytes-per-pair, a d-pad as four
  bits in a button byte, and `A` is the *right* face button because Nintendo's
  labels are mirrored.

## The part that earns its keep

The fixtures feed padmap's **real** decoders and the test asserts the events
match:

```python
pad = fakepad.get("steam-controller")
src = triton.Source(...)
want = pad.press("a")                       # [KEY:BTN_SOUTH=1]
got  = src._decode_state(pad.report()[1:])  # padmap's actual decode
```

The fixture declares its expectations from the hardware's documentation; the
decoder is independent code. Agreement is evidence, not tautology — which is
why `HidrawController` stores axis values in **hardware units** and declares
its own inversion rule in `to_evdev`. A fixture that pre-inverted Y would let
a decoder that forgot to invert pass.

This is how the Steam Controller decode gets tested at all. It is a wireless
receiver: an unpaired slot reads nothing, forever, and that is
indistinguishable from a decoder that is wrong.

## Running it

    XDG_RUNTIME_DIR=$(mktemp -d) nix develop --command \
        python3 tests/check_fakepad.py
    nix develop --command python3 tests/check_capture_via_fakepad.py

The two `spawn()` tests skip themselves when `/dev/uinput` is not writable.
Everything else runs anywhere.

`check_capture_via_fakepad.py` is the one that pays for the whole framework.
`capture.py` exists largely because of that GameCube adapter, and until now it
was only ever tested against a trigger resting at zero — the shape of the bug,
not the numbers. It now drives the wizard with the real ones, including the
re-arm that used to leave a trigger dead after a single press.

## Adding one

1. Find the driver that publishes it and read the ids, the button list and
   `input_set_abs_params` out of it. Put the citation in `source`.
2. Subclass `EvdevController` or `HidrawController`.
3. Add it to `CONTROLLERS`.

The cross-controller tests pick it up automatically — they iterate
`fakepad.every()` — so a new fixture is immediately asserted to have coherent
codes, usable ranges, rest values inside those ranges, a d-pad that cancels
opposites, and a citation.
