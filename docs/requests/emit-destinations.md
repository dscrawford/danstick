# `emit` needs to be told where to write

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
