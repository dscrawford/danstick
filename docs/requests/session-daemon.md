# A daemon that starts with the session, unseated, and ends with it

**What happens now.** `padmap ensure-daemon` spawns `padmap serve` detached
and forgets it. The daemon then outlives everything: the picker that started
it, the game that used it, the evening. On startup `Server.restore()`
republishes whatever `assignments.json` says, so the next session opens with
yesterday's seats already taken and yesterday's pads already grabbed under
`EVIOCGRAB` — which, on a desktop, means the real controllers are invisible
to everything but padmap until somebody finds and kills it.

Right now on the machine this was written on: a daemon six hours old,
reparented to pid 1, holding an Xbox pad and a Steam Controller that nobody
is playing with.

**What GOTG wants instead.** Every session starts with nobody seated. Opening
the picker, or launching a game, is prefaced by the same thing: pick up a
controller, hold a button, be player one. That is the whole model — a seat is
something you take in front of the screen you are about to use, not something
the machine remembers you having. And when the session ends, the daemon ends
with it, releasing every pad.

GOTG can do the second half badly on its own — the keeper that already polls
the game's pid could kill the daemon it started — but not the first half at
all, because there is no way to unseat anybody:

- `forget_pad` forgets a *profile* and opens a layout wizard; the seat stays.
- `accept` with no claims is refused ("nothing assigned yet").
- `cancel` puts the previous assignments back.
- `begin` stops republishing but grabs every pad, and is a session.

Deleting `assignments.json` behind the daemon's back is the only route, and
that is exactly the kind of reach into padmap's state that has taken down a
user's real daemon from a test before (`STORIES.md`, "restarting must be
scoped to one socket").

**What would be enough.** Three small things, any one of which is useful on
its own:

1. **Follow a pid.** `padmap serve --follow <pid>` (and `ensure-daemon
   --follow <pid>`) exits, releasing everything, once that pid is gone. A poll
   is fine; `PR_SET_PDEATHSIG` does not survive the reparenting `ensure-daemon`
   does. The picker `execvp`s into the game, so its pid survives that hop; a
   Steam launch has no picker and would pass the launcher's own pid.

2. **Start unseated.** `padmap serve --fresh` (or `PADMAP_NO_RESTORE=1`)
   skips `restore()`. Profiles, calibrations and mappings are kept — those
   *are* the controller's, and follow it. Only the seats are forgotten,
   because seats are the session's.

3. **Unseat, by command.** `{"cmd": "unseat"}` — or `{"cmd": "unseat",
   "player": N}` — drops the seat, stops its clone, ungrabs its pad, and
   emits `state`. For a daemon that is already running when the session
   starts, which is the case until (1) exists everywhere.

With (1) and (2), `ensure-daemon --follow $$ --fresh` from the picker and
from the launcher is the entire integration. With (3) alone, GOTG sends
`unseat` on connect and kills the daemon it started on exit.

**The one thing that must survive all of this.** `seating` — the always-on
listening from `always-seating.md`. A second player turns up in the middle of
a game, and the picker that opened seating is gone by then; today the daemon
keeps seating open after the last client disconnects, which is correct and
is what makes mid-game joining work. `--follow` should end the daemon, not
seating early; `--fresh` should not start with seating closed if the client
asks for it in the same breath. And `unseat` must not close seating either,
since "hold a button to take a seat" is the next thing that happens.

**What GOTG does with it.** Opens the picker unseated, with the strip saying
`hold a button on a controller`. Launches a game unseated, with `gotg-seat`
asking for the hold it already knows how to ask for. Never shows anyone the
controller screen to get there. And leaves nothing running when the evening
ends. The tests that hold GOTG to this are `tests/e2e/test_controllers.py`;
the one for starting unseated is marked `xfail(strict=True)` against this
request, so the day padmap delivers, it fails loudly until the marker comes
off.
