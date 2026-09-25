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
{"cmd": "seating", "open": true, "players": 4, "hold": 1.5}
{"cmd": "seating", "open": false}
```

While it is open, holding a button on a controller that **holds no seat**
claims the lowest free one after the hold -- the same hold `begin` uses -- and
padmap republishes and rewrites every consumer's config exactly as
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

### The order a seat is announced in

`claim`, then `state` with the new seat in it, then the consumers' files are
written, then `controller` with `"action": "added"`. The seat is live -- its
clone is on the air and forwarding -- by the time `state` says so, and nothing
in `state` depends on the emulators' files, so it does not wait for them: how
full the room is no longer decides how soon a front-end sees somebody sit
down. On one desk, `claim` to `state` is about a millisecond for the first
seat and the fourth alike; the files follow some tens of milliseconds later.

**Wait for `controller` `added` before reading the files.** It is the event
that names them -- the clone's node, the RetroArch profile, the SDL line --
and it is sent after they are written. A launch that reads `launch.cfg` or
`env.sh` the moment it sees `state` can read the previous room's.

### How long the hold is

`hold` is that length in seconds, `0.05` to `10.0`, and `0.25` for anybody who
does not ask. The right length differs by screen -- a launch gate wants
deliberation, a mid-game join wants to be quick -- so it is on the command
rather than only on the daemon, and it needs no restart. **Omitted, it leaves
the length as it was**, so a caller that does not care never resets one that
does.

`PADMAP_HOLD_SECONDS` sets what a daemon starts with, for a front-end that
starts its own (`serve --fresh --follow`) and would rather not say it twice.

Nothing here is refused. A length that is missing, unparseable or outside the
range is `0.25`: a comfort setting is not worth failing to open seating over.
`progress` is already a fraction of the hold, so a reveal fills over whatever
length is set.

One thing to rely on: **a hold in flight when the length changes is dropped**,
not re-measured. A press that became a claim because the number moved
underneath it is the accident a longer hold exists to prevent.

**A hold is timed from the press itself**, by the kernel's stamp on the event,
not from when padmap got round to reading it. A claim just before can keep
padmap busy for a good part of a second, and a hold that started counting
only afterwards made the next person's `claim` late by that much. So a seat
lands its hold's length after the button went down, whoever claimed just
before; the same holds for a held space bar.

### `progress`: who is filling, and where they would sit

One event per pad with a hold in flight, every tick:

```json
{"event": "progress", "frac": 0.42, "node": "event9",
 "name": "Xbox Wireless Controller", "player": 2}
```

`frac` is that pad's own fraction of the hold. `node` is the pad, and is the
field to key on: two people pressing at once are two fills, and without it a
front-end sees one fill jumping between two values. `player` is the seat this
hold takes **if it finishes now** -- not a reservation, which is the next
paragraph.

**Seats go in the order the buttons went down**, not the order the pads were
plugged in, and nothing is reserved at press time. So a hold that does not
finish claims nothing, and the seats go to whoever does finish, earliest press
first. Two people hold, the one in front lets go at 80%, and the other takes
seat *one*.

**A release is said out loud**, as a final `frac: 0` for that pad with no
`player`:

```json
{"event": "progress", "frac": 0.0, "node": "event9",
 "name": "Xbox Wireless Controller"}
```

It is sent once, when the fill stops: a button released, or a pad that went
away mid-hold. A hold that *completes* is not a release -- it ends on
`frac: 1.0` and then a `claim`.

Two things that do not fill at all, and are deliberate: a pad that already
holds a seat (holding B to block in a fighting game must not reseat anybody),
and a button that was already down when `seating` opened -- the hold begins at
a press the daemon saw, so a button held across the open is ignored until it
is let go and pressed again.

### `state` says whether seating is listening, and for how long

```json
{"event": "state", "...": "...", "seating": true, "hold": 1.5}
```

`seating` is whether an unseated pad holding a button would take a seat right
now; `hold` is how long that takes, in seconds. A front-end that knows both
can stop re-sending `seating` when nothing has changed -- which is worth doing,
since a `seating` whose `hold` differs from the one already set drops every
hold in flight.

### `full`: a hold that finished with nowhere to sit

```json
{"event": "full", "name": "Xbox Wireless Controller", "node": "event9", "seats": 4}
```

Sent when a hold completes and every seat is taken, followed by that pad's
`frac: 0`. Only that pad's hold is dropped: the fifth person at the party
picking up a spare must not cancel the fourth person joining, and the other
fills carry on. The pad is not remembered as having claimed anything, so it
can hold again the moment a seat frees.

Without this a front-end drew a fill that reached the end and then stopped,
with nothing to put on screen.

The same events, in the same shape, come out of an assignment session
(`begin`); there the seat a fill names is the next one that session will hand
out.


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
they are `1209:0001` on `BUS_VIRTUAL`; under `PADMAP_PAD_IDENTITY=xbox360`
every clone is a wired Xbox 360 pad, `045e:028e` version `0x0110` with the
layout `xpad` gives it, so every SDL program maps it from the database it was
built with and needs no mapping handed to it. The source's inputs are
translated onto that layout through its stored capture (or code for code for
a pad that follows the kernel's convention); a control the source lacks is
never pressed. Two clones share one GUID under it -- SDL tells them apart by
index, ares by slot, RetroArch by name; Ryujinx, which blanks the name CRC,
cannot, and is the one consumer this identity does not suit.
`PADMAP_PAD_IDENTITY=xbox360-numbered` is the same pad with the player number
in its version (`0x0001` for player 1, and so on), so every clone has a GUID of
its own and Ryujinx tells them apart too. SDL finds its mapping all the same:
when no database entry has the exact version it matches one with the version
set aside. `identity_mode` says which is in force. Match on `guid`, which is
computed from it.

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
* A seat's `state` is sent before its consumers' files are written, and its
  `added` event after them: `added` is the one to wait for before reading
  a file padmap writes.

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

Seats the keyboard as the next free player. Nothing is grabbed -- the keyboard
stays the compositor's and the game's. What changes is every emulator's
configuration: player N is now the emulator's own keyboard device, in the
emulator's own default keys where it has them and padmap's where it does not
(`docs/EMULATORS.md`, "The keyboard"). A `claim` goes out first:

```json
{"event": "claim", "player": 1, "name": "Keyboard and Mouse", "node": "", "icon": "keyboard-mouse", "configured": true}
```

then `state`, whose `players[]` entry for the seat is
`{"player": 1, "name": "Keyboard and Mouse", "icon": "keyboard-mouse",
"configured": true, "published": false, "keyboard": true, "mouse": true}`.
A pad seated after it takes the seat after: keyboard first then pad gives
player 1 keyboard, player 2 pad, carried into ports that number by device
(ares, RetroArch) as well as by seat.

**The seat is both devices.** The person at the keyboard has the mouse under
their other hand, and some games want it -- a PC port's camera, Dolphin's
Wii pointer, the N64 and SNES mice in ares, a RetroArch core with a mouse or
lightgun. So the seat is named for both, and the mouse is bound to that
player wherever an emulator has a pointer for a port (`docs/EMULATORS.md`,
"The keyboard"). `keyboard` and `mouse` are both true on it; either one
tells a front-end this seat has no pad behind it. Nothing is grabbed: the
mouse stays the compositor's, exactly as the keyboard does.

**A held space bar does the same thing, from anywhere.** While seating is
open and the keyboard has no seat yet, padmap reads every keyboard on the
machine and times a held `KEY_SPACE` for the same `hold` the pads use. A
front-end no longer has to time it, and no longer has to be the thing with
focus: the picker has `execvp`'d into the game by the time somebody wants to
join, and there is nothing of GOTG left listening.

It is reported exactly as a pad's hold is, so an overlay can draw the
keyboard arriving without knowing it is not a pad:

```json
{"event": "progress", "frac": 0.4, "name": "Keyboard and Mouse", "node": "", "player": 1}
```

then `frac: 0` with no `player` if it is let go early, and the `claim`
and `state` above when it runs its length. The `node` is empty: the seat has
no device of its own, and a front-end keying a fill by node and falling back
to name draws it as the keyboard.

**Read, never grabbed.** The space bar reaches the game too, so a character
may jump while somebody joins. That is deliberate: grabbing the keyboard
would take it from the game and from the desktop, which is worse than a
stray jump. Only `KEY_SPACE` is looked at, so typing cannot take a seat.
A pad's own keyboards -- a Steam Controller in lizard mode publishes four --
are left out of this: padmap already grabs those beside the pad, and reading
them here would let a trackpad click bound to space seat "the keyboard".

**`PADMAP_NO_KEYBOARD_HOLD=1` turns it off**, and `seat_keyboard` still
works over the socket. Worth knowing before you decide:

- padmap holds a read-only fd on every keyboard while it is listening, and
  it is listening for as long as seating is open -- which, for GOTG, is the
  whole game. No keystroke is stored, logged or sent anywhere: the only
  thing looked at is `KEY_SPACE`, and the only thing on the socket is the
  `frac` above. A client does learn when the space bar goes down and up,
  to about 20ms.
- anything that can put a space bar into evdev can take a seat, a
  remote-input daemon included -- Sunshine, Input Leap, `ydotool` all
  publish keyboards indistinguishable from the desk's. This is the trust
  padmap already places in pads, said out loud.
- a space bar already held when padmap starts reading is invisible until it
  is let go and pressed again. Linux reports edges from the moment a reader
  opens the node, and padmap deliberately does not ask the kernel what is
  already down -- the same reason a pad held before seating opened does not
  claim.

Refused, with an `error`, while a session is open, when the keyboard already
holds a seat, and when every seat is taken. A keyboard that has a seat is no
longer read at all, so it types into the game as it always did. `unseat` and `unseat` with its
player drop it like any seat; `--fresh` forgets it; `--follow` ends with it.
`seating` ignores it and is not closed by it.

**Unseated, the keyboard is still somewhere.** With no seat of its own it
sits on the first port after the pads, which is where the emulators that
have a keyboard default would have put it anyway, moved off a port a pad
holds. Seating it pins it to a seat ahead of pads seated later; that is the
difference.

## A seated pad's keyboard and mouse are held too

A Steam Controller in lizard mode is four keyboards and four mice; an Xbox pad
over Bluetooth carries a `Keyboard` and a `Mouse` node beside its joystick.
While a pad is seated, or seating is listening to it, padmap grabs those
siblings as well, and releases them with the seat, so a Share button cannot
type into the game and a trackpad cannot move the desktop pointer behind it.
A front-end that was holding them itself in the picker can stop.

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

## `bind`: one control, and a second input for it

```json
{"cmd": "bind", "player": 1, "control": "righttrigger"}
{"cmd": "bind", "player": 1, "control": "righttrigger", "scope": "console:gamecube", "add": true}
```

Captures the next press onto one control, rather than walking the whole
wizard for it. The events are the wizard's, for one step: a `mapping` naming
that control, then `mapping` with `done` when the press lands. Everything
else the pad has bound is left alone -- the run is seeded from what is
stored, so the other controls keep their inputs.

Without `add` the press replaces that control's binding. With `add` it
becomes a **second input for the same control**: both work, and the control
is down while either is. A control's first input is always its binding,
whatever `add` says. Refused with an `error` for a control the layout does
not have, and for a name no control answers to.

The pad's own layout is used, so the front-end does not have to know it.
Legal with no session open, like `map`; it opens only that player's pad.

**On disk.** A control with one input is the object it always was; a control
with more is a list whose first entry is that same object:

```json
"righttrigger": [
  {"kind": "button", "index": 5},
  {"kind": "axis", "index": 5, "value": 1}
]
```

Everything downstream reads the first entry and is unchanged -- the SDL line,
every emulator's config -- because the virtual pad still has one button for
that control. A file with no second inputs is byte-for-byte what it was, so
a rollback keeps working. Re-running the wizard keeps a control's second
inputs, unless the new capture gave that input to some control as a first.

## Seats that exist before the people do: `reserve`

```json
{"cmd": "reserve", "players": 4}
```

A launch is handed the `/dev/input` it starts with (`padmap-rs exec` binds
every node present and covers the raw pads), and nothing can be added to that
namespace afterwards. So a clone published *after* the game starts does not
exist for it: somebody joining mid-play reaches nothing however well the seat
is claimed. `reserve` publishes a clone per seat the launch allows, before it
starts, so all of them are bound. Taking one keeps that exact device -- same
node, same SDL instance id -- rather than replacing it.

The reply is a `state` carrying the seats nobody has taken yet:

```json
{"event": "state", "...": "...", "reserved": [
  {"player": 2, "node": "/dev/input/event21",
   "name": "padmap Player 2", "guid": "0300000005ac0000c405000000000000"}
]}
```

`name` and `guid` are what SDL will report, so ports 2-4 can be bound in an
emulator's config at launch rather than only the seats already taken. A seat
leaves `reserved` when somebody claims it; the device does not change.

padmap writes the reserved seats into what it writes for anybody else: the SDL
database and `env.sh`, and Cemu, Dolphin, ares and Ryujinx. **RetroArch's
launch config is the exception** -- it reserves ports for *seated* players only,
since its indices are worked out per launch from what is plugged in. A
RetroArch game gets the joining player's pad at the index the launch config
gave it, which is the seat they took.

**`players: 0` gives them all back**, which is how a launch ends. Reserved
seats are also adopted by a rebuild, so restoring or accepting a session does
not strand a game bound to them.

Two things to know:

* **It needs a 360 identity** (`PADMAP_PAD_IDENTITY=xbox360` or
  `xbox360-numbered`, or `identity` below). A reserved clone's layout has to be known before its pad
  is, and only that identity's is; `mirror` takes the layout from the pad
  behind the clone, which nobody has picked up yet. Asked for under another
  identity, `reserve` answers with an `error` and changes nothing.
* **Reserve before the launch, not after.** The nodes have to be there when
  `exec` builds its bind plan.

An empty seat is a connected pad that sends nothing, which is what an empty
seat is. It starts sending when somebody takes it.

**One edge, said out loud rather than found later.** `unseat` on a seat that
was reserved destroys that clone like any other, and a clone published after
the launch started is outside its `/dev/input` -- so a player leaving mid-game
takes the seat with them and nobody can take it for the rest of that game.
Joining works; leaving and being replaced does not. Fixed slots (below) are
the answer when a seat has to survive its player: there a leave keeps the
clone where it was.

### From the launch itself: `padmap-rs exec --reserve N`

```sh
padmap-rs exec --reserve 4 -- dolphin-emu -e game.rvz
```

The one step that knows when the bind plan is built is `exec`, so it can do
all of the above itself. Before it reads `env.sh` or looks at `/dev/input` it
makes seats 1 to N exist -- the seated ones as they are, the rest reserved --
switching the daemon to the 360 identity first if it publishes neither 360
identity. Then
it runs the game as it always did. When the game exits, `exec` gives back
what it took: the reservation goes back to what it was, and the identity to
the one it found. A launcher needs nothing else; `--reserve` only ever touches
the daemon already running, and with none running the game still starts,
with a warning, just without the extra seats. Past sixteen it asks for
sixteen, RetroArch's limit.

If `exec` is killed rather than let finish it cannot give anything back; a
daemon started with `--follow` ends with the session anyway, and otherwise
`{"cmd": "reserve", "players": 0}` and `identity` put it right.

## Changing identity without losing anybody: `identity`

```json
{"cmd": "identity", "mode": "xbox360"}
```

`mirror`, `padmap`, `xbox360` or `xbox360-numbered`, as `PADMAP_PAD_IDENTITY`
names them. Every
clone is made again under the new identity and **every seat is kept**: the
same players, the same pads, still published. A front-end sees one `state`,
with the new `identity` and the same `players[]`; an unknown mode is an
`error`, and asking for the identity already in use changes nothing but
still answers with `state`. Refused while a session is open.

The clones are new devices at new nodes, so do this before a launch rather
than during one: a game that already has a clone open keeps the old device,
which no longer sends anything. Switching to `mirror` or `padmap` gives back
any reserved seats, since only a 360 identity can have them.

**`ensure-daemon` compares identities.** It replaces a running daemon whose
`identity` differs from the one it would start, so an `ensure-daemon` run
while a launch has borrowed the 360 identity -- from an environment without
`PADMAP_PAD_IDENTITY=xbox360` -- replaces the daemon and ends every seat. Run
it before the launch, or with the same identity the launch uses.

## Slots that stand before anybody sits in them: `slots`

```json
{"cmd": "slots", "mode": "fixed", "count": 4, "on_leave": "stay", "layout": "position"}
```

Every field is optional; one left out keeps what is in force. The same
settings are read at startup from `PADMAP_SLOTS`, `PADMAP_SLOT_COUNT`,
`PADMAP_ON_LEAVE` and `PADMAP_LAYOUT`, and `serve` takes them as `--slots`,
`--slot-count`, `--on-leave` and `--layout`.

| Setting | Default | Alternatives |
|---|---|---|
| `mode` | `on-demand`: a clone per claim, made when the seat is taken (everything above) | `fixed`: `count` clones made when the daemon starts, kept for its whole life |
| `count` | 4 | 1 to 16 |
| `on_leave` | `stay`: the slot's clone stays at its node and goes quiet; the next hold may take it | `destroy`: the clone goes, and the slot is made again at a new node |
| `layout` | `position`: the bottom face button is the 360's A, whatever it is labelled | `label`: the button labelled A is the 360's A, wherever it sits |

**`layout` applies to any 360 clone**, fixed or on demand, and to nothing
under `mirror` or `padmap`, which copy the pad as it is. A pad's labels are
its capture's layout, or else the console its icon names; a Switch, SNES or
Wii U layout swaps A with B and X with Y, and a layout whose labels are not
the 360's letters -- PlayStation's symbols, Genesis's C -- keeps position,
since there is no label to keep. GameCube's letters already sit where the
360's do. Changing `layout` drives every seated clone again at the same node.

**In `fixed` mode an empty slot is a connected pad that sends nothing.** They
are listed in `state`'s `reserved[]` exactly as reserved seats are, with the
node, name and GUID a game will see, and seated players in `players[]`. A
claim fills the lowest free slot and drives that slot's clone -- the device
that was already there, at the same node -- and a leave under `stay` puts it
back, every button up and every stick at rest. A rebuild keeps every slot's
node too: unseating player 2 no longer makes player 1's clone again. So an
emulator is bound once, to `padmap Player 1..N`, and a controller picked up
mid-game reaches it. `exec` needs no `--reserve` then; asked for no more seats
than the slots, it leaves the daemon alone, and a `reserve` never takes the
slots away.

**A fixed slot is a 360 pad.** Its layout has to be known before its pad is,
so `fixed` needs `xbox360` or `xbox360-numbered`. A daemon on `mirror` or
`padmap` is switched to `xbox360-numbered` -- every slot its own GUID, which
Ryujinx needs -- when `fixed` is chosen, at startup or by `slots`, and
`identity` refuses `mirror` and `padmap` while slots are fixed. Choose
`xbox360` explicitly for the one shared GUID.

**`state` says which is in force**: `slot_mode`, `slot_count`, `on_leave`
and `layout`. A daemon that predates them is on demand, by position. `ensure-daemon` compares them as it
compares `identity`, so run it with the same `PADMAP_SLOTS` the daemon was
started with.

Change it before a launch, not during one: switching to `on-demand` or to
fewer slots destroys the slots nobody sits in, and a game bound to them keeps
a device that no longer sends anything. A count outside 1 to 16 or a name
padmap does not know is an `error`, with nothing changed.
