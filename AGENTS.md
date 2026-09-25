# danstick

Controller assignment for Linux: hold a button per pad, and danstick republishes
each one through uinput as a stable `danstick Player N`. One Rust binary
(`danstick-rs`, wrapped as `danstick`) built and shipped through a Nix flake.
Its main consumer is GOTG (`~/Documents/GOTG`, branch `danstick-integration`),
which pins this repo as a flake input and files what it needs under
`docs/requests/`.

## Setup

- `direnv allow` — enters the flake devShell via nix-direnv (first entry
  evaluates the whole flake, which builds Pegasus and RetroArch; one-off).
- Without the full devShell: `tools/cargo <args>` runs cargo from a `nix shell`
  of just the toolchain (it also sets `PKG_CONFIG_PATH` for `libudev.pc`).
- Inside the devShell, `danstick` on `PATH` is built from the working tree.
- `CARGO_HOME` is `.cargo-home/` in the repo (set by `.envrc`), so a nix build
  of `./rust` never copies a registry into the store.

## Build / Run

    cd rust && cargo build
    nix build .#danstick-rs             # what GOTG consumes; `git add -N` a new file first
    nix run .#danstick -- list
    nix run .#danstick-start            # daemon + udev hide rules
    danstick serve --fresh --follow $$  # a daemon that ends with this shell

Never `path:.`: a path flake copies the whole directory into the store,
gitignored `rust/target` (14GB) included, and again whenever it changed -- it
filled a disk. The git flake already builds uncommitted edits to tracked files.

Other flake outputs: `danstick-play`, `paddump`, `icons`, `nixosModules.danstick`.

## Test

**Run the suite on the cluster, not on a desk.** The journeys make real pads
through `/dev/uinput` and start real daemons, several at once: on somebody's
desktop that is load they feel and fake pads their games can see.

    tools/cluster-test                                      # everything, in a pod (k8s/tests/README.md)
    tools/cluster-test -p danstick-rs --test daemon_journey -- <name>
    DANSTICK_CLUSTER_LINT=1 tools/cluster-test                # fmt and clippy there too

Locally: build, fmt, clippy. The commands below run the same suite in place,
for a machine nobody is using:

    cd rust && cargo test                                   # everything
    python3 tests/run.py [--coverage] [--lint]              # same, one answer from the root
    cargo test -p danstick-core <filter>                      # unit tests by name
    cargo test -p danstick-rs --test daemon_journey -- <name> # one integration test

- Integration tests that need `/dev/uinput` **skip silently** (they print
  "skipped" and return ok). A green run on a machine without uinput proves
  less than it looks; check the eprintln output.
- Journeys spawn a real daemon per test, in parallel. Scope every test by
  `XDG_RUNTIME_DIR` under a temp root and `DANSTICK_ONLY_DEVICE=<substring>`;
  a test that touches `$XDG_RUNTIME_DIR/danstick/` has killed a user's live
  daemon before (STORIES.md, "restarting must be scoped to one socket").
- Every parallel journey publishes its own `danstick Player 1`. Read evidence
  from the daemon's own files (`sdl_controllers.txt`, `assignments.json`,
  state events), never from `/sys/class/input`.
- `rust/crates/danstick-core/tests/corpus/*.json` are answers the deleted Python
  implementation gave, replayed by `*_differential.rs`. They are frozen. A
  deliberate behaviour change edits the recorded answer and says why in the
  commit. `COMMANDS` in `command.rs` is asserted as an ordered prefix: append
  new commands, never reorder.

## Lint & Typecheck

    cd rust && cargo fmt --all --check
    cd rust && cargo clippy --all-targets --all-features -- -D warnings

Both must pass; `checks.rust-lint` in the flake runs exactly these.
Workspace lints warn on `unwrap_used`, `todo`, `unsafe_code` (crates holding
ioctls deny it with targeted allows) and `missing_debug_implementations`.
`rustfmt.toml`: `max_width = 100`.

## Code Style & Conventions

- Comments are one sentence: what a thing is for, not how it works. The
  reasoning goes in the commit body.
- Pure decisions in `danstick-core` as functions over plain data, tested
  without hardware. I/O stays in `danstick-input`; policy stays out of it.
- New pad quirks get a fixture in `danstick_input::fakepad` built from a real
  capture (see `docs/FAKEPAD.md`); if no capture exists, the fixture's
  source string says so.
- Errors reach the user as an `error` event on the socket, never a panic:
  `handle_command` and `tick` are wrapped in `catch_unwind` for that reason.
- Environment variables are `DANSTICK_*` and documented where a consumer
  would look (`docs/EVENTS.md`, README). *(Review: inferred from practice.)*
- `docs/requests/` holds what is still **open**. Answering a request deletes
  its file in the same commit that answers it, so the directory is a to-do
  list rather than an archive: if a file is there, somebody is waiting.
  The answer lives in the commit body, in the docs the change touched, and in
  `FINDINGS.md` when there was a wound. `git log --diff-filter=D --
  docs/requests/` finds what was asked and when it went.

## Architecture

    rust/crates/danstick-core/    decisions: assignment, capture wizard, tuning, config formats, socket protocol
    rust/crates/danstick-input/   the machine: evdev/uinput/udev, discovery, republish, seating I/O, fakepad
    rust/crates/danstick-daemon/  the event loop: server.rs (commands, ticks), seating, publish, events
    rust/crates/danstick-rs/      the CLI binary: main.rs dispatch, commands.rs, exec sandboxing
    tools/                      Python probes and the socket watcher (`padctl.py watch`); no runtime code
    docs/EVENTS.md              the socket contract consumers read; change it with the protocol
    docs/STEAM-DECK.md          the Deck's built-in controls, and the four ways they are not an Xbox pad
    docs/STORIES.md             what a person at the box is promised, and the test that holds each
    docs/requests/              GOTG's open requests only; answering one deletes it (see below)
    FINDINGS.md                 the incident record; every guard in the code has a wound written here

Runtime state lives in `$XDG_RUNTIME_DIR/danstick/` (socket, `assignments.json`,
`launch.cfg`, autoconfig, log); profiles under `$XDG_CONFIG_HOME/danstick/`.

## Boundaries / Do Not Touch

- `tests/corpus/` — frozen evidence (see Test).
- `rust/Cargo.lock` — the flake's `cargoLock` reads it; change only via cargo.
- `result`, `result-*`, `.cargo-home/`, `.direnv/`, `.claude/`, `.swarm/`,
  `ruvector.db` — build output and local tooling, all ignored.
- The user's live daemon and `$XDG_RUNTIME_DIR/danstick/` outside a test root.
- No secrets exist in this repo; do not add any.

## Commits & PRs

- Conventional: `type(scope): lowercase subject`, then a prose body that
  says what was wrong, why this shape, and what was measured. Recent
  commits are the style guide.
- One commit per concern (a feature, its docs) — not one per file.
- Before committing: full `cargo test`, fmt, clippy, all clean.
- Work lands on `main`. `rustify` is the completed-port branch; other
  branches are worktree scratch. *(Review: confirm which branch PRs target.)*

## Gotchas

- `danstick ensure-daemon` replaces a running daemon whose build id or
  identity differs from the binary invoking it. Run from the devShell and it
  replaces the user's installed daemon with the dev build.
- Nothing must outlive a session: use `serve --fresh --follow <pid>` or send
  `{"cmd":"unseat"}`; a plain `serve` restores yesterday's seats and grabs
  those pads under `EVIOCGRAB`.
- Steam, when running, grabs a freshly attached pad for about a second; a
  hold made under that grab reaches nobody, danstick included.
- A daemon restoring real pads republishes them before greeting its first
  client; wait for the `state` event rather than reading it immediately.
- A Steam Virtual Gamepad (`28de:11ff`) mirrors a real pad. Discovery keeps
  as many mirrors as there are controllers danstick cannot read itself (more
  mirrors than readable pads, or a Deck whose `28de:1205` hidraw has no event
  node), lowest Steam slot first, and drops the rest. A triton pad (2026 Steam
  Controller) cannot be grabbed at all.
- A Steam Deck's own controls (`28de:1205`) are a pad only while nothing holds
  the hidraw node, and Steam holds it: with Steam up there is no `Steam Deck`
  node, just the lizard keyboard and mouse and Steam's mirror. Its d-pad is
  keys, `ABS_HAT0` is the left trackpad, the triggers are `ABS_HAT2`, and X and
  Y arrive on each other's codes. All of it in `docs/STEAM-DECK.md`.
- `nix build` of the Rust package needs several GB free; a 99%-full root
  disk fails it mid-build with a misleading error.
- `.envrc` still exports `PYTHONPATH=$PWD/src`; there is no `src/` any more.
  *(Review: stale line, harmless.)*
