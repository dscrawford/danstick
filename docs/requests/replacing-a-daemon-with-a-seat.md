# A daemon with a pad seated can no longer be replaced

**Regression, between `f4356e4` and `4a7bfad`.** `ensure-daemon` replacing a
running daemon that has a pad seated used to take half a second; it now
fails after ten, and the front-end is told there is no current daemon.

## Measured

Same machine, same script, same synthetic pad. Start a daemon, open seating,
hold a button so the pad is seated and its clone published, then ask
`ensure-daemon --fresh --follow <pid>` to replace it (the daemon it finds
belongs to no session, so a replacement is correct):

```
f4356e4    0.51 s  rc=0  restarted; build /nix/store/2jlk2301…
4a7bfad   10.54 s  rc=1  daemon is current but belongs to another session
                         replacement daemon did not come up within 10s
```

With no pad seated the replacement is fine in both. It is the seat that does
it.

## Where to look

The likely culprit is `bd63c85`, "a seated pad's keyboard and mouse are held
with it" -- the answer to `controllers-that-are-keyboards.md`, and welcome.
A seated pad now carries extra `EVIOCGRAB`s on its keyboard and mouse nodes.
When the replacement starts while the old daemon is still exiting, those
nodes are still held, and the new daemon appears not to come up. Whatever
the mechanism, the shape is: more grabs to hand over, and the handover does
not wait for them.

`45a8186` in GOTG holds the same nodes in the picker and had to cope with
exactly this -- a node already grabbed is skipped and said once, rather than
retried -- which may be the cheaper shape here too.

## Why it matters to GOTG

Three paths replace a daemon that has somebody seated:

- a launch that wants a different pad identity (`PADMAP_PAD_IDENTITY=xbox360`
  for the decompiled ports, `look-like-an-xbox-pad.md`) after the picker has
  already seated a player -- the common case for DK64 from the grid;
- a launch whose daemon belongs to another session;
- a sync that leaves the running daemon on older code.

Each now warns "padmap has no current daemon; controllers will be whatever
SDL finds" and plays on with the clones the old daemon published. Nothing
crashes; the game just does not get what it asked for.

`tests/e2e/test_controllers.py` has two tests that catch it --
`test_a_new_session_opens_with_nobody_seated_whatever_padmap_remembers` and
`test_a_launch_from_steam_meets_the_gate_first_on_a_daemon_of_its_own` --
marked `xfail(strict=True)` against this file, so they fail for passing the
day it is fixed.

---

## Answered

Fixed upstream, at the pin GOTG now follows. Both strict xfails --
`test_a_new_session_opens_with_nobody_seated_whatever_padmap_remembers` and
`test_a_launch_from_steam_meets_the_gate_first_on_a_daemon_of_its_own` --
turned into XPASSes and their markers are gone.
