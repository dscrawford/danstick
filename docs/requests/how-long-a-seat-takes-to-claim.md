# How long a hold has to be before it claims a seat

`padmap-core/src/assign.rs` sets `HOLD_SECONDS = 0.25` and nothing can change
it. A quarter of a second is too short at the front of a launch: picking a
controller up, or resting a thumb on it while reading the screen, claims a
seat nobody meant to claim, and on a sofa with four pads out the wrong one
becomes player one. Asked for in GOTG as "the pair is too fast, can you make
it 1.5s?", twice.

**What is wanted: a way to say how long, with 0.25 s kept for whoever does not
ask.** 1.5 s is the length GOTG would set.

## Why the front-end cannot do it

The hold is timed inside the daemon against the device, and the first a
front-end hears is the `claim` that has already happened. The only thing GOTG
could do is watch raw pads through SDL and keep `seating` shut until it has
seen a long enough hold itself — which means acting on input from an unseated
pad, the one thing GOTG's controller rule forbids, and it would still be a
quarter-second race once seating opened.

## What already fits

`Assigner::new(hold_seconds)` takes the length as a parameter. Only
`impl Default for Assigner` hardcodes the constant, and both consumers build
one that way:

* `padmap-daemon/src/session.rs:75` — `Assigner::default()`, the `begin` path.
* `padmap-daemon/src/seating.rs:19` — `assigner: Assigner` under
  `#[derive(Default)]`, the seating path, which is the one GOTG uses.

So the change is small, and `Tick::progress` needs nothing: it is already
`elapsed / hold_seconds`, so a front-end's reveal fills over whatever length
is set.

## Two knobs, either or both

**An environment variable**, read where `Default` is built:

```
PADMAP_HOLD_SECONDS=1.5
```

Enough on its own for GOTG, which starts its own daemon (`--fresh --follow`).

**A field on the command**, which is the better of the two:

```json
{"cmd": "seating", "open": true, "players": 4, "hold": 1.5}
```

because the right length differs by screen — a launch gate wants deliberation,
a mid-game join wants to be quick — and it needs no restart. That is
`Command::Seating` (`padmap-core/src/command.rs:68`, parsed at :180) carrying
an `Option<f64>`, routed at `padmap-daemon/src/server.rs:673` into
`Seating::open(seats, hold)`. Note the exhaustive match in
`padmap-core/tests/command_differential.rs:49` will want the new field too.

Omitted, it should leave the length as it was rather than reset it.

## What it should refuse, and how

Nothing. A comfort setting is not worth failing to start over: out of range,
unparseable or missing is the default. A range of about `0.05..=10.0` seconds
keeps it a hold — zero is a press, and a minute is not a hold anybody holds.

One thing worth getting right: **a hold in flight when the length changes
should be dropped**, not re-measured. A press that became a claim because the
number changed underneath it is exactly the accident this exists to prevent.

## How it would be checked

* `hold_from(Some("1.5")) == 1.5`, and `""`, `"soon"`, `"0"`, `"-2"`, `"600"`,
  `"nan"` all give 0.25 — as a function of the text, so no test has to export
  a variable into a process shared with every other test in the binary.
* An `Assigner::new(1.5)`: a button down at 0.0 claims nothing at 0.3 or 1.4,
  and claims at 1.51.
* Its `progress` at 0.75 s reads about 0.5.
* `set_hold_seconds` mid-hold: the old hold never claims; a fresh press does.
* Through the socket: `seating` opened with `"hold": 1.5`, a pad held for one
  second gets no `claim`; held for two it does, with `progress` reaching 1.0
  at about 1.5 s rather than 0.25 s.

## What GOTG does when it lands

`gotg-seat` and the picker send `hold` with `seating` (and the launcher
exports `PADMAP_HOLD_SECONDS` for the games' daemon), at 1.5 s.

GOTG's own pause between pairing and readying up — every pad quiet for a
second before a press can start the game — stays either way. It exists
because the *pairing* press used to roll straight into the go hold, and that
is a front-end problem, not this one.

---

## Answered

padmap `98fd757`. `hold` on the `seating` command, 0.05 to 10 seconds, 0.25
for anybody who does not ask, and omitted it leaves the length as it was;
`PADMAP_HOLD_SECONDS` sets what a daemon starts with. A hold in flight when
the length changes is dropped rather than re-measured, and nothing is refused
-- a length outside the range is the default. `docs/EVENTS.md`, "How long the
hold is".

GOTG asks for **1.5 s**: `theme.timeouts.pair_hold`, sent on every `seating`
by `gate.decide` and the picker's `assign.Watch`, and exported as
`PADMAP_HOLD_SECONDS` by `padmap.sh` and `gotg_ui.padmap.ensure_daemon` so a
session -- padmap's own wizard -- takes the same length.
