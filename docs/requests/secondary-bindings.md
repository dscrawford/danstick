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
