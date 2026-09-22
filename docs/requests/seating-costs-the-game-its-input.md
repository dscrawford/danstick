# Seating open costs a game its input

**Bug, not a feature request.** With seating mode open, a press takes about
a tenth of a second to reach the program reading the clone. Melee was
unplayable: a jagged stick, buttons that answered late. It is not the port
and not the pad.

## Measured

A synthetic uinput pad, seated by a hold, read back through its clone's
event node on an otherwise idle desktop. Milliseconds from `write()` on the
source to `read()` on the clone, 120 presses each:

| | p50 | p95 | max |
|---|---|---|---|
| the harness itself (the pad's own node, ungrabbed) | 0.01 | 0.01 | 0.03 |
| clone, **seating closed** | 0.03 | 0.26 | 5.78 |
| clone, **seating open** | **107.93** | 119.91 | 378.64 |

Nothing is lost -- a 1000-sample stick ramp arrives complete and in order
either way -- it is all just late. The same shape shows in the axis: p50 61
ms, max 253 ms.

## Why

`refresh_seating` calls `scan.pads()` before it decides whether anything
changed (`server.rs`), and `Scan::pads` is `get_or_insert_with(discover)` on
a `Scan` made once per tick. So `discover()` runs on **every 20 ms tick**
while seating is open. One `discover()` here costs about **100 ms**: the
udev walk, plus `triton::slots(true)`, whose `probe` opens and reads every
one of this machine's 13 hidraw nodes to decide whether a slot is live.

The loop is therefore never idle: each tick starts a scan that outlasts the
tick, and a source that becomes readable waits behind it. 108 ms of latency
is one scan.

`would_change` guards the epoll churn, which was the earlier fix, but it is
*after* the scan that costs the time.

## And a hold can be missed outright

The same starvation costs claims, not only latency. With seating open, a
0.7 s hold on a second pad -- long past `HOLD_SECONDS` -- is sometimes never
claimed at all, and a person is left pressing a button that does nothing.
GOTG's e2e sees it as flake: the test that joins a second controller at the
launch gate passes alone and fails when the machine is busier, and it now
holds for 1.2 s and retries five times to be reliable. Reported from a real
launch as "I am unable to pair my second controller".

That is the stronger reason to fix the scan: 100 ms of lag is bad, and a
hold that reaches nobody is a controller that cannot join.

## What would be enough

Any of these, and the first is probably the whole thing:

- **Throttle the seating scan** the way auto-setup already throttles its own
  (`PAD_SCAN_SECONDS = 1.0`). A pad switched on mid-game can wait a second
  to become claimable; a press cannot wait 100 ms.
- **Do not probe hidraw on every scan.** `slot_is_live` is the expensive
  half. Cache it per node, invalidate on a `controller` event, or probe only
  nodes the scan has not seen before.
- **Scan off the hot loop** -- a worker thread, or on hotplug only, with the
  result handed to the loop. The loop would then do nothing but forward.

A number to hold it to: with seating open, a press should still reach the
clone inside a frame (16.7 ms), and ideally in the tens of microseconds it
takes with seating closed.

## What GOTG does meanwhile

`gotg-seat` sends `{"cmd": "seating", "open": false}` as it hands over to the
game, so a launch is fast and a pad cannot join mid-level. That is a real
loss -- joining mid-level is what `always-seating.md` asked for and got --
and it goes the day this is fixed. The test that proves the latency is in
GOTG's `tests/e2e/test_controllers.py`, with a strict xfail
(`test_a_pad_can_join_mid_game_without_costing_the_game_its_input`) that
fails loudly when seating open is cheap enough to keep.

---

## Answered

Fixed upstream, at the pin GOTG now follows. The suite's strict xfail
`test_a_pad_can_join_mid_game_without_costing_the_game_its_input` turned into
an XPASS, which is what that marker was for: a press reaches the game in under
a frame with seating open. `gotg-seat` no longer closes seating before the
game, so a controller switched on in the middle of a level can take a seat
again.
