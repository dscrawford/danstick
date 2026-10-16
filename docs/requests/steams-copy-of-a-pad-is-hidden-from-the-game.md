# Steam's copy of a pad is hidden from the game

## What GOTG saw

Paper Mario on PaperBoat, launched from Steam on the desktop, with a Steam
Controller (hidraw, Triton) paired *over the game*. A would not jump. Opening
PaperBoat's menu with Select and closing it again made Mario jump exactly
once. The clone was fine: `danstick Player 1` emitted a clean `BTN_SOUTH`
down and up for every press (read off `/dev/input/event257` beside the game).

The game held a second pad open on the same port: `/dev/input/event256`,
`Microsoft X-Box 360 pad 1`, `28de:11ff`, no phys, no uniq -- Steam Input's
copy of the Steam Controller. From danstick's log:

    01:35:40  the game starts; `exec` binds event256 into its sandbox
    01:35:57  the 1.5 s hold on the Steam Controller takes seat 1
    01:35:58  "event256 is Steam's copy of Steam Controller; holding it with
              the seat" -- grabbed while the held button is still down

The release never reaches the game's SDL. That pad reports A held for the
rest of the session, and the port's pads are ORed. Paper Mario acts on the
press *edge*: a button held forever never fires; the menu zeroes the pad;
closing it re-asserts A -- one jump. Pairing in the picker first hides it
(the grab precedes the open, so nothing is frozen), which is why the e2e
and most evenings never see it.

## Why

`isolate::plan` keeps every node that is a clone or not one of the raw pads
danstick seats. Steam's copy is neither: the seat goes to the hidraw, and the
copy is only known as a twin at claim time (`claimed.twins`, `note_twins`),
when the game has had it open for as long as it has been running. The journey
test says the copy "is left for a game under Steam to read" before a seat is
taken -- but a game under danstick reads clones, and a program that opened
the copy live and then lost it to a grab keeps whatever that copy last said.

## What would be enough

- `exec` hides Steam's virtual pads (`28de:11ff`) the way it hides raw pads,
  always. Whatever danstick seats through such a node -- a Deck's controls
  with Steam Input running -- reaches the game as a clone, as everything else
  does; the game never needs the copy itself.
- A test: a game run under `exec` beside a `28de:11ff` uinput pad and a
  hidraw-reachable Steam Controller does not have the copy's node open, and a
  seat taken over the game (the hold grabbing the copy mid-press) leaves the
  clone's `BTN_SOUTH` at rest in what the game reads.
