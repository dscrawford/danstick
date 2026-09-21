# A controller's keyboard and mouse should be padmap's to hold

**What happened.** The picker refuses every joystick padmap has not
published, and a Steam Controller moved its cursor anyway. So did an Xbox pad
over Bluetooth. Neither arrived as a joystick: the Puck is, in hardware, four
keyboards and four mice (lizard mode -- d-pad as arrow keys, A as Enter, the
trackpad as a mouse) until something opens its hidraw and sends lizard-off;
the Xbox pad's HID descriptor carries `Keyboard`, `Mouse` and `Consumer
Control` collections that the kernel exposes as their own nodes, on the same
`Uniq` as the joystick. A front-end takes the keyboard and the mouse on
purpose and cannot tell these from a real one -- SDL hands it a key, not the
device it came from.

```
I: Bus=0003 Vendor=28de Product=1304          I: Bus=0005 Vendor=045e Product=028e
N: Name="Valve Software Steam Controller Puck Keyboard"   N: Name="Xbox Wireless Controller Keyboard"
H: Handlers=sysrq kbd event1                  U: Uniq=0c:35:26:75:f6:2b
(x4, one per wireless slot, plus a Mouse each)  H: Handlers=kbd event259
```

**What GOTG does now.** `src/ui/gotg_ui/hush.py`: while the picker runs, it
holds `EVIOCGRAB` on every keyboard or mouse node that belongs to a
controller -- Valve's by vendor, anything else by sharing a `Uniq` (or, off
Bluetooth, a `Phys`) with a joystick node. Released on exit. Tested against
the `/proc/bus/input/devices` this machine produced with both pads attached,
and end to end with a uinput keyboard that shares a phys with a uinput pad:
a second reader on the node gets nothing while the grab is held.

**Why it should be padmap's.** The grab is released when the picker execvps
into a game, and from then on nobody holds those nodes: the Puck's lizard
mode is off only if padmap opened it, and the Xbox pad's `Keyboard` and
`Mouse` are never off. A Share button that types into the emulator, a
trackpad that moves the desktop pointer behind a fullscreen game -- these
are the same leak in a game that GOTG just closed in the picker, and padmap
already opens the joystick node of every pad it seats. Its device model
knows the siblings (`pad.rs` walks udev; the `Uniq` is right there).

**What would be enough.** While a pad is seated, or while seating is open
and the pad is a candidate, hold its keyboard and mouse siblings as well as
its joystick. Release them with the seat. `exec`'s isolation already hides
every raw node from the game; this is the same intent for the compositor.
Optionally a `controller` event field, `held: ["event259", "event260"]`, so
a front-end can stop doing it itself the moment padmap has.

**One thing to keep.** Not by `Phys` over Bluetooth. Every device on an
adapter reports the adapter's address as its phys; BlueZ's own media-control
keyboard (`nixos #1 (MCS)`) shares it with the pad and is somebody's phone.
`Uniq` is the device's own.

## What was built

While a pad is seated, or is one seating is listening to, its keyboard and
mouse siblings are opened and grabbed beside its joystick, and released with
the seat -- `padmap_input::siblings`, driven from the daemon's tick and
recomputed only when the machine's input nodes or the pads that matter change.
A sibling is a node that types letters or moves a pointer and shares the pad's
`uniq`, or, for Valve hardware, its vendor, since the Puck's lizard nodes carry
no uniq. Never by `phys`, for the reason given above: BlueZ's media-control
keyboard is somebody's phone. The daemon lets go of everything on exit.

Not built: the `held` field on `controller`. The front-end can stop grabbing
the moment it pulls this; nothing it needs to know per event.

Tests: the classifier on the Xbox pad's three nodes (keyboard and mouse held,
consumer control not), BlueZ's MCS keyboard, the Puck's vendor-only nodes, a
generic pad with no uniq, and the pad's own node; and a journey with a uinput
keyboard beside a uinput pad of the same vendor, where the test's own grab
fails while the pad is seated and succeeds once it is unseated.
