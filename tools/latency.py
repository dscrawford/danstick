"""Measure what a republisher costs, end to end, against a synthetic pad.

    python3 tools/latency.py                      # the Python republisher
    python3 tools/latency.py --no-bridge          # the control: run this first
    python3 tools/latency.py --command '...'      # anything that republishes

A uinput device stands in for the controller, so the measurement needs no
hardware and no hands:

    synthetic pad --(danstick grabs and republishes)--> clone --> this process

Each frame carries a sequence number in MSC_SCAN -- the one event type the
input core neither deduplicates nor rewrites through the fuzz filter -- and is
timed from the write that injected it to the read that saw it return.

Three things this reports that a naive version does not, each because leaving
one out produced a confident wrong answer here:

  * **the kernel's own queueing time** on the clone, via EVIOCSCLOCKID, beside
    the time this process managed to read it. If those disagree, the harness is
    the slow one and none of it may be charged to danstick.
  * **which frames were late, by injection order.** A stall at startup and a
    stall in steady state are different defects and are indistinguishable in a
    percentile. This is what caught a 250ms "tail" that was entirely `danstick
    run` writing launch configs before entering its loop.
  * **--no-bridge**, the control. Whatever it reports is the floor the harness,
    the kernel and the scheduler impose with no danstick in the picture at all.

Report the tail, never the median: a median is the one number that cannot show
the defect. Needs /dev/uinput to be writable.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import fcntl
import select
import shlex
import struct
import signal
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

import evdev                        # noqa: E402
from evdev import AbsInfo, ecodes   # noqa: E402

# A name nothing else on the machine will have, and the one danstick is pointed
# at with DANSTICK_ONLY_DEVICE so an isolated run never fights the live daemon
# for a grab on the real controllers.
SOURCE_NAME = "danstick latency source"
CLONE_NAME = "danstick Player 1"

# The axis events travel on. ABS_X, because it is an axis every consumer
# treats as a stick and because a full sweep of it is what a player actually
# does -- a button gives two distinguishable values and an axis gives 256.
AXIS = ecodes.ABS_X
AXIS_MIN, AXIS_MAX = 0, 255

# How long to wait for the republisher to notice the pad and publish a clone.
CLONE_TIMEOUT = 20.0

# EVIOCSCLOCKID: _IOW('E', 0xa0, int). python-evdev does not wrap it.
#
# Without it an event's timestamp is CLOCK_REALTIME, which cannot be compared
# with time.monotonic() at all -- and wall time can jump backwards, so it
# cannot be compared with itself either. With it, the kernel stamps each event
# on the monotonic clock as it queues it, which is what separates "the
# republisher was slow" from "this harness was slow to read".
EVIOCSCLOCKID = 0x400445A0


def use_monotonic_timestamps(device) -> bool:
    try:
        fcntl.ioctl(device.fd, EVIOCSCLOCKID, struct.pack("i", time.CLOCK_MONOTONIC))
        return True
    except OSError:
        return False


def make_source() -> evdev.UInput:
    """A uinput device shaped like an ordinary USB gamepad.

    Enough capability bits that `devices._looks_like_joypad` accepts it: at
    least one absolute axis and at least one code in the BTN_JOYSTICK range.
    """
    capabilities = {
        ecodes.EV_KEY: [ecodes.BTN_SOUTH, ecodes.BTN_EAST, ecodes.BTN_START],
        # The sequence tag rides on MSC_SCAN, and a uinput clone can only emit
        # what its source declared: danstick copies the source's capabilities
        # verbatim, so an undeclared EV_MSC here means every tagged frame is
        # refused by the clone and nothing is ever matched.
        ecodes.EV_MSC: [ecodes.MSC_SCAN],
        ecodes.EV_ABS: [
            (AXIS, AbsInfo(value=(AXIS_MIN + AXIS_MAX) // 2, min=AXIS_MIN,
                           max=AXIS_MAX, fuzz=0, flat=0, resolution=0)),
            (ecodes.ABS_Y, AbsInfo(value=(AXIS_MIN + AXIS_MAX) // 2,
                                   min=AXIS_MIN, max=AXIS_MAX, fuzz=0, flat=0,
                                   resolution=0)),
        ],
    }
    return evdev.UInput(events=capabilities, name=SOURCE_NAME,
                        vendor=0xF055, product=0x0001, version=1)


def find_device(name: str, deadline: float) -> evdev.InputDevice | None:
    while time.monotonic() < deadline:
        for path in evdev.list_devices():
            try:
                device = evdev.InputDevice(path)
            except OSError:
                continue
            if device.name == name:
                return device
            device.close()
        time.sleep(0.05)
    return None


def write_assignment(pad_path: str, state_dir: Path) -> None:
    """The one file `danstick run` reads to know what to republish.

    Written directly rather than by driving `danstick setup`, which would need a
    button held for a quarter of a second on the synthetic pad and a second
    process to hold it. The format is cli._save_assignments'.
    """
    state_dir.mkdir(parents=True, exist_ok=True)
    (state_dir / "assignments.json").write_text(json.dumps([{
        "player": 1, "path": pad_path, "name": SOURCE_NAME,
        "phys": "", "vid": 0xF055, "pid": 0x0001,
    }], indent=2))


def percentile(sorted_values: list[float], fraction: float) -> float:
    if not sorted_values:
        return math.nan
    index = min(len(sorted_values) - 1,
                max(0, int(round(fraction * (len(sorted_values) - 1)))))
    return sorted_values[index]


def histogram(values_ms: list[float], width: int = 48) -> list[str]:
    """A log-ish histogram, because the interesting part is the tail."""
    edges = [0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0,
             math.inf]
    counts = [0] * len(edges)
    for value in values_ms:
        for index, edge in enumerate(edges):
            if value < edge:
                counts[index] += 1
                break
    peak = max(counts) or 1
    lines = []
    for edge, count in zip(edges, counts):
        label = f"<{edge:g}ms" if edge != math.inf else ">=100ms"
        bar = "#" * int(width * count / peak)
        lines.append(f"  {label:>9}  {count:6d}  {bar}")
    return lines


def report(deltas_ms: list[float], kernel_ms: list[float],
           write_cost_ms: list[float], injected: int, frame_ms: float) -> int:
    if not deltas_ms:
        print("No events arrived on the clone at all.")
        return 1
    ordered = sorted(deltas_ms)
    dropped = injected - len(deltas_ms)
    late = sum(1 for value in deltas_ms if value > frame_ms)

    print(f"\n{len(deltas_ms)} of {injected} events arrived"
          f"{f', {dropped} lost' if dropped else ''}")
    print("\nadded latency, source write -> clone read:")
    for label, fraction in (("p50", 0.50), ("p90", 0.90), ("p99", 0.99),
                            ("p99.9", 0.999)):
        print(f"  {label:>6}  {percentile(ordered, fraction):8.3f} ms")
    print(f"  {'max':>6}  {ordered[-1]:8.3f} ms")
    print(f"  {'mean':>6}  {sum(ordered) / len(ordered):8.3f} ms")

    print("\ndistribution:")
    for line in histogram(deltas_ms):
        print(line)

    print(f"\nlater than one {frame_ms:g}ms frame: {late}"
          f"  ({100.0 * late / len(deltas_ms):.2f}%)")
    # The number the port has to move. A median that looks healthy beside a
    # tail two orders of magnitude worse is exactly what was reported as "it
    # feels more like it's just arriving at a very slow rate".
    over_50 = sum(1 for value in deltas_ms if value > 50.0)
    over_100 = sum(1 for value in deltas_ms if value > 100.0)
    print(f"over 50ms:  {over_50}")
    print(f"over 100ms: {over_100}")

    if write_cost_ms:
        ordered_write = sorted(write_cost_ms)
        print("\ncost of the injection itself (charged to nobody):")
        print(f"  {'p50':>6}  {percentile(ordered_write, 0.50):8.3f} ms")
        print(f"  {'p99.9':>6}  {percentile(ordered_write, 0.999):8.3f} ms")
        print(f"  {'max':>6}  {ordered_write[-1]:8.3f} ms")

    if kernel_ms:
        # The same frames, to the moment the kernel queued them on the clone.
        # Anything the republisher is responsible for is in here; anything in
        # the difference between this and the figures above is this harness
        # failing to read promptly, and must not be charged to danstick.
        ordered_kernel = sorted(kernel_ms)
        print("\nsame frames, to the kernel's own queueing time on the clone:")
        for label, fraction in (("p50", 0.50), ("p99", 0.99), ("p99.9", 0.999)):
            print(f"  {label:>6}  {percentile(ordered_kernel, fraction):8.3f} ms")
        print(f"  {'max':>6}  {ordered_kernel[-1]:8.3f} ms")
        late = sum(1 for value in kernel_ms if value > frame_ms)
        print(f"  frames the republisher itself delivered late: {late}")
    if dropped:
        print("\nLost events are worse than late ones: a press that never "
              "arrives cannot be felt as lag, it is felt as a broken pad.")
    return 0


def run(args: argparse.Namespace) -> int:
    state_dir = Path(args.state_dir)
    source = make_source()
    # udev has to settle before anything can discover the node.
    time.sleep(args.settle)

    physical = find_device(SOURCE_NAME, time.monotonic() + 10.0)
    if physical is None:
        print("The synthetic pad did not appear. Is /dev/uinput writable?")
        return 1
    source_path = physical.path
    physical.close()
    print(f"synthetic pad at {source_path}")

    write_assignment(source_path, state_dir)

    environment = dict(os.environ)
    environment["DANSTICK_ONLY_DEVICE"] = SOURCE_NAME
    environment["XDG_RUNTIME_DIR"] = str(state_dir.parent)
    environment["PYTHONPATH"] = str(REPO / "src") + os.pathsep + environment.get(
        "PYTHONPATH", "")
    # Mirroring would read the source's ids off its node, which is fine, but
    # pinning it keeps two runs comparable when the harness changes.
    environment.setdefault("DANSTICK_PAD_IDENTITY", "danstick")

    if args.no_bridge:
        # No republisher at all. Every hop the measurement cannot avoid is
        # still here -- one uinput write, one kernel delivery, one read -- and
        # nothing danstick does is. A tail that shows up here is not danstick's.
        print("no bridge: reading the synthetic pad's own node")
        clone = find_device(SOURCE_NAME, time.monotonic() + 10.0)
        if clone is None:
            print("The synthetic pad vanished.")
            return 1
        return measure(source, clone, args)

    print(f"starting: {args.command}")
    child = subprocess.Popen(
        shlex.split(args.command), env=environment, cwd=str(REPO),
        stdout=subprocess.DEVNULL if args.quiet else None,
        stderr=subprocess.DEVNULL if args.quiet else None,
        start_new_session=True,
    )

    clone = None
    try:
        clone = find_device(CLONE_NAME, time.monotonic() + CLONE_TIMEOUT)
        if clone is None:
            print(f"No {CLONE_NAME!r} appeared within {CLONE_TIMEOUT:g}s.")
            return 1
        print(f"clone at {clone.path}")
        return measure(source, clone, args)
    finally:
        if clone is not None:
            clone.close()
        try:
            os.killpg(os.getpgid(child.pid), signal.SIGTERM)
        except (OSError, ProcessLookupError):
            pass
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
        source.close()


def measure(source, clone, args: argparse.Namespace) -> int:
    """Inject frames and time them, whatever is (or is not) in between."""
    # Let the republisher finish opening and grabbing before timing
    # anything: the first read of a fresh descriptor is not the steady
    # state, and the steady state is what a player experiences.
    time.sleep(args.warmup)
    # python-evdev has no set_blocking on this version, and read_one() on
    # a blocking descriptor would wait for an event that has not been sent
    # yet -- which is the measurement deadlocking on itself.
    os.set_blocking(clone.fd, False)
    monotonic_stamps = use_monotonic_timestamps(clone)
    if not monotonic_stamps:
        print("warning: could not switch the clone to CLOCK_MONOTONIC; "
              "only read-time latency will be reported")
    drained = 0
    while clone.read_one() is not None:
        drained += 1
    if drained:
        print(f"drained {drained} settling event(s)")

    interval = 1.0 / args.hz
    frame_ms = 1000.0 * interval
    # Each frame carries a sequence number in MSC_SCAN as well as the axis
    # move, so a delta is matched to the write that caused it rather than
    # to whichever write happens to be the same ordinal. EV_MSC is the one
    # type the input core forwards without deduplicating or rewriting it
    # -- EV_KEY is dropped unless the bit changes and EV_ABS is rewritten
    # through the fuzz filter -- and danstick forwards it (FORWARD_TYPES).
    sent_at: dict[int, float] = {}
    deltas: list[float] = []
    # The same frames, timed to when the *kernel* queued them on the clone
    # rather than to when this process managed to read them. The gap
    # between the two is this harness's own reader latency, and without
    # measuring it there is no way to tell it apart from the republisher's.
    kernel_deltas: list[float] = []
    # How long the injection syscalls themselves take. The send timestamp is
    # taken before them, so anything spent here lands in every delta as though
    # the republisher had caused it.
    write_cost: list[float] = []
    # Which frames were late, by injection order. A stall at startup and a
    # stall in steady state are different defects and look identical in a
    # percentile.
    late_positions: list[int] = []
    value = AXIS_MIN
    step = 1

    print(f"injecting {args.count} events at {args.hz}Hz "
          f"({frame_ms:.2f}ms apart)...")

    def drain(until: float) -> None:
        """Read whatever the clone has, waiting until `until` at the latest.

        Draining once per write is what the first version of this did, and
        it measured the wrong thing: an event written microseconds ago has
        usually not come back yet, so it was matched against the *next*
        iteration's read and every delta came out at exactly one injection
        interval. The number looked plausible, which is the dangerous part.
        """
        while True:
            remaining = until - time.monotonic()
            if remaining <= 0:
                return
            ready, _, _ = select.select([clone.fd], [], [], remaining)
            if not ready:
                return
            arrived = time.monotonic()
            while True:
                event = clone.read_one()
                if event is None:
                    break
                if event.type != ecodes.EV_MSC or event.code != ecodes.MSC_SCAN:
                    continue
                started = sent_at.pop(event.value, None)
                if started is not None:
                    late_by = 1000.0 * (arrived - started)
                    deltas.append(late_by)
                    if late_by > frame_ms:
                        late_positions.append(event.value)
                    if monotonic_stamps:
                        queued = event.sec + event.usec / 1_000_000.0
                        kernel_deltas.append(1000.0 * (queued - started))

    next_at = time.monotonic()
    for sequence in range(args.count):
        now = time.monotonic()
        if now < next_at:
            # Waiting for the next frame is also when the reply to the last
            # one arrives, so spend the wait reading rather than sleeping.
            drain(next_at)
        value += step
        if value >= AXIS_MAX or value <= AXIS_MIN:
            step = -step
        sent_at[sequence] = time.monotonic()
        source.write(ecodes.EV_MSC, ecodes.MSC_SCAN, sequence)
        source.write(ecodes.EV_ABS, AXIS, value)
        source.syn()
        write_cost.append(1000.0 * (time.monotonic() - sent_at[sequence]))
        next_at += interval

    # Anything still in flight. Generous, because the tail is the point:
    # a frame stalled behind a quarter-second scan has to be counted, not
    # discarded as missing.
    drain(time.monotonic() + 2.0)

    if late_positions:
        print(f"\nlate frames by injection order: first={min(late_positions)} "
              f"last={max(late_positions)} of {args.count}")
        print(f"  first twenty: {sorted(late_positions)[:20]}")
    return report(deltas, kernel_deltas, write_cost, args.count, frame_ms)

def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--command", default="python3 -m danstick.cli run",
        help="what to start as the republisher. Point it at the Rust daemon "
             "to compare.")
    parser.add_argument("--hz", type=float, default=125.0,
                        help="injection rate. 125 is a USB gamepad's default; "
                             "1000 is an overclocked adapter.")
    parser.add_argument("-n", "--count", type=int, default=2000)
    parser.add_argument(
        "--warmup", type=float, default=5.0,
        help="seconds to wait after the clone appears before timing anything. "
             "Five, not one, because `danstick run` creates the clone and then "
             "spends over a second writing launch configs before its loop "
             "starts -- a one-second warm-up times that startup gap and "
             "reports it as though it were steady-state latency. It did.")
    parser.add_argument("--settle", type=float, default=0.5,
                        help="seconds to let udev notice the synthetic pad")
    parser.add_argument(
        "--state-dir",
        default=str(Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp"))
                    / "danstick-latency" / "danstick"),
        help="an isolated XDG_RUNTIME_DIR/danstick, so this never disturbs a "
             "running daemon's assignments")
    parser.add_argument("--quiet", action="store_true",
                        help="hide the republisher's own output")
    parser.add_argument(
        "--no-bridge", action="store_true",
        help="start no republisher and read the synthetic pad's own node. The "
             "control: whatever this reports is the floor the harness, the "
             "kernel and the scheduler impose, and none of it is danstick's.")
    args = parser.parse_args(argv)

    if not os.access("/dev/uinput", os.W_OK):
        print("/dev/uinput is not writable by this user; nothing to measure.")
        return 1
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
