# What padmap adds to a controller, measured

Run it yourself:

    nix develop --command python3 tools/latency.py -n 1500 --hz 125 --quiet

A uinput device stands in for the controller, padmap grabs and republishes it,
and the harness times each frame from the write that injected it to the read
that saw it come back off the clone. That delta is the whole of what padmap
adds: one kernel delivery, the republisher's read, its decision, its write, and
one more kernel delivery. It does not include the USB or Bluetooth polling
interval, which no userspace program can change and which is far larger than
anything below.

## Baseline: the Python republisher

`python3 -m padmap.cli run`, 1500 frames at 125Hz, idle desktop,
Linux 6.18.44, CPython 3.14.7, python-evdev.

    1500 of 1500 events arrived

         p50     0.067 ms
         p90     0.114 ms
         p99     0.407 ms
       p99.9    94.697 ms
         max   102.759 ms
        mean     0.550 ms

        <0.05ms     235  ###########
         <0.1ms    1025  ################################################
         <0.2ms     205  #########
         <0.5ms      21
           <1ms       0
           <2ms       1
           <5ms       0
          <10ms       1
          <20ms       1
          <50ms       4
         <100ms       6
        >=100ms       1

    later than one 8ms frame: 12  (0.80%)
    over 50ms:  7
    over 100ms: 1

## Reading it

**The median is not the problem and never was.** 67 microseconds against an
8000 microsecond frame is 0.8% of the budget. A port that halved it would move
nothing anybody could feel, and reporting a median is the one way to make this
measurement say nothing at all.

**The tail is the problem.** p99.9 is 94ms and the maximum is 103ms -- three
orders of magnitude above the median, on an otherwise idle machine, with one
pad attached. Twelve frames in fifteen hundred arrived later than the frame
that should have replaced them. That is the shape of the original report:

> there's a lag on the controllers... doesn't seem to work great when entering
> two inputs... it feels more like it's just arriving at a very slow rate

and it matches what FINDINGS.md measured from the other end -- gaps of 104,
128, 128, 160 and 176ms in a stream whose median gap was 7.94ms.

**The cause is known.** `devices.discover()` runs `udevadm info` as a
subprocess per input device -- 32 of them on this machine, 257-294ms -- from
the same thread that forwards events. It is rate limited to once a second and
short-circuited on an unchanged `/dev/input` listing, which is why the stalls
here are occasional rather than constant, but the scan still runs whenever
anything about the device set changes, mid-game, on the input path.

## The Rust republisher, measured the same way

`rust/target/release/padmap-rs run`, same harness, same machine, same session,
three rounds of each interleaved so drift cannot favour one side.

    1500 of 1500 events arrived

         p50     0.028 ms
         p90     0.041 ms
         p99     0.094 ms
       p99.9     0.126 ms
         max     0.128 ms
        mean     0.031 ms

        <0.05ms    1432  ################################################
         <0.1ms      59  #
         <0.2ms       9
           ...         0   (every bucket above 0.2ms is empty)

    later than one 8ms frame: 0  (0.00%)
    over 50ms:  0
    over 100ms: 0

Side by side, worst round of each:

| | Python | Rust |
| --- | --- | --- |
| p50 | 0.067 ms | 0.028 ms |
| p99 | 160.6 ms | 0.094 ms |
| p99.9 | 248.6 ms | 0.126 ms |
| max | **256.7 ms** | **0.137 ms** |
| frames later than one 8ms frame | 12-32 of 1200-1500 | **0** |
| frames over 100ms | 1-20 | **0** |

The median moved by 39 microseconds, which nobody can feel and which is not the
point. **The maximum moved by three orders of magnitude, and the count of
frames that arrived after the frame that should have replaced them went to
zero.** That is the defect the original report described, and it is gone.

## Why, specifically

Not "Rust is faster". Three structural changes, each of which removes a way for
something expensive to land on the forwarding path:

* **udev is read through libudev, not through 32 subprocesses.** The Python's
  `devices.discover()` runs `udevadm info` once per input device -- 257-294ms
  on this machine -- and that is the measured cause of the tail. Rust asks the
  same database in-process.
* **The 20ms tick is a `timerfd` in the same epoll set.** The Python called
  `_tick()` after every return from the selector, so at seven pads it ran about
  1200 times a second rather than 50. Everything in it was cheap or time-gated,
  so it was survivable -- but it is how a quarter-second scan ended up on the
  input thread in the first place, and a tick that is a descriptor cannot do
  that.
* **A frame is one `write(2)`.** python-evdev performs an `fcntl` before every
  single event write, so the Python issued two syscalls per event; this issues
  one per frame, whole, up to and including its `SYN_REPORT`.

The first of those is the one that matters. It is also, honestly, an
*algorithmic* fix rather than a language one -- the same change could be made
in Python by reading `/run/udev/data` directly. What the port buys on top is
that the expensive thing is no longer reachable from the loop by accident.

## What this measurement does not cover

* **The USB or Bluetooth polling interval.** A 125Hz pad samples every 8ms and
  averages 4ms of delay before padmap sees anything; Linux's default BLE
  connection interval is 30-50ms. Both are far larger than anything above and
  neither is padmap's to fix. If a pad feels laggy over Bluetooth, that is why.
* **Calibration.** The Rust republisher forwards axes verbatim, because the
  profile store is still Python's. A pad that does not centre itself will read
  deflected under `padmap-rs run` and correctly under `padmap run`.
* **hidraw pads.** Switch-family controllers are read over `/dev/hidraw*` by
  the Python and not yet by the Rust, which will fall back to an evdev node
  that carries nothing.
