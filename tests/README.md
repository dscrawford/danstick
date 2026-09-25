# danstick's tests

    nix develop --command python3 tests/run.py              # everything
    nix develop --command python3 tests/run.py --coverage   # with a number
    nix develop --command python3 tests/run.py --lint       # plus fmt+clippy

Or, equivalently, `cd rust && cargo test`.

## Where they are

danstick is one Rust workspace and cargo decides where tests live: unit tests
beside the code they test, integration tests in each crate's `tests/`. There
is no separate suite directory, and `tests/run.py` collects nothing — it
exists so "how do I run the tests" has one answer that does not depend on
knowing where cargo wants to be invoked from.

| | where | what |
|---|---|---|
| unit | `rust/crates/*/src/**.rs` | the decisions, exhaustively |
| integration | `rust/crates/*/tests/` | whole files, real devices, a real daemon |
| corpus | `rust/crates/danstick-core/tests/corpus/` | recorded answers, replayed |

## The corpus

`tests/corpus/*.json` are answers the **Python** implementation gave, recorded
before it was deleted, and replayed by the `*_differential.rs` tests. They are
the reason the port can be trusted: a hand-written expectation encodes what
the porter *believed* the old code did, and that belief is the thing most
likely to be wrong.

They caught real divergences — Python's `round` is ties-to-even where Rust's
rounds away from zero, `//` floors where `/` truncates, `str(None)` is the
four characters `"None"`, and `title_for` read a trailing slash differently
from the two functions that must agree with it. None of those would have been
written into a unit test by hand.

The Python is gone, so the corpus is now frozen evidence rather than something
regenerable. A deliberate behaviour change means editing the recorded answer
and saying why in the commit.

## Tests that need real hardware

Several create a uinput device and drive it: `daemon_journey.rs` stands up a
real `danstick serve` and presses a pad at it. They **skip rather than fail**
where `/dev/uinput` is not writable, so the suite still runs on a machine that
cannot make one.

They are safe to run on a live machine by construction: their own runtime,
config and profile directories; `DANSTICK_ONLY_DEVICE` so no real controller is
ever grabbed; and the fixture's signature written into the live daemon's
`prompted` file first, because creating a joystick node is not a neutral act
while a daemon is watching for unfamiliar controllers.

## Fixtures

`danstick_input::fakepad` is real controllers written down: an Xbox 360 pad, an
Xbox Series X pad and a Mayflash GameCube adapter, each citing where its
numbers came from. The GameCube adapter is there because everything awkward
about it is real — triggers on `ABS_RX`/`ABS_RY` resting at 24 of 0-255, which
broke capture, re-arming and half-axis binds at once.

The two controllers danstick drives over hidraw have no evdev node to build, so
their report bytes are fixtures instead: `triton_protocol.rs` and
`nintendo_differential.rs`.

## Coverage

    nix develop --command python3 tests/run.py --coverage

`danstick-core` — every pure decision danstick makes — is the part worth holding
high, and is 94–100% per file.
