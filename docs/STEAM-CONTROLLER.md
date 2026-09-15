# The 2026 Steam Controller, and why padmap cannot see it

Status: **blocked on the kernel.** Nothing in padmap is broken. Written
2026-09-15 against kernel 6.18.44.

You said you have it working in another program. That is almost certainly
Steam, and the reason it works there is the whole of this document: Steam is
not using the kernel driver at all, and it does not have padmap's problem.

---

## The device

`28de:1304`, seven USB interfaces, serial `FXB996050187C`:

    0-1  CDC-ACM, internal comms, not HID
    2-5  four wireless slots  -> hidraw7..10
         05 01 09 02      mouse,    report 0x40   |
         05 01 09 06      keyboard, report 0x41   | lizard mode
         06 00 ff 09 01   usage FF000001, reports 0x42/0x43/0x44/0x45/0x79/0x7b
    6    pogo-pin dock     -> hidraw11
         06 00 ff 09 02   usage FF000002, 54 bytes, stripped down

Two facts that are easy to miss and that shaped everything below.

**It is a receiver, not a controller.** Mainline calls it "Steam Controller
(2026) Puck". Up to four pads connect *through* it. Right now the kernel
publishes eight input devices for it -- a Mouse and a Keyboard per slot -- and
zero joypads.

**"Lizard mode" is the default, not a fault.** Valve controllers boot
pretending to be a keyboard and a mouse so they work in a BIOS and a login
screen. The real gamepad state lives on the vendor-defined collection and stays
silent until something sends the command to leave that mode. That command is
part of the same undocumented protocol you would need the mode to be off to
observe.

---

## Why it works in the other program

Steam speaks the Valve protocol **in userspace, over hidraw**, and then
publishes a virtual pad through uinput. That virtual pad is `28de:11ff`, named
`Microsoft X-Box 360 pad` -- which is why your very first report on this was
"it's listed as xbox 360". It was never an Xbox pad; it was Steam impersonating
one because every game already has a mapping for that.

So the other program:

* needs no kernel driver, because it brought its own protocol implementation;
* did not have to reverse-engineer anything, because Valve wrote both ends;
* publishes one pad and is done -- it does not care about stable identity
  across replugs, player order, or coexisting with four other controllers.

Close Steam and the userspace driver goes away, and the device reverts to being
a keyboard and a mouse. That is the "if steam is closed it stops acting like"
half of your report, exactly.

---

## Why it is hard here

Not because padmap is doing anything unusual. The order of the obstacles:

1. **The kernel driver exists but is newer than the kernel.** `hid-steam`
   gained this model in **Linux 7.3**; this machine runs 6.18.44. Verified in
   `hid-ids.h`: `PROTEUS` (= `0x1304`) is present at v7.3-rc1 and absent at
   v7.2, v7.1, v7.0 and v6.18.

2. **Force-binding the old driver does not work.** I said it did. It does not.
   v6.18's `steam_raw_event` opens

       if (size != 64 || data[0] != 1 || data[1] != 0)
               return 0;

   and every report this model sends is 54 bytes or fewer under its own report
   id. The bind succeeds and every report is dropped in silence, which is worse
   than not binding, because something now looks attached.

3. **Decoding it by hand has a chicken-and-egg problem.** The usual method --
   `tools/hidprobe.py`, press one control, watch which byte moves -- needs the
   device to be sending. It will not send on the vendor interface until it is
   commanded out of lizard mode, and that command is the part of the protocol
   you do not have. This is not like the Switch Pro, where the pad talks first
   and the mode command only improves the data.

4. **A receiver multiplies the work.** Four slots, each needing to be
   associated with whichever physical controller is paired into it, plus the
   pairing and battery reports, plus the dock to ignore.

None of this is padmap-specific except (4). Anything that wants this device as
a *controller*, rather than as a keyboard, hits (1)-(3) identically.

---

## Options

### A. Kernel 7.3  — recommended

`nixos-unstable` has `linuxPackages_testing` at **7.3-rc2**, which contains the
driver.

```nix
boot.kernelPackages = pkgs.linuxPackages_testing;
```

* **Cost:** running a release-candidate kernel until 7.3 is final.
* **Result:** the Puck presents ordinary evdev pads. padmap needs **no new
  code** -- it works the same day, through the same path as every other pad,
  and it works with Steam closed.
* **Risk:** an RC kernel is an RC kernel. `linuxPackages_latest` is 7.2.5 and
  does *not* have it, so there is no non-RC option from nixpkgs today.

### B. Out-of-tree module on the current kernel

Build mainline `hid-steam.c` against 6.18 via `boot.extraModulePackages`. A
DKMS packaging of exactly this exists (`JSmithRobotics/dkms-hid-steam`), though
it is one day old with no users, so treat it as a starting point rather than a
dependency.

* **Cost:** a small Nix derivation, and it breaks whenever the HID API moves.
* **Result:** same as A, on a stable kernel.
* **Unverified:** whether mainline's `hid-steam.c` compiles unmodified against
  6.18 headers. That is the first thing to test if A is unacceptable.

### C. Implement the protocol in padmap

`src/padmap/hidraw.py` already has the shape this needs -- `Source`,
`_request_full_mode`, `_decode` -- because the Switch Pro required the same
pattern of "command it out of its simple mode, then decode".

* **Cost:** high, and the risky part is not the decode but obtaining the
  leave-lizard-mode command. Realistically that means reading someone else's
  implementation rather than discovering it.
* **Result:** works on any kernel; duplicates a driver that already exists and
  that will arrive on this machine anyway.
* **Recommendation:** do not, unless A and B are both ruled out. This is
  reimplementing, in a gamepad remapper, a driver that shipped upstream three
  weeks ago.

---

## What padmap does in the meantime

`padmap-rs list` diagnoses it rather than reporting "No joypads found":

    Valve Software Steam Controller Puck (28de:1304)
    is a four-slot wireless receiver for 2026 Steam Controllers.
    ...
    hid-steam learned 28DE:1304 in Linux 7.3; this kernel is 6.18.
    Upgrade the kernel, or build the upstream module out of tree.

Once the kernel drives it, that message disappears on its own -- the check is
`has_joypad()`, so a working device is never reported.

---

## What I got wrong

Recorded because two of these were advice you acted on.

1. **"Force-bind it with `new_id`."** Wrong; see obstacle 2. Given twice.
2. **The sysfs path in that command.** I computed
   `/sys/bus/hid/drivers/steam` by stripping `hid-` from the module name. The
   directory is `hid-steam`. Through `sudo` a missing directory fails as a
   *permission* error, so it reads as the kernel refusing the write.
3. **"Its gamepad channel is `/dev/hidraw11`."** That is the pogo-pin dock --
   stripped down, and the one interface on the device guaranteed to send
   nothing. I then aimed `hidprobe.py` at it and reported that nothing arrived,
   which reads exactly like a controller asleep. The slots are hidraw7-10.

Root cause common to all three: deriving a fact that was available to be read.
Full write-up in `FINDINGS.md`.

---

## Open questions for you

1. **Which program has it working, and was Steam running at the time?** If it
   was not Steam, my model is wrong and I want to know immediately -- something
   else is speaking this protocol and I should read it.
2. **Is an RC kernel acceptable on this machine?** That decides A vs B.
3. **Do the four slots matter, or is one controller the whole use case?**
   Affects nothing for A or B; affects the cost of C considerably.

## Resolution

*To be filled in.*
