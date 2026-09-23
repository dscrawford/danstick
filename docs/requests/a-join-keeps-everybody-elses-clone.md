# A join keeps everybody else's clone

**The republish half is answered** (see the commit that trimmed this file): a
claim adds one clone -- the new seat's -- and every clone already open stays
the same device at the same node.
`test_a_join_leaves_the_players_already_in_the_game_plugged_in` should pass;
`a_join_leaves_the_players_already_in_the_game_plugged_in` in
`daemon_journey.rs` is padmap's own version of it.

Two things are still open.

## 1. A join still costs more the fuller the room

`test_a_join_costs_the_same_however_full_the_room` is not answered. Making the
clones again is gone, but `state` still goes out only after every *seated*
player's autoconfig and SDL mapping are rewritten (`rewrite_consumers` ->
`publish::write_all`), which is still work proportional to the room. The
measured 274/594/869/1462 ms was both costs together; padmap has not measured
the remainder at four seats, so please re-run the check rather than trust a
guess. Making that half incremental means knowing which consumers' files
actually depend on the other seats -- RetroArch's joypad indices do -- so it
is a real piece of work rather than a smaller edit.

## 2. The game's sandbox is fixed at launch

Unchanged and still the larger half. `padmap-rs exec` binds `/dev/input` as a
tmpfs holding the nodes that existed when the game started
(`padmap-input/src/isolate.rs::bwrap_argv`), so a clone created mid-game never
appears inside it and SDL's udev hotplug does not cross the user namespace. A
joiner therefore still reaches nothing in any emulator -- but the people
already playing are no longer cut off, which was the worse half.

The two shapes proposed, unchanged:

1. **Seats exist before people do.** Under `exec`, create a clone for every
   seat the launch allows (`--players N`), empty ones included, and bind them
   all in. A claim attaches its source pad to a clone that is already there.
   This needs an identity whose GUID and layout are known before anybody sits
   down: `padmap` (1209:0001, per-player version) or `xbox360`. `mirror`
   cannot do it -- the GUID comes from a pad nobody has picked up yet.
2. **The sandbox follows the clones.** Bind the real `/dev/input` and cover
   only the raw pads, including later ones, with `SDL_JOYSTICK_DISABLE_UDEV=1`.
   Cheaper but racier: a raw pad switched on mid-game is visible until covered.

Shape 1 changes what a padmap pad *is* for every consumer, which is a decision
for GOTG rather than one to make on its behalf: say which and it can be built.
