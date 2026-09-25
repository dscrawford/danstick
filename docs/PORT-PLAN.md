# Replacing the Python

**Done.** danstick was 13,747 lines of Python across 23 modules; it is now a
Rust workspace and a `danstick` binary with no interpreter behind it.

This file is kept for the method, which is the part worth reusing.

## The method

`tools/gen_corpus.py` called the real Python functions and recorded every
answer under `rust/crates/danstick-core/tests/corpus/`. `differential.rs` and
its siblings replay them. From the header of the first one:

> A hand-written expectation encodes what the porter *believed* the Python
> did, and that belief is the thing most likely to be wrong.

That was not a theory. The corpus caught, among others:

* Python's `round` is ties-to-even where Rust's rounds away from zero, and
  `//` floors where `/` truncates — both on the per-event path;
* `str(None)` is the four characters `"None"`, which was being written into a
  profile as though somebody had chosen an icon called None;
* `title_for` read a trailing slash differently from the two functions that
  have to agree with it, so a directory-shaped ROM got an empty title.

None of those would have been written into a hand-made test.

So, per module: **record** every input shape that matters, including the ugly
ones; **fail** against the recorded answers with a `todo!()`; **implement**
until it agrees; **decide the disagreements**, because a failure is a
divergence and the question is which side is right; and only then delete.

### Porting went bottom-up; deleting went top-down

The first version of this plan said "delete the Python module and its suites
in the same commit that replaces it", and phase 1 proved that impossible on
the first try. `protocol.py` had no *outgoing* dependencies, which is what
made it a good place to start — and ten *incoming* ones, every one of them
still Python. A leaf by import order is the last thing that can be removed,
not the first.

Which meant Python shrank late and mostly at once, when `cli` and `server`
flipped. Worth knowing in advance rather than discovering at phase 5: the
intermediate state is both implementations installed, which is exactly the
state the corpus exists to police.

### What a corpus cannot record

Sockets, device grabs, uinput, epoll. For those the pattern was: make a real
thing. A uinput device is a real joypad the kernel publishes; a `/dev/ptmx`
pair is a real character device that `open_hidraw`'s `fstat` accepts. Both
work unprivileged, and both skip rather than fail where the device node is
unavailable. `daemon_journey.rs` stands up a real daemon and presses a real
pad at it, and is what found the two bugs that would have hung it.

## What was removed rather than ported

* **`artwork` (593 lines)** and **`titles` (250)**. Box art is handled
  elsewhere; the MAME title table existed only to match art to set names and
  nothing else read it.
* **`ui/`**. PySide6 and QML, and a fallback for a front-end that is itself
  the real interface.
* **`safeio` (47 lines)**. All of it existed because Python's
  `Path.read_text()` raises `UnicodeDecodeError` on a bad byte — a
  `ValueError`, which `except OSError` misses — and that hole was found in six
  separate places. Rust has no such trap:
  `String::from_utf8_lossy(&fs::read(path)?)` is the whole of it. The module
  died with its last caller and contributed no Rust.

## What is left of the corpus

It is frozen evidence now: the Python that produced it is gone, so
`gen_corpus.py` went with it. A deliberate behaviour change means editing the
recorded answer and saying why in the commit — which is the right amount of
friction for changing something a user's stored profile depends on.
