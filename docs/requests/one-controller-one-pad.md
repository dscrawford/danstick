# One physical controller should be one pad

> **Done.** The dedupe is in discovery and `list` says what it dropped. The
> live measurement is still owed; see "What was built" at the end.

**What happens now.** With Steam running, a Steam-launched front-end sees the
Steam Controller twice: once as the pad padmap drives itself through the
puck (`28de:1304`, the triton path in `padmap-input/src/triton.rs`), and once
as the **Steam Virtual Gamepad** (`28de:11ff`) that the Steam client
publishes for any application it launches while Steam Input is handling a
controller on its behalf. That second node is a real evdev joystick with
`ID_INPUT_JOYSTICK=1`, so `pad::discover` (`padmap-input/src/pad.rs`, the
udev walk) accepts it like any other pad. padmap knows the id -- it is
`icons::STEAM_VIRTUAL_ID` in `padmap-core/src/icons.rs` -- but only to
choose an icon. Nothing excludes it: the only device filter in discovery is
`PADMAP_ONLY_DEVICE`, which is include-only and meant for tests.

Your own note flagged this as the thing to check before shipping
(`docs/STEAM-CONTROLLER-SDL3.md`, "Coexistence with Steam running").

**Why it matters more now.** The assigner keys holds by pad index and gives
each index its own seat. One physical press arrives on both nodes, so one
person claims player 1 *and* player 2. Once ambient seating is on in the
picker (which GOTG is about to do, per [always-seating](always-seating.md)),
that happens silently on the first button anybody holds.

**What would be enough.** In discovery, after the udev walk and the triton
slots are unioned:

- when a `28de:11ff` node is present **and any other pad is present**, drop
  the `11ff` nodes. Steam's virtual gamepad is a mirror of a pad padmap can
  read directly, whichever pad that is -- an Xbox pad under Steam Input gets
  one too;
- when `11ff` nodes are the **only** pads, keep them. A controller Steam alone
  can drive is still a controller.

A pure function beside `ambiguous_groups` in `pad.rs`, testable without
hardware, is the shape; a fixture for the virtual gamepad beside `XBOX_360`
in `fakepad.rs` is enough to test it.

And say so: `padmap list` should show the dropped node as dropped, with the
reason, rather than have it vanish. The whole cost of this bug was that it
was silent.

**Why this is padmap's and not ours.** The dedupe has to happen where pads
are discovered, before seats are handed out; a front-end filtering its own
view would still let the daemon seat one person twice.

**What GOTG has done on its side.** The picker's Steam launcher and the
client both clear `SDL_GAMECONTROLLER_IGNORE_DEVICES` and its `_EXCEPT`
variant, which Steam sets for the same reason it creates the virtual pad and
which otherwise hid padmap's clones from SDL in the game.

**What we could not yet measure.** The exact pairing on a live machine. With
Steam running but nothing launched from it, no `11ff` node exists at all,
and the puck reports "nothing paired" while the controller is off. A
recording with the controller on and the picker started from Steam --
`padmap list --json`, `/dev/input/by-id`, and the HID driver per node -- is
the one input this request still wants, and GOTG will supply it.

## What was built

`pad::discover` now drops Steam's virtual gamepad when the pad it mirrors is
present, and keeps it when it stands alone.

* **`without_steam_mirrors`** in `padmap-input/src/pad.rs` is the pure function
  asked for, beside `ambiguous_groups`: given the pads, it returns those to
  offer and those left out with a reason. When any non-`11ff` pad is present it
  drops every `28de:11ff` node; when the mirrors are the only pads it keeps
  them. Tested without hardware, including the case of a puck slot beside the
  mirror.
* **`discover_all`** returns both the pads and what was dropped;
  `discover` is unchanged for callers that only want the pads. The daemon's
  discovery is `discover`, so a mirror is never seated.
* **`fakepad::STEAM_VIRTUAL`** is the fixture, beside `XBOX_360`: Steam's id
  over the Xbox 360 button table with a slot index appended to the name, which
  is how it presents. `Fixture::pad(event)` turns any fixture into a
  discovered `Pad` for the test.
* **Said, not vanished.** `padmap list` prints a "Left out, on purpose:"
  section naming each dropped node and why. `list --json` appends the dropped
  node as an entry with `"player": null`, `"virtual": null`, and
  `"dropped": "<reason>"`, so a launcher counting controllers filters on
  `.dropped == null` and a person still sees what went where.

The dedupe is in discovery, before seats are handed out, so the daemon cannot
seat one person twice whatever the front-end shows.

### Still owed

The live measurement. With the controller off and nothing paired, no `11ff`
node exists to record, so the fixture is built from the id and the Xbox table
rather than from a capture. The `STEAM_VIRTUAL` source string says so. A
recording with the controller on and the picker started from Steam --
`list --json`, `/dev/input/by-id`, the HID driver per node -- would replace the
constructed fixture with a measured one; the dedupe logic would not change.
