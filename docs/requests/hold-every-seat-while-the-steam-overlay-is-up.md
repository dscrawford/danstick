# Hold every seated pad back from the game while the Steam overlay is up

## What GOTG is trying to do

On a Deck (gamescope), pressing the Steam button brings Steam's overlay over
the game. Steam mutes its own virtual pad while the overlay is up, so a game
that reads Steam Input goes quiet. A game under danstick does not: danstick
reads the raw pad and keeps forwarding to the clone, so every press somebody
makes *in the Steam overlay* -- the d-pad through Steam's menus, A on a menu
item -- also lands in the game. A person closing the overlay finds the game
has moved on without them.

GOTG's overlay (`gotg-killswitch`) can tell when the Steam overlay is up:
under gamescope the overlay window carries `STEAM_OVERLAY` and takes input
with `STEAM_INPUT_FOCUS` (gamescope's `steamcompmgr.cpp`, `OVERLAY_PROP`,
`steamInputFocusAtom`), both X properties it can watch. What it cannot do is
hold everybody back.

## What danstick does today

`focus` (docs/EVENTS.md) holds **one** player's pad back from its clone, which
goes to rest, and reports that pad's controls to the caller -- the shape a
menu for one player needs. "One player is focused at a time; opening on
another moves it." There is no way to say "every seat, at rest, until I say
otherwise", and opening `focus` per seat in turn is not it: each open moves
the previous one back into the game.

## What would be enough

```json
{"cmd": "hold"}
{"cmd": "hold", "open": false}
```

Every seated pad's clone goes to rest (as `focus` leaves one) and stays there
while the hold is open; pads keep their seats, seating stays open, `native`
keeps reporting the pads' own controls (so a chord can still be watched, and
so the overlay can ignore it while Steam has the pads). No `focus`-style
control events are needed. `open: false` lets every clone resume from the
pad's next change, as `focus` does, so a button still held when the overlay
closes is not pressed into the game. Closed by the connection going away.
`state` carries `"hold": true` while it is on. A seat taken while the hold is
open is held too. Refused with an `error` while a session is open, like
`focus`; an older daemon's "unknown command" is what GOTG treats as "not
there", as it does for `focus` and `native`.

GOTG's side (`gotg-killswitch`, `steam_overlay.rs`) is written against this
and tolerates the refusal until the pin moves.
