# A clone should be able to look like an Xbox 360 pad

**What GOTG wants.** A third identity for the clone, beside `mirror` and
`padmap`: the wired Xbox 360 controller, `045e:028e` on USB, version `0x0110`,
with the layout the `xpad` driver gives it -- so every SDL program on the
machine maps it from the database it was built with, draws Xbox prompts for
it, and never needs `SDL_GAMECONTROLLERCONFIG`, a mapping file, or a
controller screen. Steam Input does exactly this for every game it launches
("Microsoft X-Box 360 pad N"), and it is why controllers "just work" there.

**Why.** The decompiled ports. Donkey Kong 64 Recompiled is SDL2 with its
own `recompcontrollerdb.txt` and its own idea of a controller; under padmap
with a Steam Controller seated it does not behave. `mirror` gives the clone
`28de:1304`, which is in no database -- the port can only know that pad from
the environment `padmap-rs exec` hands it, and only if it reads it before
loading its own file. The same clone wearing `045e:028e` is a pad every SDL
since 2.0 knows by heart. GOTG's own QA harness chose that identity for its
fake pad for the same reason: "the one GUID every SDL build maps out of the
box".

**What would be enough.** `PADMAP_PAD_IDENTITY=xbox360`:

- **Identity.** vendor `0x045e`, product `0x028e`, bus `0x03`, version
  `0x0110`. The version matters: SDL's GUID carries it, and the database
  entry is `030000005e0400008e02000010010000`. The player number goes
  somewhere else -- `phys` (`padmap/pN`) is already there -- not in the
  version word as `mirror` and `padmap` do, or the GUID misses and the
  whole point is lost. Two clones with one GUID are fine: SDL tells
  instances apart by index, ares by its slot, RetroArch by name.
- **Name.** Still `padmap Player N`. The name is what padmap's own
  `is_padmap_clone` tests, and what GOTG's picker tests through the CRC SDL
  puts in the GUID; SDL renames the device from the database for display
  anyway, so nothing is lost by keeping it.
- **Layout, fixed, whatever the source is.** Keys `BTN_SOUTH EAST NORTH WEST
  TL TR SELECT START MODE THUMBL THUMBR`; axes `ABS_X/Y/RX/RY` at
  `-32768..32767` (fuzz 16, flat 128), `ABS_Z/RZ` at `0..255`, `ABS_HAT0X/Y`
  at `-1..1` -- what `xpad` advertises. The source's inputs are translated
  into those positions through the profile padmap already has for it
  (control -> source binding), the way the SDL mapping string is already
  derived from it. A control the source lacks is simply never pressed.
- **Everything else as it is.** Grab, seating, the hold, `unseat`, `emit`,
  the launch config -- unchanged. `mirror` stays the default until this has
  been lived with; GOTG would set `xbox360` for the environments that need
  it first (the recomp ports), and everywhere once it is boring.

**What GOTG would do with it.** Export `PADMAP_PAD_IDENTITY=xbox360` from the
launcher for the ports that want it, and later for all. Nothing else changes:
the picker's filter matches the CRC of the name, the ares writer matches the
GUID padmap publishes, and both would go on doing so.
