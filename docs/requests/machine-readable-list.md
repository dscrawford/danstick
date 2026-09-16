# Something a launcher can parse

> **Done**, with one correction worth reading: `motion` is real, but padmap
> does **not** forward it. See "What was built" at the end.


**What GOTG does.** Before a game starts, the launcher needs to know what is
plugged in: how many pads, which player each is, and whether a pad has motion
(Switch games that want a gyro) — in a shell script, with `jq`, on every
launch. Today it asks its own SDL helper, which prints JSON.

**What padmap does today.** `padmap-rs list` prints for a person:

```
3 pad(s), in the order RetroArch would enumerate them:

[0] Xbox 360 Controller ...
```

and the daemon's socket answers `{"cmd":"status"}` with a `state` event
carrying `players`. The socket is the right answer for a front-end that is
already connected — GOTG's picker will use exactly that — but it is a poor fit
for a launcher script that runs once, needs one answer, and should not have to
start a daemon to get it.

**What would be enough.** `padmap-rs list --json`, printing what the command
already knows:

```json
[{"player": 1, "name": "padmap Player 1", "node": "/dev/input/event24",
  "guid": "...", "motion": true, "retroarch_index": 0, "hidden": false}]
```

The fields GOTG needs are the player number, the node, and whether the pad has
motion. The rest is whatever is cheap to include.

**Why it matters more than it looks.** "What is plugged in" is asked on every
launch, in the one place where being wrong is invisible: a launcher that
mis-parses prose does not crash, it silently binds nobody, and the first thing
anyone notices is a controller that does nothing in a game. Prose output is
also free to improve — a column added for a person breaks a caller that was
matching on position, and neither side finds out until a game will not move.

## What was built

`padmap list --json`, printing an array with no daemon needed:

```json
[
  {
    "player": 1,
    "controller": {
      "name": "Nintendo Switch Pro Controller",
      "path": "/dev/input/event18",
      "vid": "057e", "pid": "2009",
      "phys": "...", "uniq": "...", "signature": "057e:2009:...",
      "configured": true,
      "retroarch_visible": false,
      "motion": true,
      "motion_node": "/dev/input/event19"
    },
    "virtual": {
      "name": "padmap Player 1",
      "node": "/dev/input/event24",
      "guid": "030000007e0500000920000001000000",
      "vid": "057e", "pid": "2009", "bustype": 3,
      "index": 0
    }
  }
]
```

The vocabulary is the `controller` event's, key for key, so the picker reading
the socket and the launcher reading this learn one shape rather than two.
Assigned players come first in seat order, so `.[0]` is player 1; an
unassigned controller has `"player": null` and `"virtual": null`.

`jq` for the three fields the request named:

```sh
padmap list --json | jq -r '.[] | select(.player) | "\(.player) \(.virtual.node)"'
padmap list --json | jq '[.[] | select(.controller.motion)] | length'
```

### `motion` does not mean what it looks like it means

**padmap's pads carry no motion at all.** A clone advertises the axes its
source's *joypad* node declares, and a gyro is never among them: hid-nintendo
publishes the IMU as a separate device ("... IMU"), hid-playstation likewise
("... Motion Sensors"), and padmap republishes neither. The two controllers
padmap decodes itself, a Steam Controller and a Switch Pro, decode no motion
either.

So `motion` is reported about the **physical controller**, and `motion_node`
is the device to open for it. Enabling gyro in an emulator that is reading a
padmap pad will get you nothing, however the field reads -- which is why it is
named on the controller rather than on the player, and why the node is given
rather than a bare flag.

For a Switch game that wants a gyro, `motion_node` is the thing to point a
motion source at. Note `retroarch_visible: false` on the same object: if
`padmap hide` is installed, the *joypad* half of that controller is hidden,
but the IMU is a different device and is not.

This was not in the request and is the one thing in it that would have been
wrong to implement as asked: `"motion": true` on the player entry would have
read as "this pad has a gyro", and a launcher acting on it would have enabled
a feature that cannot work.

### A note on `PADMAP_ONLY_DEVICE`

Clone nodes are read straight from sysfs rather than through discovery, and
are deliberately not subject to that switch. It exists so padmap cannot open
or grab hardware it was not told about; a clone is padmap's own output, and
filtering it out of a read-only listing protected nothing while making a
seated player's node read `null`.
