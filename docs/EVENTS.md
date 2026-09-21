# The `controller` event

## A controller that binds itself

`controller.autobound` in `list --json` says padmap can bind this pad
correctly with no capture: it speaks the kernel's gamepad convention, so its
controls are read off the codes rather than guessed at. A picker can use it to
say *this already works, remap only if you want to* instead of sending
everybody through a wizard.

It is separate from `configured`, which means a capture has been **recorded**.
A pad can be `autobound: true, configured: false` -- working, and never
walked through anything -- and a capture always wins where one exists, so
remapping stays available and stays optional.

## Tuning a controller that misbehaves

```json
{"cmd": "tune", "player": 2, "deadzone": 0.15}
{"cmd": "tune", "signature": "0079:1843:...", "debounce_ms": 30, "ignore_axes": [2]}
{"cmd": "tune", "player": 2, "reset": true}
```

Calibration measures where a stick rests; this is what a person *sets* when
measuring is not enough -- a stick that wanders, a switch that bounces, a
trigger that fires on its own. Every setting lives with the **physical
controller** and follows it to whatever seat it takes, and is applied to
everything padmap publishes for it, after calibration: the virtual pad and
the motion server both see the tuned stream.

| field | meaning |
|---|---|
| `deadzone` | one number, 0 to 1: a band of that fraction of each stick's and trigger's travel that reads as untouched, on every axis `ABS_X`..`ABS_RZ` the pad declares. Or an object of ABS code to number, for one axis. Around the middle for a stick, above the minimum for a trigger; outside it the travel is stretched so full deflection still reaches the end. |
| `debounce_ms` | hold every release back this long, and swallow a press that arrives inside it. A bouncing switch produces exactly that pair, which a game reads as a double tap. At most 500. |
| `ignore_axes`, `ignore_buttons` | event codes dropped entirely, for a part that is simply broken. |
| `reset` | start from nothing before applying the rest. |

Only what is mentioned changes. `player` names a seated pad; `signature` finds
one that is plugged in whether seated or not, so a stick noticed drifting on
the setup screen can be tuned before anyone presses anything. The answer is
one event:

```json
{"event": "tuned", "player": 2, "signature": "...", "tuning": {"deadzone": {"0": 0.15, "1": 0.15}, "debounce_ms": 30}}
```

or an `error` saying which field was not what it claimed to be. A seated pad
is retuned **in place**: its clone is not rebuilt, so a game in progress sees
no disconnect. `list --json` reports the same object under
`controller.tuning`, and `padmap tune` is the same thing from a shell.

## Seating: taking a seat with no session

```json
{"cmd": "seating", "open": true, "players": 4}
{"cmd": "seating", "open": false}
```

While it is open, holding a button on a controller that **holds no seat**
claims the lowest free one after `HOLD_SECONDS` -- the same hold `begin` uses
-- and padmap republishes and rewrites every consumer's config exactly as
`accept` does. The same `progress` and `claim` events are emitted, so a
front-end draws it the way it draws a seat taken on the setup screen.

The difference is that **no session is opened and nothing is grabbed**. The
pads are read ungrabbed, so a press still reaches whatever has focus and
nobody else's controller stops working. That is the point: the moments a
controller needs a seat -- somebody arriving mid-game, a pad swapped for a
charged one, a controller switched on after the picker started -- are the
moments a modal screen is most expensive.

Two bounds make it safe to leave on:

* **Only unseated pads.** A pad holding a seat is being played with, and
  holding B to block in a fighting game must not reseat anybody.
* **Only free seats.** With every seat taken a held pad does nothing until
  somebody leaves. A seat whose controller is merely *away* still counts as
  taken -- it is coming back.

Seating suspends itself while an assignment session is open: the session grabs
every pad and is about to rewrite the roster, so reading underneath it would
claim a seat the user is in the middle of assigning. `begin` is still the way
to *reorder* seats, which is something done with everybody's attention.


padmap's premise is that a program attaches to it and gets stable virtual
gamepads instead of configuring controllers itself. That worked at launch and
not during play: a controller plugged in mid-game produced nothing a running
program could act on, so the only way to pick it up was to quit and start
again.

This is the event that closes that. Receiving one is enough to bind a new pad
live -- no file to read, nothing further to ask padmap, and no need to know how
padmap works.

    python3 tools/padctl.py watch

## `pads` follows the room

`{"event": "pads", "count": N}` is sent when a session opens and again
whenever the set of pads in it changes: a controller switched on during the
session is admitted and counted, one switched off is counted out (its claim
kept for its return). A front-end saying "no controllers found -- plug one in"
can stop saying it the moment that is no longer true.

## Shape

```jsonc
{
  "event": "controller",
  "action": "added",          // added | removed | unconfigured
  "player": 2,                // the slot that changed; 0 when unconfigured
  "changed": { /* one entry, below */ },
  "roster":  [ /* every attached player, by player number */ ],
  "scope":   {"console": "n64", "game": "..."},
  "build":   "/nix/store/...",
  "reason":  "unreadable"      // only on unconfigured

}
```

`changed` and each element of `roster`:

```jsonc
{
  "player": 2,
  "controller": {
    "name": "Nintendo Switch Pro Controller",
    "vid": "057e", "pid": "2009",
    "path": "/dev/input/event9",
    "phys": "usb-0000:08:00.1-4/input0", "uniq": "...",
    "signature": "057e:2009:Nintendo Switch Pro Controller",
    "configured": true,
    "retroarch_visible": false
  },
  "virtual": {
    "name": "padmap Player 2",
    "node": "/dev/input/event92",
    "phys": "padmap/p2",
    "vid": "057e", "pid": "2009", "bustype": 3,
    "guid": "030089a67e0500000920000001000000",
    "identity_mode": "mirror"
  },
  "retroarch": {
    "port": 2,                // 1-based, as input_playerN_* counts
    "index": 1,               // 0-based joypad index, or -1
    "profile": "/run/user/1000/padmap/autoconfig/padmap Player 2.cfg",
    "binds": {"input_a_btn": "97", ...}
  },
  "sdl_mapping": "030089a6...,padmap Player 2,a:b0,..."
}
```

## The three actions

| action | meaning | `virtual` present |
|---|---|---|
| `added` | live now; padmap is republishing it | yes |
| `removed` | was live, has gone. Its slot is **kept** | yes |
| `unconfigured` | arrived, but no mapping has ever been recorded for this model, so there is nothing to bind | no |

`unconfigured` is announced rather than passed over because "a controller
lights up and does nothing" is the situation a user most needs told. Setup for
it is offered separately, and only once a game is not running.

It carries a `reason`:

| reason | meaning |
|---|---|
| `unmapped` | nobody has ever mapped this model |
| `unreadable` | it has a mapping, but its device node could not be opened |

`unreadable` is worth telling a user about differently: it usually means a
permission, not a missing configuration, and no amount of running setup will
fix it.

## A node exists before it is readable

Worth knowing if you write something similar. udev applies the `uaccess` ACL
that grants the logged-in user access *after* the device node appears, and
padmap looks the moment `/dev/input` changes -- so the first open of a freshly
plugged controller can fail with `EACCES` and succeed a fraction of a second
later. This was not theoretical; it was hit against a live daemon the first
time this path ran for real.

padmap retries for about five seconds before giving up, and a controller that
is unplugged and plugged in again always gets a fresh set of attempts. The
consequence for a consumer is that an `added` event may arrive a few hundred
milliseconds after the device node does.

## Things worth knowing before you consume it

**Every event carries the whole roster.** A consumer that connected a moment
ago, or that was busy, can apply the latest message it holds and be correct.
There is no log to replay and no way to end up subtly out of step. The cost is
a larger message on a socket that carries a few per hour.

**A slot is not freed when its controller leaves.** A Bluetooth pad that drops
for four seconds and returns comes back as the same player. Freeing the slot is
how a four-player game becomes a three-player game with everyone shifted up
one. A new controller takes the lowest *never-used* slot, so an arrival after a
departure fills the hole rather than opening a slot beyond the live pads.

**`virtual.vid`/`pid` are not always the hardware's.** By default the clone
*mirrors* the controller, so they match; under `PADMAP_PAD_IDENTITY=padmap`
they are `1209:0001` on `BUS_VIRTUAL`. `identity_mode` says which. Match on
`guid`, which is computed from whichever is in force.

**`index` is not `port`.** RetroArch's `input_playerN_joypad_index` is a
0-based position in its own enumeration, and hidden pads are not in it. `-1`
means padmap could not place this player, and binding by index would point at
someone else's pad.

**Binds are scoped.** `scope` says which console and game they were resolved
for. A consumer caching them needs it, or it applies N64 binds to a SNES game
and nothing says why the buttons moved.

**`signature`, not `name`, identifies a controller.** Two identical pads share
a name.

## Turning it off

`PADMAP_NO_AUTOATTACH=1` keeps the announcements but stops padmap claiming a
player slot for an arriving controller -- for a caller that wants to decide the
roster itself and treat padmap purely as a source of events.

`PADMAP_NO_AUTOSETUP=1` is separate and older: it suppresses the setup screen,
not these events.

## Guarantees

* The clone's device node exists before the event naming it is sent.
* `guid` is computed from the `vid`/`pid`/`bustype` printed beside it, never
  re-derived, so the two cannot disagree.
* Nothing is announced while the setup screen is open; that session owns every
  pad and is about to rewrite the roster.
* A failed republish is not fatal and produces no `added` event.

All of these are checked by `tests/check_controller_events.py`.

## The `finish` event, and a rebind that keeps what it has

The mapping wizard has three meanings for the A/South button, told apart by how
long it is held:

| gesture | seconds | meaning |
|---|---|---|
| tap | < 0.8 | bind the current control |
| short hold | 0.8 | skip the current control (`SKIP_HOLD_SECONDS`) |
| long hold | 2.0 | finish: end the run and keep what is bound (`FINISH_HOLD_SECONDS`) |

The long hold emits a `finish` event as it fills, so a front-end can draw it as
a ring filling beside the progress bar:

```json
{"event": "finish", "player": 1, "frac": 0.42}
```

`frac` runs 0 to 1. It reaches `1.0` once, when the run ends, and then a
`mapping` with `"done": true, "stored": true` follows. Releasing the button
before the tier resets `frac` to `0`. The gesture is the daemon's, not a
key: no button leaves the wizard from a keyboard the person at the television
does not have.

A run does **not** start empty. It is seeded from the capture already stored
for that pad and scope, so the conflict guard -- which refuses an input another
control holds and names the holder -- holds across runs. A partial remap
followed by an early finish therefore leaves a whole mapping: the controls not
touched keep their stored bindings, and no two controls end up sharing an
input. `forget` is still the way to start from nothing.

## Mapping, choosing a layout, and calibrating without a session

`map`, `choose_layout`, `choose_scope`, `map_for_game` and `calibrate` are legal
with **no session open**. They grab only the named player's pad for the run and
release it when the run ends; everyone else keeps playing, their controllers
never stopped. The events are unchanged: the same `mapping` steps, the same
`conflict`, the same `layout_choice`, the same `calibration`, the same `done`.

When a session is open these commands still use it, as before. `begin` remains
the way to reassign seats for the whole room, which is a session because it
grabs every pad.

**One pad this cannot grab.** A triton pad (the 2026 Steam Controller, driven
through the puck) cannot be grabbed at all -- `Source::grab` is a no-op for it.
Its presses therefore reach whatever has focus during a run, in addition to the
wizard. A front-end should ignore that pad's own SDL events while its rebind is
open. Every other pad is grabbed for the duration and reaches only the wizard.

## Seats belong to the session: `unseat`, `--fresh`, `--follow`

A seat is something taken in front of the screen about to be used, not
something the machine remembers you having. Three pieces make that true, each
useful on its own.

**Unseat, by command.**

```json
{"cmd": "unseat"}
{"cmd": "unseat", "player": 2}
```

Drops that seat -- every seat, with no player named. The clone stops, the pad
is released, consumers are rewritten for the seats that remain (a launch config
naming nobody, when nobody is left), and the seat is gone from
`assignments.json`, so a restart does not bring it back. A `controller` event
with `action: "removed"` and `reason: "unseated"` is emitted per pad, then
`state`. Seating is left exactly as it was: with it open, the next hold takes
the freed seat straight back, because that is the next thing that happens. A
seat whose controller is merely *away* is dropped the same way.

Refused, with an `error`, while a session is open (`cancel` it first), while a
controller is being set up, and for a seat nobody holds.

**Start unseated.** `padmap serve --fresh`, or `PADMAP_NO_RESTORE=1`, skips
restoring saved seats. Profiles, calibrations and mappings are the
controller's and follow it; only the seats are forgotten. The file itself is
left alone until the first seat taken in the new session overwrites it, so a
plain `serve` after a `--fresh` one that seated nobody still restores what was
there before.

**Follow a pid.** `padmap serve --follow <pid>` exits, releasing everything --
seating closed, clones stopped, socket removed -- once that pid is gone. It is
polled four times a second, since `PR_SET_PDEATHSIG` does not survive the
reparenting `ensure-daemon` does. A pid already gone at startup ends the daemon
at once. `state` carries the pid as `following` (`null` otherwise). Until the
pid goes, nothing changes: seating stays open after the last client
disconnects, as before, so a second player still turns up mid-game.

**From a front-end.** `padmap ensure-daemon --fresh --follow $$` is the whole
integration, from the picker and from the launcher alike. A lifetime flag
names a session: a running daemon that already follows that pid is left alone
(the picker's daemon survives the `execvp` into the game, seats and all), and
one that belongs to another session -- or to none, outliving everything with
seats restored -- is replaced. `--check` reports that as a discrepancy and
changes nothing. Without either flag `ensure-daemon` behaves as it always has.

## The keyboard as a player: `seat_keyboard`

```json
{"cmd": "seat_keyboard"}
```

Seats the keyboard as the next free player. No device is read and nothing is
grabbed -- the keyboard stays the compositor's, and padmap never sees its
keys; the front-end times the hold itself. What changes is every emulator's
configuration: player N is now the emulator's own keyboard device, in the
emulator's own default keys where it has them and padmap's where it does not
(`docs/EMULATORS.md`, "The keyboard"). A `claim` goes out first:

```json
{"event": "claim", "player": 1, "name": "Keyboard", "node": "", "icon": "keyboard", "configured": true}
```

then `state`, whose `players[]` entry for the seat is
`{"player": 1, "name": "Keyboard", "icon": "keyboard", "configured": true,
"published": false, "keyboard": true}`. A pad seated after it takes the seat
after: keyboard first then pad gives player 1 keyboard, player 2 pad, carried
into ports that number by device (ares, RetroArch) as well as by seat.

Refused, with an `error`, while a session is open, when the keyboard already
holds a seat, and when every seat is taken. `unseat` and `unseat` with its
player drop it like any seat; `--fresh` forgets it; `--follow` ends with it.
`seating` ignores it and is not closed by it.

**Unseated, the keyboard is still somewhere.** With no seat of its own it
sits on the first port after the pads, which is where the emulators that
have a keyboard default would have put it anyway, moved off a port a pad
holds. Seating it pins it to a seat ahead of pads seated later; that is the
difference.

## `input`: what is under the thumb while the wizard runs

During a mapping run padmap holds the pad and holds back its clone, so the
front-end's SDL sees nothing from it. This is the window: one event per raw
input on the pad being mapped, sent whether or not it binds anything.

```json
{"event": "input", "player": 1, "kind": "button", "index": 3, "value": 1}
{"event": "input", "player": 1, "kind": "hat", "index": 0, "value": 1}
{"event": "input", "player": 1, "kind": "axis", "index": 2, "value": 0.95}
```

`kind` and `index` are the same triple a profile's binding uses, so the table
a front-end already has names it. `value` is `1`/`0` for a button, a
direction bit for a hat (`1` up, `2` right, `4` down, `8` left, `0` centred),
and `-1..1` for an axis, rounded to twentieths; the same event is never sent
twice in a row, so a resting stick is one event, not a stream. Only the pad
under the wizard reports, and only while it runs.
