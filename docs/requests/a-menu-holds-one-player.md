# A menu over the game holds one player's pad, and hears it

## What GOTG is building

The overlay over a game (`gotg-killswitch`) is becoming a menu. A player holds
**L + R + A for a second** and a panel comes down: the seated controllers, a way
to rebind one, a way to change the order, and a held "exit" that stops the game.
**B held** closes it. Only the player who opened it drives it; everyone else
keeps playing.

## Why this needs danstick

While the menu is open, that player's presses must reach the menu and **not the
game** -- A on "exit" must not also jump in the game, and moving down a list
must not walk the character off a ledge. Today the overlay reads each seat
through SDL off the clone, which is exactly what the game reads.

danstick already does half of this for `map`: the named pad is held, its clone
held back from the game (at rest), and the wizard hears it (`input` events).
What is missing is that without a walk -- held for a menu, heard as *controls*.

## What would be enough

```json
{"cmd": "focus", "player": 2, "open": true}
{"cmd": "focus", "player": 2, "open": false}
```

1. **Open**: player 2's clone goes to rest (every button up, sticks centred --
   what a `stay` leave already does) and stays there; the pad's input is no
   longer forwarded to it. Everyone else is untouched.
2. **Heard**: while focused, each change on that pad is sent as a control, after
   the pad's walk for the scope in play (so A is whatever that player bound as
   A):
   ```json
   {"event": "focus", "player": 2, "control": "a", "down": true}
   {"event": "focus", "player": 2, "control": "dpdown", "down": false}
   {"event": "focus", "player": 2, "stick": "left", "x": 0.0, "y": -0.9}
   ```
   Controls in SDL's names, as `map` and `captured` use.
3. **Close** on `open: false`, **or when the connection that opened it goes**
   (the same lease `scope` has), so an overlay that dies cannot leave a player
   dead in the game. The clone resumes from the pad's current state -- a button
   still held when the menu closes is not "pressed" into the game.
4. `state` says who is focused (`"focus": 2`, or absent), so a second client
   knows.
5. A `map` started from inside the menu (rebinding that player) takes over the
   hold for its run and hands it back when it ends.

## How GOTG will check it

Open the menu in DK64 with player 1: the character stops dead while the list
is walked, player 2 keeps moving, and closing the menu with a button held
does not press it in the game. Kill `gotg-killswitch` with the menu open:
player 1 is live again.
