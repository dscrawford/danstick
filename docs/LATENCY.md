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

## What the port has to beat

Not the median. These three:

| number | Python | target |
| --- | --- | --- |
| p99.9 | 94.7 ms | under one frame |
| max | 102.8 ms | under one frame |
| frames later than 8ms | 12 of 1500 | 0 |

The mechanism is not "Rust is faster". It is that the Rust daemon reads udev's
database directly instead of spawning a subprocess per device, and that the
20ms tick lives on a timerfd in the same epoll set rather than running once per
event -- so there is no quarter-second of work that *can* land on the
forwarding path.

## Honest accounting of what Rust does not buy

Per-event cost is a fraction of a microsecond of interpreter time. Measured
against a 125Hz pad the language is worth roughly 12 microseconds of the 8000
in a frame. If the tail were already flat, this port would be indefensible on
latency grounds and would have to be argued on the other things it buys --
which are real, but are not lag.
