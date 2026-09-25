# padmap

Controller assignment for Linux, as stable virtual gamepads.

- Linux can't tell identical controllers apart, so no config file can pin player order.
- padmap asks instead: hold a button on each controller, in the order you want.
- Each controller is republished through uinput as `padmap Player 1`, `padmap Player 2`, …
- Games and emulators bind to those names, which never change.
- Button mappings and stick calibration are learned once per controller model and reused.

## Usage

    nix run .#padmap -- list       # what is plugged in
    nix run .#padmap-start         # start the daemon, hide the physical pads

| Command | What it does |
| --- | --- |
| `list [--json]` | controllers and their players |
| `setup` | assign player order by holding a button |
| `map` / `calibrate` / `tune` | record buttons, stick centres, deadzones |
| `forget` | drop what was learned about a controller |
| `run` / `serve` | republish the pads (`serve` adds a socket) |
| `ensure-daemon` | start the daemon, or replace a stale one |
| `play` / `launch` | start a game bound to the assigned order |
| `exec -- <program>` | run a program seeing only padmap's pads |
| `hide` | udev rules hiding the physical pads |

## What it produces

- Virtual pads: `/dev/input/event*`, named `padmap Player N`
- SDL database: `~/.config/padmap/sdl_controllers.txt`
- RetroArch autoconfig: `$XDG_RUNTIME_DIR/padmap/autoconfig/udev/`
- Emulator configs: Cemu, Dolphin, ares, Ryujinx ([docs/EMULATORS.md](docs/EMULATORS.md))
- Control socket: `$XDG_RUNTIME_DIR/padmap/padmap.sock` ([docs/EVENTS.md](docs/EVENTS.md))
- Learned profiles: `~/.local/share/padmap/devices/`

## Project structure

    rust/crates/padmap-core/     pure logic: assignment, mappings, config formats, protocol
    rust/crates/padmap-input/    evdev, uinput, udev: discovery and republishing
    rust/crates/padmap-daemon/   the event loop behind the socket
    rust/crates/padmap-rs/       the `padmap` CLI
    tools/                       probes, socket watcher, cluster test runner
    k8s/tests/                   the pod the test suite runs in
    docs/                        socket contract, emulators, Steam Deck, latency
    FINDINGS.md                  incident record: why each guard exists

## Development

- `nix develop`: the toolchain, with `padmap` on `PATH` built from the working tree.
- `tools/cluster-test`: the test suite, in a Kubernetes pod ([k8s/tests/README.md](k8s/tests/README.md)).
- `(cd rust && cargo test)`: the same locally. It creates real uinput pads, so use an idle machine.

## License

MIT. See [LICENSE](LICENSE).
