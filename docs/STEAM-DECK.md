# The Steam Deck's own controls

Status: **recorded and tested, with two known disagreements.** Written
2026-09-21 against a Deck (Galileo) on SteamOS neptune `6.16.12-valve24.5`.

## The device

`28de:1205`, three HID interfaces on one USB device:

    3-3:1.0   05 01 09 02   mouse     -> hidraw0, "Valve Software Steam Controller"
    3-3:1.1   05 01 09 06   keyboard  -> hidraw1, "Valve Software Steam Controller"
    3-3:1.2   06 ff ff 09 01  vendor  -> hidraw3, the gamepad and the IMU

Like the Puck, it boots in lizard mode: the first two interfaces are a real
mouse and a real keyboard, so the thing works in a BIOS. Unlike the Puck, the
kernel drives it — `hid-steam` is bound to all three, and on the vendor
interface it publishes two nodes of its own:

    Steam Deck                 event, js  -- the pad
    Steam Deck Motion Sensors  event, js  -- accelerometer and gyro

## The thing that surprises everybody

**`hid-steam` withdraws the pad for anyone who opens the hidraw node**, and
Steam is such a client. So on a Deck as it normally runs, there is no
`Steam Deck` node at all: `danstick list` sees the lizard keyboard and mouse, and
Steam's virtual `28de:11ff` mirror standing in for the built-in controls.

    systemctl --user stop app-steam@autostart.service   # the node appears
    systemctl --user start app-steam@autostart.service  # and goes again

That is not a fault and there is nothing to fix in the kernel: it is how one
device gets driven by one thing at a time. It does mean the two representations
are never both present. So danstick keeps one Steam mirror standing in for the
Deck whenever a `28de:1205` hidraw node is present with no `Steam Deck` event
node, even beside another pad; see `pad::without_steam_mirrors`.

In Game Mode Steam also wraps every other pad it can -- an Xbox pad, and each
of danstick's 360 clones, which is every fixed slot. Seating watches all of
Steam's pads and tells them apart by timing (`danstick_core::echo`): the one
repeating a clone is nobody, the one repeating an Xbox pad is that pad and the
seat goes to it, and the Deck's, repeating nothing danstick reads, is the Deck.

## Why it is not an Xbox pad

Four things, all measured, all in `fakepad::STEAM_DECK`:

| | an Xbox pad | a Steam Deck |
| --- | --- | --- |
| d-pad | `ABS_HAT0X/Y`, -1..1 | `BTN_DPAD_UP..RIGHT`, four keys |
| `ABS_HAT0X/Y` | the d-pad | **the left trackpad**, -32767..32767 |
| triggers | `ABS_Z`/`ABS_RZ`, 0..255 | `ABS_HAT2Y`/`ABS_HAT2X`, 0..32767 |
| x and y | `BTN_WEST`/`BTN_NORTH` | **each other's codes** |

The last one is the one that bites. `hid-steam` writes `BTN_X` for the west
button, and `BTN_X` *is* `BTN_NORTH` — the legacy spelling of a positional
code. Anything that reads the codes positionally, danstick included, comes out
with X and Y the wrong way round. SDL does not, because it has an entry for
this GUID in its built-in database, and that entry says `x:b5,y:b6` where
danstick's guess says `y:b5,x:b6`.

`ABS_HAT0X/Y` was the other one that bit, and it is fixed: `guess.rs` now
refuses a hat that is not -1..1, so the d-pad comes from the keys and not from
the trackpad the player's left thumb rests on.

## What is tested

`rust/crates/danstick-input/tests/steam_deck.rs` holds the fixture against SDL's
own database line for the same GUID, and asserts:

* the two agree on every control but four, named in the test;
* the d-pad comes from keys, and a real hat still reads as a d-pad;
* the grips are SDL's four paddles, in the order the fixture names them;
* the triggers are the digital `BTN_TL2`/`BTN_TR2`, and why.

`fakepad.rs` holds the declaration itself: codes, ranges, fuzz.

Also corrected here: Steam's mirror (`STEAM_VIRTUAL`) was recorded from this
Deck at the same time. Its sticks stop at -32767, not xpad's -32768 — the
fixture used to claim it was byte-for-byte xpad's table, and it is not.

## What is not done

**The x/y swap is recorded, not corrected.** danstick's guess is wrong for this
model and the test pins that it is wrong. It is right where it matters: the
daemon's `fallback_line_for` asks SDL's database first
(`publish::carried`), and SDL knows this device. A shipped per-model override
would close the gap for the paths that guess directly; there is no such table
in danstick today.

**The triggers are digital.** SDL binds them to `a9`/`a8`, counting
`ABS_HAT2X/Y` among the axes because their range is not a hat's.
`binding::axis_index` excludes `0x10..0x18` outright, so danstick cannot spell
those indices at all and falls back to `BTN_TL2`/`BTN_TR2`. Fixing it means
teaching the axis numbering the same rule `guess.rs` just learned, and that
changes frozen corpus answers.

## Reproducing the capture

`tools/deckprobe.py` dumps one evdev node's whole declaration as JSON. Copy it
to the Deck, stop Steam, run it, start Steam:

    scp tools/deckprobe.py deck@<host>:/tmp/
    ssh deck@<host> 'systemctl --user stop app-steam@autostart.service &&
                     python3 /tmp/deckprobe.py "Steam Deck";
                     systemctl --user start app-steam@autostart.service'
