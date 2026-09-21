# A control can only be bound to one input

`Mapping.buttons` is a `BTreeMap<String, Binding>` — one control, one binding.
There is nowhere to put a second input for the same button, so a front end
cannot offer one however it asks.

## What GOTG wants to do

Its binding screen now draws the pad, walks it with a d-pad and says what each
control is bound to, read out of `~/.local/share/padmap/devices/*.json`. The
next thing somebody asks for on that screen is "and also this button" — a
second input on the same control. Concretely:

- a shoulder that has stopped working, and the player wants the trigger to do
  it as well rather than remapping and relearning
- a Steam Controller where both the grip paddle and the bumper should be R
- two-handed play on a handheld, where Z is easier to reach in two places

## Why it belongs in padmap and not here

padmap is the thing that reads the physical pad and writes the virtual one. A
second input for one control is a second `if` in that loop, and everything
downstream — the SDL mapping, the emulator config — keeps its single binding,
because the virtual pad still has one button. Nothing else in the stack has to
learn about it.

Doing it anywhere else is not possible for the same reason. SDL's mapping
string binds one input per control. RetroArch's config is one binding per
control. A front end can only store a preference nothing reads.

## What would be enough

`buttons` holding a list rather than a single binding, with the first entry
being what everything already reads:

```json
"righttrigger": [
  {"kind": "button", "index": 5},
  {"kind": "axis", "index": 5, "value": 1}
]
```

Old profiles are a single object where the new one is a list, so the reader
has to take either — which is also what lets a rollback keep working.

And a way to add one: the capture wizard walks every control, which is a long
way round for one extra button. Something like
`{"cmd": "bind", "player": 1, "control": "righttrigger", "scope": "console:gamecube", "add": true}`
that captures the next press onto that control alone, and answers with the
same `mapping` event the wizard uses.

## Until then

GOTG's screen shows the one binding and does not offer to add another, rather
than offering something it cannot store.

## What was built

Both halves, as asked.

**The format.** `Mapping.buttons` keeps one binding per control, and a new
`extra` holds the rest. On disk a control is the object it always was, or a
list whose first entry is that object and whose rest are its second inputs,
so an old reader and a rollback both keep working and a file with no lists
is byte-for-byte unchanged. `Mapping` serialises itself rather than deriving
it, because the shape of one field now depends on another.

**The second `if`.** `padmap_core::twins` is that loop: a control with more
than one input comes out as its first, down while any of them is. A twin on
a button primary presses it; a twin on an axis primary drives it to the end
of its travel; a twin on a hat primary pushes that direction. Letting go of
one while another is held changes nothing, which is the whole point. The
second input still reaches the clone as itself, since the clone mirrors the
pad and it is a real input on it. Under `PADMAP_PAD_IDENTITY=xbox360` the
same union happens inside that translator instead, where every control is
already synthesised.

**Adding one.** `{"cmd": "bind", "player": N, "control": C, "scope": S,
"add": true}` captures the next press onto that control alone and answers
with the same `mapping` events the wizard uses, as the request asked.
Without `add` it replaces the control's binding, which is the cheap rebind
the wizard was the long way round for. A run is seeded from what is stored,
so binding one control does not disturb the others, and a control's first
input is always its binding whatever `add` says.

Tests: five on the profile format (single, list, round-trip, junk, empty),
five on the twins loop, three on the one-control run, one on the command's
parse, and a journey that binds B alone, sees one object on disk, adds a
second input, and sees a two-entry list whose first entry is untouched.
