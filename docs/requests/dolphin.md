# Dolphin, as a config target

> **Done.** `--dolphin-dir`, writing both files. See "What was built" at the
> end -- including one thing padmap does that GOTG's version does not.


**What GOTG does.** GameCube and Wii are Dolphin, and they are a large part of
the library — Four Swords Adventures, Super Mario Sunshine, every GameCube
game. GOTG writes `GCPadNew.ini` for the ports a launch uses, plus the
`SIDevice` lines in `Dolphin.ini` that say each port holds a standard
controller.

**What padmap does today.** `docs/EMULATORS.md` covers RetroArch, Cemu,
Ryujinx and ares. Dolphin appears in padmap only as a *libretro core* name in
the libretro core table behind `layout::for_core`
(`rust/crates/padmap-core/src/layout.rs`, exercised by
`crates/padmap-core/tests/scope_and_layout.rs`, verified against
`Source/Core/DolphinLibretro/Input.cpp`) — that is Dolphin-inside-RetroArch,
not the standalone emulator GOTG runs.

**What would be enough.** A `dolphin_config` destination alongside the others,
writing the `[GCPadN]` sections of `GCPadNew.ini`.

Dolphin is the easiest of the four targets and is worth saying why, because it
looks like it should be the hardest: **its SDL backend names inputs by standard
gamepad element**, not by raw index. A binding is `Button A`, `Left Y+`,
`Pad N` — Dolphin does the per-model lookup itself. So there is no capture to
translate and no table per controller, which is the opposite of ares.

The part that does have to be right is the device line:

```ini
[GCPad1]
Device = SDL/0/padmap Player 1
Buttons/A = `Button A`
```

`SDL/<n>/<name>`, where `n` counts devices already sharing that *name*. With
padmap's pads the name is unique per player, so `n` is always 0 — which is
simpler than what GOTG has to do today against physical pads, where it counts
devices sharing a GUID.

GOTG's current implementation is `src/client/lib/pads-dolphin.sh` (150 lines,
including the ini text) and its tests are `tests/client/pads-dolphin.bats`. The
strings there were copied from a `GCPadNew.ini` Dolphin itself wrote for a real
pad, which is the same ground-truth rule padmap uses for Cemu and ares.

**Until then** GOTG keeps its own Dolphin writer, which means the rule "all
controller handling lives in padmap" has one exception — and an exception that
has to be explained every time somebody reads the launcher.

## What was built

`padmap emit --dolphin-dir "$STATE/config/dolphin-emu"`, writing both files:

* **`GCPadNew.ini`** -- a `[GCPadN]` section per seated player, with
  `Device = SDL/0/padmap Player N`. Every `[GCPad1..4]` section is replaced,
  not merely the ones being written, so a controller unplugged since the last
  run does not keep its port. Every other section is left exactly as it was.
* **`Dolphin.ini`** -- `SIDevice0..3` under `[Core]`, edited key by key.

The bindings are the strings from `src/client/lib/pads-dolphin.sh`, which were
themselves copied from a `GCPadNew.ini` Dolphin wrote for a real pad. A test
asserts them one by one; if they drift, a GameCube game binds the wrong
buttons and nothing says so.

### One difference from GOTG's version

**Unmanaged ports are emptied**, not left alone. GOTG sets `SIDevice` for the
ports it uses and says nothing about the rest, so a port still declared from a
session with more players is a phantom controller in the next game. padmap
writes `SIDEVICE_NONE` to ports 2..4 when only player 1 is seated -- the same
rule it already applies to RetroArch reservations and `--nodevice`.

If that is wrong for GOTG -- if something else is meant to own those ports --
say so and it can be made conditional.

### Worth knowing when you switch

`--dolphin-dir` has to be passed like the others. A caller that redirects
Cemu, ares and Ryujinx but not Dolphin will have Dolphin's config written to
`~/.config/dolphin-emu`, because an absent flag means the default location.
The test for the other four caught exactly this when Dolphin was added, which
is why it now covers all five.

`SIDeviceN` being zero-based while `[GCPadN]` is one-based is the off-by-one
that binds player one's pad and then ignores it. It has a test of its own.
