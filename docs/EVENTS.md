# The `controller` event

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
