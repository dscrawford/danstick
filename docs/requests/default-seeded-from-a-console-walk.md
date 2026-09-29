# A default copied from a console walk survives if that walk was redone

## What was seen

After 9112fb7 ("a console's walk is that console's"), both controllers on the
desktop still carry a default that is an old GameCube walk, and every clone is
built from it (see `clone-from-the-console-being-played.md`). DK64 got no
Select and no left trigger.

`~/.local/share/danstick/devices/`, 2026-09-28:

| file | `seeding` | `""` (default) | `console:gamecube` |
|---|---|---|---|
| `045e_028e_Xbox_Wireless_Controller.json` | absent | layout gamecube, 16 controls | layout gamecube, 20 controls |
| `28de_1304_Steam_Controller.json` | absent | layout gamecube, 16 controls | layout gamecube, 16 controls |

## Why

The repair drops an old default only when it binds **exactly** what some
console scope binds. The Xbox pad's GameCube walk was redone later (its
`console:gamecube` gained the four left-stick controls), so the copy made from
the first walk matches nothing and stays. The Steam Controller's is dropped
only if its sixteen bindings happen to equal the scope's to the last value.

## What would be enough

On reading a profile without `"seeding": "generic"`, drop a default whose own
`layout` is not the generic one -- whatever it binds. A default is only ever
supposed to come from a generic walk now, and a default recording that it was
walked as a GameCube (or N64, or Switch) layout is, by its own word, a copy of
a console walk. With it gone the pad falls through to SDL's database or the
kernel's convention, which for an Xbox pad is every button.

## How GOTG will check it

After the pin moves: the two profiles above have no default (or a generic one),
and a clone for the Xbox pad carries Back and the left trigger.
