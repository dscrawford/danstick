"""A mapping missing controls its layout defines must say so.

Layouts gain controls. The GameCube layout gained a C-stick; the pads mapped
before that kept their twelve-control captures, and nothing revisits a stored
capture when the layout changes underneath it. The stale mapping stays
resolvable, stays chosen, and stays *reported as a mapping* -- `has_mapping`
is true, the setup screen shows the pad as configured, and a profile is
written for it.

What the profile does not contain is any key for the four controls that did
not exist at capture time. An absent binding emits no line at all, so there is
no wrong value to notice and no warning to read. The control is dead in game
and every artefact says the controller is fine.

The only place that difference is visible is the moment the mapping is
resolved for a launch, which is why the line lives there. Three properties:

  * a mapping short of its layout's controls names exactly the ones missing
  * a complete mapping stays quiet -- otherwise every launch logs a non-event
    and the line stops meaning anything
  * a pad with no bindings at all stays quiet, because that is the
    never-mapped path and it has its own handling; reporting sixteen missing
    controls for it would bury the real cases

Nothing here opens a device or writes a profile.

    python3 tools/check_unmapped_controls.py
"""

from __future__ import annotations

import logging
import os
import sys
import tempfile
from pathlib import Path

_SANDBOX = tempfile.mkdtemp(prefix="padmap-unmapped-")
for _var in ("XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    os.environ[_var] = str(Path(_SANDBOX) / _var.lower())
    Path(os.environ[_var]).mkdir(parents=True, exist_ok=True)

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import layouts, retroarch  # noqa: E402
from padmap.devices import Pad  # noqa: E402
from padmap.mapping import Binding  # noqa: E402


def pad() -> Pad:
    return Pad(path="/dev/input/event900", name="Test Pad", phys="usb",
               uniq="", vid=0x057E, pid=0x2009, syspath="")


class Captured(logging.Handler):
    """Collects the messages `_log_unmapped` emits, fully rendered."""

    def __init__(self) -> None:              # noqa: D107
        super().__init__()
        self.lines: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.lines.append(record.getMessage())


def run(bindings: dict[str, Binding], layout_id: str) -> list[str]:
    handler = Captured()
    log = logging.getLogger("padmap.retroarch")
    # setLevel, not `log.level = ...`: assigning the attribute
    # leaves Logger._cache holding an isEnabledFor answer worked
    # out under the old level, so a logger that has already
    # declined an INFO record goes on declining it.
    previous = log.level
    log.setLevel(logging.INFO)
    log.addHandler(handler)
    try:
        retroarch._log_unmapped(pad(), "console:" + layout_id, layout_id,
                                bindings)
    finally:
        log.removeHandler(handler)
        log.setLevel(previous)
    return handler.lines


def button(index: int) -> Binding:
    return Binding(kind="button", index=index, value=0, ra_index=index)


def complete(layout_id: str) -> dict[str, Binding]:
    return {
        control.canonical: button(i)
        for i, control in enumerate(layouts.get(layout_id).controls)
    }


def check_missing_controls_are_named() -> None:
    print("\na mapping short of its layout's controls names them:")
    bindings = complete("gamecube")
    dropped = ["rightstick_up", "rightstick_down",
               "rightstick_left", "rightstick_right"]
    for name in dropped:
        bindings.pop(name)

    lines = run(bindings, "gamecube")
    if not lines:
        raise SystemExit(
            "FAIL: a mapping missing four of its layout's controls logged "
            "nothing. The pad still counts as mapped and the profile is "
            "written without those keys, so the controls are dead in game "
            "with no evidence anywhere that they were ever meant to exist")
    said = " ".join(lines)
    for name in dropped:
        if name not in said:
            raise SystemExit(
                f"FAIL: {name!r} has no binding and was not named in the log. "
                f"Naming only some of the gap sends the reader looking for a "
                f"different problem than the one they have.\n  logged: {said}")
    print(f"  ok  {said}")


def check_a_complete_mapping_is_quiet() -> None:
    print("\na mapping with every control stays quiet:")
    lines = run(complete("n64"), "n64")
    if lines:
        raise SystemExit(
            "FAIL: a complete mapping logged a gap:\n  "
            + "\n  ".join(lines)
            + "\nEvery launch would carry this line and a reader would stop "
              "reading it, which costs the real cases their only signal")
    print("  ok  nothing logged")


def check_an_unmapped_pad_is_quiet() -> None:
    print("\n...and a pad with no bindings at all is quiet too:")
    lines = run({}, "n64")
    if lines:
        raise SystemExit(
            "FAIL: a never-mapped pad was reported as missing controls:\n  "
            + "\n  ".join(lines)
            + "\nThat pad takes the derive-from-libretro path instead, and "
              "listing its whole layout here buries the stale-capture case "
              "this line exists for")
    print("  ok  nothing logged")


def check_every_layout_can_be_reported() -> None:
    print("\nevery layout resolves for reporting:")
    for entry in layouts.catalogue():
        layout_id = entry["id"]
        lines = run({}, layout_id)          # must not raise
        if lines:
            raise SystemExit(f"FAIL: {layout_id} reported a gap for no "
                             f"bindings")
        got = layouts.get(layout_id)
        print(f"  ok  {layout_id:10s} {len(got.controls):2d} control(s)")


def main() -> int:
    check_missing_controls_are_named()
    check_a_complete_mapping_is_quiet()
    check_an_unmapped_pad_is_quiet()
    check_every_layout_can_be_reported()
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
