# The keyboard should be able to take a seat

**What GOTG wants.** On the picker's grid, holding the space bar seats the
keyboard as the next player, exactly as holding a button seats a pad: the
strip fills in a keyboard icon in the player's colour, badge N, and from then
on player N in every game is the keyboard. `unseat` drops it like any seat.

**What GOTG does now.** The picker times the hold itself -- padmap never sees
the keyboard's keys, and should not -- reveals the keyboard icon over 0.6 s,
and sends

```json
{"cmd": "seat_keyboard"}
```

once, at the end of the hold. Today padmap answers `unknown command
"seat_keyboard"` and nothing is seated; the test that holds GOTG to the
feature (`tests/e2e/test_controllers.py`) is `xfail(strict=True)` against this
request.

**Why it is padmap's.** A seat is what padmap writes into every emulator's
configuration -- player N's bindings in ares' `settings.bml`, Dolphin's
`GCPadNew.ini`, Ryujinx's `Config.json`, Cemu's profiles. For a pad that is
the clone's SDL mapping; for the keyboard it is each emulator's keyboard
device and a default layout (arrows, Z/X, Enter, or whatever each already
calls its keyboard default). GOTG has no business writing those files; it has
one place that knows how, and it is padmap.

**What would be enough.**

- `{"cmd": "seat_keyboard"}` takes the lowest free seat for the keyboard. No
  clone, no grab: the keyboard stays the compositor's. `state.players[]` gains
  `{"player": N, "name": "Keyboard", "icon": "keyboard", "configured": true,
  "keyboard": true}`; a `claim` with the same goes out first. Refused, with an
  `error`, when every seat is taken, when the keyboard already holds one, and
  while a session is open.
- The consumers are rewritten as for any seat change, with player N bound to
  the emulator's keyboard using its own default keyboard layout. A pad seated
  after it takes N+1 as usual.
- `unseat` and `{"cmd": "unseat", "player": N}` work on it; `--fresh` forgets
  it like any seat; `--follow` ends with it like any daemon.
- `seating` mode ignores it -- there is no device to hold a button on -- and
  is not closed by it.

**One thing to keep.** A machine with one keyboard and one pad, in that
order: player 1 keyboard, player 2 pad. Emulators that number by *device*
rather than by seat (ares' port order, RetroArch's index) need the keyboard's
seat carried, not skipped, or the pad lands in port 1 and the keyboard in
nothing.
