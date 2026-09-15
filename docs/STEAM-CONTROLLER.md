# The 2026 Steam Controller

Status: **works, without the kernel driver.** padmap speaks the protocol
directly. Written 2026-09-15 against kernel 6.18.44.

An earlier version of this document concluded "blocked on the kernel, upgrade
to Linux 7.3". That was wrong, and it was wrong in a way worth recording: the
thing I called undocumented had been published for ten months.
`docs/STEAM-CONTROLLER-SDL3.md` is the correction, written by someone who had
the controller working while I was explaining why it could not be.

## The device

`28de:1304`, seven USB interfaces, a **four-slot wireless receiver** rather
than a controller:

    0-1  CDC-ACM, internal comms, not HID
    2-5  four wireless slots  -> hidraw7..10
         05 01 09 02      mouse,    report 0x40   |
         05 01 09 06      keyboard, report 0x41   | lizard mode
         06 00 ff 09 01   usage FF000001, reports 0x42/0x43/0x45/0x47/0x79
    6    pogo-pin dock     -> hidraw11
         06 00 ff 09 02   usage FF000002, sends nothing

Every slot boots in "lizard mode", pretending to be a keyboard and a mouse so
it works in a BIOS. The real gamepad state is on the vendor collection and
stays silent until something asks for it.

## What padmap does

Drives it. Both halves: `src/padmap/triton.py` for the daemon, and
`rust/crates/padmap-input/src/triton.rs` for `padmap-rs`. It arrives in
`padmap list` as an ordinary controller, with no note, because there is
nothing left to say about it.

`padmap-rs` wraps the two kinds of source in one enum, so the clone, the
calibration and the forwarding loop cannot tell a Steam Controller from a pad
the kernel drives — which is the point. A controller needing a workaround
should still be a controller.

### The protocol

`src/padmap/triton.py`, a port of SDL's `SDL_hidapi_steam_triton.c` (zlib,
upstream 2025-11-12) and its two headers. Three things in it are worth knowing
before changing anything:

**Leaving lizard mode is a feature report**, sent with `HIDIOCSFEATURE`:

    01 87 03 09 00 00   then 58 zeros   (64 bytes)

report id 1, `ID_SET_SETTINGS_VALUES`, one packed `ControllerSetting`,
`SETTING_LIZARD_MODE`, `LIZARD_MODE_OFF`. The Switch Pro path next door writes
its mode command with `os.write`, which is an *output* report and silently
wrong here -- the ioctl is a different channel.

**It has to be re-sent every three seconds, forever.** The controller reverts
on its own. Sent once, a pad works and then turns back into a keyboard
mid-game, which reads as failing hardware.

**An empty slot stalls the transfer with `EPIPE`**, and that is the only cheap
way to tell a slot with a controller in it from one without. Without the
probe, padmap finds four controllers for one physical pad and offers four
players, three of which never send an event.

## Two things that do not work the way the rest of padmap does

**There is no evdev node.** `devices.discover()` starts from
`/sys/class/input`, and with no `hid-steam` there is no joypad there to find
-- only a mouse and a keyboard per slot. So Triton pads are *synthesised* and
appended to the scan, with the hidraw node as their `path`. They are marked
`retroarch_visible=False`, truthfully: nothing else on the machine can see
them, which also keeps them out of RetroArch's pad-index arithmetic where they
would shift every other player by one.

**A controller waking up changes nothing in `/dev/input`.** The receiver's
hidraw nodes exist from the moment it is plugged in and never change. The
daemon's arrival poll therefore folds `triton.live_signature()` into its
change detection -- four ioctls, on the same throttle as the directory
listing.

## Kernel 7.3

Still the better long-term answer, and no longer a prerequisite. `hid-ids.h`
gained `IBEX`/`IBEX_BLE`/`PROTEUS`/`NEREID` in v7.3-rc1 and has them in no
earlier tag (v7.0, v7.1, v7.2 and v6.18 all checked). When it lands, the
kernel will drive the receiver and publish ordinary joypads.

padmap gets out of the way when that happens: `triton.slots()` skips any
device whose HID driver is something other than `hid-generic`, so the kernel's
driver wins and the two never fight over the same reports.

## What is verified, and what is not

Verified here:

* the lizard-mode packet, byte for byte against SDL's source and its headers;
* the state report's field offsets, against `TritonMTUNoQuat_t` under
  `#pragma pack(1)`;
* slot discovery against the real receiver -- four slots found, the dock
  correctly excluded;
* the empty-slot probe: with nothing paired, all four stall and padmap
  reports no controllers rather than four phantom ones;
* the decode, against synthetic reports -- `tests/check_triton.py`.

**Not verified: a live controller.** Nothing was paired to this receiver while
this was written, so no state report has been decoded from real hardware. The
byte layout is read off the source and tested against constructed reports.
What remains unproven is the wire, not the arithmetic.

Also untested: four controllers at once, rumble, the trackpads, and
coexistence with Steam running. Steam holds all five hidraw nodes while it is
up; hidraw allows that, but whether both drivers fighting over lizard mode
produces something sensible is unknown.

## If it does not work

    nix develop --command python3 tests/check_triton.py   # the decode
    padmap list                                           # what padmap sees

`padmap list` showing nothing with a controller switched on means the probe
found no live slot: the pad is asleep, or paired to a different receiver. A
slot that appears but sends no events means the decode is wrong, and
`tools/hidprobe.py` against that slot's node is the next step -- it prints
every report with the bytes that changed.
