# Move a controller to another seat, mid-game

## What GOTG is building

In the overlay's menu (see `a-menu-holds-one-player.md`) the seated controllers
are listed in seat order. A player highlights a controller, **holds A** to pick
it up, moves it up or down the list, and lets go: that controller is now in the
seat it was dropped on. People sit down in whatever order they picked their
pads up; the game's player one should be whoever they decide.

## Why this needs danstick

There is no command that changes which seat a seated pad is in. `unseat` and a
new hold get there, but the pad passes through "nobody", the hold takes the
*lowest free* seat rather than the one wanted, and with four seated there is
no free seat to land in at all.

## What would be enough

```json
{"cmd": "move", "player": 1, "to": 3}
```

- The pad in seat 1 is now in seat 3. If seat 3 had a pad, it moves to seat 1 --
  a swap, so nobody is dropped. An empty seat 3 leaves seat 1 empty.
- **Under fixed slots the clones stay where they are.** Seat 3's clone -- the
  node the game opened as player 3 -- is now driven by the moved pad; nothing
  is destroyed or made, so a running game sees the controls change hands and
  never a device come or go. Each clone is driven from its new pad's walk for
  the scope in play, and let go (at rest) before it is.
- Each pad keeps what is its own: its walk, tuning, and a focus
  (`a-menu-holds-one-player.md`) follows the pad to its new seat.
- One `state` after, with the new seats, so a front-end redraws once.
- An `error` for a seat outside the slots, or a `player` nobody is in.

## How GOTG will check it

Two pads seated in DK64 (fixed slots). `move` 1 to 2: the pad that drove Kong
one now drives Kong two and the other way round, without the game noticing a
controller leaving; `state` lists them swapped; the menu's list redraws in the
new order.
