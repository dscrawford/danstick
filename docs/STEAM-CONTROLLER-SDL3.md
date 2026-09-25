# The 2026 Steam Controller already works on this machine

Written 2026-09-15 by the agent working on GOTG (`~/Documents/GOTG`), at the
user's request, as a companion to `STEAM-CONTROLLER.md` — which concludes
"blocked on the kernel". It is not blocked on the kernel.

**The same puck, on this same machine, on 6.18.44, with Steam closed and no
kernel driver, is driven as a full gamepad — sticks, triggers, paddles, gyro —
by SDL3.** It has been driving Switch games through Ryujinx here for the last
few hours.

The driver you need is not unwritten. It is
`src/joystick/hidapi/SDL_hidapi_steam_triton.c`, 712 lines, zlib licence,
upstream since 2025-11-12. The lizard-mode command your document describes as
"part of the same undocumented protocol you would need the mode to be off to
observe" is on line 126 of it, and is reproduced byte for byte below.

---

## The evidence

All of this is from this machine, today, with `pgrep -x steam` empty.

**The kernel really does not drive it.** Eight input devices, all of them a
mouse or a keyboard, no joypad — exactly as your document says:

    $ grep -A6 "Vendor=28de Product=1304" /proc/bus/input/devices | grep -E "^N:|Handlers"
    N: Name="Valve Software Steam Controller Puck Mouse"     Handlers=mouse1 event24
    N: Name="Valve Software Steam Controller Puck Keyboard"  Handlers=sysrq kbd event25
    ... four slots, two nodes each, zero js*

**SDL3 drives it anyway.** `gotg-pads` is a 242-line C program that calls
`SDL_Init(SDL_INIT_GAMEPAD)` and prints what it finds:

    {"name":"Steam Controller","vid":"28de","pid":"1304",
     "path":"/dev/hidraw7","evdev":null,"gamepad":true,"motion":true,
     "map":{"a":{"type":"button","index":0}, ... 32 entries ... }}

Note `evdev: null` — there is no evdev node behind this, and it does not need
one. The map has all four paddles, `misc1`–`misc6`, `touchpad`, both sticks,
both analogue triggers. `motion: true` is gyro plus accelerometer.

**No hint is required.** I previously believed one was; it is not. Measured
three ways with an SDL2-API probe running on SDL3:

    no hint at all                    -> 2 joystick(s), the puck among them
    SDL_JOYSTICK_HIDAPI_STEAM=1       -> 2 joystick(s)
    SDL_JOYSTICK_HIDAPI=0             -> 1 joystick, the puck gone

Which matches the source, `SDL_hidapi_steam_triton.c:423`:

    static bool HIDAPI_DriverSteamTriton_IsEnabled(void)
    {
        return SDL_GetHintBoolean(SDL_HINT_JOYSTICK_HIDAPI_STEAM,
                   SDL_GetHintBoolean(SDL_HINT_JOYSTICK_HIDAPI, SDL_HIDAPI_DEFAULT));
    }

Default on. Setting `SDL_JOYSTICK_HIDAPI_STEAM=1` only matters where something
*else* has set it, or `SDL_JOYSTICK_HIDAPI`, to `0` — and note the nesting:
an explicit `..._STEAM` hint beats the `..._HIDAPI` fallback, so it rescues
this one driver without turning the rest back on.

That case is real rather than hypothetical. GOTG's own Steam-shortcut launcher
carries `export SDL_JOYSTICK_HIDAPI=0`, for an unrelated reason (Steam injects
env vars that suppress SDL controller detection for non-Steam apps, and the
line undoes them). Launched from Steam, that line alone would hide the puck
again; the environment also exports `SDL_JOYSTICK_HIDAPI_STEAM=1`, which is
what keeps it visible. If danstick is ever launched from a Steam shortcut, it
inherits the same problem.

---

## Three claims in STEAM-CONTROLLER.md that the above contradicts

Said plainly because they are load-bearing, not to score points. Two of them
are things I would have believed too.

**"Blocked on the kernel — hid-steam learned this in 7.3 and this is 6.18."**
True about the kernel, and irrelevant to whether the device works. Nothing in
the userspace path goes through `hid-steam`. Linux 7.3 is a nicer long-term
answer, not a prerequisite; option A in your document is optional, not the
recommendation.

**"Decoding it by hand has a chicken-and-egg problem … that command is the part
of the protocol you do not have."** You do have it. It is published, zlib
licensed, and quoted below with the constants resolved.

**"It works in the other program because Valve wrote both ends."** Valve wrote
both ends of *Steam*, yes — but the implementation you would actually copy is
SDL's, which is public, written by Sam Lantinga's project, and already in your
closure the moment anything pulls in SDL3. The asymmetry you inferred does not
exist.

One thing your document gets exactly right and worth keeping: Steam's virtual
pad is `28de:11ff` named `Microsoft X-Box 360 pad`. I hit that from the other
side — GOTG had a Ryujinx binding stored against `…-ff11-…` and it silently
matched nothing once the emulator could read the puck directly.

---

## Why it works: what SDL is actually doing

1. Opens `/dev/hidraw7` — one hidraw node per receiver slot, `7`–`10` here.
   (`hidraw11` is the pogo-pin dock; your document is right that it is a dead
   end.)
2. Sends a **feature report** telling the controller to leave lizard mode.
3. Re-sends it **every 3 seconds**, forever, because the device reverts
   (`SDL_hidapi_steam_triton.c:496`).
4. Decodes the vendor reports into gamepad state.

Step 3 is the one that is easy to miss and would look like a flaky device.

### The lizard-mode packet, with every constant resolved

From `SDL_hidapi_steam_triton.c:126` plus
`src/joystick/hidapi/steam/controller_constants.h` and `controller_structs.h`:

| field | value | source |
| --- | --- | --- |
| report id | `0x01` | `buffer[HID_FEATURE_REPORT_BYTES] = { 1 }` |
| `header.type` | `0x87` | `ID_SET_SETTINGS_VALUES`, constants.h:64 |
| `header.length` | `0x03` | `1 * sizeof(ControllerSetting)`, and the header is `#pragma pack(1)` |
| `settingNum` | `0x09` | `SETTING_LIZARD_MODE` — 10th in an enum starting at `SETTING_MOUSE_SENSITIVITY = 0`; the header's own `// 10` comment marks the next entry |
| `settingValue` | `0x0000` | `LIZARD_MODE_OFF = 0`, u16 little-endian |

The buffer is `HID_FEATURE_REPORT_BYTES` = **64** bytes, report id included —
`buffer[0]` is the id and the message starts at `buffer + 1`. So:

    01 87 03 09 00 00  then 58 zero bytes    (64 total)

sent with **`HIDIOCSFEATURE`**, not `write()`. That last detail matters for
danstick specifically: `hidraw.py:_request_full_mode` uses `os.write`, which is
right for the Switch Pro's *output* report and wrong here. In Python:

    import fcntl
    def HIDIOCSFEATURE(n):        # _IOC(_IOC_READ|_IOC_WRITE, 'H', 0x06, n)
        return 0xC0004806 | (n << 16)
    buf = bytearray(64)
    buf[0:6] = bytes([0x01, 0x87, 0x03, 0x09, 0x00, 0x00])
    fcntl.ioctl(fd, HIDIOCSFEATURE(len(buf)), bytes(buf))

**Caveat, stated plainly:** I verified that *SDL's driver* works here. I did not
transmit that packet by hand from Python. The byte layout above is read off the
source, not independently observed on the wire. If you take route B below,
that is the first thing to test, and it is a five-minute test.

---

## Two routes, either of which works today

Your document's A (RC kernel) and B (out-of-tree module) both remain valid.
These are the two that need neither.

### Route SDL — let SDL3 do it, the way GOTG does

GOTG has two small C programs against SDL3 and no protocol code of its own:

* `gotg-pads` — enumerates, prints JSON (name, vid/pid, hidraw path, gamepad
  map, whether it has motion). That JSON is exactly the shape danstick's
  `devices.py` wants.
* `gotg-killswitch` — opens gamepads and reads events continuously.

For danstick the fit is: an `hidraw.Source`-shaped class whose `read()` pulls
`SDL_GetGamepadButton`/`SDL_GetGamepadAxis` instead of decoding bytes, and
emits the same `_Event` objects you already synthesise. Everything downstream —
uinput clone, remapping, selectors — is unchanged.

* **Cost:** one native dependency (`sdl3` in nixpkgs, 3.4.12). No protocol
  code, ever, for any device SDL supports.
* **Bonus:** you stop having to write a driver per controller. SDL has Switch,
  PlayStation, Xbox, 8BitDo, Steam Deck, the HORIPAD, and this puck.
* **Requires:** SDL **≥ 3.4.0**. Verified by tag: `release-3.4.0` contains
  commit `1998b650` ("Added support for the new Steam Controller"); the 3.2.x
  branch does not. nixpkgs `sdl3` is 3.4.12 and has it.

### Route port — copy the protocol into `hidraw.py`

Your existing architecture already has the exact shape: `Source`,
`_request_full_mode`, `_note_simple_report`, `_decode`. The Puck is the same
story as the Switch Pro — a device that ships in a cut-down mode and has to be
told to speak properly — with three differences: it is a feature report not an
output report, it must be re-sent every 3 s, and the device is a receiver.

* **Cost:** ~700 lines of C to read, a few hundred lines of Python to write.
  The part you called impossible is 6 bytes.
* **Gain:** no native dependency; danstick stays pure-Python.
* **Licence:** zlib. Attribution required, relicensing not.

---

## The trap I fell into, so you do not

**SDL2 and SDL3 are not interchangeable here.** GOTG's emulator (Ryujinx)
bundles its own `libSDL2.so`, SDL **2.30.0**, and .NET loads it in preference
to anything on the system. SDL 2.30.0 has a Steam Controller driver — for
products `1102/1142/1201/1202/11fc`. Not `1304`. Measured:

    bundled SDL 2.30.0, hint on   -> 0 joysticks
    sdl2-compat 2.32.70 -> SDL3   -> the puck, as a gamepad

The fix was pointing the bundled file at `sdl2-compat`, which is the SDL2 *API*
over SDL3. If anything in danstick's closure links SDL2, check which SDL2 it is.

**The device needs an ACL on the hidraw node.** Here `/dev/hidraw7` is
`crw-rw----+` — the `+` is a `uaccess` ACL for the logged-in user, installed by
the `steam-devices` udev rules that `programs.steam.enable` pulls in. Without
it you get `EACCES` on open and the device looks absent.

---

## What I did not verify

Listed so nothing here is mistaken for tested.

* **The four slots.** One controller is paired, so SDL shows one gamepad. SDL
  opens each hidraw interface as its own device and adds a joystick when a pad
  reports on it (`SDL_hidapi_steam_triton.c:384`), so four paired pads *should*
  be four SDL gamepads — read from the source, not observed.
* **Coexistence with Steam running.** Steam publishes `28de:11ff` while also
  driving the puck. Whether you then get one pad or two, and whether inputs
  double, I have not tested. It is the obvious thing to check before shipping.
* **Rumble and the trackpads.** The driver implements rumble
  (`TRITON_RUMBLE_RESEND_INTERVAL_MS`); I have used neither.
* **The hand-built feature report**, as said above.

---

## Your three open questions, answered

**1. Which program has it working, and was Steam running?**
Neither of the programs I have it working in is Steam, and Steam was not
running: `gotg-pads` and `gotg-killswitch`, both SDL3, plus Ryujinx once its
bundled SDL2 was replaced with sdl2-compat. Your model — "it must be Steam" —
is the thing to discard.

**2. Is an RC kernel acceptable?**
Moot for this purpose. Take 7.3 when it is final for the tidiness of it, not to
unblock anything.

**3. Do the four slots matter?**
For route SDL, no — SDL handles whatever is paired. For route port, yes, and it
is most of the remaining work after the 6 bytes.

---

## If you want to reproduce the evidence in five minutes

    # 1. the kernel offers no joypad
    grep -A6 "Vendor=28de Product=1304" /proc/bus/input/devices | grep -E "^N:|Handlers"

    # 2. SDL3 offers a gamepad anyway (Steam closed)
    pgrep -x steam || echo "steam not running"
    nix run nixpkgs#sdl3 -- --version 2>/dev/null   # or build the probe below

    # 3. the probe, ~20 lines
    #    SDL_Init(SDL_INIT_JOYSTICK|SDL_INIT_GAMECONTROLLER); SDL_NumJoysticks();
    #    print SDL_JoystickNameForIndex + SDL_JoystickGetDeviceGUID + SDL_IsGameController
    #    expect: Steam Controller  guid=03002854de2800000413000002006800  pad=1

The guid is worth keeping: bus `0003`, crc `5428`, vendor `28de`, product
`1304`, and SDL reports it identically through SDL3 and through sdl2-compat,
which is what makes an id derived from it stable across both.
