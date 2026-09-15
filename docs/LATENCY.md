# What padmap adds to a controller, measured

    nix develop --command python3 tools/latency.py -n 1200 --hz 125 --quiet
    nix develop --command python3 tools/latency.py --no-bridge          # control
    nix develop --command python3 tools/latency.py --command '.../padmap-rs run'

A uinput device stands in for the controller, padmap grabs and republishes it,
and each frame is timed from the write that injected it to the read that saw it
come back off the clone. Frames carry a sequence number in `MSC_SCAN`, which is
the one event type the input core neither deduplicates nor rewrites.

## The steady-state answer

1200 frames at 125Hz, idle desktop, Linux 6.18.44, one pad.

| | p50 | p99.9 | max | frames later than one 8ms frame |
| --- | --- | --- | --- | --- |
| no bridge (control) | 0.017 ms | 0.101 ms | 0.104 ms | 0 |
| `padmap run` (Python) | 0.046 ms | 0.240 ms | 0.265 ms | **0** |
| `padmap serve` (Python daemon) | 0.047 ms | 0.293 ms | 0.395 ms | **0** |
| `padmap-rs run` (Rust) | 0.030 ms | 0.113 ms | 0.135 ms | **0** |

**There is no steady-state latency defect in the Python, and the Rust port does
not fix one.** Every figure here is a fraction of a percent of an 8ms frame.
Rust is roughly 1.5x better at the median and 2x at the maximum, on numbers
nobody can perceive, and the honest summary of the port's latency benefit is
that it is real, small, and not why you would do it.

The control matters as much as the two arms: a bridge cannot be faster than
0.017 ms here, so the Python is adding about 0.03 ms and the Rust about 0.013.

## The defect that is real, and where it actually is

`devices.discover()`, measured directly on this machine:

    devices.discover():          596.5 ms   (33 input devices)
    retroarch.visible_order():   567.0 ms   (calls discover)
    retroarch.autoconfig_dirs(): 161.0 ms

596ms, because it runs `udevadm info` **as a subprocess per input device**.

`cli.cmd_run` calls `install_profiles`, `write_launch_config` and
`write_launch_args` *after* `_start()` has created the uinput clone and *before*
`republisher.run()` enters its loop. Two of those call `visible_order()`. So for
well over a second after the controller's clone appears, nothing is reading the
controller -- and every press in that window queues in the kernel and arrives in
a burst when the loop finally starts.

Measured: with a one-second warm-up, the first **27 frames of 1200** arrive late,
the worst by 220ms, and **not one frame after number 27 is late at all**. That is
the whole of the "tail" this document previously reported.

    late frames by injection order: first=0 last=26 of 1200

It is a real bug -- a player holding a direction while padmap starts gets a
quarter-second of nothing followed by a burst -- but it is a *startup* bug, and
it is not what "there's a lag on the controllers" during play describes.

## Correction

An earlier version of this file, and the commit message of the change that added
the Rust republisher, reported:

    p99.9  94.7 ms -> 0.126 ms
    max   256.7 ms -> 0.137 ms

and attributed the Python's tail to `devices.discover()` running on the
forwarding thread. **Both the comparison and the attribution were wrong.**

* The harness warmed up for one second, which landed inside `cmd_run`'s startup
  work. Every late frame was a queued startup frame. The Rust arm showed none
  because `padmap-rs` does not write launch configs -- a missing feature, not a
  faster loop.
* `padmap run` has no tick and never calls `devices.discover()` in its loop. The
  once-a-second scan is in `server.py`, and it is gated on a settled front-end
  *and* short-circuits on an unchanged `/dev/input` listing, so it does not run
  in steady state either. The tail FINDINGS.md measured was already fixed.

What caught it: reporting *which* frames were late by injection order, and a
`--no-bridge` control. Four instrumented runs of the republisher had already
shown that time inside `_forward` never exceeded 0.41ms, that no `select` ever
blocked more than 20ms with data waiting, and that the garbage collector ran 19
gen0 collections and no gen2 -- three negative results that should have been
enough to doubt the conclusion earlier than they were.

## How not to repeat it

The harness now defaults to a five-second warm-up, prints the injection-order
position of every late frame, and reports each frame twice: once to when this
process read it, and once to when the *kernel* queued it on the clone, using
`EVIOCSCLOCKID` to put those stamps on `CLOCK_MONOTONIC`. If those two disagree,
the harness is the slow one and nothing may be charged to padmap.

`--no-bridge` is the control. Whatever it reports is the floor that the
harness, the kernel and the scheduler impose. Run it first.

## What this measurement still does not cover

* **The USB or Bluetooth polling interval.** A 125Hz pad averages 4ms of delay
  before padmap sees anything; Linux's default BLE connection interval is
  30-50ms. Both dwarf everything above and neither is padmap's to fix.
* **A loaded machine.** Every figure here is from an idle desktop. The
  interesting question for Python is what the tail does under contention, and
  this has not been asked.
* **More than one pad**, which is what the original report described.
* **Calibration**, which the Rust republisher does not yet apply, and **hidraw**,
  which it does not yet speak.
