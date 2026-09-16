# Something a launcher can parse

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
