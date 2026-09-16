# A Triton pad pairs in a session and is never read

> **Resolved at `983bfc3`** (the Rust daemon), verified on the Puck itself.
> The Python revision GOTG pins, `8553d54`, has the bug; moving the pin fixes
> it. See "What was checked" at the end.


**Bug, not a feature request.** Holding a button on a 2026 Steam Controller
("Puck", `28de:1304`) during an assignment session claims no seat. It pairs and
then says nothing.

## What the daemon logs

With only the Puck attached:

```
client connected (1 total)
/dev/hidraw2: controller paired
session open: 1 pad(s), 4 slot(s), 0 already assigned
session cancelled after 0 claim(s)
```

Compare a synthetic Xbox pad — `fakepad.get("xbox360").spawn()` — through the
same client and the same session:

```
pad fd 12 first read -> assigner
{"event": "progress", "frac": 0.881}
...
{"event": "claim", "player": 1, "name": "Microsoft X-Box 360 pad", ...}
```

The Puck produces **no `first read -> assigner` line at all**, so no progress
and no claim. Nothing is logged as an error.

## The device is not the problem

padmap's own triton source reads it happily, in the same session, on the same
machine:

```python
from padmap import devices, triton
pads = [p for p in devices.discover() if triton.owns(p)]   # [('Steam Controller', '/dev/hidraw2')]
src = triton.open_source(pads[0])
total = sum(len(src.read() or []) for _ in range(60))      # 0.05s apart
print(total)                                               # 2528
```

**2528 events in about three seconds with nothing pressed** — the gyro, which
only streams once lizard mode is off. So `owns`, `open_source`, the lizard-off
feature report and the decoder are all working. What is not working is
whatever connects that Source to the assigner.

## Read this against the Rust port first

Everything below was reproduced against the **Python** daemon, at
`8553d54` — the revision GOTG's flake still pins, so it is what a GOTG launch
runs today. `983bfc3` deleted that implementation, and the Rust port does not
have the ordering problem the last section guesses at: `triton::Source::open`
sends the lizard-off report at open (`rust/crates/padmap-input/src/triton.rs`),
before the descriptor is ever handed to the reactor, rather than only from
inside `fetch_events`.

So the symptom is worth re-testing on Rust before anybody hunts for it. If the
Puck now claims a seat, this request is already closed and only the pin needs
moving. If it still does not, the shape of the failure is unchanged and the
loop to look at is `Watched::Session(index) -> on_session_read -> session.read`
in `rust/crates/padmap-daemon/src/server.rs`: a pad that is never readable is
never read, whatever sends the keepalive.

## Where it looked like it went wrong

A guess, offered as a place to start rather than a diagnosis: `read()` is what
sends the keepalive —

```python
def read(self):
    self._keepalive()
```

— so anything that waits for the fd to become readable *before* calling
`read()` has a dependency the other way round from the one the device needs.
The Puck is silent until it is told to leave lizard mode, and it is told inside
the call that is waiting for it to speak. A pad that happens to be streaming
already (mine was, from the earlier open) would hide this, which may be why it
looks intermittent.

## Why it matters here

This is the controller on the machine GOTG is being built on, and for a person
holding it the failure is total and silent: the screen says "hold a button on
the controller for player 1", they hold it, nothing happens, and no log
anywhere says why. Every other pad tested claims a seat in under a second.

GOTG cannot work around it. Reading the pad ourselves to notice the hold would
be a second implementation of padmap's one job, racing the daemon for the same
device.

## What was checked

Against the real controller (`28de:1304` on `/dev/hidraw2`), a session opened
with only the Puck attached now logs:

```
/dev/hidraw2: controller paired
session open: 1 pad(s), 4 slot(s), 0 already assigned
session pad 0 (hidraw2) first read          <- the line that was missing
```

The read happens 3ms after the session opens. The guess in the section above
was right about the cause: `triton::Source::open` sends the lizard-off feature
report *at open*, before the descriptor is ever handed to the reactor, so the
pad is already streaming by the time anything waits on it. The Python sent it
from inside the read that was waiting for the pad to speak.

Two things were added rather than only re-tested:

* **The missing log line is now permanent.** `Session::read` logs the first
  read from each pad. A pad that is never readable is never read, and that
  failure is otherwise entirely silent -- which is what made this cost a day.
* **A test that needs no Steam Controller.**
  `tests/triton_protocol.rs::a_press_on_a_puck_claims_a_seat` drives the whole
  chain a seat depends on -- a real `triton::Source` over a pty, its decoded
  events, and the same `Assigner` the daemon's session feeds -- and asserts
  that holding A claims player 1 while an untouched pad claims nothing. It
  protects the path on every machine rather than only on one with a Puck
  plugged in.

Every button the Triton decoder emits is `BTN_SOUTH` or above, so all of them
clear the `BTN_FIRST` floor that makes the assigner ignore keyboard codes --
which was the next link worth doubting and is now covered by that test.
