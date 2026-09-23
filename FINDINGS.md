# Controller identity and port ordering: what actually works

Research notes from investigating why RetroArch controller ports shuffle.
Conclusions are specific to the hardware measured here, but the mechanism
is general.

## How RetroArch assigns pad indices (udev driver)

`input/drivers_joypad/udev_joypad.c`:

- `udev_joypad_init` (~line 1050) builds a `udev_enumerate` matching
  `ID_INPUT_JOYSTICK=1` on subsystem `input`.
- libudev returns that list **sorted by syspath**, as a plain string sort.
  Verified empirically with
  `udevadm trigger --dry-run --verbose --subsystem-match=input --property-match=ID_INPUT_JOYSTICK=1`
  — the output order matched a naive syspath sort exactly.
- Each entry with a devnode goes to `udev_check_device` →
  `udev_find_vacant_pad()`, i.e. **first vacant slot in enumeration order**.

So: pad index = position in syspath-sorted order, with hotplug filling gaps.

Because the sort is lexicographic, a set of nodes spanning a digit-width
boundary (`input8`, `input9`, `input10`) sorts as `input10, input8, input9`.
Relative order is otherwise stable across reboots, since the leading path
components are USB topology.

## Existing reservation feature

RetroArch already has device reservation (`tasks/task_autodetect.c:982`,
`reallocate_port_if_needed`):

```
input_playerN_reserved_device         = "<value>"
input_playerN_device_reservation_type = 1  # 1=preferred, 2=reserved
```

`<value>` is matched two ways:

- `"vvvv:pppp"` — hex vid:pid
- anything else — exact match against device name or display name

`phys` is **not** consulted, though it is fully plumbed
(`udev_joypad.c:498-504` reads `EVIOCGPHYS`/`EVIOCGUNIQ` into `pad->phys`,
concatenated with no separator; stored via `input_config_set_device_phys`,
readable via `input_config_get_device_phys`). Autoconfig *profile* files do
support an `input_phys` key worth +/-10 in match scoring
(`task_autodetect.c:201-206`) — but that selects a button-mapping profile,
not a port.

Adding a `phys:` match arm to the reservation parser is ~30 lines. **It was
not worth doing** — see below.

## Measured hardware

Eight pads across three adapters:

| pads | adapter | HID parent | phys | uniq | distinguishable? |
|---|---|---|---|---|---|
| 4 | Mayflash GC (0079:1843) | `…1843.000A` | `usb-0000:00:14.0-4.2/input0` | empty | **no** |
| 2 | HuiJia (0e8f:3013) | `…3013.000B` | `usb-0000:00:14.0-4.3/input0` | empty | **no** |
| 2 | USB GamePad (0079:1879) | `…000C` / `…000D` | `…-4.4/input0` / `…-4.4/input1` | empty | yes |

The four Mayflash nodes are byte-identical on every evdev attribute: name,
phys, uniq, vid, pid, version, properties. They share one USB interface and
one HID device. The only difference is the `inputN` ordinal.

The Mayflash also presents all four pads whether or not controllers are
plugged into its ports.

## Measured behaviour (2 controllers, 8 nodes)

With one GameCube controller (Mayflash port 1) and one N64 controller
(USB GamePad adapter) connected:

- **Idle noise: none.** All 8 nodes silent over 5s untouched
  (`tools/idlenoise.py`). Floating/unpopulated ports do not spray events at
  rest, so a quiet node is genuinely quiet.
- **No mirroring.** Each press landed on exactly one node
  (`tools/watch.py`): GC -> `event23`, N64 -> `event29`. No adapter presents
  one controller as several input devices.

So press-to-activate is viable: one press identifies exactly one node.

An earlier `pressorder.py` run registered presses on `event27`/`event28`
(empty HuiJia ports) that neither of the above tests reproduces. Unexplained;
most plausibly transients from cable handling during the run. Mitigation
regardless: **require a deliberate signal** (button held ~250ms, or
press-then-release of the same code) rather than a single rising edge. A
spurious transient cannot satisfy that; a human cannot notice it.

`tools/pressorder.py` as written assigns on the first edge per node and so is
vulnerable to exactly that. It needs the hold-to-confirm change before it is
used for anything real.

### Why the existing config was broken

Six of the eight nodes are dead phantom ports. Live pads sit at index 0 (GC)
and index 6 (N64). The pre-existing config read:

    input_player1_joypad_index = "5"   -> event28, empty HuiJia port
    input_player2_joypad_index = "1"   -> event24, empty Mayflash port

Both players were bound to phantom ports. Multi-port adapters publish all
their ports unconditionally, so index-based assignment has to be re-derived
whenever the set of *populated* ports changes -- which no static config can
do.

## Conclusion

**No static identifier can distinguish these pads.** Not phys, not uniq, not
name, not vid:pid, in any combination. The kernel does not expose one. A
config-file scheme keyed on device identity cannot work here, which is why
the RetroArch phys patch was abandoned — it would have fixed 2 of 8 pads.

For indistinguishable pads, **a human pressing a button is the only available
source of identity.** Press-to-activate is therefore not a UI nicety layered
over port assignment; it is the assignment mechanism.

### Chosen approach: uinput republishing

1. Open the physical nodes by explicit path, `EVIOCGRAB` them.
2. Setup dialog: user presses a button on each pad in the order they want.
3. Create virtual pads via uinput **in that order**, assigning `phys` values
   we control, so identity is unique by construction.
4. Hide the physical nodes from RetroArch with a udev rule clearing
   `ID_INPUT_JOYSTICK` (this is exactly the property matched at
   `udev_joypad.c:1053`), leaving only our virtual pads visible.
5. `input_playerN_joypad_index = N-1` is then trivially correct.

Side benefits: phantom Mayflash ports are filtered naturally; a pad that
sleeps and reconnects can be reattached to its existing virtual pad rather
than landing in a new slot.

Costs: battery/LED/gyro extras are lost unless proxied, and one extra process
hop in the input path. Force feedback is *not* a loss -- python-evdev exposes
`UInput.begin_upload`/`end_upload`/`begin_erase`/`end_erase` plus
`InputDevice.upload_effect`, which is enough to proxy rumble through.

`/dev/uinput` is writable without root on this machine via a logind seat ACL.
`evsieve` (in nixpkgs) is a working reference for this style of republishing.

## Verified against a live RetroArch

Confirmed by running `retroarch --verbose` with all 8 pads connected:

- **Enumeration order prediction is exact.** RetroArch mapped event23..event30
  to pad indices 0..7, precisely matching the syspath sort. This is what makes
  `retroarch.compute_pad_indices()` trustworthy.
- Virtual uinput pads are classified `ID_INPUT_JOYSTICK=1` and the name/phys
  we set survive to sysfs, so RetroArch enumerates them normally.
- `UInput.from_device` clones EV_KEY/EV_ABS and preserves EV_FF.
- With `input_max_users = 8` and 8 physical pads, all player slots are
  consumed and virtual pads land at indices 8+.

### Reservation-by-name: CONFIRMED WORKING

Earlier attempts failed to confirm this and the notes here recorded it as
unverified. That was an observability problem, not a feature problem. The
reservation-decision lines are `RARCH_DBG`, which is compiled in but gated at
runtime -- setting **both** `log_verbosity = "true"` and
`frontend_log_level = "0"` makes them appear. `padmap launch --log` sets both.

With `input_player{1,2}_reserved_device = "padmap Player {1,2}"` and
reservation type 2:

```
[Autoconf] Examining reserved device for player 1 type 2: padmap Player 1 against 1209:0001.
[Autoconf] Reserved device matched.
[Autoconf] Device "padmap Player 1" (1209:1) is reserved for player 1, ...
[Autoconf] Device "padmap Player 2" (1209:1) is reserved for player 2, ...
```

Non-padmap devices are correctly rejected against the same reservations
(`Device "USB GamePad USB GamePad" (79:1879) is not reserved for any player
slot`). So the unique names our virtual pads carry are sufficient to pin
players **with no RetroArch patch**, exactly as hoped.

Note the earlier misreading that caused the confusion: `[Autoconf] X
configured in port N` reports `autoconfig_handle->port + 1`, the **pad
index**, not the player slot. It is not a reservation outcome and never
reflects one.

One real hazard, found by reading the source: `reallocate_port_if_needed()`
computes `first_free_player_slot` while *skipping* RESERVED slots, then
early-returns if that index is `>= input_max_users` -- before reaching the
reservation matching loop. Setting `input_max_users` to the number of reserved
players therefore guarantees reservations never match. padmap deliberately
does not set `input_max_users`.

**Design response, unchanged:** padmap still writes explicit
`input_playerN_joypad_index` values computed from the live enumeration, and
still emits reservation lines. Both mechanisms are now verified, and they
agree; keeping both means a change in enumeration order between config
generation and RetroArch startup is caught by the reservation.

### Physical pads remain visible unless hidden

Observed with 3 physical nodes plus 2 virtual: RetroArch enumerates all five
and configures ports 1-5. The virtual pads land at the correct indices and
players, but the grabbed physical pads still occupy slots and clutter the
input menus. `EVIOCGRAB` stops event *delivery*; it does not remove the node
from udev enumeration. Only the `ID_INPUT_JOYSTICK` udev rule (`padmap hide`)
removes them.

### Hiding pads must not use the same filter padmap discovers with

`padmap hide` clears `ID_INPUT_JOYSTICK`, and `devices.discover` originally
filtered on exactly that property. Installing the rules would therefore have
made every controller invisible to padmap as well, so `padmap setup` would
find nothing and the assignment step could never run again -- unrecoverable
without hand-removing the udev rules, which on NixOS means a rebuild.

Discovery is now capability-based (absolute axes plus a button in the
`BTN_JOYSTICK`..`BTN_THUMBR` range, which is what udev's own `input_id`
builtin uses), with `ID_INPUT_JOYSTICK` recorded separately as
`Pad.retroarch_visible`. Only `compute_pad_indices` filters on it, via
`discover(retroarch_only=True)`, because pad-index prediction must count
exactly what RetroArch counts.

Verified: the capability check matches the three connected pads and nothing
else across every input device on the system -- no keyboard or mouse
false positives.

### NixOS cannot install these rules imperatively

`/etc/udev/rules.d` is a symlink into the Nix store, so `sudo tee` fails
there. Two working routes:

- `/run/udev/rules.d/99-padmap.rules` -- udev reads it, it is tmpfs, and a
  reboot undoes it. The right place to *try* the rules.
- The flake's `nixosModules.default`, exposing `programs.padmap.hideDevices`
  as a list of `"vvvv:pppp"` strings, for a permanent install.

### uinput advertises force feedback it does not have

`evdev.UInput` writes `ff_effects_max` into the uinput setup unconditionally,
defaulting to 96, **even when `EV_FF` is absent from the capability set**.
RetroArch read that back and logged "Pad #3 supports 96 force feedback
effects" for a virtual pad whose source (`event23`, ff_effects_count = 0) has
no `EV_FF` node at all. `virtual.create` now passes `max_effects` mirroring
the source, so a non-rumbling pad reports 0.

Consequence for testing: none of the pads measured in this session support
force feedback, so the FF proxying path in `Republisher._proxy_upload` remains
unexercised. It needs a pad that actually rumbles.

## The override was leaking into retroarch.cfg, and silence was not neutral

Reported symptom: launching an N64 game with **one** assigned controller gave
four players in RetroArch, with the single N64 pad duplicated across ports.

Two mechanisms, compounding.

### `--appendconfig` is persisted by `config_save_on_exit`

`--appendconfig` merges into the *live* config. `config_save_on_exit` -- on in
this config, and RetroArch's own default -- then writes the whole merged result
back over `retroarch.cfg` at quit. So padmap's per-launch values were being
made permanent, one launch at a time.

Direct evidence: `retroarch.cfg` held

    input_player1_reserved_device = "padmap Player 1"
    input_player2_reserved_device = "padmap Player 2"   # 2-player session, long over
    input_player1_device_reservation_type = "2"
    input_player2_device_reservation_type = "2"

for keys padmap has never written to that file. The module docstring's claim
that it "leaves the user's config untouched" was simply false.

Fix: the launch override now sets `config_save_on_exit = "false"`, which is
what actually makes a per-launch override per-launch. Cost: settings changed
in the RetroArch menu during a padmap launch are not saved. Remaps (`.rmp`)
and core options (`.opt`) are written through separate paths and are
unaffected.

### An unwritten player slot is not an empty one

`launch_config` only emitted `input_playerN_joypad_index` for *assigned*
players. Every other slot kept whatever `retroarch.cfg` held -- and those
values are neither empty nor inert. RetroArch's own default is
`input_playerN_joypad_index = N-1`, and this config had accumulated:

    input_player1_joypad_index = "0"   # padmap, correct
    input_player3_joypad_index = "0"   # stale, same pad, still honoured

Two player slots on one pad index is not rejected by RetroArch; both ports
receive that pad's input. Mupen64Plus-Next declares four controller ports, so
the result was four players with player 3 mirroring player 1.

Fix: **write all sixteen slots, every launch.** Unmanaged slots get

- a *vacant* pad index -- one at or above the count of RetroArch-visible pads,
  distinct per slot. Not RetroArch's `N-1` default: with the physical pads
  unhidden, index `N-1` is a real controller.
- `reserved_device = ""` and `device_reservation_type = "0"`, so a reservation
  from a session with more players cannot hold a slot open for a virtual pad
  that no longer exists.
An assignment whose virtual pad is missing from the enumeration is now emptied
too. Previously it was skipped with a comment, which is the worst case: the
slot keeps its stale value for a player padmap believes it is in charge of.

That fixed the *duplication*, and only the duplication. The core still
presented four controllers -- see below.

## Emptying an unassigned core port is not a config setting

With the duplicate gone, one assigned controller still produced four players
in an N64 game. The first attempt at this wrote

    input_libretro_device_pN = "0"      # RETRO_DEVICE_NONE

into the launch override. **It does nothing.** `configuration.c` touches that
key in exactly two places, `input_remapping_load_file` and
`input_remapping_save_file` -- it lives in `.rmp` remap files and is neither
loaded from nor saved to `retroarch.cfg`. Nothing reads it out of
`--appendconfig`.

`command.c`, `command_event_init_controllers`, is what actually decides:

```c
for (port = 0; port < num_core_ports; port++) {
   unsigned device = RETRO_DEVICE_NONE;
   for (i = 0; i < num_active_users; i++)          /* == input_max_users */
      if (port == settings->uints.input_remap_ports[i])
         { device = input_config_get_device(port); break; }
   core_set_controller_port_device(port, device);
}
```

So a core port is emptied only if nothing sets `input_libretro_device[port]`,
or if `input_max_users` stops the inner loop before reaching it.

### Measuring this needs a probe core

Two obvious observables are both worthless:

- mupen64plus's `Game controller N (Standard controller) has ...` lines are
  printed inside `retro_load_game`, *before* RetroArch runs
  `CMD_EVENT_CONTROLLER_INIT`. They do not change even with `-N 1`.
- The `[Input] Input device ID %u is unknown` warning never fires for a bogus
  id, because `libretro_find_controller_description` returns NULL for an
  unknown one and the warning is guarded on a non-NULL `desc`.

`tools/probe_libretro.c` is a ~120-line libretro core that declares four
controller ports and logs every `retro_set_controller_port_device` call. Run
under `Xvfb` (the `null` video driver aborts with "Cannot initialize input
driver"), it settles the question directly. Measured, one assigned player:

| lever | result |
|---|---|
| nothing | ports 0-3 all `JOYPAD` |
| `input_libretro_device_p{2,3,4} = "0"` in appendconfig | ports 0-3 all `JOYPAD` -- **no effect** |
| `--nodevice 2 --nodevice 3 --nodevice 4` | port 0 `JOYPAD`, ports 1-3 `NONE` |
| `input_max_users = "1"` in appendconfig | port 0 `JOYPAD`, ports 1-3 `NONE` |

### Why `--nodevice` and not `input_max_users`

Both work. `input_max_users` collides with the reservations: RetroArch skips
RESERVED slots when computing `first_free_player_slot` and early-returns if
that index reaches `input_max_users`, so constraining it to the player count
disables the order-independent binding padmap relies on as its safety net.
A command-line flag has no such interaction -- verified end-to-end through the
real `padmap-play` wrapper, where the reservation still matched
(`Device "padmap Player 1" ... is reserved for player 1`) while ports 1-3 went
to `NONE`.

Flags are emitted for every unassigned slot up to 16. Ports above the core's
own `num_core_ports` are never visited by that loop, so padmap does not need
to know which core is about to run.

The cost is a second artefact: a front-end runs `padmap-play <rom>` and knows
nothing about players, so the flags are written to `launch.args` beside
`launch.cfg` and the wrapper splices them in. Anything that cannot be said as
a config setting has to travel that way.

### Cleaning up what already leaked

`padmap clean-config` rewrites only the keys padmap could have written --
padmap-named reservations, their types, and `joypad_index` back to RetroArch's
`N-1` -- and only where they are already present. Deliberately no rule for
`input_libretro_device_pN`: RetroArch never writes it to `retroarch.cfg`, so it
cannot have leaked there, and a rule for it could only damage a `.rmp` file
someone pointed `--config` at. It backs up to `retroarch.cfg.padmap-backup`
first.
Explicitly invoked, never part of `launch`: it is the only code in padmap that
writes to the user's RetroArch config.

Measured on the real config: 7 lines changed out of 3382, idempotent on a
second run.

## Two fixes shipped without a test, and both were wrong

The port work above was reasoned entirely from source and never run against
the real chain. Twice. The failure modes were different and neither was
visible from reading:

1. `input_libretro_device_pN = "0"` in the launch override does nothing.
   Reading `command_event_init_controllers` made it look right; the key is
   only ever read from `.rmp` remap files.
2. The replacement (`--nodevice`) was correct **and still did not take
   effect**, because the code was not what was running. The daemon had been
   started before the change and kept regenerating `launch.cfg` from the old
   module, and Pegasus pointed at the previously built `padmap-play`, which
   knows nothing about `launch.args`. Both artefacts on disk looked fine.

The second one is the more dangerous: source correct, behaviour unchanged, and
nothing in the file contents to indicate it. Any check that reads the generated
config rather than running it would have passed.

### tests/e2e_ports.py

Drives the whole chain with nothing stubbed:

    Pegasus  ->  padmap-play  ->  retroarch  ->  core

Pegasus launches the game itself. The generated theme polls `api.allGames`
and calls `.launch()` on the first game once the scan completes, so the
handoff is Pegasus's own rather than a subprocess call faked by the script --
which matters, because the wrapper and its environment are half of what broke.

The core is `tools/probe_libretro.c`: four declared ports, and it prints the
device type RetroArch assigns to each, then calls `RETRO_ENVIRONMENT_SHUTDOWN`
so the front-end that spawned it is not left suspended.

Measured:

| invocation | ports 0-3 | |
|---|---|---|
| `e2e_ports.py` | `1,0,0,0` | PASS |
| `e2e_ports.py --players 2` | `1,1,0,0` | PASS |
| `e2e_ports.py --drop-args` | `1,1,1,1` | FAIL, as required |

`--drop-args` removes `launch.args` and reproduces the original bug exactly.
It is there so the test is known to be able to fail; a green run of something
that cannot go red proves nothing.

Defaults worth keeping: `retroarch.cfg` is a **copy of the real one**, stale
player bindings included, because overriding those is the entire point of
writing all sixteen slots -- a pristine config would not exercise it. And
`--live` uses `$PADMAP_PLAY`, the wrapper a front-end would really spawn,
rather than building a fresh one, so it cannot pass while the deployed chain
is still broken.

Needs Xvfb. RetroArch's `null` video driver aborts with "Cannot initialize
input driver", so a real display is required; a virtual one keeps the run off
the screen.

### mupen64plus-next does honour RETRO_DEVICE_NONE

Worth confirming separately, since the probe core only proves what RetroArch
*sends*. `libretro/libretro.c:2204`, comment "Needed to be able to detach
controllers for Lylat Wars multiplayer":

```c
case RETRO_DEVICE_NONE:
   if (controller[in_port].control) controller[in_port].control->Present = 0;
   else                             pad_present[in_port] = 0;
```

and `Present` is read on the joybus path
(`backends/plugins_compat/input_plugin_compat.c:90`), which returns
`M64ERR_SYSTEM_FAIL` for an absent controller -- i.e. the game sees an empty
port, not an idle one.

### Operational trap: the daemon outlives the code

`padmap serve` holds the modules it started with. It rewrites `launch.cfg` on
every reassignment, so a long-running daemon will happily overwrite a
freshly generated config with old-format output, and will not write
`launch.args` at all. After changing anything in `retroarch.py`, restart it.
`e2e_ports.py --live` is the check that catches this.

## Keeping the daemon from going stale

`padmap serve` holds the modules it was started with. After a rebuild it keeps
answering and keeps writing a plausible `launch.cfg`, generated by the old
code -- with nothing on disk to show it. That is how a correctly fixed port
bug went on reproducing.

Three pieces:

**A build id.** `protocol.build_id()` returns `$PADMAP_BUILD_ID`, which the
flake wrapper sets to the store path of `./src`. That path changes with every
source edit, which is exactly the property wanted. Outside Nix it falls back
to the newest `*.py` mtime in the package directory, so a dev shell gets a
usable id too. The daemon reports it in every `state` event, alongside its
pid.

**`padmap ensure-daemon`.** Starts a daemon if none is listening, replaces one
whose build id differs, and does nothing if it is current. `--check` reports
without changing anything.

**Restore on startup.** The prerequisite for the other two. The daemon used to
write `assignments.json` and never read it, so restarting cost the user their
controller order -- far too expensive to do automatically. `Server.restore()`
now republishes the saved assignments on startup, matching pads by device
path, skipping any that are genuinely gone rather than faking them (a dead
virtual pad in the enumeration would shift every index after it).

The Pegasus wrapper runs `ensure-daemon` before starting the front-end.
Deliberately non-fatal: a front-end that refuses to open because a daemon
would not start is worse than one with no controllers, since the latter can
still be driven by keyboard to fix things. `PADMAP_SKIP_DAEMON_CHECK=1`
disables it.

### Restarting must be scoped to one socket

First cut of `_stop_daemon` found its target with
`pgrep -f "padmap.cli serve"`. That matches **every** padmap daemon the user
is running, whatever `XDG_RUNTIME_DIR` it is on. Running the new e2e test --
which deliberately starts its own daemon on a temporary runtime dir -- killed
the real one as a side effect, and it was only noticed because a health check
immediately afterwards said "no daemon running".

The daemon now reports its pid and `ensure-daemon` signals exactly that.
SIGTERM, never SIGKILL: the handler releases every `EVIOCGRAB` on the way out,
and a killed daemon leaves the machine with no working controllers at all.

The fallback for a daemon too old to report a pid is `protocol.daemon_pids()`,
which is worth using everywhere rather than `pgrep -f`. It filters by
`XDG_RUNTIME_DIR` from `/proc/<pid>/environ`, and matches argv
**structurally** -- `argv[-2:] == ["padmap.cli", "serve"]` with `-m` present --
rather than as a substring. `pgrep -f "padmap.cli serve"` also matches any
shell whose command line mentions the string, which during this work meant a
diagnostic command matching *itself*: it looked convincingly like a second
daemon had appeared on the real runtime dir, and had the fallback fired it
would have SIGTERMed the terminal. Both e2e tools use the same helper for
their cleanup.

`tests/e2e_daemon.py` now asserts that a daemon on the real runtime dir
survives the run, so this cannot regress silently.

### tests/e2e_daemon.py

Starts a daemon claiming an obviously stale build id, runs the real Pegasus
wrapper, and checks the daemon was replaced by one reporting the store build.
Entirely inside a temporary `XDG_RUNTIME_DIR`, with no assignments, so it
grabs no pads.

| invocation | result |
|---|---|
| `e2e_daemon.py` | PASS -- stale daemon replaced, bystander survived |
| `e2e_daemon.py --skip-check` | FAIL, as required |

`--skip-check` sets `PADMAP_SKIP_DAEMON_CHECK=1`, so the wrapper's check does
not run and the stale daemon survives. Same purpose as `--drop-args` on the
other test: proof that a green run means something.

Two practical notes for anyone running these. Pegasus does not exit on
`Qt.quit()` from a theme, so the test polls for the daemon swap -- which
happens before Pegasus even starts -- and then kills the process group. And
both tools pick the first free X display rather than a fixed number, because
Xvfb's lock outlives the process and two runs back to back would otherwise
collide for a reason unrelated to what they test.

## One command to start the machine

`nix run .#padmap-start` makes the daemon current, then hands off to Pegasus.

Worth being clear that this adds a *name*, not a mechanism: `nix run
.#pegasus` already did the same thing, because the Pegasus wrapper runs the
check itself. That has to stay -- Pegasus also gets started by a session
manager or a `.desktop` file, and the check has to happen wherever it is
started from, not only via a command someone remembered to use.

Verified cold, with no daemon running at all: `padmap-start` logged "no daemon
running; starting one", the daemon came up, restored the saved assignment,
regenerated `launch.cfg` and `launch.args`, and Pegasus loaded its theme.

`PADMAP_SKIP_DAEMON_CHECK=1` is honoured by both, and `padmap-start` sets it
for the Pegasus it execs so the check is not paid for twice. Confirmed by the
log: one "no daemon running" line, not two.

`e2e_daemon.py --entry pegasus|start` covers both, since they reach the check
by different routes -- the wrapper calls it directly, `padmap-start` calls it
and then suppresses the wrapper's.

| invocation | result |
|---|---|
| `e2e_daemon.py --entry pegasus` | PASS |
| `e2e_daemon.py --entry pegasus --skip-check` | FAIL, as required |
| `e2e_daemon.py --entry start` | PASS |
| `e2e_daemon.py --entry start --skip-check` | FAIL, as required |

## The launcher path was baked into the collections

Reported after all of the above was verified working: N64 games still gave
four human controllers in Smash.

`export-pegasus` writes an **absolute path** into every `launch:` line, and
that path was `$PADMAP_PLAY` -- a Nix store path. It changes on every rebuild.
The collections had been exported months earlier, so every game was still
being launched by:

    /nix/store/93yjgj...-padmap-play/bin/padmap-play

whose entire body is

    exec retroarch --appendconfig "$config" "$@"

with no `--nodevice` anywhere, because it predates them. The daemon was
current, `launch.cfg` and `launch.args` were correct, `nix build .#padmap-play`
produced a wrapper that read `launch.args` -- and none of it mattered, because
that is not the binary Pegasus ran.

This is the third instance of the same shape: **the code was right and the
thing being executed was something else.** First a stale daemon, then a
previously built wrapper, now a path frozen into generated metadata.

### Why the tests missed it

`e2e_ports.py` wrote its own `metadata.pegasus.txt` pointing at a freshly
built padmap-play. It faithfully tested the chain as it *would* be if
exported today, and was blind to the one on disk.

`--installed` fixes that: it reads the `launch:` line out of the real
collections and uses that launcher. Run against the unfixed system it
reproduced the report exactly -- `ports0-3 = 1,1,1,1` -- the first time any
test had.

### The fix: a stable path padmap maintains

Launch lines now name `~/.local/share/padmap/bin/padmap-play`, a symlink
repointed at the current wrapper by `pegasus.install_player_link()`, called
from both `export-pegasus` and `ensure-daemon`. A rebuild therefore fixes the
collections without re-exporting them. The symlink is swapped with
`os.replace` rather than unlink-then-create, since Pegasus may be spawning a
game through that exact path.

`ensure-daemon` additionally warns about collections whose launch line still
names something else, since those predate the link and will never pick it up
on their own. Verified both directions: a store-path launch line is reported
stale, a link-based one is not.

Note the same hazard remains for the **core** path (`-L /nix/store/...`),
which is copied from the RetroArch playlist's `default_core_path`. Not
addressed here: it comes from the user's playlists rather than from padmap,
and a missing core fails loudly at launch instead of silently doing the wrong
thing.

### mupen64plus's controller log is not an observable, definitively

Worth closing out, having been misread more than once. The
`Game controller N (Standard controller) has ... plugged in` loop in
`mupen64plus-core/src/main/main.c:1668` has **no `Controls[i].Present`
guard** -- it prints all four unconditionally, and only describes pak
configuration. It does not change when a port is genuinely disconnected, at
any point, ever. `tools/probe_libretro.c` exists precisely because of this.

The chain that does decide it, all source-verified:
`retro_set_controller_port_device(port, NONE)` sets `control->Present = 0`
(or `pad_present[port] = 0`, which `inputInitiateControllers` copies into
`Present` -- either ordering lands correctly), and
`input_plugin_compat.c:90` returns `M64ERR_SYSTEM_FAIL` for a port whose
`Present` is 0, so the game sees an empty port.

## Setting up a controller should not require knowing where the settings are

Adding a controller left it unconfigured until the user navigated to a
settings screen -- which only helps someone who already knows the screen
exists and that a new pad needs something doing to it.

The daemon now watches for a controller **model it has no profile for** and
opens the setup screen itself. No front-end change was needed: entering the
assigning state is what surfaces the screen, and the theme already follows the
daemon rather than assuming it is the only thing that can start a session.
That was written for testability and paid off here.

Keyed on `profiles.is_known`, not on arrival: a pad that has been set up
before is silently republished, and only a model with no stored profile is
worth interrupting anyone for. Signature-keyed, so a four-port adapter prompts
once rather than four times.

### The dangerous part is when, not whether

Opening a session stops republishing and takes `EVIOCGRAB` on every pad. Fire
at the wrong moment and it does not merely show an unwanted screen -- it takes
the controllers away from whatever was using them. It is therefore suppressed
when:

- **a game is running.** Nothing in the daemon knew this: from its side a game
  is just RetroArch reading virtual pads it already published. `padmap-play`
  now writes `$XDG_RUNTIME_DIR/padmap/playing` containing its pid, and removes
  it on exit. **This is why padmap-play no longer `exec`s** -- something has to
  outlive RetroArch to clean up. The pid means a launcher that was killed
  outright cannot disable the feature until reboot; the daemon treats a marker
  whose process is gone as stale and removes it.
- **no front-end is connected** -- and specifically, no client that has been
  connected for more than a moment. Grabbing every pad to show a screen
  nothing is displaying would leave the machine with no working controllers
  and no way out, which is exactly what the first version did: `padmap
  ensure-daemon` and padctl connect for a few milliseconds to read status,
  that counted as a front-end, and the very next `ensure-daemon` left the
  daemon in `assigning` with every pad grabbed and no virtual pads at all.
  A front-end stays connected; a query does not, so the test is now dwell
  time rather than existence.
- **a session is already open**, or `PADMAP_NO_AUTOSETUP=1`.

### Declining has to stick

`ensure-daemon` restarts the daemon on every front-end launch, so an
in-memory record of what had been offered would have turned "asked once" into
"asked every single time you start Pegasus" for any controller the user chose
not to configure. The prompted set is persisted to `$XDG_RUNTIME_DIR`, so
declining lasts the login session and a fresh boot offers again.

### tests/check_autosetup.py

Eleven cases over a real `Server` with `devices.discover` and `_begin` stubbed
-- deliberately not a live daemon, since a second one would try to grab pads
the real one is holding.

Two things it caught in itself, both of which would have made it useless:

- The first version left the `playing` marker behind, so **every later
  negative case passed for the wrong reason** -- they were all "a game is
  running", not the condition under test. Each case now owns the marker.
- Once declining was persisted, an early positive case recorded the pad and
  silently blocked every case after it. Negatives now run first, followed by
  the same pad with nothing blocking: if any earlier case leaked a condition,
  that control stays quiet and gives the block away.

## Configuration must follow the press, not the screen

Two faults from a real session, both in the theme rather than the daemon, and
both created by the daemon learning to open the screen by itself.

**Configuration was offered before anything had been pressed.**
`ControllerSetup.open()` called `maybeOfferSetup()` immediately, and
`maybeOfferSetup` read `api.padmap.players` -- which at that instant describes
whatever session came *last*. That was harmless while the only way in was a
user pressing Details from an idle daemon. Once the daemon started opening the
screen itself, it did so from a state that already had players in it, so the
theme asked to calibrate a player the new session held no claim for.

**And then it hung.** The daemon refuses that
(`_begin_calibration` -> `"no controller assigned to player N"`) and nothing in
`CalibrationOverlay` listened for `errorReported`, so it waited for a phase
that was never coming. The overlay renders "Starting..." for the empty phase,
which is exactly what was reported -- with the pads still grabbed, so there
was no way forward and no way out either.

Fixes, in the theme:

- Nothing is offered until a controller has actually claimed a slot.
  `claimsSeen` counts claims since the screen opened, and `maybeOfferSetup`
  returns early while it is zero. The count is incremented from `onClaimed`
  but the offer is still made from the state event, because the two arrive
  separately and in no guaranteed order -- counting on one and reading the
  players list on the other means the list is always populated when read.
- `maybeOfferSetup` also requires the daemon to still be `assigning`.
- The overlay listens for `errorReported` while waiting to start, and has a
  4s watchdog for the case where no error arrives either. Both land on a
  `problem` step that shows the reason and dismisses on any button, rather
  than a screen that cannot be left.
- An error *after* measuring has begun is ignored: the daemon owns the flow
  from there, and an unrelated error should not tear down a measurement.

### tests/check_theme_setup.py

Loads the real theme QML against a stub `api`, so this is checkable without a
daemon, a front-end, or grabbing a controller. Five cases: opening with a
stale unconfigured player offers nothing; a state event alone offers nothing;
a genuine claim offers exactly once for the player that pressed; a refusal
shows its reason instead of hanging; an error mid-measurement is ignored.

Confirmed it fails against the pre-fix theme -- and in doing so identified
which of the two suspected triggers was real: the **state event**, not
`open()`. Worth having, since the two are indistinguishable from the symptom.

Two PySide6 details this needs, both of which produce "Internal C++ object
already deleted" on every property read: the object from
`QQmlComponent.create()` needs `setObjectOwnership(..., CppOwnership)`, and it
needs a C++ parent, because the component is a local and letting it go takes
its creation with it.

### Backing out of setup must not cost you your controllers

Found while checking the above, and worse than either reported fault.

`_begin` stops republishing, so `self._republisher` is None for the whole
session. `_cancel` then read `STATE_READY if self._republisher else
STATE_IDLE` and landed on **idle, with no virtual pads at all** -- nothing
republished, and only a daemon restart to bring them back. The same path runs
when the front-end disconnects mid-session, which `_drop_client` does
deliberately so the grabs are released.

That was survivable while the only way into a session was choosing to open
one. Now that the daemon opens setup by itself, *declining an unwanted screen
took the user's controllers away*.

`_cancel` now puts the previous assignments back on the air before deciding
the state, falling back to idle only if republishing genuinely fails (a pad
unplugged during the session). Verified against the live daemon:

    before:       ready      ['padmap Player 1']
    in session:   assigning  []
    after cancel: ready      ['padmap Player 1']

### The live ports test was leaving the daemon stranded

`e2e_ports.py --live` starts a real Pegasus against the real runtime dir, so
it is a settled client of the user's own daemon. With an unconfigured
controller attached the daemon quite correctly opened setup for it -- and the
test then killed Pegasus, ending the session and (before the fix above)
leaving the daemon idle with nothing republished. That is how it was noticed.

The test now releases any session it caused before exiting. Worth stating
plainly: a test that runs against live state has to put that state back.

## The theme was the fourth thing to go stale

Reported after the theme fixes above: still hanging on "Starting...".

`~/.config/pegasus-frontend/themes/padmap` was a symlink into the store, made
by hand on the day it was first installed. Every rebuild since changed the
store path without changing where it pointed, so Pegasus was reading QML from
a build predating the fix. `grep -c claimsSeen` against the *live* theme
returned 0 while the fresh build had 4.

Same shape as the stale daemon, the previously built padmap-play, and the
launcher path frozen into the collections. Four times now, in four different
places, always: **the code is right and the thing being executed is something
else.** Anything installed by copying or linking a store path once is a
candidate; the fix is always to have padmap maintain the pointer.

The Pegasus wrapper now repoints the theme at its own build before starting,
so `nix run .#pegasus` and `padmap-start` both refresh it. It replaces only a
symlink or nothing -- a real directory there is someone's own theme and not
ours to overwrite. `e2e_daemon.py` asserts the link is repointed, so this
cannot quietly regress a fifth time.

### lastrun.log is the way to debug the front-end

Worth recording, because it turned a guess into a fact in one line.
Pegasus writes `~/.config/pegasus-frontend/lastrun.log`, and everything the
daemon reports as an error surfaces there through the QML API. The hung
session's final line was:

    [w] padmap: no controller assigned to player 1

which is exactly the refusal the overlay had no handler for -- the diagnosis
confirmed from the running machine rather than inferred. QML `console.log`
from a theme lands in the same file.

## Two memories, and `forget` only cleared one

Reported as: no setup prompt at all, even after `padmap forget`.

There are two independent records, and conflating them is easy because both
sound like "padmap remembers this controller":

| what | where | written when | meaning |
|---|---|---|---|
| profile | `~/.local/share/padmap/devices/*.json` | calibration completes (`_store_profile`) | "this model has been configured" -- drives `is_known` |
| prompted | `$XDG_RUNTIME_DIR/padmap/prompted` | the daemon offers the setup screen | "we have already asked about this model" |

`forget` deleted profiles and left the prompted record alone. The result is a
controller that correctly reports itself as never configured while the daemon
stays silent, which is indistinguishable from the feature being broken.
Confirmed on the live machine: the Mayflash had no profile at all and was
still listed in `prompted`.

Both are needed. Without the profile record, calibration would run every
session; without the prompted record, declining would re-offer a second later
and again on every front-end launch, which matters because `ensure-daemon`
restarts the daemon on each one.

Two fixes:

- `forget` now clears the matching prompted entries too (all of them under
  `--all`), and says so.
- The daemon reloads `prompted` when the file's mtime changes. It caches the
  set in memory, so without this, forgetting would only take effect after a
  restart -- and "run forget, nothing happens" was the whole complaint.

`check_autosetup.py` covers it, and the check has teeth: with the reload
removed it fails, because the harness's Server predates the forget exactly as
a running daemon would.

## Configuration follows the claim, and `forget` has to clear both memories

Requested plainly after several rounds of it not appearing: hold a button, the
controller registers as a player, *then* the setup menu opens.

That is now the only trigger for configuration. `ControllerSetup` tracks
`claimedHere` -- the players that claimed a slot **on this screen** -- and
offers setup only for those, and only when the model has no profile. A player
merely present in `players` is ignored, however it got there: restored
assignments, a previous session, or the daemon having opened the screen
itself. Covered by `check_theme_setup.py`, including a player that appears in
the list without ever claiming, and a claim from a controller that has been
configured before.

### Why `forget` still did nothing

Two faults, both in the version written one round earlier:

- It cleared the 'already asked' record only for signatures of profiles it had
  just **deleted**. A controller with no profile has nothing to key off, so
  its record was never cleared -- and that is exactly the state worth
  forgetting: never configured, so it reports itself as new, but already asked
  about, so the daemon stays silent.
- Worse, with no matching profile it returned early -- `"No stored profiles"`
  or `"No profiles match a currently connected controller"` -- before reaching
  the clearing code at all.

`forget` now clears the record for every connected controller regardless of
whether a profile existed, and no longer bails out before doing so. Measured
on the live machine: with the Mayflash holding no profile and one stale
record, plain `padmap forget` went from doing nothing to `Cleared 1 'already
asked' record(s)`.

The lesson is the same one as the launcher path and the theme link: state that
suppresses a prompt needs a way to be cleared that works from the state people
will actually be in when they want to clear it.

## A padmap-owned mapping wizard: what was verified first

Requested after using Pegasus's Gamepad Editor: it does not auto-select the
padmap pad, the mapping does not survive, and it does not configure RetroArch.
Checked all three before designing.

**Auto-select.** Real, with a concrete cause: `virtual.create` gives each
virtual pad the *source* controller's vid:pid (`pad.vid or PADMAP_VID`), so
they carry `0079:1879` rather than padmap's own `1209:0001`. Which means
`PADMAP_ONLY_VIRTUAL=1` -- the switch whose entire purpose is hiding physical
pads from SDL -- filters on `0x1209/0x0001` and matches **nothing**, hiding
the virtual pads too. That escape hatch has never been able to work.

**Persistence.** The mapping *was* written, and SDL's own line is on disk:

    0600c9a7790000007918000001000000,padmap Player 1,a:b1,b:b2,...

but it is keyed to the name `padmap Player 1` -- the **slot**, not the
controller. The same pad assigned to player 2 next time is a different device
as far as SDL is concerned. Fragile for a subtler reason than "not saved".

**RetroArch.** Largely already working, contrary to the report: padmap copies
the physical pad's upstream libretro profile, and both connected controllers
resolved to real ones (the Fightstick to
`Mayflash_Arcade_Fightstick_F300_DINPUT.cfg`). The genuine gaps are pads
absent from that database, and the fact that a mapping made in the Gamepad
Editor never reaches RetroArch at all.

### Design: normalise at the republish layer

padmap *creates* the virtual pad, so it decides what shape that pad has. Rather
than emitting a different mapping per controller to two different consumers,
the wizard records "which physical button is A" once and the virtual pad is
published in a canonical layout. Pegasus and RetroArch are then both told
about one standard controller, which is what makes auto-select and persistence
fall out rather than needing to be maintained.

Storage is keyed by the profile signature (`vid:pid:name` of the *physical*
pad), so a mapping follows the controller rather than the slot it happened to
claim.

### The format layer, pinned against real artefacts

Both output formats fail silently -- a mapping under the wrong GUID is never
matched and SDL says nothing; a RetroArch binding naming a button that does
not exist still reports the pad as configured. So `src/padmap/mapping.py` is
verified against artefacts the real software produced, not by inspection:

- `sdl_guid()` reproduces `0600c9a7790000007918000001000000` **exactly** --
  the GUID SDL itself wrote for `padmap Player 1`. That pins bus
  (`BUS_VIRTUAL`, since these are uinput devices), field order, padding, and
  the CRC-16/ARC of the device name that SDL 2.26+ stores in bytes 2-3.
- SDL and RetroArch disagree about `a`/`b`: SDL's `a` is the bottom face
  button, and RetroArch calls that `input_b_btn`. The mapping table crosses
  them deliberately; getting it wrong swaps confirm and cancel in every game,
  which reads exactly like "the mapping did not take".
- Hats, axes and buttons each have two spellings (`h0.1` vs `h0up`, `+a2` vs
  `+2`), covered case by case.
- Controls nobody pressed are omitted rather than emitted as zero.

`tests/check_mapping.py` holds these. Everything above is the foundation only:
the capture flow, its UI, and wiring it in place of the Gamepad Editor handover
are still to come.

## Layouts are data, and the coordinates are the artwork

The mapping wizard shows the controller with an arrow at the control being
mapped, and has to cope with SNES, arcade, N64 and eventually per-game
layouts. Two ways to do that: ship artwork per controller plus a table of
where each button sits on it, or make the positions themselves the drawing.

The second, because the first has two files that must agree and nothing to
notice when they stop agreeing -- an arrow pointing at the wrong button looks
exactly like an arrow pointing at the right one. `layouts.py` holds normalised
coordinates on a 2:1 canvas and `MappingOverlay.qml` draws from them, so a new
layout is a data entry and there is no artwork to drift out of step.

A layout carries three things, and the third is what makes it more than a
picture: where each control sits, what the *hardware* calls it ("Z", "C-up",
"Coin"), and which canonical control it is. Layouts differ in which controls
exist, not merely where they are -- an N64 pad has no X or Y, a SNES pad no
analogue triggers, and the four C-buttons behave as a right stick everywhere
downstream, which is why they carry right-stick canonical names and emit
`-righty` to SDL and `input_r_y_minus_btn` to RetroArch.

`check_mapping.py` walks every layout and refuses one whose control has no SDL
field or RetroArch key, is asked for twice, or sits off the canvas -- the
failure modes of adding a layout by hand.

### Rendering caught what reading could not

The first render was unusable: the body drew as a row of giant overlapping
discs. Radii were being scaled by canvas *width* while the canvas is 2:1, so
every circle came out twice its intended size. Nothing about the numbers looked
wrong, and no test would have caught it -- `0.14` is a perfectly reasonable
radius either way.

Hence `tools/preview_mapping.py`: it renders a layout to a window, or to a PNG
with `--shot`, with no daemon and no controllers. Checking a layout is then a
matter of looking at it, which is the only thing that works for a drawing.

## The mapping wizard, wired end to end

`capture.MappingRun` walks a layout, watching the pad's raw event stream. The
hard part is refusing input, not reading it -- every guard exists because
something otherwise fills several controls in with one accidental press:

- the button that opened the wizard is usually still held when the first
  prompt appears, so nothing counts until everything held at the start has
  been released
- autorepeat is ignored, or holding one button walks the whole layout
- a code already used cannot answer a second prompt, so a stuck button cannot
  fill in everything
- axes need to travel past a threshold from the centre of their declared
  range, because a pad streams noise at rest and an analogue trigger rests at
  one *end* rather than the middle

Bindings are stored against the physical controller's signature, so a mapping
follows the controller rather than the slot it happened to claim.

### Two numberings, kept separately

A `Binding` carries both SDL's button index and RetroArch's, because the two
consumers start counting at different codes (`BTN_JOYSTICK` vs `BTN_MISC`).
Storing one and recomputing the other later needs the pad's key list to still
be around, and silently shifts every binding on a pad carrying sub-0x120
codes. Axes are stored by *index* too, not evdev code -- `ABS_RZ` is code 5
and may be axis 3.

### Virtual pads now carry padmap's own vid:pid

Reversing an earlier decision, and the old comment argued the opposite, so it
was rewritten rather than left to contradict the code. Mirroring the source
pad's ids gave each *model* its real SDL GUID, which sounded right, but:

- `PADMAP_ONLY_VIRTUAL` filters SDL to `1209:0001` to hide the physical pads.
  With mirrored ids it matched nothing and hid the virtual pads too, so the
  switch had never once worked.
- The GUID depended on which controller was plugged in, so padmap could not
  write a mapping until after the fact.

The objection mirroring answered -- that pads sharing a vid:pid differ only by
name, i.e. by slot, so a different controller in slot 1 inherits the previous
occupant's mapping -- is handled by regenerating the SDL mapping for whatever
holds each slot on every accept. Nothing is expected to survive a
reassignment, so nothing goes stale.

### "Configured" means mapped

It used to mean "a profile exists", which several things write -- calibration,
and finishing a session. So a controller counted as set up before anyone had
said where its buttons were, and the wizard was offered exactly once and then
never again. It now means the profile carries button bindings.

### Regenerating the Pegasus patch rather than editing the diff

The theme needs `startMapping`, `skipControl`, and the mapping properties, so
`0001-padmap-api.patch` had to grow. Hand-editing a diff is a good way to
produce one that no longer applies; instead the source was unpacked, the patch
applied, the files edited, and the diff regenerated with `diff -ruN`. Pegasus
then compiles, which is the only real check that the C++ is valid.

One PySide detail the theme harness needed: a QML call with one argument does
not match a two-argument `@Slot`, and the call vanishes silently. The stub
carries both `@Slot(int)` and `@Slot(int, str)` to mirror the C++ default
parameter.

## A d-pad wired to an analogue axis answered two prompts at once

Reported from a real N64 adapter: in the mapping wizard, pressing left filled
in both left *and* right.

The d-pad on that adapter is an analogue axis rather than a hat. Releasing it
lets the stick spring back **through** centre and overshoot far enough to pass
the capture threshold in the opposite direction -- which is indistinguishable,
event for event, from a deliberate push the other way.

An axis (or hat) must now return near centre before it can answer another
prompt: capturing disarms it, and coming back inside `AXIS_RELEASE` re-arms it.
The threshold alone could not fix this, since the overshoot is a genuine full
deflection; what makes it not a press is that the axis never went back to rest
in between.

Covered by `check_capture.py` with the exact sequence -- push left, spring-back
overshoot, settle, push right -- asserting one control captured per press and
`-a0` and `+a0` recorded distinctly. The same rule applies to hats, where a
cheap adapter can report the opposite direction on release.

## Controls are drawn by kind, and artwork is optional

Two pieces of feedback on the picture.

Every control was drawn as a circle, which made shoulder buttons read as stray
dots floating above the body rather than tabs on its top edge. Layouts already
carried a `kind` per control and nothing used it; the overlay now shapes by it
-- shoulders as lozenges, d-pads as rounded squares, everything else round.
The N64's L/R/Z and its d-pad cross became legible without moving a single
coordinate.

A layout may also name an `image`, drawn behind the control dots in place of
the shapes. `shapes` stays required even then, and is what gets drawn if the
file is missing: an image and a set of coordinates are two things that can
drift apart, and an arrow pointing at the wrong part of a photograph looks
exactly like one pointing at the right part. There is always something to fall
back to that cannot go out of step.

## Skip could not have been a button, and the d-pad bug was in emission

Two reports on the mapping wizard, and both had a cause other than the
obvious one.

### "Skip is Select, which is only mapped halfway through"

True, and worse than stated: it could never have worked from a controller at
all. The daemon holds `EVIOCGRAB` on the pads for the whole session and
republishing is stopped, so the front-end receives **no controller input**
while the wizard runs. `Keys.onPressed` in the overlay can only ever have
fired from a keyboard. Picking a different button -- A, B, anything -- would
not have helped; the input does not reach the front-end.

Skip therefore has to be a gesture the *daemon* recognises, and it cannot name
a button, because nothing is mapped yet. It is now **hold any button**, which
needs no prior mapping, works from the first prompt, and is the same gesture
that already claims a player slot.

That moved binding from the press to the release, since how long a button was
held is only known once it comes back up. Two things fell out:

- Autorepeat stopped needing a special case; the release is what binds.
- The "settling" guard got better. It used to clear on the first release,
  which ate one press when nothing had been held. The daemon now reads
  `device.active_keys()` when the run starts, so it knows exactly what was
  held rather than inferring it, and a wizard opened with nothing down binds
  on the very first press.

### "Both left and right are set when pressing the d-pad"

Not the spring-back fix failing -- a second, independent bug, in emission
rather than capture, and it would have survived any amount of work on the
capture side.

An axis binding was written as `input_left_btn = "-0"`. RetroArch parses a
`_btn` value with `strtoull`, so `"-0"` and `"+0"` both come out as **button
0**: the two directions of one axis collapse onto the same button, and
pressing either activates both. Confirmed against `configuration.c`
(`input_config_parse_joy_button`); only `input_config_parse_joy_axis`, reading
the `_axis` key, understands the sign.

Axis bindings now go under `input_<name>_axis`; buttons and hats keep `_btn`.
It is a silent misparse -- nothing warns and the pad still reports as
configured -- so it is pinned by a check that fails if an axis ever appears
under a `_btn` key.

Worth noting the stored profile at the time held no captured buttons at all:
the bindings RetroArch was using were the *copied* libretro ones. So the
report was of a wizard run whose output had never been written, and the bug
was found by reading what would have been emitted rather than what had been.

## A capture has to be followed by a gap

Reported, and a better diagnosis than the two fixes that preceded it: "there
is no delay between buttons being set -- if I hold the d-pad too long it
registers as two."

The earlier fixes each addressed one *route* by which a single input answered
two prompts: an axis springing back through centre, and an axis emitted under
a `_btn` key so both directions parsed as button 0. Neither addressed the
shape of the problem, which is that recording a control advances to the next
one *instantly*, leaving whatever the user is still holding pointed at a fresh
prompt. Any number of things then land on it -- an analogue axis oscillating
while held, a hat bouncing, a repeat arriving on a different code entirely.

Nothing is accepted now for `CAPTURE_GAP_SECONDS` (0.35s) after a control is
recorded or skipped. Long enough to outlast a release and its bounce, short
enough not to be felt.

The per-axis arming rule stays. It is narrower -- one axis, springing back --
but it is also what allows a *deliberate* opposite push soon after; the gap
alone would either be too short to help or too long to work through.

One detail worth keeping: releases are still tracked during the gap even
though nothing is accepted. Otherwise a button pressed inside the gap and
released after it is still considered down, and either binds on a release the
user did not intend or leaves the run believing something is held. Covered by
a check that presses inside the gap, releases outside it, and asserts both
that nothing was bound and that the button still works afterwards.

## padmap's autoconfig profiles were never being read

Reported as RetroArch complaining the pad was not configured. The log gave it
away in one word:

    [Autoconf] Config files scanned: driver udev, pad name padmap Player 1
               (1209/0001), phys padmap/p1, affinity 0
    [Autoconf] padmap Player 1 (4617/1) not configured.

**affinity 0** -- it scanned and matched nothing.

RetroArch scans exactly one autoconfig directory, `joypad_autoconfig_dir`,
which here pointed into the Nix store copy of libretro's database:

    joypad_autoconfig_dir = "/nix/store/...-retroarch-joypad-autoconfig-1.22.0/..."

padmap wrote its profiles to `~/.config/retroarch/autoconfig/udev/`, which is
**not that directory**. Every profile padmap has ever generated was ignored.

It appeared to work for months because the virtual pads mirrored the physical
vid/pid: libretro's own entry for the underlying controller then matched on
vid/pid and scored 50, so the pad was configured -- by the database, using the
*physical* controller's bindings, which is right often enough not to be
noticed. Giving the pads padmap's own identity removed that accidental match
and exposed it.

### Overriding the directory entirely

The launch override now sets `joypad_autoconfig_dir` to
`$XDG_RUNTIME_DIR/padmap/autoconfig`, holding only padmap's own profiles.

Not merely so they are read. The database was also *competing* with them: an
entry there can outscore a profile generated from a mapping the user recorded,
and can bind controls that were never captured. Since the physical pads are
hidden, the only pads RetroArch can see are padmap's, so a directory holding
just their profiles is complete rather than partial.

The `udev` subdirectory matters: RetroArch looks in `<dir>/<driver>` first and
only falls back to the base directory when that is empty. Stale profiles are
cleared on every write, since one for a player who no longer exists would
still be scanned.

Verified against RetroArch itself -- the same run that produced `affinity 0`
now reports:

    [Autoconf] Config files scanned: ... affinity 50
    [Autoconf] padmap Player 1 configured in port 1.

`tests/check_launch.py`, which had been living in a scratch directory all
along, now lives in the repo and asserts the setting is present.

## "Configured" and "working" are different things

RetroArch reported the pad configured, and Start did nothing.

`retroarch.cfg` held `input_player1_start_btn = "9"` while padmap's autoconfig
profile said `input_start_btn = "8"`. The per-player bind wins --
`input_driver.c` is explicit:

    joykey = (bind_joykey != NO_BTN) ? bind_joykey : autobind_joykey;

An explicit bind is only ignored when it is `nul`. So autoconfig matching says
nothing about which bindings are actually in use: the profile matched at
affinity 50, RetroArch logged "configured in port 1", and every button whose
per-player bind was still set went on using the old value.

These are more leakage from `config_save_on_exit` -- RetroArch wrote them
there itself. Same shape as the stale `joypad_index` and reservations, and
missed the first time because `clean_user_config` only looked at the keys
padmap had a name for.

Two changes:

- The launch override sets every per-player bind to `nul` for the managed
  players, handing them back to autoconfig. Only managed slots: an unmanaged
  one has no pad, so what its binds say cannot matter.
- `clean_user_config` clears them from the user's file as well, so running
  RetroArch outside padmap is not left with them either.

`check_launch.py` asserts start/a/b/select are `nul` for an assigned player,
since an override that quietly stops emitting them would restore the bug with
no visible symptom beyond a button that does nothing.

## Nothing let you say which console a controller is

`layouts.py` has had per-console key tables since the wizard was built, each
verified against its core's source. Nothing could select one. `_begin_mapping`
inferred a layout from `icons.for_pad`, so an N64 adapter whose icon had never
been set got the **generic** pad and was asked to press an X, a Y and two
analogue triggers it does not have. The only thing left to press is the stick,
and the capture then binds face buttons to axes -- which is exactly how a real
mapping ended up with cancel on `-a3`.

### The picker cannot be a front-end widget

The constraint is the same one that killed "press Select to skip": during a
session the daemon holds EVIOCGRAB on every pad and republishing is stopped, so
**the front-end receives no controller input at all**. A picker the theme
navigated could only ever have been worked from a keyboard, on a screen that
exists to set up a controller. And nothing is mapped yet, so no gesture may
name a button.

So the daemon owns the selection and the theme draws it (`capture.LayoutChoice`,
`{"cmd": "choose_layout"}` -> `{"event": "layout_choice"}`):

- **push left/right** to move, read from the raw axis. ABS_X and ABS_HAT0X are
  horizontal on every pad ever made, which needs no mapping to know.
- **hold any button** to accept -- the same gesture that claims a slot and
  skips a control, and for the same reason: it needs nothing mapped, and a tap
  cannot trigger it, so the press that is still travelling when the picker
  opens cannot choose a console by itself.

The catalogue travels as *whole layouts* (`layouts.catalogue`), so the theme
holds no console list of its own -- one there would silently fall behind, and a
console added to `layouts.ALL` would simply never appear.

### It is the same overlay as the wizard, deliberately

`MappingOverlay` gained a `choosing` mode rather than there being a second
screen. The pad drawn while choosing is the pad the wizard then asks about,
from one set of coordinates: a separate list of console names could offer a
console whose layout says something else, and nothing would notice. Rendered
with `preview_mapping.py --choose` before being believed, which is how the
2:1 radius bug was found the first time.

### The picker needs a *different* axis rule from the wizard

The wizard re-arms an axis only once it is back inside `AXIS_RELEASE` (30%) of
centre, which is what stopped a spring-back answering the next prompt. Reusing
it here would have been a bug: the N64 adapter's stick rests at 36% deflection
uncalibrated, so it would never re-arm and the picker would stop responding
after one move. The picker treats anything below the push threshold as
released. A wrong step costs a nudge back, not a mis-recorded binding.

Reached automatically the first time a controller claims a slot, and
deliberately with Prev-page for a pad that has already been set up -- otherwise
the first capture's layout is the one it keeps forever.

## An unmapped pad had no buttons, and the front-end is where the wizard lives

Virtual pads were changed to advertise padmap's own `1209:0001`, which is what
makes `PADMAP_ONLY_VIRTUAL` able to hide the physical pads. The cost was not
noticed at the time: SDL has never heard of `1209:0001`, so until padmap wrote
a mapping the pad had nothing. Pegasus falls back to a blind default with the
d-pad on b12-b15, which a hat-based pad does not have -- leaving only a stick
that on this N64 adapter rests at 36% against a 0.5 navigation deadzone. The
wizard that fixes the controller is inside the front-end the controller cannot
drive.

### What SDL actually matches on, measured

Reasoning about the GUID would have got this wrong. Asked directly, with
`SDL_GameControllerMappingForGUID` against SDL's own built-in database:

| GUID built from | resolves to |
|---|---|
| bus 3, 0079:1830, version 0x0111, real name | Arcade Fightstick F300 |
| bus 3, 0079:1830, version 0x0111, **other name** | Arcade Fightstick F300 |
| bus 3, 0079:1830, **version 1**, other name | Arcade Fightstick F300 |
| **bus 6**, 0079:1830, version 0x0111, other name | **nothing** |

SDL zeroes the name checksum before comparing and ignores the version. The
**bus** is what decides the match -- so mirroring vid/pid alone, which is what
the pads used to do, would have changed nothing at all: uinput reports
BUS_VIRTUAL and every database entry for a USB pad is under BUS_USB.

### The identity is now a switch, and the bus travels with it

`virtual.identity_for` is the single place it is decided, returning bus,
vendor, product and version together because all four go into the GUID:

- **mirror** (default) -- the source controller's bus and ids, so SDL's
  database and libretro's autoconfig match the virtual pad exactly as they
  would the real thing.
- **padmap** -- `1209:0001` on BUS_VIRTUAL, selected by `PADMAP_PAD_IDENTITY`
  or implied by `PADMAP_ONLY_VIRTUAL=1`, since hiding the physical pads by
  vid/pid only leaves the virtual ones behind if they are the only pads
  carrying those ids.

Everything that computes a GUID or writes a vid/pid goes through it --
`controllercfg.virtual_guid`, `sdl_line_for`, `retroarch_profile` -- because
this class of disagreement is completely silent: a mapping under a GUID SDL
never looks up simply does nothing, and says nothing.

Measured end to end, through `virtual.create` and `controllercfg` with the
real Fightstick as the source and real SDL reading the file padmap wrote:

| identity | SDL guid | padmap computed | `SDL_IsGameController` |
|---|---|---|---|
| mirror | `0300c861790000003018000011010000` | same | **true** |
| padmap | `0600c861091200000100000001000000` | same | **true** (with padmap's line) |

and with no padmap line at all, the padmap-identity pad reports **false** --
the reported breakage, reproduced.

Worth not overselling: mirroring restores whatever SDL already knew. For the
N64 adapter, which is absent from its database, that is still a guessed layout
with Accept on raw button 0. Usable enough to reach the wizard, which is all it
is claimed to be.

### A capture still wins over the database

In mirror mode the virtual pad's GUID differs from the physical controller's
only in the name checksum. That turns out to be exactly what is needed: SDL
ignores the checksum when *matching* but honours it when *choosing between*
two candidate lines, so padmap's line (checksummed on "padmap Player 1") wins
for the virtual pad while the database entry (checksum 0) keeps serving the
physical one. Verified by adding a line binding `a:b7` over a database entry
saying `a:b1` and asking SDL which it would use: `a:b7`.

### And padmap always writes something

Independent of the identity switch, because in padmap mode nothing else can
help. `controllercfg.fallback_line_for` writes a line for every republished
pad that has no capture yet:

1. **carried over** -- the physical pad's GUID looked up in the user's
   `sdl_controllers.txt`, then in `SDL_GAMECONTROLLERCONFIG_FILE`, then in
   SDL's built-in database. Bindings transfer verbatim, because the virtual
   pad clones the source's EV_KEY and EV_ABS sets exactly and SDL therefore
   numbers its buttons, axes and hats identically.
2. **guessed** -- face buttons in the order SDL's own default assumes, so the
   guess can never be worse than the one the front-end would make unaided,
   plus the parts that are *not* guesses and are what actually matters: the
   d-pad from the pad's hat (or its BTN_DPAD_* keys) and the sticks from its
   real axes.

SDL's database has no file to read -- it is a C array compiled into the
library -- so it is asked rather than hunted for, in a subprocess with
`SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT=0xffff/0xffff` so the probe loads the
database and opens no devices at all (`joysticks 0`, 0.4s). Out of process so
the daemon never holds SDL's threads and cannot be taken down by it.

One trap found immediately: the probe inherited `SDL_GAMECONTROLLERCONFIG_FILE`
and handed a line padmap had written straight back as though the controller had
come with it. The probe's environment is scrubbed, and any line naming a
`padmap Player N` is refused whatever its source.

### tests/e2e_picker.py

Both halves are driven through a real daemon on an isolated `XDG_RUNTIME_DIR`,
restricted with `PADMAP_ONLY_DEVICE` to a single uinput pad the test owns, so
the live daemon's controllers were never touched: claim -> picker opens on the
guessed layout -> d-pad and stick both move the selection -> hold confirms ->
the wizard walks **the chosen layout** -> the stored profile records
`layout: n64` with 14 bindings -> accept writes an SDL line under the mirrored
GUID and a RetroArch profile carrying the same ids.

The test pad's signature is written into the live `prompted` file first and
removed afterwards. Creating a joystick node is not a neutral act while a
daemon is watching for unfamiliar controllers: without that, the machine opens
a setup screen on the user's television and grabs every pad.

Two things it caught in itself. A pad carrying BTN_TOOL_PEN (0x140) is
classified as a tablet and never discovered at all, which reads exactly like
the daemon being broken -- the fake pad's buttons stop at 0x13f. And run
against the old behaviour, where the wizard used the *inferred* layout, it
fails with `chose n64, wizard walks generic`, which is the reported bug
stated in one line.

## One controller is not one mapping

Reported from real use: an N64 game played with a GameCube controller. padmap
stored exactly one binding set per controller (`profiles.Profile.buttons` plus
a single `layout`), so there was no way to say "these buttons when I play N64,
those the rest of the time". Asked for, verbatim: "the ability to specify the
mapping that my gamecube controller uses for n64 games, and then a universal
configuration in general", plus overrides for a single game.

The thing that makes this more than a preference is that the console decides
which controls *exist* and which RetroArch key each one is emitted under.
mupen64plus-next reads N64 B from RetroPad **Y**; dolphin reads GC A from
RetroPad **A**; the N64 layout has no X or Y at all and four C-buttons that
are a right stick. So "where is A on this pad" genuinely has more than one
answer, and a single stored answer has to be wrong for one of the consoles.

### Storage: a dict of scopes, not three fields

A profile now holds `mappings: dict[str, Mapping]`, where a `Mapping` is a
capture plus the layout it was taken under, and the key is a scope string:

    game:<console>/<stem>     this controller, this game
    console:<layout id>       this controller, this console
    ""                        this controller, everything else

Resolution is `scope_order()` -- a list of candidate keys, most specific first
-- and one loop that takes the first hit. Two shapes were rejected:

- **Three named fields** (`buttons`, `console_buttons`, `game_buttons`).
  Precedence then lives in every caller that reads them, which is exactly the
  kind of rule that gets implemented twice and diverges silently.
- **A layer of indirection** -- named binding sets, plus a scope-to-name table
  so one set could serve several scopes. It answers "controller profiles saved
  to each controller" more directly, and it is genuinely more expressive, but
  the expressiveness buys a second consistency problem (a scope pointing at a
  name that no longer exists) for a case nobody has yet described wanting. A
  `Mapping` carries a `name` field so a set can be labelled; sharing one across
  two scopes costs a duplicate entry, which is a few hundred bytes.

Console identity is a `layouts.ALL` id rather than a new enum, because the
console is precisely what decides the control set and the key table, and that
is what a layout already is. A parallel console list would be a second thing to
keep in step -- the failure `layouts.catalogue()` was introduced to avoid.

The game key is `<console>/<normalised filename stem>`, not the ROM path and
not a content hash. The path is what the front-end happens to hold today and
changes when a library moves or a drive is remounted, and a per-game mapping
that silently stops applying is worse than one never made. A hash means reading
hundreds of megabytes at launch, and makes a patched dump a different game to
padmap while being the same game to the person holding the controller. The
console prefix is what stops `Sonic` on an arcade board sharing a mapping with
`Sonic` on a console.

### Migration is a read, not a pass

`Profile.from_json` folds a legacy flat `buttons`/`layout` into the universal
scope when no `mappings` key is present. In place rather than as a one-shot
upgrade, so a profile is migrated the first time it is looked at -- including
one restored from a backup later -- and there is no separate path to forget.
`to_json` still writes the flat pair as well, mirroring the universal scope:
nothing padmap ships reads it, but a profile store is user data that outlives
any one version, and a rollback then finds the controller mapped instead of
finding it blank and offering the wizard again.

Verified against the actual profile on this machine (`0079:1879 USB GamePad`,
n64, 14 bindings): it comes back with the capture under `""`, still reporting
`configured`, and rewrites with the flat mirror intact.

### The first capture is also the default

Someone whose first act is "map this pad for N64 games" would otherwise have no
universal mapping at all -- so no SDL line for Pegasus to navigate with, and a
guess on every other console. `Profile.record` seeds the default from the first
capture and never touches it again: saying "and for N64, this instead" must not
change what every other console does. Both halves are checked, because they
fail in opposite directions and each looks fine on its own.

An empty capture under a scope is skipped during resolution rather than
matched. It would otherwise shadow the more general mapping that does have
bindings -- the one case where "most specific wins" is not what anybody means.

### Where resolution happens, and why it is not in the daemon

The daemon writes RetroArch autoconfig profiles at republish time, which is
long before anything knows what will be played. `padmap-play` is handed
`-L <core.so>` and the ROM, and is the only place both are available.

So: the daemon writes each controller's **default** mapping into
`$XDG_RUNTIME_DIR/padmap/autoconfig` exactly as before, and `padmap.launch`
rewrites *that same directory* immediately before RetroArch starts, with
whichever mapping resolved. Considered and rejected: pre-building one directory
per console at accept time and picking between them at launch. The launch
override names exactly one `joypad_autoconfig_dir` and RetroArch scans exactly
one, so selecting a directory means editing the override too -- two files that
have to agree instead of one, for a directory that is per-session runtime state
and is cleared on every write anyway.

Rewriting in place also degrades the right way. If `padmap.launch` never runs
-- padmap-play bypassed, an unrecognised core, a Python that will not start --
what is on disk is the default mapping, which is what padmap did before any of
this existed. It never fails a launch; it prints and returns 0.

`padmap-play` calls Python rather than doing it in shell. Deciding a console
from a core name and a key from a ROM path are table lookups that already exist
on the padmap side, and a copy in the launcher would be a table with nothing to
notice when it fell behind -- the failure this project has already had with the
launcher path, the theme symlink and the daemon itself.

The core table (`layouts.CORE_LAYOUTS`) is only as good as its entries. The
four consoles padmap draws were each verified against their core's source when
the layouts were built, and those four core names carry that provenance in a
comment; the rest are near neighbours from libretro's own naming. A wrong entry
resolves a mapping the user did not intend; an **absent** one falls through to
the default, which is the pre-existing behaviour. Neither corrupts anything.

### The SDL-is-universal hypothesis held

Worth stating because it removed a whole half of the problem. Checked rather
than assumed, in Pegasus's own source:

- `GamepadManagerSDL2` loads `sdl_controllers.txt` once and holds one global
  mapping per device. There is no per-collection or per-game gamepad
  configuration anywhere in the frontend -- `GamepadButtonNavigation` and
  `GamepadAxisNavigation` are single objects owned by `GamepadManager`.
- RetroArch's udev joypad driver does not read SDL's database at all; its
  bindings come from `joypad_autoconfig_dir`, which is the thing padmap
  overrides.
- The virtual pads themselves are console-agnostic: `virtual.Republisher`
  forwards events one-for-one and applies only axis calibration. Nothing about
  a mapping is baked into the device, so all of the console specificity lives
  in the files.

So the SDL line is written from the universal mapping alone, and only the
RetroArch autoconfig varies per launch. If it had gone the other way, the
virtual pad would have had to be republished differently per game, which would
mean tearing down and recreating uinput devices at launch and shifting every
pad index -- a much larger and much worse design.

### Asking what a mapping is *for*

The layout picker already solves the hard input problem: during a session the
daemon holds EVIOCGRAB and republishing is stopped, so the front-end receives
no controller input at all, and nothing is mapped yet so no gesture may name a
button. Push left/right, hold any button. A scope picker needed the same two
gestures, so `capture.LayoutChoice` became `capture.Chooser` over a list of
`Option(id, label, layout, mapped)` and both questions use it. One mechanism,
one set of gestures, one overlay -- and the entry drawn while an option is
selected is the pad the wizard will then ask about, which is what stops the
picture and the questions disagreeing.

The flows are deliberately different:

- **First run** is unchanged: claim a slot, "which controller is this?", map,
  filed as the default. Someone who has just plugged a pad in wants it to
  work, not to be asked to think about scopes.
- **Prev-page** now asks "what is this mapping for?" -- Any game, one of the
  four consoles, or the game last played. That is the deliberate route, for
  when the default turns out not to be enough.

A console scope does not then ask which layout to walk. The console *is* the
control set: a mapping for N64 games has to be captured against the N64
layout whatever the pad physically is, because those are the controls the core
reads. Asking afterwards could only produce a contradiction. "Any game" is the
exception, and there the layout question is the real one, so the layout picker
follows.

Options carry `mapped`, drawn as a tick. Re-mapping a scope replaces it, and
without a mark there is no way to tell which ones that would destroy.

The per-game scope exists only because `padmap.launch` records what it just
started in `$XDG_RUNTIME_DIR/padmap/lastgame.json`. The setup screen is reached
from the front-end and never from inside a game, so nothing there otherwise
knows which game is meant -- and "the controls were wrong in the game I just
played" is exactly when someone wants a per-game mapping.

`generic` is offered as a *layout* and never as a console scope. No core
reports it, so `console:generic` could never resolve.

### The icon had to stop following the layout

`_store_mapping` used to take the controller's icon from the layout id when
none was set. With scopes that is wrong in a way that lasts: a GameCube pad
mapped *for N64 games* is captured against the N64 layout, and the pad would
have relabelled itself an N64 controller on the setup screen forever after.
Only an unscoped capture -- the flow that actually asks "this is what my
controller is" -- may now say what it looks like.

## A mapping written mid-session never reached the running front-end

Reported: "the new controller config isn't immediately loaded into Pegasus, and
it just uses the old SDL default." True, with a precise cause:
`GamepadManagerSDL2::start` calls `load_user_gamepaddb` once and nothing reads
`sdl_controllers.txt` again. Everything padmap writes after that is for the
*next* process -- and the moment it matters most is the instant after finishing
the wizard.

The daemon now broadcasts `{"event": "sdl_mapping", "lines": [...]}` whenever it
writes those lines, and on every client connect (a front-end that started before
the last write is running on whatever the file said then). The C++ client feeds
each line to `SDL_GameControllerAddMapping`. The file is still written: this is
the running process, the file is what the next one starts from.

### Which needed measuring, not assuming

The whole fix rests on `SDL_GameControllerAddMapping` re-binding a controller
that is **already open**, rather than only affecting ones opened later. If it
did the latter the code would compile, run, log success and change nothing --
the shape of failure this project has hit four times.

`tests/check_sdl_live.py` measures it against real SDL with a real uinput pad:
give the pad a mapping with `a` on b0, open it, replace the mapping with one
putting `a` on b7, and then *press buttons*. The final assertion is an event,
not SDL describing its own state: raw b7 arrives as `SDL_CONTROLLER_BUTTON_A`
and raw b0 no longer does.

It runs twice, because SDL has two paths and only one of them is what actually
happens:

| before state | SDL's answer to AddMapping | live pad re-binds? |
|---|---|---|
| a stored line (Pegasus's blind default) | 0, "replaced" | **yes** |
| no stored line, SDL auto-generated one | 1, "added" | **yes** |

The second row was found by running the real frontend and noticing the log said
*added*. The cause: SDL never registered anything for that pad because it
already considered it a game controller -- it manufactures a mapping from the
standard `BTN_SOUTH`/`BTN_EAST`/... codes the device advertises, which is
exactly what padmap's virtual pads carry, since they clone their source's evdev
key set. So the common case is the *add* path, and it had to be measured
separately. It behaves identically, but nothing about the API said so.

The return code is therefore not the observable and nothing asserts on it. What
matters is which line SDL uses afterwards.

### And it was read back, not trusted

`Padmap::applyMappings` asks `SDL_GameControllerMappingForGUID` what SDL now
holds and logs it. A mapping stored under a GUID nothing will ever look up is
indistinguishable from a working one from the caller's side -- SDL reports
success either way and never mentions it again.

`tests/e2e_sdl_reload.py` runs the patched Pegasus under Xvfb with a uinput pad
that exists *before* the frontend starts, so the frontend opens it -- the exact
state the bug report describes -- then a stub daemon sends one `sdl_mapping`
event, and the frontend's own log is read back:

    [i] padmap: mapping added for 03008703091200000400000001000000:
        03008703091200000400000001000000,PADMAP RELOADTEST,a:b7,...,crc:0387,

`crc:0387` is SDL's own doing: it moves the name checksum out of the GUID into a
field. Its presence is a second confirmation that the line went in under the
GUID SDL computes for that device rather than one padmap merely believes in.

### New tools

- `tests/check_scopes.py` -- storage, migration of a real legacy profile,
  resolution order, core and game keys, launch argument parsing, and what
  actually lands in the `.cfg` for four different launches. The two captures
  are told apart by keys only one of them can produce (`input_a_btn` cannot
  appear in an N64 profile at all), not by "something was written".
- `tests/check_sdl_live.py` -- SDL's live re-binding, both paths, proved with
  presses.
- `tests/e2e_sdl_reload.py` -- the patched frontend applying a mapping it is
  handed mid-session.
- `tests/e2e_picker.py` gained a second half: re-open setup, choose "Nintendo 64
  games", walk the N64 layout, and then run `padmap.launch` with a real N64 core
  name and a real ROM path and check the emitted profile changed -- and that a
  SNES launch afterwards puts the default back. Both processes, as they run.
- `tools/preview_mapping.py --scopes` renders the scope picker, built from the
  daemon's own option builder rather than a hand-written payload. A preview that
  constructs its own data can look perfect while the daemon sends something else.

### What is not covered

- The core-to-console table is verified for four core names and plausible for
  the rest. Nothing exercises a core absent from it beyond confirming that it
  resolves to the default.
- There is no way to *delete* a scoped mapping. Re-mapping replaces it, which
  covers "I got it wrong"; it does not cover "I want this console to fall back
  to my default again". Worth adding when someone wants it.
- The per-game scope is only offered for a game launched through padmap-play in
  the current login session, since `lastgame.json` lives in XDG_RUNTIME_DIR.
  Choosing a game from the library directly would need the theme's own game
  selection, which is a different screen.
- `e2e_sdl_reload.py` proves the patched frontend puts padmap's line into SDL
  and that SDL resolves the pad's GUID to it. That Pegasus's *navigation* then
  follows is inferred from `check_sdl_live.py`, which measures the same SDL
  call delivering re-bound events -- it is not driven through Pegasus's UI.

## An analogue trigger rests at one end, and the wizard measured from the middle

Reported: "when I registered a gamecube controller, pressing R causes it to
stay stuck in the interface".

`capture.MappingRun._feed_abs` decided whether an axis had been pushed by
normalising it about the **centre of its declared range** and taking the
absolute value:

    centre = (minimum + maximum) / 2
    position = (event.value - centre) / ((maximum - minimum) / 2)
    if abs(position) < AXIS_THRESHOLD: return False

The comment directly above it claimed "a trigger that rests at its minimum
reads as fully negative, so only a push *towards* an end counts" -- which is
what `abs()` prevents. The comment described the intent and the code did the
opposite; nothing tested the case, so the two sat there disagreeing.

An analogue trigger does not rest in the middle of its range. Reading the
absinfo of everything plugged in here:

    mayflash MAYFLASH GameCube Controller Adapter   ABS_RX rest=24  of 0-255
    (all four ports)                                ABS_RY rest=25  of 0-255
    MAYFLASH Arcade Fightstick F300                 all axes centred
    USB GamePad (x2)                                all axes centred

So on the GameCube adapter -- and only there, which is why nothing else showed
it -- the untouched L and R triggers read as **81% deflected**. Note they are
`ABS_RX`/`ABS_RY` on this adapter, not the `ABS_Z`/`ABS_RZ` the name would
suggest; assuming which codes the triggers live on would have found nothing.

Three faults followed, in the order the user would meet them:

1. The first event of a press captured while the trigger was still *low*,
   recording sign -1: the direction it was travelling away from, not towards.
2. A resting report -- drivers emit them -- could answer a shoulder prompt
   with nothing touched at all.
3. It could never re-arm. Re-arming wanted the axis back within `AXIS_RELEASE`
   of *centre*, and a trigger at rest is a full range away from centre. After
   one press the trigger was dead, and every later press was dropped in
   silence. That is the "stuck".

The fix is one function, `capture.deflection`, measuring from a **rest** value
carried alongside the range. `Server._absolute_ranges` now reads it from the
driver (`AbsInfo.value`) as a picker or wizard opens -- the best moment there
is, since nothing is being held yet -- and falls back to the midpoint when the
value is outside the range, which is the old behaviour.

### The capture gap was swallowing releases

Found while writing the regression check, not by the report. `feed()` refuses
everything for `CAPTURE_GAP_SECONDS` after a capture, and it was refusing
*without* updating the arming state. A release takes roughly a tenth as long
as the gap lasts, so essentially every release landed inside one and was
dropped -- leaving the axis disarmed with nothing left to re-arm it, because
an axis that has settled stops reporting entirely.

It survived until now only by accident: a push always starts near rest, so the
early events of the *next* press re-armed the axis a moment before the later
ones captured. That is a coincidence of dense event streams, not a design.

Re-arming is now split into `_rearm` and runs during the gap. The two rules
stay independent -- the gap blocks captures, arming blocks a spring-back --
and a spring-back overshoot is still caught, because it happens well inside
the 0.35s gap.

### A change no test could fail is not a change

While fixing this I raised `AXIS_RELEASE` from 0.30 to 0.40, reasoning that an
adapter reporting a stale resting value (the N64 one here reads 36% off true
centre) needed the headroom. Mutation-testing the new checks showed the 0.30
mutation still passed: a release reports every value on the way back, so it
passes *through* whatever the driver called rest whether or not it settles
there. The bump bought nothing measurable, so it went back to 0.30 and the
comment now records why the headroom is unnecessary rather than implying it is
load-bearing.

The other two mutations -- measuring from the midpoint, and dropping `_rearm`
inside the gap -- both fail their checks, so those two guards are real.

## The "stuck to the left" stick was a trigger padmap called a stick

Reported after the trigger fix above: "it may actually be the analog stick.
seems to be stuck to the left".

It was not the analogue stick, and it was not a calibration problem. The
published SDL line for the pad read:

    ...,leftshoulder:+a3,rightshoulder:+a4,...,leftx:a0,lefty:a1,
    rightx:a3,righty:a4,platform:Linux,

`a3` and `a4` are ABS_RX and ABS_RY, which on this adapter are the analogue L
and R triggers -- the same two axes the previous finding was about. Computing
what SDL reads from the virtual pad while nothing is touched:

    a0 ABS_X  value=126  ->  SDL   -385  (-1%)     leftx   fine
    a1 ABS_Y  value=130  ->  SDL   +642  (+2%)     lefty   fine
    a3 ABS_RX value=27   ->  SDL -25828  (-79%)    rightx  hard left, always
    a4 ABS_RY value=26   ->  SDL -26085  (-80%)    righty  hard up, always

So the front-end saw a right stick shoved into its corner and held there. The
left stick -- the one the user reasonably suspected -- was centred to within
2% the whole time.

`mapping.STICK_AXES` mapped evdev code to SDL stick field as a fixed table:
ABS_X/ABS_Y are the left stick, ABS_RX/ABS_RY the right. The first half is
safe; `_stick_and_dpad_fields` even says so, "ABS_X is the left stick's X on
every pad ever made". The second half is a guess, and this adapter breaks it.
An axis code does not say what the control is.

`stick_fields` now refuses two kinds of axis:

* one that does not rest near the middle of its range (`rests_centred`,
  tolerance 0.5 of half-range -- the measured pads sit inside 0.04). A stick
  centres, a trigger does not, and that is the distinction the code number
  cannot carry. Needs absinfo, threaded through as `axes`; without it the old
  guess stands, since dropping sticks when unsure would cost navigation on
  pads that work today.
* one a capture already claims, which is independent of absinfo and catches
  the same pad by a different route. `leftshoulder:+a3` and `rightx:a3` in one
  line means the two disagree about what is pressed.

RetroArch was never affected: `retroarch_profile` emits only `input_l_x_*` and
`input_l_y_*` from axes 0 and 1, and no right stick at all.

### Calibration would not have fixed this

Worth recording, because it was the natural next guess. `profiles.axes` is
empty for this pad, and calibration measures rest and reach per axis -- which
sounds exactly like the fix. It is not: calibration would have faithfully
recorded that ABS_RX rests at 24, and padmap would have gone on publishing
that axis as `rightx`. The bug was never in the measurement, it was in
deciding what the axis *is*. Calibration remains worth having for a stick that
genuinely drifts; nothing here is evidence for it.

## Resetting a controller, and why the key has to be on the keyboard

Asked for: "I should be able to reset a controller config with a keyboard
binding".

Keyboard is not a stylistic choice here, it is the only thing that can work.
The daemon holds EVIOCGRAB on every pad for the duration of an assignment
session and republishing is stopped, so the front-end receives no controller
input at all while the setup screen is open. This is the same constraint that
killed "press Select to skip" in the wizard.

Bound to the **number keys**, one per slot, rather than to a single key acting
on "the last controller claimed" -- which is what every other shortcut on that
screen does. With two controllers assigned, "the last one" is precisely the
ambiguity someone is trying to resolve when they reach for a reset. A slot
that holds no controller is not bound at all: the daemon would only reject it,
and a key that fails silently is worse than one that does nothing visible.

`forget_pad` removes **every** scope, not just the default. "Reset this
controller" meaning "reset some of this controller" leaves someone re-running
the wizard and still meeting old behaviour from a per-console mapping they had
forgotten was there. The `prompted` record goes too, or a freshly forgotten
controller becomes one padmap never offers to set up again.

The patch is a unified diff whose new-file hunks carry explicit line counts
(`@@ -0,0 +1,409 @@`). Adding a method to Padmap.h/.cpp without correcting
those makes the patch fail to apply, and nothing says so until the Pegasus
build breaks. Both counts were recomputed and the frontend was rebuilt to
prove it applies.

## Two player: what was checked, and what was actually wrong

Reported: "two-player seems to have issues... P1 seemed to have issues when p2
was added".

Verified *correct*, so these can be ruled out:

* both virtual pads exist -- `padmap Player 1` on event26, `padmap Player 2`
  on event31
* the pad indices are right. `visible_order()` predicts RetroArch will
  enumerate the four physical GameCube ports at 0-3 and the two virtual pads
  at 4 and 5, and `compute_pad_indices` returns exactly the `{1: 4, 2: 5}`
  that launch.cfg contains. The virtual pads sort *after* the physical ones
  despite lower event numbers, because libudev sorts by syspath.
* both autoconfig profiles are written, with the right layouts and the
  crossed N64 keys for the pad mapped as an N64 controller

What is wrong is that **the installed udev rules had fallen behind the
hardware**:

    /run/udev/rules.d/99-padmap.rules
      0079:1830   MAYFLASH Arcade Fightstick F300
      0079:1879   USB GamePad
      (no 0079:1843 -- the GameCube adapter)

`padmap hide` generates the rules once from whatever is plugged in at the
time. The GameCube adapter arrived later, so it is absent, so RetroArch sees
its four physical ports *as well as* the virtual pads padmap builds from them
-- six pads where there should be two. Nothing anywhere noticed, which is the
recurring shape of every bug in this file: padmap generates a thing, the
system drifts, and the two are never compared again.

`hide.unhidden` now reads the installed file and reports adapters padmap
republishes that it does not cover, and `ensure-daemon` prints it. Reading the
file rather than remembering what was written, for the usual reason.

Not proven to be the cause of the P1 regression -- it is a real defect found
while looking, and it is the only discrepancy found between what padmap
intends and what the system is actually doing.

### The daemon had no log

Every diagnosis above had to be reconstructed from files on disk, because
`_spawn_daemon` sent stdout and stderr to `/dev/null`. The daemon is the only
process that sees a controller claimed, a mapping captured or a launch config
written, and all of it was being discarded -- in a project where "use the
logs" has been the instruction twice.

It now writes to `$XDG_RUNTIME_DIR/padmap/padmap.log`, truncated per daemon so
the file describes the current run.

One trap avoided while doing it: the obvious `serve --verbose` cannot be used.
`--verbose` belongs to the main parser, so it would have to precede the
subcommand, and `protocol.daemon_pids` matches argv *structurally* on
`argv[-2:] == ["padmap.cli", "serve"]`. A flag there makes every running
daemon invisible to `ensure-daemon`, which would then start a second one
beside the first.

### QC: the reset destroyed before it validated

Caught reviewing the above, not by a test. `_forget_pad` deleted the stored
profile and *then* called `_begin_layout_choice`, which has its own guards --
so with no session open, or a device that had closed, the controller lost its
configuration and got no wizard to build a new one. Strictly worse than the
wrong mapping it started with, and the reset key is reached precisely when
someone is already unhappy with their mapping.

Reachable only from the setup screen, which always has a session, so it was
latent rather than live. The order is now: resolve the pad, prove the wizard
can open, and only then throw anything away. Mutation-tested by removing the
guard, which fails the new check.

The general form is worth stating, since this file has several instances of
it: validate everything before performing the destructive half. A partial
failure that leaves nothing behind is worse than doing nothing at all.

## The udev rules were generated from the assignment, not the hardware

Reported: "got the arcade fightstick connected and configured as player 1 but
I think the gc autoconfigured joysticks took up the first four slots".

Confirmed by running `tests/e2e_ports.py --live --installed`, which drives the
real chain on a virtual display, and reading RetroArch's own autoconfig log:

    [Autoconf] First unconfigured / unreserved player is 2.
    [Autoconf] Device "mayflash ... GameCube Controller Adapter" (79:1843)
               is not reserved for any player slot.
    [Autoconf] Earlier free player slot found, reassigning to player 2.
    ... and again for players 3, 4 and 5
    [Autoconf] Config files scanned: pad name padmap Player 1 (0079/1830),
               affinity 50
    [Autoconf] padmap Player 1 configured in port 5.
    [Autoconf] Reserved device matched.
    [Autoconf] Device "padmap Player 1" is reserved for player 1, updating.
    [Autoconf] Preferred slot was taken earlier by "(null)", reassigning to 1.

So the adapter's four ports really do take player slots 2-5, and the
Fightstick is first configured into slot 5 before the reservation drags it
back to 1. The reservation mechanism works; it just cannot stop devices from
occupying every other slot.

Core ports were unaffected -- the probe reported `ports0-3=1,0,0,0`, one
JOYPAD and three RETRO_DEVICE_NONE -- because `--nodevice` empties them
regardless of which player slot a pad landed in. That is why this presents as
clutter and confusion rather than a broken game, and why it survived so long.

The cause is one line in `cmd_hide`:

    pads = [a.pad for a in assignments] or devices.discover()

Rules were generated from whatever held a player slot at that moment. The
GameCube adapter was not assigned when the file was written, so it never got
a rule. Worse, regenerating with a single controller assigned would have
*dropped* the rules covering the others -- turning a stale file into a
actively wrong one, and un-hiding pads that were correctly hidden.

`hide.targets` now returns every physical pad on the machine, unioned with
anything assigned. Which controller holds a slot changes every session; which
adapters exist is a property of the hardware, and that is what a udev rule
describes.

The `unhidden` warning added earlier had been reading `devices.discover()` all
along, which is why it correctly flagged 0079:1843 while the generator that
was supposed to fix it would not have emitted a rule for it. The check and the
thing it checks now agree.

### `padmap hide` installs rather than dictates

Asked for: "can I just have it run for me rather than echoing the bash
script?" -- fair, since the script it printed was to be pasted back into the
same root shell the user had already opened.

Running as root is now taken as the instruction to install. There is no other
reason to run this command with privileges, and a copy-paste step that exists
only to be got wrong is not a safety feature. `--print` keeps the old output
for someone who wants to read the rules first; `--install` without root says
so and gives the one command that works.

Two things the installer must not skip, both mutation-tested:

* **The reload.** udev holds its rules in memory, so a file written without
  `udevadm control --reload-rules` changes nothing until the next boot. A
  controller still visible after padmap has said it hid it is precisely the
  silent gap this project keeps finding.
* **Honesty about doing nothing.** Installing identical rules reports "already
  up to date" rather than claiming a change.

One subtlety worth recording. Under `sudo`, `XDG_RUNTIME_DIR` points at
root's, so the assignment file usually cannot be read at all. Had `cmd_hide`
still derived its pad list from the assignment, `sudo padmap hide` would have
generated rules covering *nothing* -- the previous bug's worst case, reached
by the very command meant to cure it. Reading the pads from /sys makes the
privileged and unprivileged runs agree.

## Per-game scope was limited to the one game just launched

Asked for: "single-game or single-console mappings for a controller... should
be possible to override per-game and per-console".

Most of this already existed and works. `check_scopes.py` proves the whole
resolution end to end, for the exact case driving it -- one GameCube pad with
a default capture and an N64 one:

    no context      -> gamecube default (input_a_btn, input_x_btn)
    n64, other game -> n64 mapping (input_y_btn, no input_a_btn)
    n64, Mario 64   -> the per-game mapping
    snes            -> back to the default

The route is Prev-page on the controller setup screen, which opens the scope
picker before the button wizard.

The gap was in what the picker could *offer*. A per-game scope can only be
offered for a game the daemon has seen launched -- the setup screen is reached
from the front-end, never from inside a game, so nothing else there knows
which game is meant. But `read_last_game` returned exactly one, so someone who
had since started something else could no longer reach the game they wanted to
fix, with no route back to it but to launch it again.

`lastgame.json` now holds a short list, newest first, deduplicated by key so a
replay moves a game up rather than filling the list with copies. Capped at
`protocol.RECENT_GAMES` (5): the strip is worked from the pad one step at a
time and sits after the console entries, so a long tail of games turns "map
this for N64" into a scrolling exercise.

The pre-list format is still read. Someone upgrading mid-session would
otherwise lose the per-game scope for the game they are playing right now,
which is precisely when they are most likely to want it.

Still missing, and worth doing: there is no way to *delete* a scoped mapping.
Re-mapping replaces one, but a scope created by mistake can only be
overwritten, never removed.

## Mapping a pad from the game, where the answers are already known

Proposed: "a keyboard action and then a controller select beforehand. This
shouldn't happen in the controller select screen but on the game view screen.
So press a key on the keyboard while focused over a game, then we get the 'Is
this mapping for console or game?', and we already know both because we're on
the game select screen."

That is a better shape than what existed, and the reason is worth stating.
The scope picker reached from the controller setup screen has to offer every
console and a handful of recently launched games, because that screen knows
nothing about what anyone wants to play. It is a list built out of ignorance.
Reached from a game in the library, both facts are in hand, so the question
collapses to two entries -- "Nintendo 64 games" or "GoldenEye 007 (USA)" --
with nothing to scroll past and nothing to get wrong. It also makes the
recently-played list irrelevant for this route: any game in the library can be
mapped for, not just one played this session.

Press **M** on a focused game. A raw key, deliberately not one of Pegasus's
named actions: every one of those is already bound here, and the gamepad half
of their bindings could not reach this anyway, because the daemon grabs every
pad the moment the session opens.

**The claim is the controller select.** Rather than presenting a list of
assigned pads, the setup screen opens as it always does and configures
whichever pad presses a button. That gesture needs nothing mapped, works on a
pad padmap has never seen, and is the same one that claims a slot -- so it is
one idiom, not two.

Two things had to be gated, both mutation-tested:

* One press must start one wizard. The claim signal and the state event both
  fire for a single press, in no guaranteed order.
* It must follow a *press*. The daemon opens this screen by itself from a
  state that already lists players belonging to a finished session, and
  mapping one of those would configure whichever pad was player 1 last time
  while the user held a different one. This is the same `claimedHere` trap
  that once left the calibration overlay stuck on "Starting...".

The first version of that second check was worthless and the mutation testing
said so: it asserted nothing happened with an *empty* player list, where the
loop never runs and the guard is never reached. It only became a real check
once it listed a player who had not claimed on this screen.

### The console and the key are computed once, by the exporter

`x-console` and `x-gamekey` are written into the collection file by
`pegasus.render`, using `layouts.for_core` and `profiles.game_key` -- the same
two functions `padmap.launch` uses to resolve a scope when a game starts. The
theme passes them through untouched and the daemon uses them as given.

Deriving either in the theme, or again in the daemon, would be a second
definition of which scope a game belongs to. The failure would be silent in
the worst way: a mapping filed under a scope the launcher never looks up,
which presents as a wizard that completes successfully and changes nothing.

### `console` is a QML global, and the theme would not load

Reported: "theme loading failed :(". Pegasus's own log said exactly why:

    theme.qml:45:9: Signal parameter "console" hides global variable.

The signal added for library-driven mapping named its first parameter
`console`, and so did the handler and a local in `Library.qml`. `console` is a
QML global (console.log); shadowing one is refused, and the refusal costs the
whole *file*, not the handler. Renamed to `consoleId` throughout.

The interesting part is not the mistake, it is that nothing caught it.
`theme.qml` was loaded by no harness at all -- the theme's checks each load one
component (Library.qml, ControllerSetup.qml) against a stub `api`, and the one
file that wires those together had no coverage. Every check passed, mypy
passed, Pegasus built, and the theme did not load.

Two attempts at a guard failed, and both are worth recording because the
obvious ones do not work:

* **Compiling every file through PySide6.** Passes on the broken source.
  PySide6 is Qt 6, Pegasus is Qt 5, and this diagnostic is Qt 5's.
* **Asserting on real Pegasus's log in `e2e_pegasus.py`.** Also passes on the
  broken source -- measured, not assumed: the broken theme was built into the
  store, confirmed present in the store path Pegasus loaded, and the run
  produced no such warning. Whatever surfaces it in a desktop session does not
  surface it there.

So nothing that *runs* the theme catches this. `check_theme_loads.py` lints for
it by reading the text instead: signal parameters, handler parameters,
function parameters and locals are checked against the names QML puts in
scope. Unclever, and it fails on the exact source that broke the theme, which
is the only property that matters.

The compile pass is kept anyway -- it covers syntax errors across all eight
files for almost nothing -- but it is documented as not covering this.

`e2e_pegasus.py` has a separate, pre-existing failure ("theme did not start
calibration for an unconfigured pad"). Confirmed pre-existing by running it
against the theme as of 1089285, which fails identically. It is not in the
`check_*` suite, so it has been failing unnoticed.

## Configuring a controller now measures its sticks

Asked for, after an analog stick behaved as though it were only off or full:
"can the controller configuration do the calibration?"

It can, and it should have all along. Calibration was only ever offered for a
pad padmap had never seen, so anyone reaching a controller through the mapping
wizard was never prompted -- and every profile on this machine had `axes: {}`.
Uncalibrated, an axis is scaled against the range the adapter *declares*
rather than the one the stick actually reaches, which is exactly the shape of
"0% or 100%".

The wizard now runs calibration when it finishes, before accepting.

**That order is forced, and it is the whole subtlety.** `accept` is what writes
the RetroArch profile and the SDL mapping, and it also ends the session and
releases the pads. Calibrating after it would be measuring a controller nobody
is holding. So the chain is: capture buttons -> measure sticks -> accept.
Abandoning the wizard still accepts and measures nothing, as before.

A pad with no analog axes is not a special case here: the daemon already
handles that, storing an empty calibration and going on to the icon step.

### The harness was counting five screens as one

The check for this failed while the behaviour was correct, which took longer to
work out than the feature did. `check_theme_setup.py` builds a fresh
ControllerSetup per scenario, and every one of them stays connected to the same
stub `api`. A signal emitted for one check is delivered to all the screens left
over from earlier ones, so anything counting calls -- `accept_calls`,
`calibrate_calls` -- was measuring the whole pile.

Deleting them was not enough either. `deleteLater()` schedules a deferred
deletion, and `processEvents()` does not run those, so the screens stayed alive
and connected while the code read as though they had gone. That is worse than
not cleaning up at all, because the counts then quietly include objects the
test believes it destroyed. `drop_setups` now flushes them with
`sendPostedEvents(None, QEvent.Type.DeferredDelete)`.

Worth remembering as a class: a test-only bug that makes a *correct*
implementation look broken costs as much as one that hides a real fault, and it
is harder to recognise because the instinct is to doubt the code under test.

### Analog gain is a workaround, so padmap only removes it once it can

`input_analog_sensitivity` was 1.6 in retroarch.cfg. That is not arbitrary: a
GameCube stick does not reach the extremes its adapter declares, RetroArch
scales the partial travel against the full declared range, and the stick feels
weak. Winding the gain up compensates. libretro's own profile for a GameCube
adapter ships the same idea, commented out:

    # input_analog_sensitivity = "1.400000"

It works, and it costs the top of the range. At 1.6 the stick is saturated at
about 62% deflection, and everything past that is the same reading -- which is
what "only 0% or 100%" felt like from the sofa.

Calibration addresses the cause instead: `AxisCalibration.apply` rescales the
measured reach onto the declared range, so a calibrated pad delivers the full
sweep and a gain on top of it double-compensates.

So padmap writes `input_analog_sensitivity = "1.000000"` into the launch
override **only when every managed pad is calibrated**, and otherwise says
nothing. The asymmetry is deliberate. Sensitivity is global, not per player, so
one uncalibrated pad in a two-player session means the boost is still doing
useful work for it; taking that away silently would make a stick worse while
claiming to fix it. The setting reverts to the user's own value the moment a
pad without calibration joins.

This is the same reasoning as the rest of launch.cfg: retroarch.cfg drifts,
padmap owns the launch. The difference is that this key is only overridden
when padmap has earned the right to -- when it is the thing setting the range.

## The session lifecycle was the one thing the log did not record

Reported: "I have to reassign controllers twice with i before they actually
get assigned. first run does nothing."

The daemon is not at fault, and that was worth establishing before changing
anything. Driving it directly through two `begin`/`cancel` cycles produced an
identical session each time -- `pads=7`, `state=assigning`, no grab failures --
so `begin` opens and grabs correctly on the first call as well as the second.

Which narrows it to the front-end path, and there the log had nothing to say.
It recorded republishing, mappings and SDL writes, but not whether a session
opened, whether the pads were grabbed exclusively, or whether a hold was ever
seen. Every one of those is what this report turns on, and none of them was
recoverable after the fact.

Now logged: session open (pads, slots, existing assignments), any pad that
could not be grabbed exclusively, each claim as it happens, and cancellation
with the number of claims it discarded.

The interesting detail from the report is that the *second* press works, and
the second press does not call `begin` at all -- `open()` skips it when the
daemon already says `assigning`. So the session the second screen uses is the
one the first press created. That points at the screen rather than the
session, and the next reproduction will say which.

Adding the lines broke `check_autosetup.py` immediately: it stands a bare
object in for the assigner, and the cancel line asked it for `.assignments`.
Fixed with getattr -- a log line in a teardown path is never worth raising
from -- but worth recording that the check caught an unsafe assumption in
instrumentation within a minute of it being written.

### Calibration would have wrecked the triggers it had just captured

Found while chasing "the analog stick didn't work", and it is a fault the
previous change created rather than one it exposed.

`calibrate.calibratable_axes` excluded triggers by listing the codes they are
conventionally reported on -- ABS_Z, ABS_RZ, ABS_GAS, ABS_BRAKE. The comment
above that list states the real rule correctly: "Triggers rest at one end of
travel, so their resting value is not a centre." The list is not that rule,
and this machine's GameCube adapter puts its analogue triggers on ABS_RX and
ABS_RY -- stick codes -- so they went straight through. Measured on the live
adapter, it offered four axes to calibrate: the two sticks and both triggers.

Centring a trigger is not a small error. Calibration takes the resting value
as the centre and maps it to the middle of the declared range, so a trigger
resting at 24 of 0-255 would read half pressed while untouched and keep half
its travel. Harmless while nothing called it -- and every profile here had
`axes: {}`, so nothing ever had. Wiring calibration into the end of the
mapping wizard changed that: every mapping would have quietly ruined the
triggers it had just finished capturing.

The resting value now decides, with the code list kept only as a shortcut.
That is the same test, and the same reasoning, that stopped those two axes
being published as a right stick -- an axis code does not say what the control
is, and this adapter has now proved it twice on the same two codes.

Measured after the change: the adapter offers ABS_X and ABS_Y, and nothing
else.

## "Calibration needs a button held first" was an accident, not a design

Asked: "how do I trigger the calibration?" -- and the honest answer turned out
to be "hold a button on the pad, *then* press Details", which nothing said and
nobody had decided.

Measured against the live daemon: opening a session and asking to calibrate
player 1 returns

    no controller assigned to player 1

about a controller that is assigned, republishing, and visible in the state
event a moment earlier. `_pad_for_player` consulted the session's claims and
nothing else, and a session starts with none. So every per-player command --
calibrate, map, forget -- failed for the opening seconds of a session, and the
only way through was to claim a slot first.

`_pad_for_player` now takes the claim when there is one and the stored
assignment otherwise. The claim has to win: mid-session, player 1 is whatever
just pressed a button, not whatever held the slot before, or re-assigning a
slot would configure the pad it replaced.

What this deliberately leaves alone is `_players_payload`, which still reports
claims only. Making *that* fall back to stored assignments would undo a fix
already recorded above: the setup screen would once again draw players from a
finished session and offer to configure a controller nobody had touched.
Resolution and display are different questions and only one of them wanted
changing.

The second half is that the key said nothing at all. `Details` with no claims
ran a loop over an empty list and returned, which is indistinguishable from a
broken key -- and this is a screen where the pads are grabbed, so there is no
other feedback to fall back on. It now says which step is missing. A key that
finds nothing to act on has to say so.

## A once-a-second disk scan on the thread that forwards controller events

Reported: "there's a lag on the controllers... doesn't seem to work great when
entering two inputs... it feels more like it's just arriving at a very slow
rate".

The daemon is single threaded. One selector loop forwards every physical event
to its virtual pad and then calls `_tick`, and `_tick` opens with
`_poll_new_controllers`. That is rate limited to once a second, which sounds
harmless until you read what the once-a-second call does: `devices.discover()`
globs /sys/class/input and reads several files per device, then `has_mapping`
parses a profile off disk for each pad. Seven pads here.

Then it asked whether setup could open at all -- and during a game the answer
is always no. So every second, mid-game, padmap did a full device enumeration
and seven JSON reads on the same thread as the input path, to conclude it must
do nothing. The cost scales with the number of pads attached, which matches
"worse with two inputs".

The blocked test is now first. It is a handful of comparisons and one small
file read, and its answer does not depend on what is plugged in, so nothing is
lost by asking it before the expensive part.

This is a periodic stall rather than constant latency, so it is unlikely to be
the whole of the report. It is, however, a real one, in the one place that
must never stall, and it was doing nothing useful at the time.

## The game-specific mapping did save, and did apply

Reported in the same message: "gamecube controller didn't seem to save
game-specific input mapping between sessions". It saved. On disk:

    scopes: ['', 'console:n64', 'game:n64/super-smash-bros-u']
      ''                            layout=gamecube  buttons=12
      'console:n64'                 layout=n64       buttons=14
      'game:n64/super-smash-bros-u' layout=n64       buttons=14

And it resolved: the key computed at launch from the core and the ROM path is
`n64/super-smash-bros-u`, identical to the stored one, with `scope_order`
placing it first. The autoconfig RetroArch actually read says so in its own
header:

    # Mapping scope: game:n64/super-smash-bros-u; resolved for Super Smash Bros. (U) [!].

So storage, key derivation and resolution are all working end to end, and the
thing to chase is why a correctly applied mapping felt wrong -- for which the
lag above is the obvious suspect, since input arriving late and unevenly is
indistinguishable from input mapped wrongly when you are holding the pad.

Worth recording because the instinct was to go looking for a persistence bug,
and there wasn't one.

### The daemon and the launcher wrote different profiles to the same file

Reported: the game-specific mapping "didn't seem to apply on the second go".

Not a persistence bug -- the mapping is stored, and `padmap.launch` resolves it
correctly and idempotently. Run twice with the same core and ROM it produced
the same game-scoped profile both times, so the launcher never drops it.

The conflict is that two different things write that file with different
answers. `padmap.launch` writes a profile resolved from the core and ROM of the
launch in progress. `Server._start_republisher` wrote one with no context at
all, which resolves to the controller's default mapping. Same path, same
filename, and whichever ran last wins.

Republishing restarts for reasons that have nothing to do with the game: a
setup session being accepted, a controller reconnecting, the daemon being
restarted to pick up new code. Any of those after a launch replaces the
game-specific profile with the default one -- and a mapping that is live one
launch and gone the next, with nothing in between that the user did, is
exactly the report.

`_start_republisher` now passes the last launched game as context, so it
regenerates the profile the launcher would rather than contradicting it. Where
nothing has been launched the context is empty and this is the same write it
always was.

Not confirmed as the cause -- it could not be reproduced on demand, because
reproducing it needs a republish to land between a launch and RetroArch reading
the file. It is a real disagreement between two writers of one file, which is a
thing worth removing whether or not it is this report.

## The controller lag, measured: a quarter-second scan once a second

Reported: "there's a lag on the controllers... doesn't seem to work great when
entering two inputs... it feels more like it's just arriving at a very slow
rate". Measured rather than reasoned about, by reading the virtual pad for ten
seconds while the stick was moved:

    1695 events over 10.0s -> 169/s
    gap ms: median 7.94  mean 5.91  max 176.0
      gaps > 50ms: 11
      gaps >100ms: 5      (104, 128, 128, 160, 176)

The steady cadence is healthy -- 7.94ms is the adapter's own ~125Hz. What is
wrong is a stall of 80-176ms about once a second. Timing the suspect directly:

    devices.discover()      : 257.3 ms   (7 pads)
    signature + has_mapping :   1.0 ms
    repeat: 276.6 / 274.0 / 293.9 ms

`devices.discover` runs `udevadm info` **as a subprocess per input device** to
read ID_INPUT_JOYSTICK, and this machine has 32 input devices. A quarter of a
second, once a second, on the single thread that forwards controller events to
the virtual pads. The profile parsing everyone would suspect first is 1ms of it.

`_poll_new_controllers` now compares a cheap signal first -- the set of
/dev/input/event* node names, plus the prompted file's mtime -- and only does
the full scan when one of them has changed. Measured at 0.032ms against
257-294ms, and a controller that appears always adds a node.

Two ordering details, one of which the checks caught immediately:

* The signature is recorded only when a scan actually runs, *after* the
  blocked test. Recording it before meant a controller plugged in during a
  game was never noticed once the game ended -- the next look saw nothing
  changed and skipped for good.
* Being blocked still returns without recording, so the offer survives until
  the reason goes away.

The deeper fix is not spawning 32 subprocesses at all -- udev's database can be
read directly from /run/udev/data -- but that changes a filter deliberately
matched to RetroArch's own (udev_joypad.c:1053), and the scan is no longer on
the hot path, so it can wait for its own change.

## What a test buildout actually found

Nine agents wrote 5,777 lines of tests across the areas this session changed,
each in its own checkout, each required to mutation-test its own file. They
reported 333 scenarios and 87 verified mutations between them.

Three real defects fell out. All three are the same shape: a guard written for
one failure mode that does not cover a neighbouring one.

**profiles.load raised instead of returning None.** `Profile.from_json` calls
`raw.get(...)` on whatever `json.loads` returned, and `load` catches only
`(OSError, ValueError)`. A file holding valid JSON that is not an object --
`null`, a list, a bare string, a number -- came back as `AttributeError`.
`is_known` calls `load` for every pad during discovery, so one corrupted file
stopped that controller being handled at all, which is precisely the contract
the existing catch exists to provide. Returning an empty `Profile` would not
do either: `is_known` is `load() is not None`, so an empty one marks the pad as
already set up and it is never offered the wizard. Damaged has to read the
same as absent. The nested containers had the same gap -- `{"axes": "nope"}`,
`{"mappings": {"": {"buttons": "x"}}}` -- where junk in one slot should cost
that slot, not the profile.

**hide.unhidden raised on a rules file that is not UTF-8.** It read with
`Path.read_text()`, and `UnicodeDecodeError` is not an `OSError`. `cli.py`
calls `unhidden(devices.discover())` unconditionally from `ensure-daemon`, so a
corrupt or binary 99-padmap.rules would traceback the entire start path. The
docstring already promised that a missing, unreadable or malformed file is
safe; it was one exception class short of true.

**game_scope_options offered a scope for a game with no console.** It tested
`console` and `key` independently, so an empty console with a key returned one
entry -- the game -- drawn with the generic layout. The daemon's own comment
relies on an empty list to report "no console known for this game". It is
reachable: `pegasus.render` writes `x-gamekey` for every entry but omits
`x-console` when the collection's core is not one padmap recognises, so a
front-end really can send that pair, and the user would capture a mapping under
a scope nothing looks up.

Two non-bugs worth recording, both from mutation testing rather than from a
failing assertion:

* `calibratable_axes` has a dead guard. `if info.max <= info.min: continue` is
  unreachable as a distinct branch, because `rests_centred` already returns
  False for any such span. Deleting it changes no behaviour on any input. Kept
  as defence in depth, but nothing can fail if it goes.
* `mapping.axis_index` filters hat codes when numbering, and no pad shape can
  exercise it: hat codes (0x10+) sort above every analogue axis code, so their
  presence cannot shift an index. The agent removed its claim rather than
  assert something unreachable, which is the right call.

And one flake, unrelated to any change here: `check_sdl_live.py` failed once in
a back-to-back suite run and passed alone and on repeat. It creates real uinput
devices, so it is timing-sensitive.

The lesson worth keeping is about where the bugs were. Not one was in the
behaviour the session had just changed -- all that held up. They were in the
error paths *beside* it: what happens when a file is damaged, when a byte is
not UTF-8, when a fact the code assumed is present is missing. Those are
exactly the paths hand-testing never reaches, because producing them requires
deliberately breaking something.

## The story sweep: what breaks when the world is not well formed

Nine agents wrote 10,178 lines against 23 user stories, half of them
adversarial. They reported roughly thirty-four defects. Not one was in a happy
path; every single one was an error path beside working code, which is now the
third sweep running to say the same thing.

Five were fatal to the daemon, and those share a shape worth naming. When
padmap raises, it does not degrade -- the process ends, its uinput nodes go
with it, and the machine has no controllers at all, mid-game, with nothing on
screen to explain it. A daemon must not be killable by the thing it exists to
serve.

**A malformed command argument ended the process.** Several commands coerce
with int(), and `{"cmd": "begin", "players": "lots"}` raised through
`_on_client_read`, the selector dispatch and `serve()`. Any client could do it
with one bad message. `_handle_command` now wraps the dispatch and answers with
an error event. Deliberately broad: the point is not to enumerate how a message
can be wrong, it is that no message may end the process.

**A `prompted` file that was not UTF-8 stopped the daemon starting.** Read in
`Server.__init__` with `read_text()`, and `UnicodeDecodeError` is not an
`OSError`. The file lives in XDG_RUNTIME_DIR where anything may write it, so
`padmap serve` simply could not start and `ensure-daemon` failed forever. This
is the third instance of exactly this bug -- after `hide.unhidden` and the
profile store -- which is a strong argument for a shared "read a text file we
do not control" helper rather than a fourth fix.

**A wizard outlived its session.** `_begin` cleared the picker and the
calibration but not the mapping, and `_tick` returns early while a mapping is
open. So for the whole of the *next* session no hold could claim a slot and the
confirm gesture never fired: the setup screen sat there inert, with nothing
logged and no error shown. That is worse than a crash, because there is nothing
to report.

**A controller unplugged mid-session ended the daemon.** `_cancel` guarded
`_start_republisher` and `_accept` and `restore` did not. In `_accept` the
exception escaped *after* `_save_assignments` had written the state file, so
the front-end never heard "accepted" or "error" and the screen waited forever.
In `restore` it is worse: `serve()` calls it outside its try/finally, so the
daemon died during startup with the pads already discovered.

**A player number outside 1..16 raised StopIteration.** `launch_config` hands
out one spare index per unmanaged slot but counted an out-of-range player as
managed without consuming one. A hand-edited or corrupted assignments.json
reaches it, and `restore` calls it at startup.

Two notes on doing this work rather than on its results. The fix for the
out-of-range players called `log.warning` in a module with no logger, and the
import test passed -- because the call sits inside a branch that only runs when
the fault occurs. Exercising the actual path is what caught it. And the first
draft of `check_daemon_survival.py` grabbed the live machine's controllers,
because `{"players": 1.5}` is a *valid* command and began a real session; the
Assigner is now replaced before any command is dispatched. A test that reaches
the hardware it is meant to be isolated from is a bug in the test, and it was
found the same way everything else here was -- by reading what actually
happened rather than what was supposed to.

The remaining defects are recorded in the sweep's own notes and are not fixed
here: a newline in a MAME description injects metadata keys into a collection
file (including a `launch:` line, which Pegasus honours); one malformed .lpl
makes the whole export produce nothing; `stale_collections` raises out of
`ensure-daemon` on an unbalanced quote or an unreadable file; `find_titles`
raises on a damaged table and silently loads a string as a one-character title;
`padmap forget` crashes on the same non-object JSON the discovery path was
already fixed for; and `scope_options` still offers a recent game whose console
is unknown, drawing one pad and walking another.

## Clearing the backlog: six defects the sweeps had found but nobody had fixed

**A newline in a game's title injected a metadata key.** metadata.pegasus.txt
is line oriented, and every field value was interpolated raw. A title holding
"Evil\nlaunch: /bin/sh" emitted a launch line of its own, *ahead* of the
collection's real one -- and Pegasus keeps the first launch command it sees, so
that game would have run it. Reachable with nobody hand-editing anything:
titles.parse_mame_xml reads <description> with re.S, so a MAME description
wrapped across two lines is enough.

`pegasus.one_line` now confines every emitted value. The first version
collapsed all runs of whitespace, which fixed the injection and quietly
rewrote every title containing a tab or a double space -- "Double  Dragon"
became "Double Dragon". A test caught it. The format cares about line breaks
and nothing else, so now neither does the guard.

**One damaged playlist emptied the whole library.** read_playlist validated the
top-level shape and then trusted everything below it, so `{"items": [1,2,3]}`
raised out of a loop that walks every .lpl in turn. The user's entire library
disappeared from Pegasus because one file was bad. Entries are now skipped
individually, a file with no usable entries at all returns None rather than an
empty collection, and export survives a playlist it cannot read.

**stale_collections could stop padmap starting**, twice over: shlex.split on a
launch line with an unbalanced quote, and read_text on a file that exists but
is unreadable. ensure-daemon calls it unconditionally on every start.

**`padmap forget` was defeated by the thing it exists to fix.** It read each
profile inside `except (OSError, ValueError)` and then called .get() on the
result, so a file holding `null` or `42` raised AttributeError -- the same
non-object JSON bug already fixed in discovery. And --all unlinked everything
it globbed, so a directory named *.json killed the one command meant to clear
up a mess. It also reported the number of files it *attempted*, which meant
telling someone five profiles were forgotten while one was still on disk.

**The scope picker offered a game whose console padmap cannot name.**
game_scope_options already refused this; scope_options never got the same
guard, and padmap.launch deliberately records every launch including one with
an unrecognised core. The entry drew the generic pad and then walked whatever
the controller was guessed to be -- the strip promising one pad and the wizard
asking about another, which is precisely how a mapping once ended up with
cancel bound to an axis.

### Two gaps that promoted themselves

The sweep agents were told to keep their files green and print a "gap:" line
where the behaviour was wrong. Two of them went further and wrote the gap so
that *fixing* the bug fails the test:

    if len(header["launch"]) == 1:
        raise SystemExit("FAIL: scan_file_exts no longer injects -- assert it instead")

That is a better construction than either a plain gap or a red test. It stays
green while the bug exists, it cannot be forgotten, and the moment someone
fixes the source it demands to be turned into a real assertion. Both have been.

## The zero-coverage sweep: 246 scenarios, and the marking that never worked

Nine agents took the areas docs/STORIES.md proved had no test at all. 246
scenarios, 107 verified mutations, roughly twenty-nine defects. Three deserve
naming.

**The MAME "not working" marking has never worked in the real front-end.**
Pegasus stores every `x-` field as a QStringList, not a string
(PegasusMetadata.cpp builds `QStringList values` and Game.h exposes that map as
`extra`). So the theme is handed `["preliminary"]`, and in QML
`["preliminary"] === "preliminary"` is false. Every arcade set MAME grades as
preliminary has been rendering as a working game: no pill, no dimmed title, no
dimmed cover.

It was never seen because `tools/preview_library.py` builds `extra` with a
bare string. Every preview and every screenshot showed the marking working
perfectly. A harness that is *more convenient* than production is a harness
that lies, and this is the clearest instance of it in the project: the feature
was verified by looking at a picture, and the picture was of something else.

**Any local process can kill the daemon with one message.** `LineReader.feed`
raises RecursionError on deeply nested JSON -- measured threshold on this build
is 52,096 brackets -- and RecursionError is a RuntimeError, not a ValueError,
so the `except ValueError` beside it misses. `server._on_client_read` has no
try either, and the guard that does exist sits one layer further in. Anything
that can open the socket can end the process and take every virtual pad with
it, mid-game.

**A hand-edited calibration can kill the republisher.** `Profile.from_json`
puts no bound on an axis's stored min/max, `AxisCalibration.apply` scales into
that range, and `virtual.py` writes the result into an evdev value -- a signed
32-bit field. Out of range it raises OverflowError, which is not an OSError, so
every guard on that path misses it. The daemon exits mid-game.

And one that likely explains a report from earlier in this session. Releasing a
*second* button cancels a confirm hold that is still down on the first, because
`_confirm_started` is keyed on the pad alone and popped on any release, with no
equivalent of the `held[0] == event.code` guard Assigner already has. A resting
thumb is enough. The user's words were "I have to reassign controllers twice
before they actually get assigned".

The UTF-8-decode hole has now been found five times in five places:
`hide.unhidden`, the profile store, `Server._load_prompted`, `hide.install` and
`cli._forget_prompted`. That is no longer five bugs, it is one missing helper
for reading a file padmap does not own.

### Two contracts that disagreed, and one regression of mine

Two agents specified `stale_collections` differently for a launch line that
cannot be lexed: skip it, or report it. Reporting wins. A false positive costs
a "re-export this" warning; a false negative silently misses the exact failure
the function exists to catch, and the collection whose launcher cannot even be
parsed is more likely to be broken than one whose can, not less.

My own export-resilience fix was caught by a contract test in the same sweep. I
had wrapped the collection *write* in the same guard as the playlist *read*, so
an unusable output directory skipped silently and reported success. A damaged
input costs one collection; a destination that cannot be written costs
everything, and reporting success then is a lie rather than resilience.

## Fixing nineteen bugs in parallel, by owning files rather than bugs

Nine agents, one disjoint set of source files each, 49 verified mutations. The
partition held exactly: every source change landed inside its owner's files and
nothing else. The only collision was in tools/, where two agents extended the
same check, and a three-way merge against the base resolved it without
conflict.

That is the lesson worth keeping from the exercise. Parallel *testing* needs
only isolation; parallel *fixing* needs an ownership rule, because two agents
editing one file cannot both be right and neither knows the other exists.
Assigning by file rather than by bug also forced the bugs into sensible groups:
the five that live in the daemon's own state machine went to one agent who
could see how they interact, rather than to five agents each holding a corner.

Two limits were reported honestly rather than worked around. The mapping agent
could not fix the server half of the sub-BTN_MISC binding, because
`_begin_mapping` passes the pad's key list unfiltered and it does not own
server.py; it made mapping.py able to *express* "RetroArch cannot see this"
and said what remains. The calibration agent could not change the reach seeding
in `CalibrationRun` for the same reason, and fixed the half that lives in
merge_reach. Both are better outcomes than an agent reaching into a file
another was editing.

### The recurring hole finally got a helper

`safeio.read_text` now exists, and hide.py and cli.py use it. The UTF-8 decode
bug had shipped five times in five places -- `hide.unhidden`, the profile
store, `Server._load_prompted`, `hide.install`, `cli._forget_prompted` -- each
found separately, each fixed separately. Five instances of one mistake is not
five bugs; it is a missing abstraction, and counting them was what made that
obvious.

### One integration trap worth recording

The combined tree passed all 42 checks and failed mypy with "Module padmap has
no attribute safeio". The file was there. The flake was not: a Nix git tree
copies only *tracked* files, so a new module that has never been `git add`ed is
invisible to every derivation while being perfectly visible to the tests run
from the working directory. Staging it fixed it. This is the same family as
every other stale-artefact failure in this log -- the thing being built was not
the thing on disk -- and it will happen again to anyone adding a module here.

## Exiting a game launched another one

Reported: "when exiting a game, for some reason it immediately starts another
game as if an A input is stuck".

Nothing grabs the virtual pads exclusively -- verified by grabbing and
releasing each of them while no game was running. So while a game is playing,
RetroArch and Pegasus read the *same* virtual pad: every button pressed in the
game also reaches the library sitting behind it. The frontend is not
insulated from the game by anything at all.

Against that, the library launched on any accept whatsoever. So quitting a game
with the accept button still down handed the library that press the instant it
came back, and it launched the highlighted game again.

The striking part is that this rule already exists everywhere else in the
project, three times over: the setup screen ignores the press that opened it
(`_drain`, `held`), the wizard ignores a button held from before (`settling`),
and a tap does not confirm. The library was the one screen that acted on an
input without asking whether it was a *fresh* one, and it is the screen where
acting means starting a program.

Two guards now, both mutation-tested. Auto-repeat can never launch: a held
button is by definition not a fresh press. And accept is disarmed for 600ms
whenever the application becomes active, because that -- not focus -- is what
returning from a game looks like from inside the theme. The library never
loses QML focus while a game runs; the game is a separate process, so
`activeFocusChanged` never fires and `Qt.application.state` is the only signal
that does.

The check for this found its own wiring by accident: the first version asserted
that a fresh press launches, and it failed, because the harness activating the
application had already disarmed accept. That is the guard working, so the
assertion became "activation disarmed accept without anything else being done"
-- which is a better test than the one intended, and pins the part that is
otherwise invisible.

Not reproduced first-hand: this needs a real game exit on the real machine, and
the fix addresses the mechanism that was verified rather than a failure that
was observed. Worth saying plainly, because everything else in this log was
measured before it was changed.

## Two padmap commands wanting the same controllers

Reported as a traceback from `padmap launch --log`, ending in

    OSError: [Errno 16] Device or resource busy

out of `evdev.device.grab`. Not a bug in evdev and not a broken pad: `run` and
`launch` are the standalone paths, so they build their own virtual pads, which
means taking EVIOCGRAB on the physical ones -- and a running daemon is already
doing exactly that. Two of padmap's own commands wanting the same hardware.

What made it worth fixing is not the failure, it is what the failure said.
Nothing in that traceback mentions the daemon, names the command to use
instead, or suggests this is anything other than padmap being broken. The user
ran it because I suggested it, and the suggestion was wrong.

`_start` now asks whether a daemon holds the pads before opening anything --
so the message does not depend on which pad happened to fail first -- and
still catches EBUSY, because a daemon is not the only thing that can hold a
controller. Both callers print it and exit 1.

Three mistakes of my own while writing this, each caught by running the thing
rather than reading it:

* `protocol` was not imported. cli.py imports it lazily inside the functions
  that need it, and I wrote module-level style into a function that had none.
* Only one of the two call sites got the handler. My pattern used a four-space
  indent, which is a *substring* of the eight-space call already inside the
  other function's try block, so the count looked right and the replacement
  landed in the wrong place. Anchoring on the following line fixed it.
* The message told the user to run `padmap stop-daemon`, which does not exist.
  An error that names a command you do not have is worse than one that names
  none, and this file has spent a long session arguing that a guard has to say
  what the user should do. It now prints `kill <pid>` with the pid it already
  looked up.

## Y mapped to C-up did nothing, and the C-stick looked fine

The mapping was right at every layer padmap owns. The capture was stored
(`rightstick_up: {kind: button, index: 3}`), the autoconfig carried
`input_r_y_minus_btn = "3"`, button 3 was the correct number for that pad --
its codes run contiguously from `BTN_JOYSTICK`, so SDL and RetroArch agree --
and RetroArch's own shipped database uses `_btn` on an analog half-axis in 40
profiles. The verbose log confirmed the profile matched and was applied:

    [Autoconf] ... pad name padmap Player 1 (0079/1843), phys padmap/p1, affinity 50
    [INFO] [Autoconf] padmap Player 1 configured in port 1.

The fault was one layer below, in `input_joypad_analog_axis`:

    res  = abs(input_joypad_axis(..., axis_plus,  normal_mag));
    res -= abs(input_joypad_axis(..., axis_minus, normal_mag));

    if (res == 0)
    {
       ... consult bind_minus->joykey / bind_plus->joykey ...
    }

The button is only read when the axis reads **exactly** zero. padmap was
emitting a button on one half of the axis and leaving an axis on the other:

    input_r_y_minus_btn  = "3"     <- Y
    input_r_y_plus_axis  = "+2"    <- C-stick down

so the axis decided the answer and Y was never consulted.

It never read zero. Two independent reasons, and both matter:

* the pad does not centre. Measured on the live `padmap Player 1` node, axis 2
  rests at **131** on a 0..255 axis. `udev_compute_axis` is
  `(value - min) * 0xffff / range - 0x7fff`, so that is **+900**.
* even a perfectly centred axis would not reach zero. That formula subtracts
  `0x7fff`, not `0x8000`, so the value that normalises to zero on a 0..255
  range is 127.5 -- there isn't one. 127 gives -128, 128 gives +129.

900 is 2.7% of full scale. Below the core's own `mupen64plus-astick-deadzone`
of 5, so the C-stick behaved perfectly while the button was dead. Nothing was
logged, nothing was misconfigured, and the one visible symptom -- stick drift --
was invisible by construction. That is why this survived several passes: every
artefact padmap produced was correct, and the check that would have caught it
had to model RetroArch's arithmetic rather than inspect padmap's output.

`input_analog_deadzone = "0.000000"` in the user's retroarch.cfg (and 0.0f is
RetroArch's compiled-in default) is what removed the last chance of rescue: the
deadzone branch that would have zeroed 900 never runs.

The fix, `drop_shadowed_axis_halves`, drops the opposing `_axis` bind whenever
a button is bound to the other half of the same analog axis. The button is the
deliberate instruction, so it wins; both halves then resolve to `AXIS_NONE`,
`res` is always 0, and the fallback fires every time. It costs that stick's
other direction, and that part is not padmap's to fix -- RetroArch has no way
to say "this button, and also that axis" on one analog axis.

The drop is unconditional today, so C-stick down is gone from that one game
mapping. It did not work before this change either -- nothing on that axis did,
because `res` was 900 and the whole bind was inert -- so nothing regressed, but
it is a real limit and worth stating plainly rather than implying the mapping
is now complete.

There is a way to keep it, not taken here. Calibrating the pad would be enough,
and only because of the floor division: `apply()` maps the measured rest to
`(min + max) // 2` = 127, which normalises to **-128**, and the positive half
clamps a negative reading to 0. `input_r_y_plus_axis` would then read exactly
zero at rest and the button would still be consulted. Making the drop
conditional on a calibrated axis would recover C-down, at the cost of a rule
whose correctness depends on a stored calibration being present and honest --
so it is written down rather than built, until someone wants that direction
back badly enough to pay for it.

Note the asymmetry if anyone does: this rescues a button on the *minus* half
only. A button on the plus half with an axis on the minus half reads -128,
never zero, and cannot be rescued on a 0..255 range at all.

Two things worth remembering beyond this bug:

* **no padmap pad currently rests at true zero on any axis.** The 0..255 range
  these adapters report has no such value. Publishing a wider, symmetric range
  (`min=-32767, max=32767`, rest 0) would give exactly zero and make half-axis
  button binds work in both directions.
* **ABS_Z and ABS_RZ are special.** RetroArch treats them as analog triggers
  and rescales them `(val + 0x7fff) / 2` when an axis's *initial* value
  normalises below -1300 (~4%). padmap publishes the C-stick Y as ABS_Z, so any
  future change to the published range must keep it from starting negative, or
  a resting stick reads +16383 -- half deflection, permanently.

Neither is a live bug today; both are traps for the next change here.

---

## The Steam Controller Puck: three wrong answers before the right one

Reported as "it's listed as xbox 360", then "it looks like a keyboard... if
steam is closed it stops acting like [a controller]", then "still don't see it
after modprobing". Every one of those was a real symptom, and the first three
things padmap said about them were wrong in a different way.

**The device.** `28de:1304`, seven USB interfaces:

    0-1  CDC-ACM, internal comms, not HID
    2-5  four wireless slots  -> hidraw7..10
         05 01 09 02      mouse,    report 0x40   |
         05 01 09 06      keyboard, report 0x41   | lizard mode
         06 00 ff 09 01   usage FF000001, reports 0x42/0x43/0x44/0x45/0x79/0x7b
    6    pogo-pin dock     -> hidraw11
         06 00 ff 09 02   usage FF000002, 54 bytes, stripped down

It is a **receiver**, not a controller. Mainline names it "Steam Controller
(2026) Puck" and gives it `STEAM_QUIRK_IBEX | STEAM_QUIRK_WIRELESS`. Up to four
controllers connect *through* it. Looking for one pad to appear is the wrong
thing to wait for, which is why `LateModel` carries a `receiver` flag rather
than burying the word in a name string.

**Wrong answer 1: the first three bytes of a report descriptor.** `lizard.rs`
tested `descriptor[0] == 0x06 && descriptor[2] == 0xFF` to find a vendor
interface. On a slot interface the vendor collection is the *third* top-level
collection -- the descriptor opens with an emulated mouse -- so the test failed
on all four slots and passed only on the dock, whose descriptor does start with
the vendor page. padmap therefore announced the dock as "its gamepad channel",
and `hidprobe.py` was pointed at it and reported "nothing arrived at all".

That reads exactly like a controller that is asleep. It was a tool aimed at the
one interface on the device guaranteed to be silent. The descriptor is now
walked as HID items and every top-level application collection is read; the
dock is excluded by the same usage mainline excludes it by:

    /* The puck's pogo pin interface should be ignored as it's stripped
     * down. It has one collection with usage page FF00 with usage ID 2. */
    return hdev->collection[0].usage != 0xFF000002;

**Wrong answer 2: the sysfs driver directory.** The remedy printed
`/sys/bus/hid/drivers/steam/new_id`, computed by stripping `hid-` from the
module name. That is not a rule -- a `hid_driver` registers whatever `.name` it
likes, and this kernel has `/sys/bus/hid/drivers/hid-steam`. Because the write
goes through `sudo`, a missing directory fails as a *permission* error, which
reads as the kernel refusing the write rather than as a bad path. Both
spellings are now tried against the filesystem instead of computed.

**Wrong answer 3: `new_id` at all.** This is the one that cost the user a
session. Force-binding is not a workaround here, and padmap said it was.
v6.18's `hid-steam` has three ids -- `1102`, `1142`, `1205` -- and contains no
reference to either 2026 codename. Its `steam_raw_event` opens:

    /* All messages are size=64, all values little-endian. */
    if (size != 64 || data[0] != 1 || data[1] != 0)
            return 0;

Every report this model sends is 54 bytes or fewer under its own report id, so
each one is dropped in silence. The bind succeeds and the pad stays dead --
strictly worse than not binding, because now something *looks* attached.

The legacy interface selector is no better: pre-IBEX it picks "the interface
that has a feature report", and all five HID interfaces here have feature
reports `0x01`/`0x02`, the dock included.

**The actual remedy is a kernel version.** `hid-ids.h` gained
`IBEX`/`IBEX_BLE`/`PROTEUS`/`NEREID` in **v7.3-rc1**, and has them in no earlier
tag (checked v7.0, v7.1, v7.2 -- absent; v6.18 -- absent). So the remedy table
is keyed on `(vid, pid) -> first kernel release`, and the advice branches on the
running kernel: below it, upgrade; at or above it, the module merely needs
loading.

**What generalises.** Three of these are the same mistake -- deriving a fact
that was available to be read. The descriptor's shape was inferred from three
bytes instead of parsed; the sysfs path was computed from a module name instead
of looked up; the driver's capability was assumed from its existence instead of
checked against its id table. The fix in each case was to read the thing.

And one that does not generalise but is worth stating: **a diagnosis that names
a remedy is a claim, and a wrong one is more expensive than silence.** "No
joypads found" would have left the user to investigate. "Run this as root" sent
them to do it, twice, for a command that could never have worked.

---

## The Steam Controller, resolved: I researched my way to the wrong answer

The previous entry ends "the actual remedy is a kernel version". It is not.
The controller works on 6.18.44, with no kernel driver, and had been working
in another program on this machine while that entry was being written.

What I got wrong, and how:

**"Decoding it by hand has a chicken-and-egg problem -- the command to leave
lizard mode is the part of the protocol you do not have."** It is
`SDL_hidapi_steam_triton.c:126`, zlib licensed, upstream since 2025-11-12, and
it is six bytes. I had already read `hid-steam.c` from three kernel tags and
the mainline `hid-ids.h`; I did not think to look in SDL, because I had
decided the answer was a driver and drivers live in the kernel.

**"It works in the other program because Valve wrote both ends."** Valve wrote
both ends of *Steam*. The implementation worth copying is SDL's, which is
public. I inferred an asymmetry from one data point -- Steam works, nothing
else does -- and then reasoned from the inference rather than checking it. The
user's own report said "another program", and I answered "that program is
Steam" without asking which.

**The recommendation followed from the framing, not from the evidence.** Every
individual fact in that entry is correct and independently verified: v6.18's
`hid-steam` really does have three ids, its `raw_event` really does drop every
report this model sends, `PROTEUS` really did arrive in v7.3-rc1. All true,
all beside the point, because the question was never "will the kernel driver
bind" -- it was "can this controller be used", and I had substituted one for
the other early enough that no amount of further verification could catch it.

The correction came from someone else's document with a working controller
behind it. Worth being plain that the verification I did was not useless but
was not sufficient either: it made the wrong answer *well-evidenced*.

### What was actually needed

    01 87 03 09 00 00   then 58 zeros, via HIDIOCSFEATURE, every 3 seconds

and a decode of a packed struct. `src/padmap/triton.py`.

Three details that would each have cost a session:

* **A feature report, not a write.** `hidraw.py:_request_full_mode` uses
  `os.write`, which is correct for the Switch Pro's output report. The same
  call here succeeds and does nothing.
* **Re-sent every three seconds, forever.** The controller reverts by itself.
  Sent once, a pad works and then stops, which reads as failing hardware.
* **An empty slot stalls with EPIPE**, and that is the only cheap way to tell
  an empty slot from a full one. The receiver publishes four either way, so
  without probing, padmap offers four players for one controller and three of
  them never send an event.

### The generalisable part

The previous entry's own lesson was "deriving a fact that was available to be
read". This is the same failure one level up: the *existence* of an
implementation was available to be read, and I derived its non-existence from
the fact that the kernel did not have one yet.

A concrete rule out of it: when the conclusion is "this cannot be done", the
next step is not more evidence for that conclusion. It is to find whoever is
already doing it. The user said someone was. That was the cheapest available
disproof and I treated it as a puzzle about Steam.

---

## "None" was a game title

Found by the differential corpus on its first run against the new Rust port of
`protocol.py`, which is what that corpus is for.

`_game_entry` built a recent-games entry with `str(raw.get("title", ""))`.
`lastgame.json` is a file padmap wrote, so the fields are strings — until it
is truncated, hand-edited, or written by a version that did something else.
Then `str(None)` is the four characters `None`, and `str(True)` is `True`, and
those go straight onto the scope picker where a game's name belongs.

Nobody reported it. It needs a malformed file to reach, and the file is small
and rarely touched. What found it was recording the Python's answers over
seventeen shapes of that file and replaying them in Rust: the Rust said `""`,
the corpus said `"None"`, and the test asked which was right.

The second half is why the rule is "strings only" rather than "handle null":

    Python  str(True)  -> "True"
    Rust    true       -> "true"

Two implementations reading one file and disagreeing about its contents, in a
way no test on either side alone would show. Stringifying whatever was in the
JSON needs per-language care to stay consistent; dropping anything that is not
a string needs none. A console that is not a string was never going to name a
layout anyway.

Both sides now drop non-strings, and a `key` that is not a non-empty string
makes the whole entry `None` rather than an entry named `7`.

**The general point.** This is the third time the corpus has paid for itself,
and all three were the same shape: a place where the two languages are each
individually reasonable and quietly disagree. Rounding (ties-to-even versus
away-from-zero), integer division (floor versus truncate), and now rendering a
non-string. None of them would have been caught by a unit test written from
reading the other implementation, because reading is exactly where the
assumption comes from.

---

## `str()` on something that is not a string, four times

Fourth instance of one mistake, found by the differential corpus each time
after the first:

    protocol._game_entry   str(title)   -> "None" on the scope picker
    server.parse_command   str(icon)    -> an icon named "None", stored

Both read a field out of something padmap did not write -- a file that can be
truncated or hand-edited, a socket any local process may write to -- and both
rendered whatever they found. `str(None)` is the four characters `None`, and
it goes straight into a picker or a profile as though somebody chose it.

The rule is "strings only" rather than "handle null", and the reason is the
second half:

    Python  str(True)  -> "True"
    Rust    true       -> "true"

Two implementations reading one message and disagreeing about its contents, in
a way neither side's own tests would show. Stringifying needs per-language care
to stay consistent; dropping anything that is not a string needs none.

**Worth generalising.** Every one of these was at a boundary where padmap reads
something it did not write, and in every case the coercion was chosen for
convenience at the call site rather than for what the field means. A field that
must be a *number* is refused when it is not one, loudly, because a wrong
number is a real argument. A field that must be a *string* should be emptied
when it is not one, because an empty layout or icon means "unset" and every
caller already handles it. Neither should be rendered.

## A Steam Deck's d-pad was the trackpad under the player's left thumb

`28de:1205` is a Steam Deck's own controls, and `hid-steam` publishes them as
an ordinary evdev pad -- but not an ordinary *shape*. Four things about it are
not what the codes suggest, and only one of them is fixed here.

`ABS_HAT0X/Y` is the left **trackpad**, -32767..32767 with a fuzz of 256. The
d-pad is four keys, `BTN_DPAD_UP..RIGHT`. `guess.rs` bound the d-pad by looking
for the hat codes and stopping there, so every direction came from the pad the
player's thumb rests on, and the four keys that are the actual d-pad bound
nothing. The guard is the obvious one once the question is asked out loud: **a
hat reports -1, 0 or 1, and anything wider on a hat code is not a d-pad.**
Unmeasured, a hat code is still taken at its word, so nothing that passes
`axes: None` -- which is every frozen corpus case -- moved.

The other three are recorded and not corrected:

* **X and Y arrive on each other's codes.** `hid-steam` writes `BTN_X` for the
  west button, and `BTN_X` *is* `BTN_NORTH` -- a legacy spelling of a
  positional code. Anything reading the codes positionally, padmap's
  `STANDARD_BUTTONS` included, comes out the wrong way round.
* **The triggers are `ABS_HAT2Y`/`ABS_HAT2X` of 0..32767**, not `ABS_Z`/`ABS_RZ`
  of 0..255, which the pad does not declare at all. `binding::axis_index`
  excludes `0x10..0x18` from the axis numbering outright, so padmap cannot
  spell those indices and falls back to the digital `BTN_TL2`/`BTN_TR2`.
* **The grips are on `0x224..0x227`**, codes the kernel headers leave unnamed.
  Mainline v6.16 `hid-steam` puts them on `BTN_TRIGGER_HAPPY1..4`
  (`0x2C0..0x2C3`); SteamOS's does not. A fixture read off mainline would have
  been wrong about the hardware in front of it.

**What made the difference was a second opinion.** A fixture and a test written
from one reading of one driver agree with each other for free, and the X/Y swap
would have survived both: the driver says `BTN_X | button X`, and the obvious
test asserts exactly that. SDL's built-in database has an entry for this GUID,
written by people with the hardware, and it says `x:b5,y:b6` where padmap's
guess says `y:b5,x:b6`. `padmap-input/tests/steam_deck.rs` holds the two
records against each other and names every control they disagree about, so the
list of disagreements is four and cannot grow by accident.

The same pass corrected `STEAM_VIRTUAL`, whose own source string had been
asking for a live recording since it was written: Steam's mirror is *not*
byte-for-byte xpad's table. Its sticks stop at -32767 where xpad's reach
-32768.

**Worth generalising.** Capturing a device is not the same as checking one.
Every fixture here came from one source, and a fixture nobody can cross-check
is a guess with a struct around it even when the numbers are real -- the
numbers can be right and the *names* still wrong. Where somebody else has
published an independent answer for the same hardware, the test should be
against theirs, not against the reading that produced the fixture.

**Not fixed, and worth knowing.** `hid-steam` withdraws the pad node for any
client that opens the hidraw device, and Steam is such a client, so on a Deck
as it normally runs there is no `Steam Deck` node at all -- only the lizard
keyboard and mouse, and Steam's `28de:11ff` mirror standing in for the built-in
controls. `without_steam_mirrors` drops that mirror as soon as any other pad is
present, on the assumption that a mirror always duplicates a pad padmap can
already see. On a Deck that assumption is false, and plugging in a second pad
costs the Deck its own controls.

## Every layout with an analog stick was missing the analog stick

`gamecube.json` listed sixteen controls. Four of them were the C-stick. The
stick a person actually holds was not one of them, and the same hole was in
`n64.json`, `switch.json` and `wiiu.json` -- the last two listing neither of
their two sticks.

It survived because nothing downstream fails when a control is absent. The
clone forwards `ABS_X`/`ABS_Y` whether or not anything captured them, so the
pad *works*; what breaks is everything that reads the profile to find out
where the stick is. An emulator whose port bindings come from the capture gets
a pad with no stick. A front-end cannot draw one, and GOTG had gone as far as
reading SDL's standard axis order instead and marking the stick in the drawing
itself, because the layout could not say. A pad whose stick is somewhere
unusual -- an adapter -- had no way to be told so.

`Control` grew four variants and `CANONICAL_ORDER` went from eighteen to
twenty-two. **Appended, never inserted.** That order is the order a profile is
written in, so moving an entry rewrites every file that was already correct,
and `the_canonical_names_are_still_the_ones_the_python_wrote_to_disk` now
asserts the Python's eighteen as an ordered *prefix* rather than as the whole
list -- the same discipline `COMMANDS` has, for the same reason.

**Worth generalising.** A vocabulary that only ever grows at the end can be
extended without touching anything that used it, and a test that asserts the
whole list forbids growing it at all. The useful assertion is the prefix: what
was there is still there, in the same order, spelled the same way. Both tests
exist now -- the prefix, and a second naming exactly what was appended -- so
neither an accidental rename nor an unannounced addition gets through.

### Three places had been safe only because the control could not exist

All three wrote the left stick from two sources at once, and all three were
safe *only* because nothing had ever mapped onto it.

**The RetroArch profile wrote the stick twice.** `emit::retroarch_profile`
appended four fixed lines -- `input_l_x_plus_axis = "+0"` and its three
siblings -- after the captured ones, so that an unmapped pad still had a
working stick. Nothing in `Control` could produce an `input_l_*` key, so those
four were the only ones in the file. They are not any more. RetroArch takes the
last line under a key, so on a pad whose stick is not on axes 0 and 1 -- the
adapter two findings up is exactly that shape -- the capture was written and
then silently overwritten with the wrong axis. Worse, a half captured onto a
*button* left the opposite half's default axis line shadowing it, which is the
wound `drop_shadowed_axis_halves` exists for, reopened from the other side
because the guard ran before the append. The defaults are now written only for
a key the capture did not name, and the guard runs over the assembled list.

**The clone's stick had two drivers.** `Translator::new` carried the source's
`ABS_X`/`ABS_Y` across untouched whenever no binding *targeted* those codes.
A capture that put the left stick on buttons, or on an adapter's `ABS_RX`/
`ABS_RY`, satisfies that test and still writes `ABS_X`/`ABS_Y` out of
`output_for` -- so the raw axis and the translated one both drove the clone's
stick, and moving the physical stick centred a direction the player was
holding. The carry now asks whether the *control* is spoken for, not whether
the code is.

**The SDL line offered the stick whole and in halves.** `sdl::stick_fields`
refuses an axis a capture has claimed -- but only by looking for a binding
whose *kind* is `Axis` at that index. Nothing makes a stick-kind prompt record
an axis: `feed_key` never consults the control's kind at all. So a half
captured as a button left the physical axis looking unclaimed, and the line
came out `-leftx:b12,leftx:a0` -- the same stick named twice, once signed and
once whole. This was not new; it applied to `rightstick_*` from the day the
C-cluster existed, and an N64 capture is *exactly* this shape, four C-buttons
and an adapter's real right axis. It had simply never been looked at. The rule
now reads off the vocabulary rather than the binding kind: a field is not
guessed whole when some control's `sdl_field()` is one of its signed halves,
whatever that control is bound to.

None of the three was found by a failing test -- they were found by reading the
diff against what each file assumed. All three have one now, and each fails
without its fix.

## A Dolphin port was renamed to please a tool that Dolphin does not consult

`e7a79e1` changed `emit --dolphin-dir` to write `Device = SDL/0/Xbox 360
Controller` instead of `SDL/0/padmap Player 1`, on the reasoning that a clone
mirrors the pad behind it, so SDL finds that pad in its own database and reports
the model. That reasoning is right about SDL's *joystick* name and wrong about
Dolphin, which names a device by the clone. Every player one was then bound to a
device that sends nothing: the file complete, the pad seated and forwarding, and
the game still.

The measurement that settles it is Dolphin's own controller log (`[Logs] CI =
True` in `Logger.ini`), on Four Swords Adventures with an Xbox pad and a Steam
Controller seated:

```
Added device: SDL/0/Xbox 360 Controller     <- the raw pad, grabbed by padmap
Added device: SDL/0/padmap Player 1         <- its clone
Added device: SDL/0/padmap Player 2         <- the Steam Controller's clone
```

Both clones are slot 0, which is the other half of the answer: the slot counts
devices already sharing a *name*, so while every pad is named for its player it
is always 0. `dolphin::sdl_slots`, which ranked clones by `/dev/input` order,
was solving a collision that only the wrong name created.

**The generalisable part is the oracle, not the string.** The original evidence
was `gotg-pads`, which enumerates SDL and does report the model -- so it agreed
with the change and disagreed with the consumer. Both readings were of SDL; only
one was of Dolphin. A binding whose failure mode is silence has to be checked
against the program that reads it, and Dolphin will say what it heard if asked.

Two things from that commit are kept, because neither is about the name:
`sdlprobe::isolated` still remembers per GUID -- `carried()` asks it for every
pad with no stored mapping, on every rewrite, at half a second a call -- and
`dolphin::device` still strips control characters, since the name arrives in a
caller's JSON and a newline in it would close the port's section and bind
everything under it somewhere else.

## Remembering what a player's files said, and what that nearly threw away

A join used to work out every *seated* player's files again -- opening each pad
to read its capabilities and asking SDL about its GUID in a half-second
subprocess -- because `write_all` derives the whole roster from the slots it is
given. Measured on four pads: claim-to-`state` grew about 50 ms a seat on top of
the clone rebuild that was the other half. So `publish::Cache` keeps each
player's derived profile text, SDL line and `Published` record, and only the
join path reuses it; everything else clears it first.

**The rule "everything else clears it" was not true, and the way it was untrue
is the finding.** `finish_mapping` stores a capture and broadcasts events; it
does not rewrite any consumer's config. Neither does `forget_pad`. Nor can it:
`padmap map` from a terminal is a different process writing the same profile
store. So before the cache, the *only* thing that ever carried a fresh capture
into the SDL database and the emulators' configs was the incidental
re-derivation on the next rewrite -- a join, a seat leaving, anything. Cache
that derivation by roster identity alone and the capture is silently dropped:
the wizard says stored, the file on disk has the bindings, and the next person
to sit down writes the guess made before it.

The fingerprint therefore includes the profile store's *contents*, not just the
pad's signature. That is one small file read next to an `open_source` and a
subprocess, and it is the only form that also covers the other process.

Two more from the same reading. A derivation made when `pad_facts` could not
open the pad has no keys, no axes and no mapping; cached, it would be served
for the rest of the session, where before the next join healed it -- so an
unsound one is not kept. And the identity was `vid:pid:name`, which two units
of one model share; it carries `phys` and `uniq` now, since a cache entry
standing in for a different physical pad is the failure this shape invites.

**Worth generalising.** Memoising anything derived from a file that another
process writes needs the file in the key, not the identity of the thing it
describes. The tell here was that no code path connected the writer to the
reader -- which had been survivable only because the reader recomputed so often.

## A seat has to exist before the person does

`padmap-rs exec` hands a game the `/dev/input` it starts with: a tmpfs holding
the nodes that were there, with the raw pads covered. Nothing can be added to
that namespace afterwards -- a bind mount into a running user namespace needs
to be made from inside it -- and SDL's udev hotplug does not cross it either.
So a clone published once the game is running does not exist for that game,
however well the seat was claimed. Somebody joining mid-play got a seat, a
`claim`, a fill that reached the end, and a controller that did nothing.

The fix is not to reach into the sandbox but to have nothing to reach in with:
`reserve` publishes a clone per seat the launch allows *before* it starts, so
they are all bound, and taking a seat adopts that exact device rather than
publishing another. `clone::create_on` takes the reserved device instead of
building one.

**What made it tractable was already there.** Under the 360 identity the clone
is built from a fixed capability list -- `xbox::KEYS` and `xbox::AXES` -- and
only the *translator* needs the pad behind it. So a device for a seat nobody
has taken is the same device, minus the translator, and `VirtualPad` did not
have to learn about sources that are not there yet. Under `mirror` it cannot be
done at all: the layout and GUID come from a pad nobody has picked up.

**Two tests that looked like they proved it and did not.** The first asserted
the reserved node still existed after the seat was claimed. The kernel reuses
`/dev/input/eventN`, so destroying the device and publishing another put a
node back at the same path and the assertion passed either way. The second held
the node's fd open across the claim -- which is what a running game does -- and
that does tell them apart, because the old fd goes dead when its device is
destroyed even if the number comes back. It still passed with the adoption
deliberately broken, because the *first* seat goes through `start_republisher`
rather than `join_republisher`, and only the latter had been sabotaged. Both
paths adopt, so both had to be broken, one at a time, to see the test fail.

**Worth generalising.** A device node's path is not its identity, and neither
is its presence. Anything asserting that a device survived has to hold
something the kernel invalidates -- an open fd -- rather than look the path up
again. The same recycling is why a seating hold follows a pad by node *and*
vid/pid/name/phys/uniq rather than by node alone.
