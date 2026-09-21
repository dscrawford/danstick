# Say which button is being pressed while the wizard runs

**What GOTG wants.** On the mapping wizard's "press X", a picture of the
button the person is actually pressing, as they press it -- so a wrong
button is seen before it is bound, and a pad whose labels do not match the
console's has a fighting chance. The launch gate already rings the control
being asked for on the console's drawing and, from `captured`, says what the
previous press became; what it cannot say is what is under the thumb *now*.

**Why GOTG cannot do it alone.** During a capture padmap holds the pad and
holds back forwarding to its clone (`sync_republish_pause` / `hold_back`),
which is right -- the press is being captured, not played. But it means the
front-end's SDL sees nothing from that pad until the run ends. The only
window on the pad during the wizard is padmap's.

**What would be enough.** One event per raw input during a capture run, sent
whether or not it binds anything:

```json
{"event": "input", "player": 1, "kind": "button", "index": 3, "value": 1}
{"event": "input", "player": 1, "kind": "hat", "index": 0, "value": 1}
{"event": "input", "player": 1, "kind": "axis", "index": 2, "value": 0.93}
```

The same triple padmap's profile uses to describe a binding, so a front-end
can name it with the table it already has (`gotg_ui.pressing.controls_for`
reads a profile the other way). A `value` of `0` for a button, or an axis back
inside the dead zone, is the release. Rate-limited as `progress` is; nobody
needs every 4 ms of an axis.

**What GOTG does with it.** On the wizard screen, beside "press X": the
pressed input in words -- "Button 3", "Hat up" -- and, once it is bound to a
control, that control ringed on the drawing as the press happens. On the
door after the wizard, GOTG already does this from the clone; this is the
same thing during the ten seconds it cannot.

## What was built

The `input` event, as asked: one per raw input on the pad under the wizard,
`{"event": "input", "player": N, "kind": "button"|"hat"|"axis", "index": I,
"value": V}`, the same `kind`/`index` a profile binding carries. A button is
`1`/`0`; a hat is its direction bit or `0` centred; an axis is `-1..1` rounded
to twentieths, and an event identical to the last is not sent, which is the
rate limit. `MappingRun::describe` is the pure part, tested on a button, a
hat and an axis; the rebind journey asserts the press and the release of the
first tap are reported before it is bound. Documented in `docs/EVENTS.md`.
