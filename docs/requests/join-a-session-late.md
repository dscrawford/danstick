# A controller switched on during a session should be able to take a seat

**What happens now.** `begin` opens a session over the pads that exist at
that moment: `Session::open(pads)` takes the list `discover()` returned
(`server.rs:648`, `:660`) and nothing adds to it afterwards. While the state
is `assigning`, `poll_controller_changes` returns before it looks
(`server.rs:2074-2076`), and auto-setup does the same (`:2335`). So a
controller switched on after `begin` is not in the session, produces no
`pads` event -- that one is emitted only from inside `begin`
(`server.rs:698`) -- and holding a button on it does nothing. The only way
to seat it is `cancel` or `accept` and `begin` again, which drops every
claim already made and grabs the room a second time.

**Where it bit.** The launch gate. Player one seated themselves and walked
the wizard; player two turned their pad on during that and could not join,
and the screen waiting for "the controller you want to play with" was
waiting for exactly the pad it could not see. GOTG has since moved the gate
off sessions entirely -- seating mode rescans, and `map` needs no session --
which is the right shape for a gate. But `begin` is still what the
assignment screen uses to *reorder* a room, and it is still what padmap
opens by itself for a pad it has never mapped, and in both a late pad is
still invisible.

**What would be enough.** While a session is open, keep scanning, and add a
pad that appears to the session as one more source -- grabbed like the
others, read by the same assigner, claimable by the same hold. Emit `pads`
again with the new count, so a front-end showing "no controllers found --
plug one in" can stop saying it. A pad that leaves mid-session is the
mirror: dropped from the sources, its claim -- if any -- kept as it is now
for a pad that comes back.

Nothing about the session's meaning changes: it still owns every pad, it
still ends with `accept` or `cancel`, and the claims still turn into seats
only at `accept`. The list it owns just follows the room.

**What GOTG would do with it.** Nothing new to send. The assignment screen's
"hold a button on the controller for player 2" would become true the moment
a second controller was switched on, which is what it already says.

## What was built

A session follows the room. While one is open the attach scan no longer
stops at the door: a pad that appears is admitted -- opened, grabbed, drained,
watched, fed to the same assigner -- and `pads` goes out again with the new
count. A node is readable a beat after it exists (udev's ACL), so an admission
that fails is retried on the next scan up to the hotplug path's limit rather
than given up on the first `EACCES`, which is exactly what the test hit.

A pad that goes away mid-session is the mirror: its node is unwatched so a
dead fd cannot spin the loop, its slot and claim are kept, `pads` reports the
smaller count, and if it comes back on the same node it takes its old place.
Nothing about the session's meaning changed: it still owns every pad, still
ends with `accept` or `cancel`, and claims still become seats at `accept`.

The journey `a_pad_switched_on_during_a_session_can_take_a_seat` opens a
session over one pad, creates a second, sees `pads: 2`, claims player 1 on
the late pad, removes it, sees `pads: 1`, and cancels cleanly.
