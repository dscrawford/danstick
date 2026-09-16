# Resuming republishing is all-or-nothing

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
