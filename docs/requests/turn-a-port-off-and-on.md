# Turn a game's port off and on, one seat at a time

## What GOTG wants

The overlay decides, per seat, whether the game hears that controller: off
while its player is in the menu, and off or on from the menu for anybody else
(a controller left on the sofa, a younger sibling's pad during a boss). Several
seats may be off at once, each switched on again on its own.

## Why `focus` is not it

`focus` switches one seat off *and* reports that pad to the menu, and one
player is focused at a time -- opening on another moves it. That fits the
menu's owner. It cannot turn two seats off, and a seat switched off for its
own sake has no presses to report to anybody.

## What would be enough

```json
{"cmd": "port", "player": 3, "open": false}
{"cmd": "port", "player": 3, "open": true}
```

- **Off**: that seat's clone goes to rest (every button up, sticks centred, as a
  `stay` leave leaves it) and the pad's input stops reaching it. The seat, the
  pad and its node are untouched -- the game still has player 3, who does
  nothing. `native` keeps reporting the pad's own controls, so its player can
  still open the menu.
- **On**: the clone resumes from the pad's *next* change, as `focus` closing
  does, so a button held across the switch is not pressed into the game.
- **Any number off at once**, each on its own. A `focus` on a seat that is off
  hears it as usual; closing the focus leaves it off.
- **Leased**, like `scope`, `focus` and `native`: every port the connection
  switched off is switched on when that connection goes, so an overlay that
  dies cannot leave a player dead in the game.
- The switch follows the **seat**: a `move` carries it with the pad as a
  focus is carried, and an unseat clears it.
- `state` lists the seats that are off (`"ports_off": [3]`), and an `error`
  for a seat outside the slots.

## How GOTG will check it

Two players in a game: switch seat 2 off from the menu -- player 2's pad does
nothing in the game while player 1 plays on, and player 2 can still hold
L + R + A for the menu; switch it on with A held on the pad -- the A does not
land in the game. Kill the overlay with seat 2 off: player 2 is live again.
