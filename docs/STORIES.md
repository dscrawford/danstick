# padmap user stories

What padmap promises a person sitting in front of the arcade box, written as
stories: who wants something, what they press or type, what they get, and what
had to be true first. Every story ends with **what has gone wrong here
before**, because almost every guard in this codebase is a scar and the guard
is meaningless without the wound. Citations are section headings in
`FINDINGS.md`.

This is a catalogue, not a design document. Where the code and this file
disagree, the code is right and this file is a bug.

---

> **The front-end was removed.** padmap used to ship a patched Pegasus and a
> QML theme, and this document was written against them: the stories below
> that name a keypress ("Details", "Filters", "Prev-page", "M", a number key)
> were describing that theme's bindings. The daemon commands they exercised
> are all still there and still reachable over the socket
> (`padmap-core`'s `command.rs` lists them), and the mapping wizard is now also
> reachable from a terminal as `padmap map`. What has gone with the front-end
> is the library browser and the collection exporter -- stories S22 and S23,
> deleted rather than rewritten, because their subject does not exist.
>
> padmap is a virtual gamepad. What draws a setup screen on top of it is
> whatever the user points at the socket.


## Reading the key names

The theme talks to Pegasus's *named actions* (`api.keys.isDetails`,
`api.keys.isFilters`, ...), which the user can rebind. The letters below are
Pegasus's defaults, recorded in `pegasus/theme/Library.qml:36-41`:

| Named action | Default keyboard key | Pad equivalent |
|---|---|---|
| Accept | `Enter` / `Return` | A / cross |
| Cancel | `Esc` | B / circle |
| Details | `I` | — |
| Filters | `F` | — |
| Prev-page | `Q` (also `A`) | L1 |
| Next-page | `E` (also `D`) | R1 |
| Page-down | `PgDn` | R2 |

Three gestures are **raw keys**, deliberately not named actions, because every
named action is already spoken for on the screen in question *and* because the
pad half of a named binding cannot reach the front-end while a session is open
— the daemon holds `EVIOCGRAB` on every pad:

* `M` — map the focused game's controller (`Library.qml:241`, `isMapKey`)
* `1`–`4` with no modifier — reset that player slot
  (`ControllerSetup.qml:478`, `resetSlotFor`)
* `PgDn` — toggle favourite, when `api.keys.isPageDown` is absent
  (`Library.qml:230`, `isFavoriteKey`)

Two gestures are made **on the controller itself** and are recognised by the
daemon, not by the front-end, for the same grab reason:

* **hold any button ~0.25 s** (`assign.HOLD_SECONDS`) — claim a slot, choose a
  picker entry, skip a wizard control
* **hold any button ~0.70 s** (`server.CONFIRM_HOLD_SECONDS`) — confirm the
  session

Daemon commands and events are `padmap-core`'s `command.rs` and `state.rs`;
the CLI is `padmap-rs/src/{main,commands}.rs`; the front-end API surface is
`pegasus/0001-padmap-api.patch`.

---

# Setup and assignment

## S1 — A new controller opens the setup screen by itself

**Actor and want.** Someone plugs a controller in. They want it to work. They
do not know a settings screen exists, and should not have to.

**Keys and commands.** None. `Server._poll_new_controllers` runs from `_tick`,
at most once a second (`PAD_SCAN_SECONDS = 1.0`); on finding a pad whose
`profiles.signature` has no stored mapping it broadcasts `{"event":
"newpad"}` and calls `_begin(slots)`. That moves the daemon to `assigning`,
and `theme.qml`'s `Connections { target: api.padmap; onStateChanged }` sets
`setupLoader.active = true` — the screen surfaces because the *state* changed,
not because anything told the theme to open it.

**Preconditions**, all of them enforced by `Server._autosetup_blocked`:

* `PADMAP_NO_AUTOSETUP` is not `"1"`;
* the daemon is not already `assigning`;
* a client has been connected for at least `AUTOSETUP_CLIENT_SECONDS` (3.0 s)
  — a *settled* front-end, not a passing query;
* no game is running — `protocol.game_is_running()` reads
  `$XDG_RUNTIME_DIR/padmap/playing`, which holds `padmap-play`'s pid;
* the model's signature is not already in
  `$XDG_RUNTIME_DIR/padmap/prompted`, and `controllercfg.has_mapping(pad)` is
  false.

**Expected outcome.** The Controller Order screen appears, once per controller
*model* (four ports of one adapter prompt once), and never again for a model
that was declined this login session.

**What has gone wrong here before.**
*"Setting up a controller should not require knowing where the settings are"*
— opening a session stops republishing and grabs every pad, so firing at the
wrong moment does not merely show an unwanted screen, it takes the controllers
away from whatever was using them. *"The dangerous part is when, not
whether"*: the first version counted any connected socket as a front-end, and
`padmap ensure-daemon` — which connects for milliseconds to read status — left
the daemon in `assigning` with every pad grabbed and no virtual pads at all.
*"Declining has to stick"*: the record was in memory, and since
`ensure-daemon` restarts the daemon on every front-end launch, "asked once"
became "asked every single time you start Pegasus". *"A once-a-second disk
scan on the thread that forwards controller events"* and *"The controller lag,
measured"*: the expensive `devices.discover()` ran before the cheap blocked
test, costing 257-294 ms on the input thread once a second, mid-game, only to
conclude it must do nothing — measured as 80-176 ms stalls in a 7.9 ms event
stream. The scan now short-circuits on an unchanged `/dev/input` listing plus
the `prompted` mtime, and asks `_autosetup_blocked` *first*.

## S2 — Details opens the setup screen deliberately

**Actor and want.** Someone in the library wants to re-order their
controllers, or fix one, without unplugging anything.

**Keys and commands.** `I` (Details) in the library →
`Library.qml:894` → `openControllerSetup()` → `theme.qml:33`. If
`api.padmap.connected` is false the theme shows a toast reading
"padmap daemon is not running — start it with `padmap serve`" and does
nothing else. Otherwise `pendingGame` is cleared and the Loader builds
`ControllerSetup`, whose `Component.onCompleted: open()` sends
`{"cmd": "begin", "players": 4}` — but only `if (api.padmap.state !==
"assigning")`.

**Preconditions.** A reachable daemon socket. Search mode must not be active:
while `root.searching`, `I` is the letter "i" and nothing else
(`Library.qml:860`).

**Expected outcome.** The Controller Order screen, four empty slots, with the
existing session left alone if the daemon already had one open.

**What has gone wrong here before.** *"The theme was the fourth thing to go
stale"* — the theme is QML read from `~/.config` at startup, and the symlink
pointing there was made once by hand, so a theme fix was live in the repo,
present in the build, and simply not running. The wrapper now repoints it on
every start. *"`console` is a QML global, and the theme would not load"*: a
signal parameter named `console` failed the whole theme, not just one file —
which is why `mapCurrent()` calls its local `consoleId`. Calling `begin`
unconditionally in `open()` would restart a session the daemon had opened by
itself and discard the claims already in it, which is what the
`state !== "assigning"` guard is for.

## S3 — Holding a button claims the next free player slot

**Actor and want.** Two people want to be player 1 and player 2, in that
order, and the pads are indistinguishable by every static attribute so a
config file cannot express it.

**Keys and commands.** Hold any `BTN_*` (code ≥ `assign.BTN_FIRST` = 0x100) on
the pad for `assign.HOLD_SECONDS` = 0.25 s. `Assigner._check_holds` emits
progress each tick (drawn as the fill on `PlayerSlot`), then appends
`Assignment(player=len(assignments) + 1, ...)` and fires `on_claim`. The
daemon broadcasts `{"event": "claim", "player", "name", "node", "icon",
"configured"}` plus a fresh `state`.

**Preconditions.** A session is open and the pads are grabbed. Slots are
handed out in order, so the hold in flight always belongs to
`players.length` (`ControllerSetup.qml:261`).

**Expected outcome.** One slot per pad, in press order. **The press that
opened the screen does not claim one**: `Assigner.__enter__` calls `_drain()`,
so only a rising edge seen *after* the session opened starts a hold, and the
lone release of an already-held button is discarded. **A tap does not claim
one either**: `_consume` deletes the pending hold on `value == 0`.

**What has gone wrong here before.** `assign.py`'s own docstring records it:
an empty adapter port registered a stray press that a first-edge scheme would
have accepted, silently burning a player slot. "A transient cannot hold; a
human cannot tell the difference." *"Configuration must follow the press, not
the screen"*: `players` when the screen opens describes whatever session came
last — and the daemon now opens this screen itself from a state that already
had players in it — so the theme tracks `claimedHere` and treats a player who
did not press *on this screen* as absent.

**A room holds at once, and each hold is its own.** Four people picking up pads
on "go" is the ordinary case, not the edge one, and every way padmap had of
dropping a hold used to drop all of them. A claim reset the whole assigner
(`one_person_taking_a_seat_leaves_the_next_person_still_holding`); so did
rebuilding the watched set when a pad arrived or left
(`a_pad_switched_on_does_not_cancel_the_hold_already_running`); so did setting
the same hold length again, and so did a fifth person finding the room full. No
down edge comes back for a thumb that never lifted, so each of those cost
somebody their fill with nothing on screen to explain it. Holds are keyed by a
pad's place in the watched set, so a rebuild now *renumbers* them
(`Assigner::remap`) rather than clearing them, and what a claim drops is its own
pad's hold. Two claims completing in one tick are two positions in a list the
first claim rebuilds, so `tick_seating` resolves them to pads before it touches
the list (`two_people_pressing_on_go_are_two_seats_not_one`).

**Joining does not unplug the people already playing.** A claim used to call
`start_republisher`, which destroys every clone and makes them again — new
nodes, new SDL instance ids, so a game mid-read saw all its controllers
disappear: a dead fd for an emulator that does not reopen, a reshuffled port
order for one that does. A seat now adds its own clone and leaves the rest at
the same device and node (`join_republisher`,
`a_join_leaves_the_players_already_in_the_game_plugged_in`). Nor does a seat cost more the fuller the
room: what a seated player's files say cannot change because somebody else sat
down, so a join writes them rather than working them out again
(`publish::Cache`, `a_join_does_not_work_the_rest_of_the_room_out_again`).

**Seats exist before the people do.** A launch is handed the `/dev/input` it
starts with — `exec` binds every node present — and nothing can be added to
that namespace afterwards, so a clone published once the game is running does
not exist for it however well the seat is claimed. `{"cmd": "reserve",
"players": N}` publishes a clone per seat the launch allows *before* it starts,
with a mapping written for each, and taking one keeps that exact device rather
than replacing it (`clone::reserve`/`create_on`,
`a_seat_reserved_for_a_launch_keeps_its_node_when_somebody_takes_it`). It needs
the 360 identity, because a reserved clone's layout has to be known before its
pad is. `docs/EVENTS.md`, "Seats that exist before the people do".

## S4 — Holding again confirms, and accepts

**Actor and want.** The order is right; the user wants to be finished and go
play.

**Keys and commands.** Hold any button on an **already-claimed** pad for
`server.CONFIRM_HOLD_SECONDS` = 0.7 s. `Assigner._consume` forwards button
activity on claimed pads through `on_claimed_event`, the daemon stamps
`_confirm_started[pad.path]`, and `_tick_confirm` broadcasts
`{"event": "confirm", "frac": ...}` each tick until 1.0, then calls
`_accept()`. The screen draws that as the bar under the status line
(`ControllerSetup.qml:305`).

**Preconditions.** At least one claim. `_accept` refuses an empty session with
`{"event": "error", "message": "nothing assigned yet"}`. No modal flow may be
in flight — `_tick` returns early while `_choice`, `_mapping` or
`_calibration` is set, and every one of those clears `_confirm_started` on
entry and exit.

**Expected outcome.** In `_accept`: assignments persisted to
`assignments.json`; `_write_controller_configs` writes `sdl_controllers.txt`
and broadcasts `sdl_mapping`; `_start_republisher` creates the uinput pads and
writes `launch.cfg` + `launch.args`; state becomes `ready`; `accepted` is
broadcast, which closes the screen (`ControllerSetup.qml:100`).

**What has gone wrong here before.** 0.7 s rather than 0.25 s deliberately:
confirming ends the session, so it must take a more decided press than the
flick that claims a slot. The `_confirm_started.clear()` calls scattered
through the modal-flow entry points are all one bug — "the button press passed
through to the confirmation underneath" (`server.py:70`): the button that
opened a picker is usually still down, and its release would otherwise resume
a confirm hold and end the session mid-wizard.

## S5 — Cancel abandons the session and releases the pads

**Actor and want.** Someone opened the screen by accident, or is done, and
wants everything back the way it was.

**Keys and commands.** `Esc` (Cancel) on the Controller Order screen →
`ControllerSetup.leave()` → `api.padmap.cancel()` then `closed()`.
`Next-page` (`E`) also leaves, via `openGamepadEditor()`, which cancels the
session on the C++ side before relaying the request. Losing the last client
does it too: `Server._drop_client` calls `_cancel()` when no clients remain
and a session is open.

**Preconditions.** None — cancelling with nothing open is legal and does
nothing but re-broadcast state.

**Expected outcome.** `Server._cancel` closes any picker or wizard properly
(so no overlay is left believing it is active), calls
`_end_session(release=True)` which ungrabs and closes every device, and — if a
session really was open — restarts the republisher for the *previous*
assignments, landing on `ready`, or `idle` if that fails. Cancelling while
already `ready` does **not** drop to `idle`.

**What has gone wrong here before.** *"Backing out of setup must not cost you
your controllers"* — opening a session stops republishing, so declining a
screen the daemon opened by itself left the machine with no virtual pads at
all and nothing but a daemon restart to bring them back. `_cancel`'s
`had_session` guard is a separate fix: cancelling while `READY` used to drop
the state to idle while the pads were still live, so the front-end believed
nothing was assigned. *"The live ports test was leaving the daemon stranded"*
is the same failure from the test side. The `_drop_client` path exists because
a front-end that crashes, is killed, or quits without sending `cancel` would
otherwise leave `EVIOCGRAB` held on every pad — every controller on the
machine dead until the daemon is restarted. `serve()`'s `SIGTERM`/`SIGINT`
handler exists for the same reason: a killed daemon never runs its `finally`.

## S6 — Filters drops all claims and starts over

**Actor and want.** Player 1 and player 2 pressed in the wrong order. Start
again without leaving the screen.

**Keys and commands.** `F` (Filters) → `ControllerSetup.qml:404` →
`api.padmap.reset()` → `{"cmd": "reset"}` → `Server._reset` →
`Assigner.reset()`, then a fresh `state` broadcast.

**Preconditions.** A session must be open; `_reset` returns silently when
`self._assigner is None`.

**Expected outcome.** Every claim dropped, the session still open and the pads
still grabbed, the front-end told. `Assigner.reset` also calls `_drain()`, so
a button still held from the previous round cannot immediately re-claim.
`Server._reset` clears `_confirm_started` for the same reason.

**What has gone wrong here before.** Nothing recorded specifically for reset —
but the `_drain()` in it and the `_confirm_started.clear()` beside it are both
instances of the recurring "the button that did the last thing is still down"
failure that produced *"Configuration must follow the press, not the screen"*
and the wizard's `active_keys()` guard in *"Skip could not have been a button"*.

---

# Mapping

## S7 — The wizard walks the layout, one control at a time

**Actor and want.** A controller padmap has never seen. The user wants its
buttons to mean the right things.

**Keys and commands.** After a first claim, `ControllerSetup.maybeOfferSetup`
sends `{"cmd": "choose_layout", "player": N}`. The user pushes the stick or
d-pad **left/right** on the pad to move the strip and **holds any button** to
choose; the daemon then starts `capture.MappingRun` and broadcasts
`{"event": "mapping", "layout": {...}, "index", "total", "control", "label",
"captured"}` per step. `MappingOverlay` draws the pad from the layout in the
event and points an arrow at the control being asked for.

**Preconditions.** An open session with a device still open for that player
(`Assigner.device_for`). The layout is `layout_id` if given, else
`icons.for_pad(pad, overrides)` — profile first, then `icons.json`, then the
device name.

**Expected outcome.** Every control of that console's layout, in the daemon's
order, each bound from one deliberate press. Finishing stores the capture
under the pad's profile against the chosen scope.

**What has gone wrong here before.** *"Nothing let you say which console a
controller is"* — `_begin_mapping` inferred the layout from the icon, so an
adapter whose icon had never been set was walked through the generic gamepad
and asked to press an X, a Y and two analogue triggers it does not have, with
no way to answer. *"The picker cannot be a front-end widget"*: the daemon
holds the pads, so the picker has to be daemon-driven and `choices` has to
carry whole layouts, or a theme could not draw what it is offering.
*"Rendering caught what reading could not"* — layout coordinates are artwork
and were wrong in ways that read fine as data. *"A capture has to be followed
by a gap"*, *"A d-pad wired to an analogue axis answered two prompts at
once"*, and *"An analogue trigger rests at one end, and the wizard measured
from the middle"* are three separate reports that all present as the wizard
filling itself in or freezing.

## S8 — Holding any button skips a control the pad does not have

**Actor and want.** An N64 pad has no X button. The user needs to get past
that prompt.

**Keys and commands.** **Hold any button** on the pad — `capture.MappingRun`
recognises the hold itself. From a keyboard, `F` (Filters) on the mapping
overlay also skips (`MappingOverlay.qml:340`, `!root.choosing`), which sends
`{"cmd": "skip_control"}`.

**Preconditions.** A wizard is running and not on the picker step.

**Expected outcome.** That control is left unbound and the wizard advances; a
*tap* of the same button still binds it.

**What has gone wrong here before.** *"Skip could not have been a button, and
the d-pad bug was in emission"*. Reported as "Skip is Select, which is only
mapped halfway through" — true, and worse: it could never have worked. The
daemon holds `EVIOCGRAB` and republishing is stopped, so `Keys.onPressed` in
the overlay can only ever have fired from a keyboard. Picking a different
button would not have helped; the input does not reach the front-end. Skip is
therefore a *gesture the daemon recognises*, and it cannot name a button
because nothing is mapped yet. Moving skip to a hold moved binding from the
press to the release, which killed the autorepeat special case and let
`device.active_keys()` replace the "settling" guess that used to eat one press.

## S9 — Prev-page asks what a mapping is FOR

**Actor and want.** Verbatim: *"the mapping that my gamecube controller uses
for n64 games, and then a universal configuration in general"*.

**Keys and commands.** `Q` (Prev-page) on the Controller Order screen →
`ControllerSetup.qml:415` → `api.padmap.chooseScope(lastClaimedPlayer)` →
`{"cmd": "choose_scope", "player": N}`. The daemon builds
`capture.scope_options(scopes, default_layout, recent)`: "any game" (the
default scope), one entry per console, then up to `protocol.RECENT_GAMES` = 5
recently launched games. Answered the same way as the console picker —
left/right on the pad, hold to choose. Choosing "any game" leads on to the
console question; any other scope goes straight to the wizard, because the
console *is* the control set.

**Preconditions.** An open session with at least one claim
(`claimed.length > 0` in the theme). Recent games come from
`$XDG_RUNTIME_DIR/padmap/lastgame.json`, written by `padmap.launch`.

**Expected outcome.** The capture is filed under `""`, `console:<id>` or
`game:<console>/<key>` on that controller's profile, beside its other scopes,
and `_players_payload` reports the scope list so the strip can mark what
already exists.

**What has gone wrong here before.** *"One controller is not one mapping"* and
*"Asking what a mapping is *for*"*. *"Per-game scope was limited to the one
game just launched"*: `read_last_game` returned exactly one game, so someone
who had since started something else could no longer reach the game they
wanted to fix. The list is capped at 5 because the strip is worked from the
pad one step at a time and sits after the console entries. The pre-list
`lastgame.json` format is still read, or a user upgrading mid-session loses
the scope for the game they are playing right now. *"The icon had to stop
following the layout"*: taking the icon from a *scoped* capture relabelled a
GameCube pad as an N64 controller forever, which is why `_store_mapping` only
adopts an icon `if not scope`. `_pending_scope` is cleared on every picker
open and consumed by `_begin_mapping`, or a scope chosen for a run that never
reached the wizard would file the *next* capture — possibly for a different
controller.

## S10 — M on a focused game maps for that game or its console

**Actor and want.** *"press a key on the keyboard while focused over a game,
then we get the 'Is this mapping for console or game?', and we already know
both because we're on the game select screen."*

**Keys and commands.** `M`, no modifier, on a focused game →
`Library.isMapKey` → `mapCurrent()`, which reads `game.extra["console"]` and
`game.extra["gamekey"]` and emits `openMappingFor(consoleId, key, title)` →
`theme.qml:45` sets `pendingGame` and opens the setup screen. The first pad to
**hold a button** claims a slot, and `ControllerSetup.maybeStartGameMapping`
sends `{"cmd": "map_for_game", "player", "console", "key", "title"}`. The
daemon builds exactly two entries with `capture.game_scope_options`, titled
"Map for <title>?".

**Preconditions.** `api.padmap.connected`. Not in search mode. The game must
carry `x-console` — with none, `mapCurrent` says "No console known for this
game" and stops, and the daemon refuses with the same message if asked
anyway. The claim is the controller select: it needs nothing mapped and works
on a pad padmap has never seen.

**Expected outcome.** Two choices — "<Console> games" or that one game — and
then the wizard for the console's layout.

**What has gone wrong here before.** *"Mapping a pad from the game, where the
answers are already known"* records both gates, both mutation-tested: one
press must start **one** wizard, because the `claimed` signal and the `state`
event both fire for a single press in no guaranteed order
(`gameMappingStarted`); and it must follow a *press*, because the daemon opens
this screen from a state that already lists players from a finished session,
and mapping one of those would configure whichever pad was player 1 last time
while the user held a different one (`claimedHere`). The first version of that
second check was worthless and mutation testing said so — it asserted nothing
happened with an *empty* player list, where the loop never runs.
*"The console and the key are computed once, by the exporter"*: `x-console`
and `x-gamekey` come from `pegasus.render` via `layouts.for_core` and
`profiles.game_key`, the same two functions `padmap.launch` uses; a second
derivation in the theme or the daemon would drift silently into a mapping
filed under a scope nothing looks up. *"`console` is a QML global"*: naming
the signal parameter `console` failed the entire theme to load. And *"A test
buildout"* found `game_scope_options` offering a game-only scope when the
console was unknown — reachable, because `pegasus.render` writes `x-gamekey`
for every entry but omits `x-console` for an unrecognised core.

## S11 — Number keys throw a slot's controller away and start over

**Actor and want.** *"I should be able to reset a controller config with a
keyboard binding"* — the mapping is wrong in a way the wizard cannot be talked
out of.

**Keys and commands.** `1`–`4`, **no modifier**, on the Controller Order
screen → `ControllerSetup.resetSlotFor` → `api.padmap.forgetPad(slot)` →
`{"cmd": "forget_pad", "player": N}`. Checked last in the key chain, so every
`api.keys` gesture wins first — those are rebindable and could be bound to a
digit.

**Preconditions.** The slot must be within `slotCount` **and** actually hold a
controller (`slot <= players.length`); an empty slot is not bound at all.
The daemon requires an open session with the device still open, and checks
that *before* deleting anything.

**Expected outcome.** `profiles.forget(pad)` removes every scope, the model's
`prompted` record is dropped, and `_begin_layout_choice` runs immediately —
straight into the wizard, not back to the menu.

**What has gone wrong here before.** *"Resetting a controller, and why the key
has to be on the keyboard"*: keyboard is forced, not stylistic — the daemon
grabs every pad, so a pad gesture could not reach the front-end, the same
constraint that killed "press Select to skip". Bound per slot rather than to
"the last one claimed" because with two controllers assigned, "the last one"
is precisely the ambiguity someone is trying to resolve. Every scope goes, or
someone re-runs the wizard and still meets old behaviour from a per-console
mapping they had forgotten was there. *"QC: the reset destroyed before it
validated"*: `_forget_pad` deleted the profile and *then* called
`_begin_layout_choice`, which has its own guards — so with no session the
controller lost its configuration and got no wizard to build a new one,
strictly worse than the wrong mapping it started with. The general form, as
`FINDINGS.md` puts it: validate everything before performing the destructive
half.

## S12 — Finishing the wizard measures the sticks before accepting

**Actor and want.** An analog stick that behaves as though it were only off or
full. *"can the controller configuration do the calibration?"*

**Keys and commands.** No key. `MappingOverlay` finishing fires
`mappingFinished(stored)`; `ControllerSetup.qml:104` sets
`acceptAfterCalibration` and calls `calibration.start(player, name, false)`,
which sends `{"cmd": "calibrate", "player": N}`. The daemon runs
`CalibrationRun` through five phases, each reported as
`{"event": "calibration", "phase", "frac", "player"}`:
`await_rest` (waits for a button) → `rest` (0.8 s, `PHASE_SECONDS`) →
`await_reach` (waits for a button) → `reach` (ends on a button press, but not
before `REACH_MINIMUM_SECONDS` = 1.2 s, with `frac` reporting swept coverage
rather than a countdown) → `icon`. The icon step is answered from the keyboard:
**Left/Right** to move, **Enter** to confirm → `{"cmd": "set_icon", "player",
"icon"}`, **Esc** to abort → `{"cmd": "configure_end"}`. Then
`calibrationFinished` fires and the theme sends `accept`.

**Preconditions.** An open session; a pad the daemon can resolve for that
player. A pad with no centring axes is not an error — `_begin_calibration`
stores an empty profile and jumps straight to `icon`.

**Expected outcome.** Axes measured against the range the stick *reaches*, not
the range the adapter declares, stored on the profile, and only then the
session accepted.

**What has gone wrong here before.** *"Configuring a controller now measures
its sticks"*: calibration was only ever offered for a pad padmap had never
seen, so anyone reaching a controller through the wizard was never prompted
and every profile on the machine had `axes: {}`. **The order is forced**:
`accept` writes the RetroArch profile and the SDL mapping *and* ends the
session and releases the pads, so calibrating afterwards would be measuring a
controller nobody is holding. *"Calibration would have wrecked the triggers it
had just captured"* is the neighbouring trap. The whole flow is modal in the
daemon because a user rotating a stick presses buttons incidentally and those
must not burn a player slot or trip the confirm gesture
(`server.py:67-74`). The overlay's 4 s watchdog exists because the daemon
refuses to calibrate a player it holds no claim for, nothing listened for the
error, and the overlay sat on "Starting..." forever with the pads grabbed —
no way forward and no way out. *"The harness was counting five screens as
one"* is the test-side scar: a correct implementation looked broken because
`deleteLater()` screens stayed connected to the shared stub `api`.

## S13 — Details recalibrates the last assigned pad

**Actor and want.** A wrong icon or a bad calibration, fixable without
unplugging anything.

**Keys and commands.** `I` (Details) on the Controller Order screen →
`ControllerSetup.qml:433` → `calibration.start(last.player, last.name,
false)` → `{"cmd": "calibrate", "player": N}`. Calibration only — the button
editor is reached deliberately with `E` (Next-page).

**Preconditions.** At least one player in `api.padmap.players`. With none, the
screen says *"Hold a button on the controller first, then press Details to
calibrate it."* rather than firing a command the daemon would refuse.

**Expected outcome.** The Calibration Overlay opens on the most recently
assigned pad and runs the S12 phase sequence.

**What has gone wrong here before.** *"'Calibration needs a button held
first' was an accident, not a design"*. Measured against the live daemon:
asking to calibrate player 1 returned `no controller assigned to player 1`
about a controller that was assigned, republishing and visible in the state
event a moment earlier. `_pad_for_player` consulted the session's claims and
nothing else, and a session starts with none — so *every* per-player command
failed for the opening seconds of a session. It now takes the claim when there
is one and the stored assignment otherwise, and the claim must win, or
re-assigning a slot would configure the pad it replaced. What that deliberately
left alone is `_players_payload`, which still reports claims only: making
*that* fall back would undo S3's fix and have the screen offer to configure a
controller nobody had touched. The second half is that the key said nothing at
all — an empty loop is indistinguishable from a broken key, on a screen where
the pads are grabbed so there is no other feedback.

---

# Playing

## S14 — Launching a game resolves the most specific mapping

**Actor and want.** Play a game and have the buttons be right, including the
ones the user corrected for this console or this game specifically.

**Keys and commands.** `Enter` (Accept) on a focused game →
`game.launch()` → the collection's `launch:` line, which names
`~/.local/share/padmap/bin/padmap-play`. That wrapper runs
`padmap play -- "$@"`, which does
`layouts.for_core(core)` → console, `profiles.game_key(console, rom)` → key,
records the launch with `protocol.write_last_game`, and calls
`retroarch.install_profiles(assignments, console=, game=, context=)`.
Resolution order is `game:<console>/<key>`, then `console:<id>`, then `""`.

**Preconditions.** Assignments exist in `assignments.json`; the ROM path
exists (it is identified by *existing*, not by position, since padmap-play
prepends flags and a front-end may append more).

**Expected outcome.** One autoconfig `.cfg` per managed player in
`$XDG_RUNTIME_DIR/padmap/autoconfig/udev/`, whose header names the scope it
was resolved from, written where RetroArch will read it.

**What has gone wrong here before.** *"Where resolution happens, and why it is
not in the daemon"*: nothing but `padmap-play` knows what is about to be
played. *"The daemon and the launcher wrote different profiles to the same
file"* — `Server._start_republisher` wrote a context-free profile that resolves
to the default, and republishing restarts for reasons that have nothing to do
with the game (a session accepted, a pad reconnecting, the daemon upgraded), so
a game-specific mapping was live one launch and silently gone the next. It now
passes `protocol.read_last_game()` as context. *"The game-specific mapping did
save, and did apply"* records the opposite finding — the instinct was to hunt
a persistence bug and there wasn't one; the real cause was S1's input lag.
`padmap.launch.main` catches every exception and returns 0, because a game
that refuses to start over a mapping is far worse than one played on the
default. *"Both left and right are set when pressing the d-pad"*: an axis
written as `input_left_btn = "-0"` is parsed by `strtoull` as button 0, so
both directions collapsed onto one button — axes now go under
`input_<name>_axis`.

## S15 — Only padmap's virtual pads reach RetroArch

**Actor and want.** One controller should be one controller, not one
controller plus the adapter port behind it.

**Keys and commands.** `sudo padmap hide` installs udev rules clearing
`ID_INPUT_JOYSTICK` on the physical adapters (S19). `launch.cfg` reserves each
managed slot by the virtual pad's *name* and clears every managed bind to
`nul`. `PADMAP_ONLY_VIRTUAL=1` additionally sets
`SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT="0x1209/0x0001"` in the Pegasus
wrapper, because SDL classifies devices from evdev capability bits and does
not honour the udev rules at all.

**Preconditions.** The rules must cover every adapter padmap republishes —
including ones plugged in after they were generated.

**Expected outcome.** `padmap list` shows hidden pads as `--` with no index;
RetroArch enumerates only the virtual pads, in a stable order.

**What has gone wrong here before.** *"Physical pads remain visible unless
hidden"* and *"Hiding pads must not use the same filter padmap discovers
with"* — hiding by the discovery filter hides padmap's own view of the pads.
*"The identity is now a switch, and the bus travels with it"*: what the
virtual pads advertise decides the SDL GUID every mapping is written under, so
`ensure-daemon` compares `identity` as well as `build` — a daemon started
without `PADMAP_ONLY_VIRTUAL` and a front-end started with it disagree about
which pads exist, and the symptom is a machine with no controllers at all.
`PADMAP_ONLY_VIRTUAL` is off by default because with it on and the daemon not
republishing, Pegasus has no controller and you need a keyboard to reach the
setup screen. *"uinput advertises force feedback it does not have"* is a
neighbouring scar.

## S16 — Unassigned core ports are emptied

**Actor and want.** One controller in an N64 game should be one player, not
four.

**Keys and commands.** `retroarch.write_launch_args` writes `--nodevice PORT`,
one token per line, to `launch.args`; `padmap-play` reads it back into an
array and passes it before the caller's own arguments, so a caller can still
override a port by hand.

**Preconditions.** `padmap-play` must be the binary actually running — see
S22.

**Expected outcome.** Ports with no assigned player get `RETRO_DEVICE_NONE`.

**What has gone wrong here before.** *"Emptying an unassigned core port is not
a config setting"* — `input_libretro_device_pN = "0"` in the launch override
does nothing; RetroArch reads that key only from `.rmp` remap files. *"Two
fixes shipped without a test, and both were wrong"*: both were reasoned from
source and shipped without ever running the real chain, and the second was
correct but never executed, because the running daemon kept regenerating
`launch.cfg` with the old code and Pegasus still pointed at a previously built
wrapper. *"An unwritten player slot is not an empty one"*: leaving a slot out
of the override does not clear it, so all sixteen are written. *"Why
`--nodevice` and not `input_max_users`"* records the alternative that does not
work.

## S17 — Two players each get their own pad, slot and profile

**Actor and want.** Two people, two controllers, both working, neither
stealing the other's inputs.

**Keys and commands.** Two claims (S3), one confirm (S4). `launch.cfg`
reserves player 1 and player 2 by their virtual pads' names and gives each the
index `retroarch.visible_order` predicts; `install_profiles` writes one
autoconfig per player, each resolved from that pad's own scopes;
`write_sdl_mappings` writes one SDL line per player, keyed on the *physical*
pad's GUID because the virtual pad mirrors its identity by default.

**Preconditions.** The udev rules must cover both adapters.

**Expected outcome.** `{1: index, 2: index}`, two profiles, two SDL lines, and
ports 3-16 emptied.

**What has gone wrong here before.** *"Two player: what was checked, and what
was actually wrong"*. Reported as "P1 seemed to have issues when p2 was
added". Everything padmap generated was verified *correct* — both virtual pads
existed, the indices matched, both profiles were right. What was wrong is that
the installed udev rules had fallen behind the hardware: the GameCube adapter
was plugged in after `padmap hide` ran, so RetroArch saw its four physical
ports *as well as* the virtual pads — six pads where there should have been
two. "Nothing anywhere noticed, which is the recurring shape of every bug in
this file: padmap generates a thing, the system drifts, and the two are never
compared again." `hide.unhidden` now reads the installed file back and
`ensure-daemon` prints the discrepancy.

---

# Maintenance

## S18 — `padmap ensure-daemon` replaces a daemon running stale code

**Actor and want.** After a rebuild, whoever starts the front-end wants the
fix they just made to actually be running.

**Keys and commands.** `padmap ensure-daemon [--check] [--timeout SECONDS]`,
run by the `pegasus-fe` wrapper and by `padmap-start` unless
`PADMAP_SKIP_DAEMON_CHECK=1`. It repoints
`~/.local/share/padmap/bin/padmap-play` unconditionally, warns about stale
collections and about adapters `hide.unhidden` reports, then compares the
daemon's reported `build` and `identity` against `protocol.build_id()` and
`virtual.identity_mode()`. A mismatch means `SIGTERM` to *exactly the pid the
daemon reported*, then `_spawn_daemon` and `_wait_for_daemon`.

**Preconditions.** `--check` reports and changes nothing, exiting non-zero.
Restarting is only affordable because `Server.restore()` republishes the saved
assignments on startup.

**Expected outcome.** A daemon running this code, with the controller order
intact, before the front-end starts. Non-fatal: the wrapper prints
"continuing without a current daemon" rather than refusing to open.

**What has gone wrong here before.** *"Operational trap: the daemon outlives
the code"* and *"Keeping the daemon from going stale"* — `padmap serve` holds
the modules it started with, so after a rebuild it keeps serving the previous
version: still answering, still writing a plausible `launch.cfg`. A
controller-port bug that was genuinely fixed went on reproducing for exactly
this reason, and nothing on disk showed why. *"Restarting must be scoped to
one socket"*: an earlier `_stop_daemon` matched on the command line and, run
from a test with its own runtime dir, **took down the user's real daemon**.
`protocol.daemon_pids` now matches argv *structurally* on the last two
elements and filters by `XDG_RUNTIME_DIR` — which is also why `_spawn_daemon`
must keep argv exactly `["-m", "padmap.cli", "serve"]`, and why
*"The daemon had no log"* could not be fixed with `serve --verbose`: a flag
there makes every running daemon invisible and `ensure-daemon` starts a second
one beside the first. `SIGTERM`, not `SIGKILL`: the handler releases every
`EVIOCGRAB`, and a killed daemon leaves the machine with no working
controllers.

## S19 — `sudo padmap hide` installs rules covering every physical pad

**Actor and want.** Stop RetroArch seeing the adapters padmap republishes.

**Keys and commands.** `padmap hide` prints the rules and an install hint;
`sudo padmap hide` installs them to `hide.RUNTIME_RULES_PATH`
(`/run/udev/rules.d/99-padmap.rules`), reloads and triggers udev. `--print`
forces printing even as root; `--install` without root prints
"Installing needs root. Re-run: sudo padmap hide".

**Preconditions.** The pads are read from `/sys` via `devices.discover()`,
**not** from the assignment: under `sudo`, `XDG_RUNTIME_DIR` points at root's,
so `assignments.json` is usually not even readable.

**Expected outcome.** One rule per vid:pid, four-digit lower-case hex, each
preceded by a comment naming the pad; ports of one adapter deduped; pads with
no vid/pid named as skipped. Idempotent. The caution is printed: while these
rules are active and padmap is not running, those controllers are invisible to
RetroArch entirely.

**What has gone wrong here before.** *"The udev rules were generated from the
assignment, not the hardware"* — rules derived from an unreadable assignment
file cover nothing at all. *"`padmap hide` installs rather than dictates"*:
printing a script for someone who already typed `sudo` to paste back into the
same shell is a step that exists only to be got wrong. *"NixOS cannot install
these rules imperatively"* is why the hint offers a module snippet instead.
And *"A test buildout actually found"*: `hide.unhidden` read with
`Path.read_text()` and `UnicodeDecodeError` is not an `OSError`, so a
non-UTF-8 `99-padmap.rules` traced back the entire `ensure-daemon` start path
— the docstring already promised a malformed file was safe; it was one
exception class short of true.

## S20 — `padmap forget` makes a controller be offered setup again

**Actor and want.** Start this controller over from nothing.

**Keys and commands.** `padmap forget` (connected controllers) or
`padmap forget --all` (every stored profile). It deletes the matching
`*.json` from `profiles.profile_dir()` **and** clears the `prompted` records
via `cli._forget_prompted`. It then points at
`~/.config/pegasus-frontend/sdl_controllers.txt` — Pegasus's own mappings,
which padmap has no business deleting silently but which are the other half of
"reset this controller".

**Preconditions.** None. The running daemon notices `prompted` changing
through its mtime, so this takes effect without a restart.

**Expected outcome.** The controller reports itself as new, and S1 offers it
again on the next scan.

**What has gone wrong here before.** *"Two memories, and `forget` only cleared
one"* / *"Why `forget` still did nothing"* — there are two records, and the
interesting case is a controller stuck in the second with nothing in the
first: never configured, so it reports itself as new; already asked about, so
the daemon stays silent. Keying the clear off the *deleted profiles* meant
there was nothing to key off, and `forget` did nothing at all — including
bailing out early with "no stored profiles" before ever reaching the clear.
The daemon's `_reload_prompted_if_changed` is the other half: without it the
daemon served the copy it read at startup, the profile was gone, the pad
reported itself as never configured, and the screen still never appeared.

## S21 — `padmap clean-config` removes padmap leftovers from retroarch.cfg

**Actor and want.** Undo values `config_save_on_exit` persisted from a launch
override written before padmap started disabling it.

**Keys and commands.** `padmap clean-config [--dry-run] [--config PATH]`
(default `~/.config/retroarch/retroarch.cfg`) →
`retroarch.clean_user_config`.

**Preconditions.** The file must exist. Explicitly invoked, **never** part of
`launch`: it is the only code in padmap that writes to the user's RetroArch
config.

**Expected outcome.** padmap-named reservations, their types, and
`joypad_index` are rewritten back to RetroArch's `N-1`, and only where already
present. Deliberately no rule for `input_libretro_device_pN` — RetroArch never
writes it to `retroarch.cfg`, so it cannot have leaked there, and a rule for
it could only damage a `.rmp` file someone pointed `--config` at. The original
is backed up to `retroarch.cfg.padmap-backup`.

**What has gone wrong here before.** *"The override was leaking into
retroarch.cfg, and silence was not neutral"* / *"`--appendconfig` is persisted
by `config_save_on_exit`"*: the symptom this fixes is a stale
`input_player3_joypad_index` equal to an assigned player's index, so one
controller drives two ports — visible as four players in an N64 game.
*"Cleaning up what already leaked"*: measured on the real config as 7 lines
changed out of 3382, idempotent on a second run.

# Coverage

Which file exercises each story **today**. Established by reading
`tests/check_*.py` and `tests/e2e_*.py`, not by guessing. `tools/preview_*.py`
and `tools/spike_*.py` are excluded: previews render, spikes explore, neither
asserts.

| Story | Promise | Exercised today by |
|---|---|---|
| S1 | An unknown pad opens setup by itself, only when it is safe | `check_autosetup.py` (blocked moments, once per model, forget re-offers), `check_poll_cost.py` (scan signature, rate limit, cost), `e2e_pegasus.py` (real Pegasus, first sight vs. later) |
| S2 | Details opens setup deliberately | **NO COVERAGE** of `Library.isDetails → openControllerSetup → theme.qml`. Partial only: `check_theme_loads.py` compiles `Library.qml` and `theme.qml` |
| S3 | A hold claims a slot; the opening press and a tap do not | `e2e_picker.py` ("holding a button claims a slot"), `check_theme_setup.py` (a player who did not claim *here* is ignored). **No `check_*.py` exercises `assign.Assigner` hold/tap/`_drain` at all** |
| S4 | Holding again confirms and accepts | `check_daemon_commands.py` (accept routed; accept with nothing claimed), `e2e_picker.py` ("accepting writes both files"). **`_tick_confirm` and `CONFIRM_HOLD_SECONDS`: NO COVERAGE** |
| S5 | Leaving by any route releases the pads | `check_autosetup.py` ("backing out of a session" → previous assignments back on air), `check_daemon_commands.py` (cancel while ready stays ready) |
| S6 | Filters drops all claims, session survives | `check_daemon_commands.py` ("resetting the claims keeps the session"). **Theme half (`F` → `api.padmap.reset`) and `Assigner.reset`'s drain: NO COVERAGE** |
| S7 | The wizard walks the layout, one control at a time | `check_capture.py` ("walking a whole layout", refusals), `check_axis_rest.py` (~40 axis/hat/trigger cases), `check_control_identity.py`, `e2e_picker.py` ("walking it, one button per prompt") |
| S8 | Hold any button to skip a control | `check_capture.py` ("holding any button skips the control"; "a tap is a binding, a hold is a skip"), `check_daemon_commands.py` (`skip_control` routed and safe with nothing running). **Keyboard skip (`F` in `MappingOverlay`): NO COVERAGE** |
| S9 | Prev-page asks what a mapping is FOR | `check_theme_setup.py` ("Prev-page asks what a mapping is FOR"), `check_recent_games.py` (the strip: any game, consoles, recents), `check_capture.py` ("choosing what a mapping is for"), `check_scopes.py`, `check_scope_store.py`, `e2e_picker.py` (second half) |
| S10 | M maps for that game or its console | `check_daemon_commands.py` ("mapping for the game the library is sitting on"), `check_recent_games.py` ("two entries wide"; "no console means no console scope"), `check_theme_setup.py` (one press → one wizard; must follow a press). **Library half (`Key_M` → `mapCurrent` → `openMappingFor`, and the "No console known" note): NO COVERAGE** |
| S11 | Number keys reset a slot's controller | `check_theme_setup.py` (number keys, empty slot, `Ctrl+1`), `check_daemon_commands.py` (validates before destroying; takes every scope; goes to the wizard), `check_autosetup.py` (same, plus "nothing thrown away when the wizard could not open"), `check_scope_store.py` (`profiles.forget`) |
| S12 | The wizard measures the sticks before accepting | `check_theme_setup.py` (measure-then-accept ordering; abandoned wizard measures nothing), `check_control_identity.py` (`calibratable_axes` refusals). **`server.CalibrationRun` phase machine, `REACH_MINIMUM_SECONDS`, `coverage()`, and `calibrate.rest_from_samples`/`merge_reach`: NO COVERAGE** |
| S13 | Details recalibrates the last assigned pad | `check_theme_setup.py` ("says what is missing" with no claims), `check_daemon_commands.py` ("calibrate works from the first moment of a session"), `check_autosetup.py` (`_pad_for_player` claim-vs-stored) |
| S14 | The most specific mapping reaches RetroArch | `check_scopes.py` (four launches, four answers), `check_scope_store.py` (keys, order, migration), `check_launch_profiles.py` (per-scope bindings in the emitted `.cfg`), `e2e_scoped_launch.py` (the real chain), `e2e_picker.py` |
| S15 | Only the virtual pads reach RetroArch | `check_hide_rules.py` (rules, coverage, install, `unhidden`), `check_launch_profiles.py` / `check_launch.py` (reservation by name, every slot written), `check_mapping.py` (GUIDs/identity), `e2e_ports.py` (real Pegasus → RetroArch). **`PADMAP_ONLY_VIRTUAL` / `SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT` in the wrapper: NO COVERAGE** |
| S16 | Unassigned core ports are emptied | `check_launch_profiles.py` ("`--nodevice` for exactly the unassigned core ports"; the wrapper's one-token-per-line read), `check_launch.py`, `e2e_ports.py` |
| S17 | Two players, two pads, two profiles | `check_launch_profiles.py` (players 1 and 3 reserved, the rest cleared), `check_launch.py`, `check_mapping.py`. **No end-to-end two-player run: `e2e_ports.py` launches with one** |
| S18 | `ensure-daemon` replaces a stale daemon and restores assignments | `e2e_daemon.py` (real wrapper, deliberately stale daemon, `--skip-check` control) |
| S19 | `sudo padmap hide` covers every physical pad | `check_hide_rules.py` (~30 cases: text, install, idempotence, `/etc` vs `/run`, non-UTF-8, NixOS snippet), `check_autosetup.py` (`unhidden`, install chain) |
| S20 | `forget` makes a controller be offered again | `check_autosetup.py` (both memories; a controller with no profile at all), `check_scope_store.py` (`forget` takes every scope, is honest twice, touches nobody else), `check_poll_cost.py` (the `prompted` mtime forces a rescan) |
| S21 | `clean-config` removes padmap leftovers | **NO COVERAGE.** Nothing in `tools/` references `clean_user_config` or `clean-config` |

> **Some of the scripts cited below no longer exist.** Removing the front-end
> took these with it: `e2e_daemon.py`, `e2e_launch.py`, `e2e_ports.py`, `e2e_sdl_reload.py`, `e2e_pegasus.py`, `e2e_favorites.py`, `check_exporter.py`, `check_library.py`, `check_favorites.py`, `check_hostile_library.py`, `check_theme_loads.py`, `check_theme_routing.py`, `check_theme_setup.py`, `preview_library.py`, `preview_favorites.py`. Every one of them either drove the QML theme
> or built and ran the front-end itself, so there was nothing left for them to
> assert. A cell naming one of them is overstating the coverage that story
> actually has, and the honest reading is "less than this says" until somebody
> re-establishes it against the socket instead. They are left in place rather
> than quietly deleted so it is visible *what* was being checked and is no
> longer.


## The rows worth acting on

Ranked by how quiet the failure would be.

1. **S21 `clean-config` — nothing at all.** It is the only code in padmap that
   rewrites the user's own `retroarch.cfg`, and it takes a backup, so a
   regression damages a real file. Its FINDINGS entry is a list of things it
   must *not* touch (`input_libretro_device_pN`), which is precisely the kind
   of rule that rots unnoticed.
2. **S22 `export-pegasus` — nothing at all.** Every launch, every scope
   resolution and every "map for this game" depends on `launch:`, `x-console`
   and `x-gamekey` being right in the collection files, and this is the third
   place a stale path has already shipped.
3. **S12's daemon half.** `CalibrationRun` is the state machine the whole
   first-run experience walks through, and no check file constructs one. The
   theme's ordering is covered; the thing being ordered is not.
4. **S4's confirm timer.** `_tick_confirm` is the only way a session ever
   ends successfully from the pad, and nothing measures it. A mutation to
   `CONFIRM_HOLD_SECONDS` or to the `>= 1.0` test would pass the suite.
5. **S3's `Assigner`.** The hold/tap distinction is the project's founding
   premise — "a transient cannot hold; a human cannot tell the difference" —
   and it is only exercised by `e2e_picker.py`, which needs uinput and a live
   daemon, so it is the first thing skipped on a constrained machine.
6. **S2, S6 and S10's theme halves.** Three keys that route into `api.padmap`
   with nothing asserting they still do. `check_theme_setup.py` shows the
   pattern for testing exactly this against a stub `api`; the library screen
   has no equivalent for `I` and `M`.
7. **S17 end to end.** The one report in `FINDINGS.md` where everything padmap
   generated was correct and the system had drifted underneath it. That class
   of failure is only visible from a real two-pad run.

---

# Interactions with no story

Everything below is reachable in the shipped code and is not described by
S1-S23. Grouped by where it lives.

## CLI subcommands (`rust/crates/padmap-rs/src/`)

| Command | What it does | Notes |
|---|---|---|
| `padmap list` | Prints the pads in RetroArch's enumeration order, marks hidden ones `--`, and names groups indistinguishable by every static attribute | The diagnostic that explains *why* assignment is done by pressing a button |
| `padmap setup [-n N]` | The whole of S3/S4 from a terminal, `Ctrl-C` to finish | `KeyboardInterrupt` keeps what was already claimed rather than discarding it |
| `padmap run` | Republish assigned pads and hold them until `Ctrl-C`; no RetroArch | The pre-daemon way to work |
| `padmap launch [--log[=PATH]] [-- ...]` | Republish, then start RetroArch with `--appendconfig` and the `--nodevice` flags, capturing output | Unknown args after `launch` are forwarded, because `argparse.REMAINDER` refuses any leading option |
| `padmap serve` | The daemon itself | argv must stay exactly `["-m","padmap.cli","serve"]` — see S18 |
| `padmap calibrate [-f]` | Terminal calibration, two phases, Enter-driven | The CLI twin of S12/S13, with `--force` to redo a configured pad |
| `padmap forget --all` | The whole store, not just connected pads | The `--all` half of S20 |
| `padmap hide --print` | Print even as root | |
| `padmap ensure-daemon --check` | Report staleness, change nothing, exit non-zero | |
| `padmap -v/--verbose` | Debug logging | Must precede the subcommand |

## Daemon commands and events (`padmap-core/src/command.rs`, `padmap-daemon/src/server.rs`)

* **`{"cmd": "status"}`** — asks for a `state` event, and is answered with a
  `sdl_mapping` event *as well*, on every connect. Pegasus reads
  `sdl_controllers.txt` once at startup, so a front-end that started before
  the daemon last wrote it is running on stale bindings; re-sending is
  idempotent because SDL replaces a mapping for a GUID it already has.
* **`{"cmd": "set_icon", "player", "icon"}`** — the last step of calibration.
  This is what retires the built-in vid/pid guess table: once a pad has been
  through setup, its icon comes from the person who owns it. Rejects anything
  outside `icons.ICON_NAMES`.
* **`{"cmd": "configure_end"}`** — the escape hatch out of any modal flow
  (`Esc` on either overlay). Closes a calibration, a picker and a wizard, in
  that order, keeping nothing from the wizard.
* **`{"cmd": "map", "player", "layout", "scope"}`** — map under an explicitly
  named layout and scope, bypassing both pickers. Exposed to QML as
  `startMapping(int, QString)` in the Pegasus patch and **called by no theme
  file**. Live API surface with no user-facing route.
* **`{"event": "newpad", "names": [...]}`** — broadcast when S1 fires, listing
  the controllers that triggered it. **No client consumes it.** The Pegasus
  patch dispatches exactly `state`, `progress`, `confirm`, `claim`,
  `accepted`, `calibration`, `mapping`, `layout_choice`, `sdl_mapping` and
  `error`; `newpad` and `pads` are both broadcast and both dropped on the
  floor, and the theme surfaces the screen from the state change instead.
* **`{"event": "sdl_mapping", "lines": [...]}`** — SDL database lines handed
  to the front-end so it can call `SDL_GameControllerAddMapping` on a
  controller that is already open. The whole of *"A mapping written mid-session
  never reached the running front-end"* and of `check_sdl_live.py` /
  `e2e_sdl_reload.py`, and no story mentions it.
* **`{"event": "progress", "frac"}`** and **`{"event": "error", "message"}`** —
  the feedback channel the slot rings and the overlays' failure text are drawn
  from. `{"event": "pads", "count"}` is sent at session start and, like
  `newpad`, is not dispatched by any client.
* **`{"event": "layout_choice", "active": false}`** — a picker closing. Sent
  explicitly, because the front-end renders overlay visibility from the
  daemon's own flags and a dropped picker would leave an overlay waiting for
  events that cannot arrive.
* **Restore on start** — `Server.restore()` republishes `assignments.json` by
  device *path*, skipping pads that are gone rather than faking them, because
  a dead virtual pad in the enumeration shifts every index after it.

## Environment switches

`PADMAP_NO_AUTOSETUP` (suppress S1), `PADMAP_NO_KEYBOARD_HOLD` (stop reading
keyboards for a held space bar; `seat_keyboard` still works),
`PADMAP_ONLY_VIRTUAL` (SDL-level pad
hiding), `PADMAP_PAD_IDENTITY` (what the virtual pads advertise, and therefore
every SDL GUID), `PADMAP_SKIP_DAEMON_CHECK` (skip S18 in both wrappers),
`PADMAP_BUILD_ID` (S18's staleness comparison), `PADMAP_AUTOCONFIG_DIRS`,
`PADMAP_PROFILE_DIR`, `PADMAP_MAME_TITLES`, `PADMAP_THUMBNAILS`,
`PADMAP_THUMBNAIL_SERVER`, `PADMAP_ONLY_DEVICE`, `PADMAP_PLAY`. None of these
appears in a story, and three of them (`ONLY_VIRTUAL`, `PAD_IDENTITY`,
`SKIP_DAEMON_CHECK`) change behaviour a user would experience as a fault.

## Wrappers (`flake.nix`)

* **`padmap-play`** — writes `$XDG_RUNTIME_DIR/padmap/playing` with its own
  pid and traps `EXIT INT TERM` to remove it. This is why it no longer
  `exec`s: something must outlive RetroArch to clean up, and the marker is
  what stops S1 firing mid-game. Falls back to a bare `retroarch` when
  `launch.cfg` does not exist, because RetroArch treats a missing
  `--appendconfig` target as fatal.
* **`pegasus-fe`** — repoints the theme symlink (but never over a real
  directory someone put there), runs `ensure-daemon`, and execs the patched
  binary by absolute path so it cannot re-exec itself.
* **`padmap-start`** — `ensure-daemon` then Pegasus, one name for "start the
  machine", setting `PADMAP_SKIP_DAEMON_CHECK=1` so the check does not happen
  twice.
* **`tools/padctl.py`** — a minimal socket client (`begin`, `watch`, …). The
  reference the C++ patch is checked against and the fastest way to see what a
  front-end actually receives.

## Known gaps in the feature set itself

Recorded in `FINDINGS.md` under *"What is not covered"* and *"Per-game scope
was limited to the one game just launched"*:

* There is **no way to delete a scoped mapping**. Re-mapping replaces one,
  which covers "I got it wrong"; nothing covers "I want this console to fall
  back to my default again".
* The per-game scope offered from the *setup screen* only reaches games
  launched through `padmap-play` this login session, since `lastgame.json`
  lives in `XDG_RUNTIME_DIR`. S10 is the route around that.
* The core-to-console table is verified for four core names and plausible for
  the rest.
* `-L /nix/store/...` core paths in collection `launch:` lines have the same
  staleness hazard S22 fixed for the launcher, and are deliberately not
  addressed: they come from the user's playlists, and a missing core fails
  loudly instead of silently doing the wrong thing.
