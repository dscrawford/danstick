# Replacing the Python

13,599 lines across 23 modules. The goal is none of them, and a `padmap`
binary with no interpreter behind it.

This is a plan, not a schedule. What matters is the *method* and the *order*;
both are chosen so that a half-finished port is never a broken program.

## The method, which already works

`tools/gen_corpus.py` calls the real Python functions and records every answer
under `rust/crates/padmap-core/tests/corpus/` — 23 files today.
`differential.rs` replays them. From its own header:

> A hand-written expectation encodes what the porter *believed* the Python
> did, and that belief is the thing most likely to be wrong.

That is not a theory. Two corpus files exist because the languages disagree
silently — Python's `round` is ties-to-even where Rust's rounds away from
zero, and `//` floors where `/` truncates — and both are on the per-event
path. A unit test written from reading the Python would have encoded the bug.

So, per module, in this order:

1. **Record.** Add a `gen_corpus.py` section that calls the Python over every
   input shape that matters, including the ugly ones. This is the step that
   must not be rushed: the corpus is the specification.
2. **Fail.** Write the Rust signature, return `todo!()`, watch the differential
   test fail against real recorded answers.
3. **Implement** until it agrees.
4. **Decide the disagreements.** A failure is not automatically a Rust bug. It
   is a divergence, and the question is which side is right. Where the Python
   was wrong, move the case to an explicit test that says so.
5. **Delete the Python, when it has no callers left.** See below: that is
   later than this list makes it sound.

### Porting goes bottom-up; deleting goes top-down

The first version of this plan said "delete the Python module and its suites
in the same commit that replaces it", and phase 1 proved that impossible on
the first try. `protocol.py` has no *outgoing* dependencies, which is what
makes it a leaf and a good place to start — but it has ten *incoming* ones,
and every one of them is still Python. A leaf by import order is the last
thing that can be removed, not the first.

So the two directions are opposite, and both are right:

* **Port** bottom-up, because a module cannot be written before what it
  depends on.
* **Delete** top-down, because a module cannot be removed before what depends
  on *it*.

Which means Python shrinks late, and mostly at once, when `cli` and `server`
flip. That is worth knowing in advance rather than discovering at phase 5: the
intermediate state is both implementations installed, which is exactly the
state the differential corpus exists to police. The rule that survives is the
narrower one — **a test suite is deleted in the same commit as the module it
tests, never before.**

### What a corpus cannot record

Sockets, device grabs, uinput, epoll. For those the pattern is the one
`tests/triton_protocol.rs` and `tests/real_devices.rs` established: make a
real thing. A uinput device is a real joypad the kernel publishes; a
`/dev/ptmx` pair is a real character device that `open_hidraw`'s `fstat`
accepts. Both work unprivileged, and both skip rather than fail where the
device node is unavailable.

## Where it stands

Seven modules have a Rust counterpart, and **none of them has replaced its
Python**: the daemon still calls the Python, so both run. Line counts say how
finished each is.

| module | python | rust | state |
|---|---:|---:|---|
| `triton` | 686 | 712 | complete, both live |
| `hide` | 293 | 318 | complete |
| `assign` | 286 | 312 | complete |
| `icons` | 178 | 336 | complete |
| `capture` | 777 | 1286 | complete |
| `retroarch` | 891 | 300 | **partial** — config writing, `clean_user_config`, launch args missing |
| `profiles` | 585 | 196 | **partial** — reads profiles, does not write them |

## The order

From the import graph. Leaves first, so nothing is ported before what it
depends on.

**Phase 1 — the leaves** (1,951 lines, no internal dependencies)

| module | lines | note |
|---|---:|---|
| `safeio` | 47 | **nothing to port** — see below |
| `protocol` | 455 | the wire format. Corpus: `encode`, `LineReader.feed` |
| `mapping` | 578 | `sdl_guid`, `stick_fields`, `rests_centred` — mostly corpused already |
| `layouts` | 621 | already loaded by `padmap-core/src/layout.rs`; finish and delete |
| `titles` | 250 | a regex pass over 43MB of XML; a build-time product |

`safeio` is the odd one. All 47 lines exist because Python's
`Path.read_text()` raises `UnicodeDecodeError` on a bad byte — a `ValueError`,
which `except OSError` misses — and that hole was found in six separate
places. Rust has no such trap: `String::from_utf8_lossy(&fs::read(path)?)` is
the whole of it, and it is already what `triton.rs` and `lizard.rs` do. There
is no `safeio.rs` to write. The module dies with its last Python caller and
contributes no Rust.

**Phase 2 — devices** (404 lines). The root of everything else. Rust
`pad::discover` already does most of it; the gap is `_ask_udev_about`'s
batching and the capability sniffing. Corpus: a recorded sysfs tree.

**Phase 3 — the device layer** (1,474 lines): `hidraw`, `virtual`,
`calibrate`. `clone.rs` and `republish.rs` already cover most of `virtual`.
Needs uinput and pty fixtures, not a corpus.

**Phase 4 — the writers** (1,594 lines): finish `retroarch` and `profiles`,
then `controllercfg`, then `announce`. These emit the files RetroArch and SDL
read, so the corpus is exact file content — `emitted_files` already does this
for some of it.

**Phase 5 — the daemon** (2,370 lines). The big one, and the reason this order
exists: everything above is a dependency of it. Not corpusable; needs a real
socket and a real client. `tests/check_daemon_*.py` describe the behaviour to
preserve, and they are the specification to port, not to delete early.

**Phase 6 — the front door** (1,162 lines): `cli`, then `launch`, then
`artwork`. `artwork` is last on purpose — see below.

## Three things that need a decision, not a port

**`pysdl2`.** Used for exactly one thing, in a subprocess: asking SDL what
mapping it already has for a GUID (`controllercfg.py:399`). Three ways out —
bundle `gamecontrollerdb.txt` and read it directly, link SDL3 and call it, or
drop the last-resort lookup. Bundling is probably right, and it is a
decision about behaviour rather than a translation.

**`artwork`, 593 lines of HTTP** against `thumbnails.libretro.com`. Porting it
means adding an HTTP client and a TLS stack to a program that currently links
`udev` and nothing else. It is also the module furthest from padmap's purpose:
it fetches box art. A reasonable answer is to leave it out of the binary
entirely and ship it as a separate tool, which is not the same as porting it.

**`fakepad`, 771 lines.** Test scaffolding, not runtime. It should grow a Rust
half rather than be replaced — the fixtures are data, and both languages
should read the same ones. See `docs/FAKEPAD.md`.

## What "done" means

Per module: the Python file is gone, its suites in `tests/` are gone, the
corpus section that recorded it is gone, and `nix develop` no longer needs
that import. Per project: `pythonEnv` drops out of `flake.nix`, and
`packages.default` points at a Rust binary.

Until then both implementations are live and both test suites earn their
place — see the note at the end of `tests/README.md`.

## The first commit

`safeio` and `protocol`. Together 502 lines, no dependencies, and `protocol`
has 83 assertions describing the wire format already. It is the smallest
change that exercises every step of the method above, including the one that
matters: deleting Python in the same commit that replaces it.
