# Requests from GOTG

GOTG (`~/Documents/GOTG`, branch `padmap-integration`) is replacing its own
controller handling with padmap, and depends on this repository as a flake
input pinned to a revision. Everything it needs and padmap does not yet do is
written down here, one file per request.

Written by the agent doing that integration, so every one of them is something
that blocked a real launch rather than a wish. Where a file names a path like
`src/client/lib/pads-dolphin.sh`, that is GOTG's tree, and it is named so the
existing implementation can be read rather than guessed at.

Each file says what GOTG is trying to do, what it does today, why padmap's
current shape does not reach it, and what would be enough. None of them ask
for a redesign; they are the seams an *abstraction layer over emulators*
needs when the thing calling it keeps each emulator in a directory of its own.

| request | why |
| --- | --- |
| [emit-destinations.md](emit-destinations.md) | GOTG isolates every environment, so config must be written where it says |
| [dolphin.md](dolphin.md) | GameCube and Wii are Dolphin, and padmap writes no Dolphin config |
| [machine-readable-list.md](machine-readable-list.md) | a launcher has to enumerate pads without a daemon and without parsing prose |
| [always-seating.md](always-seating.md) | a controller that arrives mid-game should be able to join without everybody stopping |
| [triton-assignment.md](triton-assignment.md) | **bug**: the 2026 Steam Controller pairs in a session and is then never read |
| [resume-republishing.md](resume-republishing.md) | **bug**: one sleeping wireless pad leaves every controller unpublished |
| [secondary-bindings.md](secondary-bindings.md) | a control can hold one input, so a second button for the same control has nowhere to live |
| [session-daemon.md](session-daemon.md) | every session starts unseated and the daemon ends with it — today it outlives everything and restores yesterday's seats |
| [controllers-that-are-keyboards.md](controllers-that-are-keyboards.md) | a Steam Controller in lizard mode and a Bluetooth Xbox pad are keyboards and mice too; GOTG holds them in the picker, padmap should hold them in the game |
| [keyboard-as-a-player.md](keyboard-as-a-player.md) | holding space on the grid should seat the keyboard as player N, bound in every emulator like a pad |
| [press-in-the-wizard.md](press-in-the-wizard.md) | during a capture the front-end's SDL sees nothing from the pad; an `input` event would let it show the button under the thumb |
| [join-a-session-late.md](join-a-session-late.md) | a session's pads are fixed when it opens; a controller switched on during one cannot take a seat |
| [look-like-an-xbox-pad.md](look-like-an-xbox-pad.md) | an `xbox360` clone identity, so decompiled ports and every SDL game map it from the database they were built with |
| [seating-costs-the-game-its-input.md](seating-costs-the-game-its-input.md) | **bug**: seating open rescans every tick and a press takes 108 ms to reach the game |
| [replacing-a-daemon-with-a-seat.md](replacing-a-daemon-with-a-seat.md) | **regression**: replacing a daemon that has a pad seated fails after 10 s; it took 0.5 s at f4356e4 |
