# The universal mapping is seeded from a console's capture, and L goes missing everywhere

## What was seen

On the Deck, 2026-09-25/26: "L is missing" in GOTG and in several games, and
GOTG's exit chord (both shoulders + Start, held) never fires. Earlier, on a
GameCube walk, "the L button was automatically set" without being pressed.

## Why, from the profiles

This desktop's `~/.local/share/danstick/devices/*.json`, for both the Xbox
Wireless Controller and the Steam Controller:

| scope | leftshoulder | rightshoulder | righttrigger | lefttrigger |
|---|---|---|---|---|
| `""` (universal) | `+a2` (LT) | `+a5` (RT) | `b5` (RB) | -- |
| `console:gamecube` | `+a2` | `+a5` | `b5` | -- |
| `console:n64` | `+a2` | `+a5` | -- | `b5` |
| `console:switch` (Steam Controller) | `+a2` | `+a5` | `b5` | **`+a1`** |

1. **Universal is a copy of the first console walked.** `Profile::record`
   (`danstick-core/src/profile.rs`) seeds `""` from the first capture under
   any scope. The first walk here was GameCube, where L and R *are* the
   triggers -- so the analog triggers were bound to L and R, correctly for
   GameCube. The universal copy carries that: `leftshoulder` is the left
   trigger, and the physical LB is bound to nothing at all. Every console
   without a capture of its own falls back to universal, so on every one of
   them LB does nothing and "L" is the trigger. GOTG's exit chord reads the
   clone's `leftshoulder`, so pressing LB never reaches it either.
2. **A stick axis was captured for a trigger.** `console:switch`'s
   `lefttrigger` (ZL) is `+a1` -- the left stick's Y axis. Nobody pushes a
   stick to answer "press ZL"; a resting or drifting stick crossed the
   threshold and the wizard took it. This is the "set by itself" report.

## What would be enough

1. **Don't seed universal from a console layout whose controls mean something
   else there.** Either seed it only from a walk of the standard/generic layout,
   or seed only the controls whose meaning is the same under both (face
   buttons, D-pad, Start/Back, sticks) and leave the shoulders and triggers
   unset -- unset falls through to what the pad reports natively. A profile
   already seeded this way should be repairable (re-seed on the next load, or a
   `forget` of the universal scope alone).
2. **An axis answers a step only by travelling from where it rested when the
   step began**, by more than the press threshold. An axis already displaced at
   the start of the step -- a drifting stick, a trigger that rests at -1 on
   one driver and 0 on another -- is not a press. Stick axes answering a
   button/trigger step at all could reasonably need a second confirmation.

## How GOTG will check it

`gotg-killswitch`'s exit chord on the Deck with LB+RB+Start, and the overlay's
rebind panel (L+R+Select), which now lights `leftshoulder` live from the clone:
LB should light L on every console that has no capture of its own.
