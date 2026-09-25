# With Steam Input active, a controller is its Steam pad -- and a clone's Steam pad is nobody

## What GOTG is doing

A Deck in Game Mode, fixed slots (DANSTICK_SLOTS=fixed, xbox360-numbered),
the Deck's own controls and an Xbox Wireless Controller over Bluetooth.
Somebody holds A on the Xbox pad to take a seat.

## What happens today (danstick 572f90a): one hold, three seats

The Deck's danstick.log, 2026-09-25:

    19:06:06.415 4 fixed slot(s) standing as xbox360-numbered: [1, 2, 3, 4]
    19:06:07.964 Microsoft X-Box 360 pad 2 attached ...
    19:06:08.716 Microsoft X-Box 360 pad 3 attached ...
    19:06:08.890 Microsoft X-Box 360 pad 4 attached ...
    19:06:09.376 seating: player 1 <- Microsoft X-Box 360 pad 0 (event10)
    19:06:40.894 seating: player 2 <- Xbox Wireless Controller (event11)
    19:06:41.042 seating: player 3 <- Microsoft X-Box 360 pad 1 (event18)
    19:07:00.474 seating: player 4 <- Microsoft X-Box 360 pad 3 (event24)
    19:07:03.534 Microsoft X-Box 360 pad 2 held a button but every seat is taken

A feedback loop through Steam Input:

1. Fixed slots make danstick's four clones 360 pads (045e:028e) from the
   daemon's start.
2. Steam Input wraps every 360 pad it sees in a virtual gamepad (28de:11ff):
   pads 1-4 appeared within two seconds, beside pad 0 (the Deck's controls)
   -- mirrors of danstick's own clones, and of the Xbox pad.
3. 871a4f7 keeps `max(mirrors - readable pads, Decks)` Steam mirrors as
   seatable: five mirrors less one readable pad, four offered.
4. The Xbox pad's hold seated player 2 on its raw node. Its presses went to
   slot 2's clone, out through Steam's mirror of that clone, and 150 ms later
   the same hold seated player 3 from the mirror; player 4 the same way.

Any seated player holding A -- to ready up at the door -- seats a phantom
the same way. Under mirror identity it did not loop only because a clone of
a Steam pad is 28de:11ff, which Steam does not wrap; fixed slots made every
clone a 360 pad, which it always does.

## What GOTG wants

**When Steam Input is active, a physical controller is its Steam virtual
pad, not its raw device.** Steam is the one driving it then -- its remaps,
its gyro, its per-game layout, the Deck's own controls, which have no other
readable node in Game Mode anyway -- so danstick seats the Steam pad and
leaves the raw device alone: one source per physical controller, never both.
With Steam Input not running, the raw device, as today.

And, what makes that safe: **a Steam pad that mirrors one of danstick's own
clones is never a source.** A clone forwards what a seat's pad sends; seating
its mirror is the loop above.

How danstick tells which Steam pad is which is its to choose -- Steam pads
carry no phys, uniq or serial. Some ways in, for what they are worth:

- Steam opens and wraps a device after it appears, so a Steam pad that
  appears within a moment of one of danstick's clones is that clone's; one
  that was there before any clone, or follows a physical pad's arrival, is a
  controller.
- danstick can drive its own clone and watch: a synthetic event written to
  an idle clone that shows up on a Steam pad names it (danstick already
  writes to clones, and a slot at rest is idle).
- Steam's mirror follows its source's input exactly, so a Steam pad whose
  every event follows one raw device is that device's; seat it in the raw
  device's place.

Knowing Steam Input is active: a Steam virtual pad (28de:11ff) exists, or
Steam holds a device's hidraw (the Deck's built-in controls, a Puck).

## How it would be checked

On the Deck in Game Mode, fixed slots, Deck + Xbox pad:

- holding A on the Xbox pad seats exactly one player, and on its Steam pad
  -- Steam's remap applies in the game;
- holding A again on a seated pad seats nobody;
- the Deck's controls seat through Steam pad 0, as now;
- `danstick list` offers one entry per physical controller and none for a
  clone's mirror.

A journey with STEAM_VIRTUAL fixtures made after four fixed slots, beside
one XBOX_360, covers the loop off the Deck.
