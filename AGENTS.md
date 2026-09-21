# padmap

Controller assignment for Linux: hold a button per pad, and padmap republishes
each one through uinput as a stable `padmap Player N`. One Rust binary
(`padmap-rs`, wrapped as `padmap`) built and shipped through a Nix flake.
Its main consumer is GOTG (`~/Documents/GOTG`, branch `padmap-integration`),
which pins this repo as a flake input and files what it needs under
`docs/requests/`.

## Setup

- `direnv allow` — enters the flake devShell via nix-direnv (first entry
  evaluates the whole flake, which builds Pegasus and RetroArch; one-off).
- Without the full devShell: `tools/cargo <args>` runs cargo from a `nix shell`
  of just the toolchain (it also sets `PKG_CONFIG_PATH` for `libudev.pc`).
- Inside the devShell, `padmap` on `PATH` is built from the working tree.
- `CARGO_HOME` is `.cargo-home/` in the repo (set by `.envrc`), so a nix build
  of `./rust` never copies a registry into the store.

## Build / Run

    cd rust && cargo build
    nix build path:.#padmap-rs        # what GOTG consumes; `path:` when the tree is dirty
    nix run .#padmap -- list
    nix run .#padmap-start            # daemon + udev hide rules
    padmap serve --fresh --follow $$  # a daemon that ends with this shell

Other flake outputs: `padmap-play`, `paddump`, `icons`, `nixosModules.padmap`.

## Test

    cd rust && cargo test                                   # everything
    python3 tests/run.py [--coverage] [--lint]              # same, one answer from the root
    cargo test -p padmap-core <filter>                      # unit tests by name
    cargo test -p padmap-rs --test daemon_journey -- <name> # one integration test

- Integration tests that need `/dev/uinput` **skip silently** (they print
  "skipped" and return ok). A green run on a machine without uinput proves
  less than it looks; check the eprintln output.
- Journeys spawn a real daemon per test, in parallel. Scope every test by
  `XDG_RUNTIME_DIR` under a temp root and `PADMAP_ONLY_DEVICE=<substring>`;
  a test that touches `$XDG_RUNTIME_DIR/padmap/` has killed a user's live
  daemon before (STORIES.md, "restarting must be scoped to one socket").
- Every parallel journey publishes its own `padmap Player 1`. Read evidence
  from the daemon's own files (`sdl_controllers.txt`, `assignments.json`,
  state events), never from `/sys/class/input`.
- `rust/crates/padmap-core/tests/corpus/*.json` are answers the deleted Python
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
- Pure decisions in `padmap-core` as functions over plain data, tested
  without hardware. I/O stays in `padmap-input`; policy stays out of it.
- New pad quirks get a fixture in `padmap_input::fakepad` built from a real
  capture (see `docs/FAKEPAD.md`); if no capture exists, the fixture's
  source string says so.
- Errors reach the user as an `error` event on the socket, never a panic:
  `handle_command` and `tick` are wrapped in `catch_unwind` for that reason.
- Environment variables are `PADMAP_*` and documented where a consumer
  would look (`docs/EVENTS.md`, README). *(Review: inferred from practice.)*

## Architecture

    rust/crates/padmap-core/    decisions: assignment, capture wizard, tuning, config formats, socket protocol
    rust/crates/padmap-input/   the machine: evdev/uinput/udev, discovery, republish, seating I/O, fakepad
    rust/crates/padmap-daemon/  the event loop: server.rs (commands, ticks), seating, publish, events
    rust/crates/padmap-rs/      the CLI binary: main.rs dispatch, commands.rs, exec sandboxing
    tools/                      Python probes and the socket watcher (`padctl.py watch`); no runtime code
    docs/EVENTS.md              the socket contract consumers read; change it with the protocol
    docs/STORIES.md             what a person at the box is promised, and the test that holds each
    docs/requests/              GOTG's requests; keep the text, append "What was built"
    FINDINGS.md                 the incident record; every guard in the code has a wound written here

Runtime state lives in `$XDG_RUNTIME_DIR/padmap/` (socket, `assignments.json`,
`launch.cfg`, autoconfig, log); profiles under `$XDG_CONFIG_HOME/padmap/`.

## Boundaries / Do Not Touch

- `tests/corpus/` — frozen evidence (see Test).
- `rust/Cargo.lock` — the flake's `cargoLock` reads it; change only via cargo.
- `result`, `result-*`, `.cargo-home/`, `.direnv/`, `.claude/`, `.swarm/`,
  `ruvector.db` — build output and local tooling, all ignored.
- The user's live daemon and `$XDG_RUNTIME_DIR/padmap/` outside a test root.
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

- `padmap ensure-daemon` replaces a running daemon whose build id or
  identity differs from the binary invoking it. Run from the devShell and it
  replaces the user's installed daemon with the dev build.
- Nothing must outlive a session: use `serve --fresh --follow <pid>` or send
  `{"cmd":"unseat"}`; a plain `serve` restores yesterday's seats and grabs
  those pads under `EVIOCGRAB`.
- Steam, when running, grabs a freshly attached pad for about a second; a
  hold made under that grab reaches nobody, padmap included.
- A daemon restoring real pads republishes them before greeting its first
  client; wait for the `state` event rather than reading it immediately.
- A Steam Virtual Gamepad (`28de:11ff`) mirrors a real pad; discovery drops
  it when any other pad is present. A triton pad (2026 Steam Controller)
  cannot be grabbed at all.
- `nix build` of the Rust package needs several GB free; a 99%-full root
  disk fails it mid-build with a misleading error.
- `.envrc` still exports `PYTHONPATH=$PWD/src`; there is no `src/` any more.
  *(Review: stale line, harmless.)*
