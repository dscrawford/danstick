# Found when GOTG pulled the working tree

> **Fixed.** `Seating` now remembers the set it was *asked for*, not the set
> that opened. See "What was fixed" at the end.

Pulled 2026-09-18 into a GOTG build with the `padmap` input pointed at this
checkout. All three requests read as done. One test fails, and it points at
a behaviour rather than a typo.

## `seating::tests::the_same_set_of_pads_is_not_a_change`

```
thread 'seating::tests::the_same_set_of_pads_is_not_a_change' panicked at
crates/padmap-daemon/src/seating.rs:193:9:
the same set must not churn the watch list every tick
```

Reproduce with `cargo test -p padmap-daemon --lib`, or through nix, which
runs the suite as part of the build:

```
nix build path:/home/daniel/Documents/padmap#padmap-rs
```

**Why it fails.** `Seating::refresh` records a pad only when
`clone::open_source` succeeds. The test's pads are `/dev/input/event1` and
`event2`, which do not exist, so nothing is recorded and `would_change`
stays true after the refresh.

**Why it is not only the test.** On a real machine the same thing happens
for any pad that cannot be opened -- a permissions problem, a node that
vanished between discovery and open. `refresh` compares the *opened* set
against the *wanted* set, they never match, and every tick reopens every
pad and logs `seating: X cannot be watched` again. The churn the test is
guarding against is real; the test just found it early.

**A shape that would fix both.** Remember what was *wanted* separately from
what was *opened*: compare `wanted` against the last wanted set, and only
when that differs clear and reopen. A pad that failed to open is then
retried when the set changes, not every tick; and the unit test passes
without a device because the wanted set is what it compares.

Nothing on the GOTG side is blocked by this beyond the build itself; the
rest of the suite passes, and the three request write-ups match what GOTG
needs. GOTG will pull again on the next tag.

## What was fixed

`Seating` now keeps the set of pads it was last **asked for** beside the set it
actually opened, and `would_change` compares against that. A pad that cannot be
opened -- a permissions problem, a node that vanished between discovery and
open -- no longer differs from the opened set forever, so it is retried when the
set changes rather than reopened on every tick, and the warning is logged once
rather than fifty times a second.

The unit test passes without a device, which is what it was for. It now uses
nodes no machine has (`event90001`), so it is not quietly passing on a
developer's machine because `/dev/input/event1` happens to exist there -- which
is exactly why this got through here and failed on your pull.
