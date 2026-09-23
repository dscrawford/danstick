# A join keeps everybody else's clone

**Answered, except the sandbox.** A claim adds one clone -- the new seat's --
and every clone already open stays the same device at the same node
(`a_join_leaves_the_players_already_in_the_game_plugged_in`). And a join no
longer works the rest of the room out again, so it does not cost more the
fuller the room (`a_join_does_not_work_the_rest_of_the_room_out_again`):
measured claim-to-`state` at four seats, 456/656/488/664 ms, with no growth
from seat one to seat four. What is left of that is one ~0.5 s subprocess
asking SDL about the *joining* pad's GUID, paid once per controller model per
daemon, not once per seat. `test_a_join_costs_the_same_however_full_the_room`
and `test_the_players_already_in_the_game_barely_notice_a_join` should both be
worth re-running.

## What is still open: the game's sandbox is fixed at launch

`padmap-rs exec` binds `/dev/input` as a tmpfs holding the nodes that existed
when the game started (`padmap-input/src/isolate.rs::bwrap_argv`), so a clone
created mid-game never appears inside it, and SDL's udev hotplug does not
cross the user namespace. A joiner therefore still reaches nothing in any
emulator. The people already playing are no longer cut off, which was the
worse half, but the bar GOTG draws over the game still ends in a pad that does
nothing.

The two shapes, unchanged:

1. **Seats exist before people do.** Under `exec`, create a clone for every
   seat the launch allows (`--players N`), empty ones included, and bind them
   all in. A claim attaches its source pad to a clone that is already there.
   This needs an identity whose GUID and layout are known before anybody sits
   down: `padmap` (1209:0001, per-player version) or `xbox360`. `mirror`
   cannot do it -- the GUID comes from a pad nobody has picked up yet.
2. **The sandbox follows the clones.** Bind the real `/dev/input` and cover
   only the raw pads, including later ones, with `SDL_JOYSTICK_DISABLE_UDEV=1`.
   Cheaper but racier: a raw pad switched on mid-game is visible until covered,
   and padmap's whole promise is that a game sees its clones and nothing else
   (STORIES.md S15).

**This one needs GOTG to choose.** Shape 1 changes what a padmap pad *is* for
every consumer -- the GUID a game binds, and what `env.sh` can carry for seats
nobody has taken. Shape 2 keeps identities as they are but weakens the
isolation that stops a game reading a raw pad and a clone at once. Say which
and it can be built; it is not a decision to take on GOTG's behalf.
