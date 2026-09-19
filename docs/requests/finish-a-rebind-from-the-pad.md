# Finishing a rebind should not need a keyboard

> **Done**, as specified: a 2s finish tier on A with a `finish` ring event,
> and runs seeded from the stored profile. See "What was built" at the end.

**What happens now.** In the mapping wizard the only ways out are keys:
GOTG's `gotg-seat` offers "S skip this button, Esc play without it", and
nothing on the pad leaves the wizard. The daemon has two hold gestures
already -- a 0.8s hold **skips** the current control
(`padmap-core/src/capture.rs`, `SKIP_HOLD_SECONDS`) and, in a session, a
0.7s hold on a claimed pad **accepts** the session
(`padmap-daemon/src/confirm.rs`) -- but neither means "I am done binding;
keep what I have".

The person in front of a television has a controller and no keyboard, which
is the whole premise.

**What would be enough.**

1. **A finish gesture in the wizard.** Holding the A/South button for a
   longer tier than skip -- we propose about 2s -- ends the run and stores
   what was bound: an `Outcome::Finished` from `MappingRun::feed_key`,
   routed in `feed_modal` to the same path `configure_end` takes. Skip at
   0.8s stays exactly as it is. The two tiers need to be far enough apart
   that somebody mashing does not end the wizard by accident, and the finish
   hold should emit a `progress`-style event so a front-end can draw it as a
   filling ring. That makes the three meanings of one button visible: tap
   binds, short hold skips, long hold finishes.

2. **Runs seeded from the stored profile.** `MappingRun::new` starts
   `bindings` and `claimed` empty, so the conflict guard -- which already
   refuses an input another control holds and names the holder -- only sees
   the current run. A partial remap after an early finish can therefore
   double-book an input the previous run bound. Seeding both from the stored
   profile for that scope makes "no two controls share an input" hold across
   runs, and makes an early finish leave a coherent mapping rather than a
   half-empty one. `forget` already provides "start from nothing" for the
   person deliberately swapping A and B.

**Why this is padmap's and not ours.** The hold timing, the debounce, the
conflict guard and the profile store are all in the daemon; a front-end
pretending to finish by cancelling would throw the bindings away.

**What GOTG would do with it.** Name the gesture first and the key second in
the wizard, draw the finish hold as its own ring beside the progress bar,
dim the controls already bound so an overlap is visible before it is made,
and offer "rebind" per seated player from the picker (see
[map-one-pad-without-a-session](map-one-pad-without-a-session.md)).

## What was built

A finish gesture on the pad, one tier above skip, and runs seeded from the
stored profile.

* **The finish tier.** Holding A/South for `FINISH_HOLD_SECONDS` (2.0s, against
  skip's 0.8s) ends the run and keeps what is bound. In `MappingRun::feed_key`
  a hold past the finish tier returns the new `Outcome::Finished`; the daemon's
  `feed_modal` routes that to the same store-and-close path `configure_end`
  takes. A hold between 0.8s and 2.0s still skips, exactly as before. A
  `const` assertion keeps the tiers at least 2x apart so mashing cannot finish.
* **A filling ring.** The hold emits a `finish` event with a `frac` from 0 to
  1 (`events::finish`, drawn each tick by `tick_mapping`), so a front-end draws
  it as a ring beside the progress bar. The run ends on time alone -- the daemon
  ticks the hold -- so a person can hold on without lifting to see it complete.
  Releasing early resets `frac` to 0. See EVENTS.md for the three meanings of
  the one button.
* **Seeded runs.** `MappingRun::seeded` starts `bindings` and `claimed` from the
  capture stored for that pad and scope (`publish::stored_mapping`, matched by
  layout). The conflict guard now sees the previous run, so no two controls
  share an input across runs, and an early finish leaves a whole mapping rather
  than a half-empty one. Re-binding a control frees the input it used to hold,
  so deliberately swapping two buttons within a run still works; `forget` still
  starts from nothing.

The hold timing, the tiers, the conflict guard and the profile store are all in
the daemon, so a front-end cannot finish by cancelling and throwing the
bindings away.
