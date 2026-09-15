# Porting padmap to Rust

What is done, what is not, and why the order is what it is.

## Why

Not latency. That was the starting assumption and the measurement refuted it:
in steady state the Python republisher adds 0.046 ms at the median and 0.265 ms
at the worst, against an 8 ms frame, with zero late frames. The Rust is about
twice as good on both, which is 0.013 ms against 0.03 ms and is not something
anybody can feel. See [LATENCY.md](LATENCY.md), including the correction --
an earlier version of this document claimed a 250 ms tail that turned out to be
the harness timing `padmap run`'s startup.

The one real stall found was at startup, in Python, and is now fixed there:
`devices.discover()` cost 596 ms -- 33 `udevadm` spawns, and an open/close of
every input node whose release costs 11 ms apiece -- and `cmd_run` called it
twice after creating the clone and before entering its loop, so presses in that
window queued and arrived in a burst. One batched `udevadm` call plus the sysfs
capability bitmaps brings it to 15.9 ms and the burst is gone. The Rust
discovery path reads libudev in-process and never had it.

The reason that survives is testability, and it is not a small one. The Python's
logic is exercised by 62 scripts under `tools/`, every one of which needs a
machine with controllers plugged into it. The same decisions are now reachable
from `cargo test`, and moving them there found four real bugs in a week-old
port and pinned a dozen shared quirks that nobody had written down.

## Shape

    padmap-core    vocabulary, layouts, mappings, scopes, calibration,
                   profiles, the wizard state machines, wire framing. No I/O,
                   no clock, no unsafe, no Linux.
    padmap-input   evdev, uinput, udev, the profile store. The only crate that
                   opens a device.
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
* **The profile store**, reading and writing the same files the Python does --
  same directory, same filenames, same JSON including the two keys written only
  so a rollback still finds the controller mapped.
  `tools/check_rust_calibration.py` writes a profile with the *Python* and
  checks every value of an axis through the *Rust* republisher against the
  Python's own `AxisCalibration.apply`.

## Not done, and what each one costs

| | cost of the gap |
| --- | --- |
| hidraw (Switch family) | those pads fall back to an evdev node that carries nothing, so they do nothing |
| the daemon socket, sessions, the wizard | `padmap-rs` cannot be driven by the front-end; use `padmap serve` |
| `padmap hide`, `export-pegasus`, `fetch-art`, `clean-config` | still Python, and should stay that way -- see below |

Order to continue in: hidraw decoding (the report decoders are pure functions
over byte slices, so they test from a recording), then the socket protocol,
then the session state machine.

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
  just reopens. Ported against `unicode-general-category` and checked on 2,117
  codepoints from the corpus, not approximated.
* **A stored axis is `min`/`max` on disk**, not `minimum`/`maximum`. The Rust
  struct serialises as the file does, because the file is older than the port.
* **`str(raw.get(...))` stringifies anything**, so the Python writes `None`
  back as the literal "None". Deliberately not reproduced -- see the
  `divergences` module in `profile.rs`.
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
