#!/usr/bin/env python3
"""Dump one evdev node's whole declaration as JSON: ids, name, uniq, phys,
capability bits and every absinfo.

This is how the fakepad fixtures get their numbers. `/proc/bus/input/devices`
gives the bitmaps but not the ranges, and a fixture that guesses a range is
a fixture that passes for the wrong reason -- so this asks the kernel with
EVIOCGBIT and EVIOCGABS and prints what it says.

Stdlib only, so it runs on a Steam Deck or anything else without a nix shell.

    python3 tools/deckprobe.py              # every node
    python3 tools/deckprobe.py "Steam Deck" # nodes whose name contains this

A Steam Deck publishes its pad only while nothing holds the hidraw node, and
Steam holds it; see docs/STEAM-DECK.md.
"""
import fcntl, json, struct, sys, glob, os

def _ioc(d, t, nr, size): return (d << 30) | (size << 16) | (t << 8) | nr
E = 0x45
READ = 2

def s(fd, nr, n=256):
    buf = bytearray(n)
    try: fcntl.ioctl(fd, _ioc(READ, E, nr, n), buf)
    except OSError: return ""
    return buf.split(b"\x00")[0].decode("utf-8", "replace")

def bits(fd, ev, n=128):
    buf = bytearray(n)
    try: fcntl.ioctl(fd, _ioc(READ, E, 0x20 + ev, n), buf)
    except OSError: return []
    out = []
    for i, byte in enumerate(buf):
        for b in range(8):
            if byte & (1 << b): out.append(i * 8 + b)
    return out

def absinfo(fd, code):
    buf = bytearray(24)
    try: fcntl.ioctl(fd, _ioc(READ, E, 0x40 + code, 24), buf)
    except OSError: return None
    v, mn, mx, fuzz, flat, res = struct.unpack("6i", bytes(buf))
    return {"value": v, "min": mn, "max": mx, "fuzz": fuzz, "flat": flat, "res": res}

def dump(path):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    try:
        idbuf = bytearray(8)
        try: fcntl.ioctl(fd, _ioc(READ, E, 0x02, 8), idbuf)
        except OSError: pass
        bus, vid, pid, ver = struct.unpack("4H", bytes(idbuf))
        props = bytearray(4)
        try: fcntl.ioctl(fd, _ioc(READ, E, 0x09, 4), props)
        except OSError: pass
        prop = [i * 8 + b for i, x in enumerate(props) for b in range(8) if x & (1 << b)]
        return {
            "devnode": path,
            "name": s(fd, 0x06), "phys": s(fd, 0x07), "uniq": s(fd, 0x08),
            "bustype": bus, "vendor": vid, "product": pid, "version": ver,
            "props": prop,
            "ev": bits(fd, 0x00, 4),
            "keys": bits(fd, 0x01, 96),
            "abs": {c: absinfo(fd, c) for c in bits(fd, 0x03, 8)},
            "rel": bits(fd, 0x02, 4),
            "msc": bits(fd, 0x04, 4),
            "ff":  bits(fd, 0x15, 16),
        }
    finally:
        os.close(fd)

want = sys.argv[1] if len(sys.argv) > 1 else None
out = []
for p in sorted(glob.glob("/dev/input/event*"), key=lambda p: int(p[16:])):
    try: d = dump(p)
    except OSError as e: continue
    if want and want.lower() not in d["name"].lower(): continue
    out.append(d)
print(json.dumps(out, indent=2))
