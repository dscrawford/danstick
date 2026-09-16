# `emit` needs to be told where to write

> **Done.** All four flags, exactly as asked. See "What was built" at the end.


**What GOTG does.** Every game runs in its own environment: a nix derivation
plus a state directory, with `XDG_CONFIG_HOME` pointed inside it. Tears of the
Kingdom at 60fps and the same game at 120fps are two Ryujinx configurations
that never see each other, and neither is `~/.config/Ryujinx`. That isolation
is the feature — a mod cannot corrupt the plain launch, and two variants of one
game keep separate settings and saves.

**What padmap does today.** `padmap-rs emit` reads the pad list on stdin and
calls

```rust
emulators::publish(&pads, &emulators::Destinations::default())
```

`Destinations` already has exactly the right shape —

```rust
pub struct Destinations {
    pub cemu_dir: Option<PathBuf>,
    pub ares_settings: Option<PathBuf>,
    pub ryujinx_config: Option<PathBuf>,
    pub env_file: Option<PathBuf>,
}
```

— and its own comment says "`None` means the real location. Overriding one
leaves the others alone". But nothing on the command line reaches it, so a
caller that is not writing to the user's home cannot use `emit` at all.

**What would be enough.** Flags on `emit`, one per field:

```
padmap-rs emit --ryujinx-config /path/to/state/config/Ryujinx/Config.json
padmap-rs emit --cemu-dir /path/to/state/config/Cemu/controllerProfiles
padmap-rs emit --ares-settings /path/to/state/config/ares/settings.bml
padmap-rs emit --env-file /path/to/state/padmap-env.sh
```

Absent flags keep today's behaviour exactly. GOTG would call `emit` once per
launch with the directories that launch is about to use.

A second form would also work and may be tidier, since the pad list is already
JSON on stdin: accept an object rather than a bare array, with the pads under
one key and the destinations under another. Either is fine; the flags are
easier to use from a shell script.

**Why not work around it.** GOTG could copy padmap's output out of `~/.config`
into the environment, but that means knowing which files padmap wrote, when it
wrote them, and what to do when the user has their own Ryujinx config — which
is reimplementing the part of padmap GOTG is trying to stop maintaining.

## What was built

The four flags, one per `Destinations` field:

```
padmap emit --cemu-dir D --ares-settings F --ryujinx-config F --env-file F
```

Absent flags keep today's behaviour, and overriding one leaves the others
alone. The object-on-stdin form was not added -- the flags are what a shell
script wants, and two ways to say the same thing is two things to keep in
step.

Two details beyond the request:

* **The paths written go to stdout, one per line.** They already did, but now
  they are the paths *you gave*, so a caller can act on what happened rather
  than assuming its own flags landed.
* **A flag with no value is refused** (exit 2), rather than falling back to the
  default. `--ryujinx-config` with the path forgotten would otherwise mean
  "the user's own Ryujinx config", which is the one file this request exists
  to avoid writing.

`tests/emit_destinations.rs` drives the real binary the way a launcher does,
and asserts the negative as well: nothing is written to the default location
when a flag points elsewhere.
