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

### tools/e2e_ports.py

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

`tools/e2e_daemon.py` now asserts that a daemon on the real runtime dir
survives the run, so this cannot regress silently.

### tools/e2e_daemon.py

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

### tools/check_autosetup.py

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

### tools/check_theme_setup.py

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

`tools/check_mapping.py` holds these. Everything above is the foundation only:
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

`tools/check_launch.py`, which had been living in a scratch directory all
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

### tools/e2e_picker.py

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

`tools/check_sdl_live.py` measures it against real SDL with a real uinput pad:
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

`tools/e2e_sdl_reload.py` runs the patched Pegasus under Xvfb with a uinput pad
that exists *before* the frontend starts, so the frontend opens it -- the exact
state the bug report describes -- then a stub daemon sends one `sdl_mapping`
event, and the frontend's own log is read back:

    [i] padmap: mapping added for 03008703091200000400000001000000:
        03008703091200000400000001000000,PADMAP RELOADTEST,a:b7,...,crc:0387,

`crc:0387` is SDL's own doing: it moves the name checksum out of the GUID into a
field. Its presence is a second confirmation that the line went in under the
GUID SDL computes for that device rather than one padmap merely believes in.

### New tools

- `tools/check_scopes.py` -- storage, migration of a real legacy profile,
  resolution order, core and game keys, launch argument parsing, and what
  actually lands in the `.cfg` for four different launches. The two captures
  are told apart by keys only one of them can produce (`input_a_btn` cannot
  appear in an N64 profile at all), not by "something was written".
- `tools/check_sdl_live.py` -- SDL's live re-binding, both paths, proved with
  presses.
- `tools/e2e_sdl_reload.py` -- the patched frontend applying a mapping it is
  handed mid-session.
- `tools/e2e_picker.py` gained a second half: re-open setup, choose "Nintendo 64
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
