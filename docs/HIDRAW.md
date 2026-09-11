# padmap has to speak HID, not just evdev

## The limit we hit

padmap's model is: grab the physical pad's evdev node, republish it as a
uinput clone, point everything downstream at the clone. That works for the
adapters this project grew up on -- the MAYFLASH GameCube adapter, generic USB
pads -- because for those, evdev is the only interface anybody uses.

It does not work for the controllers SDL knows best. SDL ships device-specific
**HIDAPI drivers** for Switch, PlayStation and Xbox pads, and prefers them over
evdev. Those drivers talk to `/dev/hidraw*` directly and put the controller
into a vendor-specific report mode. Steam does the same thing with its own
userspace driver.

When that happens the kernel driver is starved. Observed here, on a Switch Pro
over Bluetooth:

    nintendo 0005:057E:2009.0075: timeout waiting for input report

The evdev node still exists. It can be opened. It can be grabbed with
EVIOCGRAB. It can be registered with a selector. It simply never becomes
readable, ever. padmap sat on it, correctly, for hours and forwarded nothing,
because there was nothing to forward.

Measured, on this machine, with nothing else holding the device:

| what was watched                             | Switch Pro events |
| -------------------------------------------- | ----------------- |
| all 7 input devices, nothing grabbed (twice)  | 0                 |
| `/dev/input/event26` raw, padmap stopped      | 0                 |
| the clone, `/dev/input/event31`               | 0                 |
| Pegasus via SDL HIDAPI on `/dev/hidraw8`      | works             |

The only path that has ever carried input from this pad is hidraw.

## Why "just turn HIDAPI off" is not the answer

It was tried. `SDL_JOYSTICK_HIDAPI=0` does move SDL onto evdev, and the two
ends then agree about which interface matters -- but they agree on the one that
carries nothing. The controller went from "works in the front-end, not in
games" to "does not work anywhere". The setting is left at its default for that
reason.

It also would not have fixed the other half. `padmap hide` clears
ID_INPUT_JOYSTICK, which gates *evdev* enumeration only. SDL enumerated a pad
with that property cleared anyway. So the hide rules cannot exclude a physical
pad from an SDL front-end on either transport, which is why a mapping wizard
prompt of "press B" was also delivered to the UI as a cancel.

## What padmap has to do instead

For pads in this class, padmap must be the process that owns the HID device:

1. **Open `/dev/hidraw*` for the pad**, not (only) its evdev node. The mapping
   from an input device to its hidraw sibling is in sysfs: a Bluetooth pad
   lives under `/devices/virtual/misc/uhid/0005:VVVV:PPPP.NNNN/`, with both the
   `input/` and `hidraw/` children under that same parent.
2. **Perform the device's init sequence** and parse its report format. For the
   Switch Pro that means the handshake and setting the full input report mode;
   the reports are then fixed-layout structs rather than evdev events.
3. **Republish exactly as now.** The uinput clone does not change at all --
   downstream, a clone fed from hidraw is indistinguishable from one fed from
   evdev. Everything already built on top keeps working.
4. **Hold it exclusively.** Only one process can usefully drive a controller
   over hidraw; two fight, which is what Steam and the kernel were doing here.
   If padmap takes the device, SDL must not: `SDL_JOYSTICK_HIDAPI=0` becomes
   correct *at that point*, or the clone simply does not match any SDL HIDAPI
   driver and SDL reads it over evdev like anything else.

That last point is what makes this coherent rather than a second special case:
padmap becomes the single owner of the physical device on whichever interface
that device actually speaks, and keeps publishing one plain evdev gamepad for
everyone else.

## Scope

Not small. It means a per-device protocol implementation for each pad family
that needs it, which is the work SDL's HIDAPI drivers already represent. The
realistic order is:

* Switch Pro / Joy-Con, since that is the pad that exposed this
* DualShock 4 / DualSense
* Xbox wireless

Reusing an existing implementation is worth investigating before writing one:
SDL's drivers are the reference, and `hid-nintendo` shows the protocol in
kernel form.

Until then the boundary should be stated plainly rather than rediscovered:
**padmap supports controllers whose evdev node carries their input. Pads that
SDL drives over hidraw are outside what it can currently republish.**

## Report mode is not a one-off

The Pro Controller powers up sending report `0x3f`: buttons and a hat, no
usable analogue data, and a byte layout unrelated to `0x30`'s. padmap asks for
`0x30` once, as the node opens, and `Source.read` filters out everything that
is not `0x30`.

That request is not reliable. It is written the instant the node opens, and a
pad that has just finished associating over Bluetooth can drop it. The write
succeeds, so nothing fails and nothing is logged; the controller simply goes
on sending `0x3f`.

Every other signal then says the controller is healthy:

* the node exists and `alive()` is true -- it compares `st_rdev`, which is
  unchanged because the device never went away
* the descriptor is live and not `(deleted)`
* reports arrive continuously, so `read` never raises `ENODEV`
* `player N: forwarding input to the clone` is already in the log, written
  once, at republish time

Measured on the pad here while it was stuck: **3001 hidraw reports in 45
seconds, 0 events on the virtual pad, nothing in the log.** From outside it is
indistinguishable from a broken mapping, and was reported as one twice.

Sending a single report-mode subcommand by hand switched it to `0x30`
immediately, with a `0x21` subcommand acknowledgement in between -- so the pad
was listening the whole time, it had just missed the first request.

So the mode is now *checked* rather than assumed: a `0x3f` is counted, the
first one is logged by name, and the request is repeated every
`SIMPLE_REPORTS_BEFORE_RETRY` reports until a `0x30` arrives. A `0x30` resets
the count, so a healthy pad is never re-asked and one that drops back into
simple mode later is caught the same way. See `tools/check_report_mode.py`.
