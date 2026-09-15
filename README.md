# padmap

Controller assignment for Linux, as stable virtual gamepads.

Plug in four identical adapter ports and nothing can tell them apart: they
report the same name, phys, uniq, vendor, product, version and properties, and
only their `inputN` ordinal differs — on a global counter that libudev sorts as
a string. No configuration file can pin player order to hardware that is
genuinely indistinguishable.

So padmap asks. You hold a button on each controller in the order you want
them, and padmap republishes each one through uinput as `padmap Player 1`,
`padmap Player 2`, … — pads whose identity *is* stable, because padmap made it.
Everything downstream binds to those.

    padmap list        what is plugged in, and what a game would make of it
    padmap setup       assign player order by pressing and holding a button
    padmap map         record which button is which
    padmap calibrate   measure where the sticks actually rest
    padmap run         republish the assigned pads and keep them alive
    padmap serve       the same, as a daemon with a socket
    padmap hide        udev rules hiding the physical pads from everything else
    padmap launch      run, then start RetroArch bound to the assigned order

## What it gives other programs

padmap is a gamepad layer, not an application. It produces four things, and
anything can consume them:

| | where |
| --- | --- |
| virtual pads | `/dev/input/event*`, named `padmap Player N` |
| an SDL controller database | `~/.config/padmap/sdl_controllers.txt` |
| RetroArch autoconfig profiles | `$XDG_RUNTIME_DIR/padmap/autoconfig/udev/` |
| a control socket | `$XDG_RUNTIME_DIR/padmap/padmap.sock` |

Point any SDL program at the database and it gets the mappings padmap
captured:

    SDL_GAMECONTROLLERCONFIG_FILE=~/.config/padmap/sdl_controllers.txt yourgame

The socket is newline-delimited JSON, documented in `src/padmap/protocol.py`.
A client sends commands and renders the events it gets back; the daemon is
authoritative and holds no expectation about who is listening. `tools/padctl.py`
is a working client in a hundred lines.

## Why the mappings are captured rather than guessed

A vendor id names a *model*, not a controller. 0x0079 is DragonRise, resold in
a great many unrelated adapters, so `0079:1879` is an N64 adapter on one
machine and a generic pad on the next. And a worn N64 stick here rests at 174
on a 0–255 axis whose nominal centre is 128 — 36% deflected, which a front-end
acts on as a held direction. Neither is a property of a product line, and both
are settled in a second by the person holding the controller.

Everything learned that way is stored per controller model under
`~/.local/share/padmap/devices/`, and follows the controller rather than the
player slot it happened to claim.

## Building

    nix run .#padmap -- list
    nix run .#padmap-start          # daemon + udev hide rules
    nix develop                     # python, cargo, clippy, evemu, perf

There is a Rust port in progress under `rust/` — see
[docs/RUSTIFY.md](docs/RUSTIFY.md) for what is ported and what is not, and
[docs/LATENCY.md](docs/LATENCY.md) for what padmap actually costs a
controller, measured.

`FINDINGS.md` is the incident record: every guard in this codebase has a
wound behind it, and that is where they are written down.
