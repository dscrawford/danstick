# A controller should be able to join at any time

> **Done**, as specified. See "What was built" at the end -- and one bug it
> found in the fix for [resume-republishing](resume-republishing.md).


**What happens now.** Taking a seat is a *session*. A front-end sends
`{"cmd": "begin", "players": 4}`, padmap grabs every pad with `EVIOCGRAB`, the
people holding them press a button in turn, and somebody sends `accept`. Until
that session is opened, holding a button on an unassigned controller does
nothing at all.

**Why that is the wrong shape for a living room.** The moments a controller
needs to join are the moments a modal screen is most expensive:

- a second player arrives while the first is already in a game;
- a pad dies and is swapped for a charged one mid-level;
- a controller is switched on after the picker has already started, which is
  the ordinary case — people turn the television on first;
- a wireless pad drops out and comes back.

In each, the game is running and the person is holding a controller that does
nothing. Getting it working means leaving the game, opening a front-end,
finding the controllers screen and starting a session — and because padmap
grabs the pads while that session is open, the *other* players' controllers
stop working for the duration. One person joining costs everybody else the
thing they were doing.

**What would be enough.** An always-on mode where a pad that holds no seat can
claim the next free one by being held, with no session in flight:

```
{"cmd": "seating", "open": true, "players": 4}   listen from now on
{"cmd": "seating", "open": false}                stop
```

While it is open, holding a button for the same duration `begin` already uses
emits the same `progress` and `claim` events a session does, and padmap
republishes and rewrites the emulator configs exactly as `accept` does today.
The difference is only that no session was opened, nothing was grabbed, and
nobody else's controller stopped working.

Two bounds would make it safe to leave on:

- **only unassigned pads.** A pad already holding a seat is being *played
  with*: holding B to block in a fighting game must not reseat anybody.
- **only the free seats.** If four are taken there is nothing to claim, and a
  fifth pad held does nothing until somebody leaves.

**Why this is padmap's and not ours.** GOTG could poll `list` and guess, but
"this pad has been held for 600ms" is exactly the measurement padmap already
makes, on devices it already has open, with the debounce and the
hold-to-confirm already written. Doing it here would be a second
implementation of the one thing padmap exists to do, racing the first for the
same devices.

**What GOTG would do with it.** Turn it on when the picker starts and leave it
on. The assignment screen stays for the case where somebody wants to *reorder*
seats deliberately — that is a session, and grabbing the pads for it is
correct, because reordering is something you do with everybody's attention
rather than in the middle of a level.

## What was built

`{"cmd": "seating", "open": true, "players": 4}` and `{"open": false}`, exactly
as proposed. While open, an unseated pad held for `HOLD_SECONDS` takes the
lowest free seat, emits the same `progress` and `claim` events a session does,
and padmap republishes and rewrites every consumer's config as `accept` does.

No session is opened and **nothing is grabbed**: the unseated pads are read
ungrabbed, so a press still reaches whatever has focus. Both bounds hold --
only unseated pads, only free seats -- and each has a test.

Seating suspends itself while a session is open, and lets go of its
descriptors when one starts. A session grabs every pad and is about to rewrite
the roster; reading underneath it would claim a seat the user is in the middle
of assigning.

`a_pad_can_take_a_free_seat_without_a_session` asserts the thing that makes
this worth having: the state never passes through `assigning`, so no session
was opened behind the scenes.

## A bug this found

Writing the "only free seats" test surfaced a real bug in the
[resume-republishing](resume-republishing.md) fix, shipped one commit earlier.

`restore()` logged *"keeping the seat"* for a pad that was not there -- and
then dropped it from the roster anyway, because a seat carries a pad and an
absent controller has none. The seat was kept in the log and nowhere else, so
the next `save_assignments` would have **erased it from disk**. A wireless pad
that slept through a daemon restart would have lost its seat permanently.

Away seats are now held explicitly: saved, counted when finding the next free
seat, and drawn by a front-end as `published: false`. A controller that comes
back takes *its own* seat rather than a new one beside it, matched the way
`assignments::resolve` matches, so a pad that woke on a different node comes
home.

The test caught it only when the suite ran in parallel -- serially, the
seating test saw a roster the other tests had not yet disturbed. Worth knowing
for anything else that reads the state file.
