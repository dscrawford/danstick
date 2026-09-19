# Rebinding one pad should not stop the room

> **Done.** `map`, `choose_layout` and `calibrate` grab only that player's
> pad with no session. The triton note is in EVENTS.md. See the end.

**What happens now.** `map` needs an open session: `session_pad` refuses
with "needs an open session" (`padmap-daemon/src/server.rs`), and a session
is `begin`, which grabs **every** pad with `EVIOCGRAB`. So one person
rebinding their controller stops everybody else's for the duration -- the
same modality [always-seating](always-seating.md) removed from joining.

The pieces for the other shape already exist: the wizard listens only to
the pad being mapped (`feed_modal` gates on `modal.pad_path`), and ambient
seating already opens individual sources without grabbing them
(`padmap-daemon/src/seating.rs`).

**What would be enough.** `{"cmd": "map", "player": N, ...}` -- and
`choose_layout`, `calibrate` -- legal with no session open, grabbing
**only that player's pad** for the run and releasing it after. Everyone
else keeps playing. Events unchanged: the same `mapping` steps, the same
`conflict`, the same `done`.

One thing to decide on your side: a triton pad cannot be grabbed at all
today (`Source::grab` is a no-op for it in `clone.rs`), so its presses reach
the front-end during a run regardless. The front-end will ignore that pad's
SDL events while its own rebind is open, so this is not blocking -- but it is
worth a line in EVENTS.md so the next front-end knows.

**Why this is padmap's and not ours.** Which pad is grabbed and when is the
daemon's alone; a front-end cannot un-grab three pads it never asked to
have grabbed.

**What GOTG would do with it.** A controllers list with one row per seated
player and the verbs the client has carried since it was written and never
called -- rebind for this console, rebind for everything, calibrate, forget
-- with the rebind drawn one control at a time over the picker. Until this
lands, the rebind verb ships behind a "this will pause the other
controllers" confirmation and uses `begin` + `map` + `accept`.

## What was built

`map`, `choose_layout`, `choose_scope`, `map_for_game` and `calibrate` now work
with no session open, grabbing only that player's pad.

* **`modal_pad`** in `padmap-daemon/src/server.rs` replaces the old
  `session_pad` for these flows. With a session open it uses it, as before.
  With none, it finds the player's pad and opens it one of two ways: through the
  clone's source if the pad is already republished (its presses held back from
  the clone for the run, so the game does not also see the wizard), or grabbed
  on its own if it is not (a new `Watched::Solo` descriptor, released when the
  run ends). Everyone else keeps playing.
* **Per-pad hold-back.** `Republisher::hold_back(index, ...)` stops one pad's
  presses reaching its clone while a modal flow reads it, in place of the old
  whole-stream `set_paused`. `sync_republish_pause` holds back only the pads a
  modal flow is reading. A test drives two clones and confirms the held pad
  forwards nothing while its neighbour keeps forwarding.
* **Events unchanged.** The same `mapping`, `conflict`, `layout_choice`,
  `calibration` and `done`. `forget` no longer needs a session either.

Which pad is grabbed and when is the daemon's alone, so a front-end cannot
un-grab pads it never asked to have grabbed.

### The triton pad

A triton pad (2026 Steam Controller through the puck) cannot be grabbed --
`Source::grab` is a no-op for it in `clone.rs` -- so its presses reach the
front-end during a run regardless. This is noted in EVENTS.md so the next
front-end ignores that pad's SDL events while its own rebind is open. It is not
blocking: the wizard still reads the pad and binds it correctly.
