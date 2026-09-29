# A clone is built from the default mapping, never the console being played

## What was seen

Donkey Kong 64 (the recompiled port, `xbox360` identity) on the desktop,
2026-09-28, an Xbox Wireless Controller seated as player 1. Controls were
missing in the game, and GOTG's rebind chord (L + R + Select, held) never
fired. danstick's own log for the session:

    Xbox Wireless Controller: mapping for scope "default" (gamecube) is missing
    4 of the layout's controls: leftstick_up, leftstick_down, leftstick_left,
    leftstick_right

## Why

Every clone is made from the pad's **default** scope. Both places that build
one ask for it with an empty console and game:

- `server.rs` ~2688: `publish::resolved(&slot.pad, "", "").1` before
  `clone::create_on` (a reserved slot)
- `server.rs` ~2767: the same, for a seat taken by a hold

`Profile::resolve(console, game)` knows how to pick `game:...`, then
`console:...`, then the default -- but nothing reaches it with anything but
`""`. So a capture stored under `console:n64` (this pad has one, 14 controls,
walked on purpose for N64) never reaches the clone an N64 game reads; only
RetroArch's autoconfig (`profile_text`) sees console scopes.

For this pad the default was an old GameCube walk (see
`default-seeded-from-a-console-walk.md`): no `back`, no `lefttrigger`, L and R
on the triggers. Under the `xbox360` identity the translator carries only what
a capture names (plus the left stick and stick clicks when unnamed), so the
clone had no Back at all -- which is why a chord that needs Select could not
fire -- and N64's Z (the left trigger) reached nothing.

It also means a remap cannot do what it says. GOTG's in-game rebind sends
`{"cmd": "map", "player": N, "layout": "n64", "scope": "console:n64"}`; the
walk is stored, and the clone the game is reading carries on from the default.

## What would be enough

1. **A clone is built from the scope of what is being played.** GOTG knows the
   console at launch (`gotg play` resolves the platform; the environment says
   which danstick layout its controls are walked in). A way to say it -- an
   environment variable read by `danstick-rs exec` / `ensure-daemon`, or a
   command (`{"cmd": "scope", "console": "n64", "game": "..."}`) -- and the
   clone built and republished from `resolve(console, game)`, falling back to
   the default as `resolve` already does. The picker has no console; its clones
   stay on the default.
2. **A `map` that finishes republishes the clone** if its scope is the one in
   use, so a rebind in the middle of a game changes the game's controls
   without relaunching it.

## How GOTG will check it

In DK64: L + R + Select brings the rebind panel down; the walk's result is
what the game reads, without a relaunch; and the startup log names the
`console:n64` scope rather than "default".
