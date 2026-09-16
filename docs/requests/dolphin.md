# Dolphin, as a config target

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
