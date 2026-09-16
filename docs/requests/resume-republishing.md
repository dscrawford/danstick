# Resuming republishing is all-or-nothing

> **Fixed.** All three parts, including "paths are not identity". See "What
> was changed" at the end.


**Severity: a controller that is switched off costs you every other one.**

`_resume_republishing` calls `_start_republisher`, which opens each assigned
pad by the path stored in `assignments.json`. The first one that is missing
raises `FileNotFoundError`, the `except OSError` around the call logs it, and
`self._republisher` stays `None` — so *no* pad is republished, including the
ones that are plugged in and working. The daemon then reports `idle` while
holding a list of assignments it is not honouring.

Observed on this machine, from GOTG's picker:

```
could not resume republishing: [Errno 2] No such file or directory: '/dev/input/event256'
```

Player 1 was a Steam Controller on `/dev/hidraw2`, present the whole time.
Player 2 was a Bluetooth Xbox pad that had gone to sleep, so its event node
was gone. The Steam Controller stopped being republished because of the pad
that was switched off, and the only way back was restarting the daemon with
the stale entry removed by hand.

This is not an unusual state. Wireless pads sleep. It takes one of them to
leave a four-player machine with no virtual pads at all.

## What would be enough

Open what can be opened and say so. A pad whose node has gone is a seat that
is empty for now, not a reason to drop the other three — the same judgement
`_open_pads` already makes for a session, where a pad that fails is skipped
and the rest are opened rather than abandoned.

Two details that matter to a front-end:

- Keep the assignment rather than deleting it. The pad is coming back, and
  its owner should not have to re-seat it because it went to sleep.
- Say which seats are live. `state` already carries `players`; a per-player
  flag for "assigned but not currently published" is what lets the strip draw
  player 2 as away instead of either lying or vanishing.

## Also worth a look: paths are not identity

The stored `path` is the whole of how a pad is re-opened, and an event node
number is not stable across a reconnect. `assignments.json` already records
`phys`, `vid` and `pid` — matching on those and treating the path as a hint
would make a pad that comes back on a different node just work, which is what
a person turning their controller back on expects.

## What was changed

The Rust daemon had the same bug -- `start_republisher` used `?` on
`clone::create`, so the first pad that could not be opened aborted the whole
roster. `padmap run` had always got this right, which made the inconsistency
easy to miss.

**Open what can be opened.** A pad that fails is logged and skipped, and the
rest are published. Only when *nothing* could be opened is it an error worth
telling a caller about, which is the case `accept` still has to report.

**Keep the seat.** An absent pad stays assigned and is warned about by name:

```
player 2: Xbox Wireless Controller (/dev/input/event9999) is not here; keeping the seat
```

**Say which seats are live.** `PlayerState` gained `published`. A front-end
draws a seat with `"published": false` as *away* rather than either lying or
making the player vanish.

**Paths are not identity.** `assignments::resolve` now matches by node first
and falls back to `(name, vid, pid, phys)` for anything left over, so a
wireless pad that comes back on a different event number keeps its seat.

The fallback is deliberately narrow: it only matches when **exactly one**
unclaimed pad fits. Four ports of one adapter agree on all four fields -- that
indistinguishability is the reason padmap exists at all -- so a second
candidate means the question cannot be decided, and guessing would hand
somebody else's controller a seat. That case is a test of its own, as is the
one where the node is present and must win.

`a_sleeping_pad_does_not_unpublish_the_others` is the reported scenario: a
real uinput pad on seat 1, a dead node on seat 2, and an assertion that seat
1's clone is on the air.

### One test had to change

`state_differential` compared padmap's `state` event to the recorded Python
one for exact equality, so adding `published` broke it. It now asserts that
everything the Python said is still said, with the same value -- a *subset*
rather than equality.

That is the guarantee worth keeping. A renamed, retyped or dropped field
breaks a front-end; an added one breaks nobody, since a client takes the
fields it knows. Forbidding additions would mean the protocol could never grow
without rewriting the evidence that it has not regressed. A test asserts the
check still catches a field that goes missing or changes value.
