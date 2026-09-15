# Porting padmap to Rust

What is done, what is not, and why the order is what it is.

## Why

Input lag, and one measurement. `tools/latency.py` times a frame from the write
that injects it to the read that sees it come back off the clone:

| | Python | Rust |
| --- | --- | --- |
| p50 | 0.067 ms | 0.028 ms |
| max | 256.7 ms | 0.137 ms |
| frames later than one 8ms frame | 12-32 of 1200 | 0 |

The median is not the reason. 39 microseconds against an 8000 microsecond frame
is nothing anybody can feel, and a port argued on that number would be
indefensible. The reason is the maximum: a quarter of a second, on an idle
machine, with one pad attached. See [LATENCY.md](LATENCY.md).

The second reason is testability, and it is not a smaller one. The Python's
logic is exercised by 62 scripts under `tools/`, every one of which needs a
machine with controllers plugged into it. The same decisions are now reachable
from `cargo test`, and moving them there found four real bugs in a week-old
port and pinned a dozen shared quirks that nobody had written down.

## Shape

    padmap-core    vocabulary, layouts, mappings, scopes, calibration, the
                   wizard state machines, wire framing. No I/O, no clock, no
                   unsafe, no Linux. 627 tests.
    padmap-input   evdev, uinput, udev. The only crate that opens a device.
    padmap-rs      a binary: `list` and `run`.

The cut line is the unix socket, not a language boundary inside one process.
The Pegasus front-end is a socket client and knows nothing about which daemon
it is talking to, so a Rust daemon that binds the same path is a drop-in and a
rollback is starting the Python one. Both read the same `assignments.json`.

This is why there is no PyO3 anywhere. A Rust core called from a Python loop
would leave the loop, the selector, the tick and the once-a-second scan in
Python -- which is all of what was actually wrong -- and would buy a
dependency whose API has broken seven times in nineteen months.

## Done

* **`padmap-core`**, entire. Control vocabulary as an enum rather than a bare
  string, so the SDL and RetroArch tables are exhaustive by construction.
  Layouts moved out of code into `data/layouts/*.json`: a new console is a file
  and a manifest line, which is the generalisation this port was for.
* **A differential corpus.** `tools/gen_corpus.py` calls the real Python and
  records 704 answers plus 2,700 swept axis readings; `tests/differential.rs`
  replays them. A hand-written expectation encodes what the porter believed the
  Python did, which is the thing most likely to be wrong.
* **A latency harness** and a recorded baseline, taken before any Rust ran.
* **The forwarding path**: discovery through libudev, epoll with the tick on a
  timerfd, frame-batched writes, force-feedback proxying, calibration applied
  in transit.

## Not done, and what each one costs

| | cost of the gap |
| --- | --- |
| calibration is not read from the profile store | a pad that does not centre itself reads deflected under `padmap-rs run` |
| hidraw (Switch family) | those pads fall back to an evdev node that carries nothing, so they do nothing |
| the daemon socket, sessions, the wizard | `padmap-rs` cannot be driven by the front-end; use `padmap serve` |
| `padmap hide`, `export-pegasus`, `fetch-art`, `clean-config` | still Python, and should stay that way -- see below |

Order to continue in: the profile store (pure logic plus two file reads, and it
unblocks calibration), then hidraw decoding (the report decoders are pure
functions over byte slices, so they test from a recording), then the socket
protocol, then the session state machine.

## What should not be ported

* **`retroarch.clean_user_config`.** It round-trips the user's `retroarch.cfg`
  through `errors="surrogateescape"` so a latin-1 ROM path comes back
  byte-identical, and splits lines on `\n` only because `str.splitlines()`
  breaks on characters RetroArch's `fgets` does not. Rust has no
  surrogateescape; a correct port must be byte-oriented throughout. It is a
  one-shot maintenance command that rewrites a file the user owns. Zero
  benefit, maximal blast radius.
* **`artwork.py`**, **`titles.py`**, **`pegasus.py`**. Offline, run once,
  failure already tolerated by design. `titles.py` runs at Nix build time.
* **`ui/`**. PySide6 and QML, and a fallback for a front-end that is itself the
  real interface. A candidate for deletion, not for porting.
* **`tools/`**, 43,000 lines. Not ported -- repurposed. They are the oracle in
  `gen_corpus.py` and they keep testing whatever stays Python.

## Things that bite

* **Python's `round` is ties-to-even; Rust's `f64::round` is away from zero.**
  On the per-event axis rescale. Pinned by the corpus.
* **`//` floors; `/` truncates.** Same function, on any range that straddles
  zero. Pinned.
* **`f64::clamp` panics when min > max** where Python's `max(a, min(b, x))`
  quietly answers `a`. A profile can hold an inverted range, and this is the
  per-event path.
* **`pathlib.Path(...).name` strips a trailing separator** and `rsplit('/')`
  does not. Bit twice, in `game_key` and in `for_core`.
* **`str.isprintable()` is a Unicode-category test** and its result is a
  profile's *filename*. Getting it wrong orphans every existing profile
  silently, because a missing profile means "never configured" and the wizard
  just reopens. Not yet ported; do this one carefully.
* **evdev 0.13.2 mis-encodes `UI_SET_PHYS`.** `libc::c_char` where the kernel
  header says `char*`; the size is part of the ioctl number, so the kernel
  answers EINVAL. Worked around by matching clones on their name as well as
  their phys, on both sides.
* **rustix reads a `&mut Vec`'s *length* as epoll's `maxevents`,** not its
  capacity, so a cleared Vec asks for zero events and gets EINVAL. Use a fixed
  buffer.

## Running it

    nix develop                        # cargo, clippy, rustfmt, evemu, perf
    cd rust && cargo test
    cd rust && cargo clippy --all-targets -- -D warnings
    nix build .#checks.x86_64-linux.rust        # the suite, in the sandbox
    nix build .#checks.x86_64-linux.rust-lint   # fmt and clippy, in the sandbox
    nix run .#padmap-rs -- list

    python3 tools/gen_corpus.py        # after any deliberate Python change
    python3 tools/latency.py --command '.../padmap-rs run'
