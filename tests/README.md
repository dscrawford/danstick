# padmap's tests

    nix develop --command python3 tests/run.py              # everything
    nix develop --command python3 tests/run.py --coverage   # with a number
    nix develop --command python3 tests/run.py --e2e        # plus the slow ones

## Where they are, and why they are not all here

| | count | location | why there |
|---|---|---|---|
| Python suites | 53 files, 380 assertions | `tests/check_*.py` | here |
| End-to-end | 3 files | `tests/e2e_*.py` | here; start a daemon, so opt-in |
| Rust unit | 301 | `rust/crates/*/src/**.rs` | cargo requires unit tests beside the code |
| Rust integration | 480 | `rust/crates/*/tests/` | cargo requires these in the crate |

Only the Python half was a choice. Cargo will not collect `#[test]` from a
directory outside the crate, so the Rust tests cannot move here; `tests/run.py`
runs them where they are instead, which is the part that actually matters — one
command, everything.

## Why the Python suites are not pytest

Each is a standalone program with a `check(name, condition, detail)` helper
that prints a line per assertion and exits non-zero. That is deliberate. These
tests open real device nodes, create real uinput devices, bind real sockets and
stand up a real daemon; a framework that owns process lifetime, captures
stdout and reorders tests got in the way more than it helped. The cost is no
fixtures and no parametrisation, which for suites this shape has not been felt.

`tests/run.py` gives each suite a fresh `XDG_RUNTIME_DIR`, because several bind
a socket or write a daemon marker and sharing one lets an earlier suite's
leftovers decide a later one's result.

## Coverage

Measured, not estimated:

    nix develop --command python3 tests/run.py --coverage

**89.9% of regions, 89.5% of lines** across the Rust workspace. `padmap-core`
— every pure decision padmap makes — is 94–100% on every file.

The remainder is not evenly spread, and it is worth knowing what it is:

| file | cover | what is uncovered |
|---|---|---|
| `padmap-rs/src/main.rs` | 0% | the binary: argument dispatch, printing, the run loop |
| `clone.rs` | 53% | force-feedback proxying, and the uinput retry path |
| `republish.rs` | 70% | feedback, and the paused-drain branch |
| `reactor.rs` | 81% | epoll error paths |

`main.rs` is the single biggest block and the most misleading: it is 455
regions of `println!` and `match args.next()`. Covering it means moving its
logic into the library, which is worth doing for its own sake and is the next
step — not writing a test that asserts a program printed something.

## Adding a test

Copy the shape of an existing suite. A new `tests/check_*.py` is picked up by
`run.py` automatically; a new Rust test goes beside the code it tests, or in
the crate's `tests/` if it needs the crate's public API only.

`tests/real_devices.rs` in `padmap-input` is the pattern for anything needing
hardware: it creates a uinput device, and skips rather than fails where
`/dev/uinput` is not writable — a sandboxed build has no business failing over
a device node it was never given.
