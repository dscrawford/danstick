# Fake pads

Every controller bug padmap has had came from some controller behaving unlike
the one in front of the person writing the code. `padmap_input::fakepad` is
those surprises, written down and executable.

```rust
use padmap_input::fakepad::{self, MAYFLASH_GAMECUBE};

let trigger = MAYFLASH_GAMECUBE.axis("lt").expect("it has one");
assert_eq!(trigger.rest, 24);   // untouched, and 81% deflected
```

Each fixture carries a `source` saying where its numbers came from — a line in
`xpad.c`, a measured absinfo, a table in SDL's driver. That is not decoration:
a fixture nobody can check is a guess with a struct around it, and a wrong
fixture is worse than none, because it makes a test that passes for the wrong
reason.

## What is in there, and why each one

| fixture | why it earns its place |
| --- | --- |
| `XBOX_360` | the unremarkable pad everything else is compared against: sticks centred at zero, triggers from zero, a hat d-pad. Anything that only works here is assuming this shape |
| `XBOX_SERIES_X` | same driver, triggers 0..1023 instead of 0..255, and one control that is a `KEY_` rather than a `BTN_` |
| `MAYFLASH_GAMECUBE` | the adapter that broke three things at once |

The GameCube adapter is the one that pays for the framework. Everything
awkward about it is real and measured:

* **the triggers are `ABS_RX` and `ABS_RY`**, not the `ABS_Z`/`ABS_RZ` the
  names suggest — code that assumed which codes a trigger lives on found
  nothing at all;
* **they rest at 24 and 25 of 0-255**, 81% deflected while untouched, which
  broke capture (it recorded the direction the axis was moving *away* from),
  answered prompts nothing had touched, and left re-arming waiting for a
  return to centre that a trigger never makes;
* **no axis rests at zero**, so half-axis binds cannot work both ways;
* it is driven by hid-generic, so it **emits `MSC_SCAN`** before every key,
  which xpad does not.

That last one is why `emits_scan` exists. A capture that mistook the scancode
for the press would work on an Xbox pad and fail on everything hid-generic
drives — and the two pads are in here together so that difference is testable
rather than anecdotal.

## The two that are not here

A Steam Controller and a Switch Pro have no evdev node to build: padmap reads
them over hidraw and decodes their reports itself. Their fixtures are the
report bytes, in `padmap-input/tests/triton_protocol.rs` and
`nintendo_differential.rs` — the same idea, one layer down.

## Running them

    nix develop --command python3 tests/run.py

Some facts about the fixtures are checked when the crate *builds* rather than
when a test runs — which driver sends a scancode is a statement about
constants, and a `const` block puts the failure at the edit that broke it.
