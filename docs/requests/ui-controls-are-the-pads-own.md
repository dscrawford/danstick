# The overlay's controls are the pad's own, whatever the game's walk says

## What was seen

2026-09-29, an emulated N64 game, the Xbox Wireless Controller seated. The
pad's N64 walk (made on purpose, for the game) puts N64's L on the left
trigger, R on the right trigger and Z on RB. GOTG's overlay reads its chords
-- L + R + A for the menu, L + R + Start to stop the game -- off the clone,
which carries that walk: so "L + R" was the *triggers* on this pad, and the
bumpers did nothing. In another game, or after a rebind, the chord moves
again. The person holding the pad asked for the obvious rule: **the controls
that drive the overlay are the pad's own and never move** -- L and R are the
bumpers, A is the bottom face button -- and a walk changes the game's
controls only.

## Why this needs danstick

The overlay cannot see the pads themselves: danstick holds them (EVIOCGRAB),
and what reaches anybody else is the clone, after the walk.

## What would be enough

1. **`focus` (a-menu-holds-one-player) reports the pad's own controls**, not
   the walk's: `a` is the bottom face button, `leftshoulder` the left bumper,
   on every pad -- by the pad's standard layout (the kernel's gamepad
   convention / SDL's database), or the pad's generic walk where it has no
   standard one. The menu is UI, and UI is the pad's own.
2. **The same, for every seated pad, for a handful of controls, without
   holding anybody**, so the overlay can see a chord start on any pad while
   everybody is playing:
   ```json
   {"cmd": "native", "open": true}
   {"event": "native", "player": 2, "control": "leftshoulder", "down": true}
   ```
   Only changes, only `leftshoulder`, `rightshoulder`, `a`, `b`, `start`,
   `back` (and the triggers, digitally, for pads whose shoulders are
   triggers). Leased to the connection like `scope`, so it stops with the
   overlay. The game keeps getting every press exactly as it does now -- this
   only lets the overlay watch.

## How GOTG will check it

With the N64 walk above: LB + RB + A held a second brings the menu down in the
N64 game and in DK64 alike; the triggers held with A do not; the menu is
walked with the d-pad and A/B of the pad itself; and a rebind that moves A
somewhere else does not move the menu's A.
