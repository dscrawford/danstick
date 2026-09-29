# Let a walk give one input to several controls

## What is asked

A person walking their buttons (`map`, from GOTG's overlay over a game) should
be able to press the same button for two controls if that is what they want --
one button for both "A" and "Start" on a pad with few buttons, a trigger for
both Z and R in a game that uses one of them, a one-handed layout. Today the
conflict guard refuses it: an input another control holds is not accepted, the
`mapping` event names the holder in `conflict`, and the step waits for a
different input. The player cannot finish the walk the way they want it.

## Why the guard exists, and why it is not enough reason

EVENTS.md: a run is seeded from the stored capture "so the conflict guard --
which refuses an input another control holds and names the holder -- holds
across runs ... no two controls end up sharing an input". That protects against
a *stray* capture (a press that bounced, a trigger read twice), which the
axis-rest rule in 9112fb7 now handles at the source. It also stops a deliberate
choice, and the person at the television has no way past it.

## What would be enough

1. The guard becomes a **warning**, not a refusal: the step accepts the input,
   and the `mapping` event still carries `conflict` naming the other control,
   so a front-end can say "also on A" beside the step. Two controls may then
   hold the same input; the clone drives both from it (the xbox translator and
   `Twins` already fan one code out to several controls).
2. If a refusal is still wanted anywhere, a field on `map`/`bind`
   (`"shared": true` or `"strict": false`) to choose, defaulting to allowing it.

## How GOTG will check it

A walk from the overlay that gives the same button to two controls finishes,
the overlay's panel shows both lit on one press, and the game reads both.
