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
