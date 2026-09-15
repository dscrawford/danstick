#!/usr/bin/env python3
"""The udev rules that stop RetroArch seeing the pads padmap republishes.

RetroArch's udev joypad driver enumerates every device with
ID_INPUT_JOYSTICK=1. padmap opens the physical pads and publishes virtual
ones, so unless the physical adapters are hidden RetroArch sees both -- the
adapter's ports *and* the virtual pad made from them.

What that looks like when it goes wrong was reported and then confirmed
against RetroArch's own autoconfig log: a GameCube adapter plugged in after
the rules were generated kept ID_INPUT_JOYSTICK, its four ports took player
slots 2 through 5, and the Fightstick was configured into slot 5 before its
reservation dragged it back to 1. Nothing failed; there were simply six
controllers where there should have been two.

Every failure in this area is silent in exactly that way, which is why the
checks below are picky about three things in particular:

  * `targets` describes the *hardware*, never the current assignment.
    Assignment changes every session; which adapters exist does not. Rules
    built from the assignment miss whatever is unplugged or unassigned, and
    regenerating with one pad assigned actively DROPS the rules covering the
    others -- a stale file turned into a wrong one.
  * `unhidden` reads the installed file. What udev applies is what is on
    disk, not what padmap remembers writing; a rebuild, a reboot or a hand
    edit puts those out of step, and this is the only thing that notices.
  * `install` reloads udev. udev keeps its rules in memory, so a file written
    without the reload changes nothing until the next boot -- padmap would
    report having hidden a controller that RetroArch can still see.

Nothing here touches the real system: RULES_PATH, RUNTIME_RULES_PATH and
_udevadm are all redirected into a temp directory, so no real udev rule is
written and no real udevadm is run.
"""

from __future__ import annotations

import contextlib
import os
import re
import sys
import tempfile
from pathlib import Path

# Before importing padmap: nothing here should be able to reach real user
# state even by accident.
_SANDBOX_HOME = tempfile.mkdtemp(prefix="check-hide-rules-")
os.environ["XDG_RUNTIME_DIR"] = _SANDBOX_HOME
os.environ["XDG_CONFIG_HOME"] = _SANDBOX_HOME
os.environ["XDG_DATA_HOME"] = _SANDBOX_HOME

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import hide  # noqa: E402
from padmap.devices import Pad  # noqa: E402

RULE_RE = re.compile(
    r'ATTRS\{idVendor\}=="([0-9a-fA-F]{4})",\s*'
    r'ATTRS\{idProduct\}=="([0-9a-fA-F]{4})"')


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def pad(name: str, vid: int, pid: int, node: str = "event9") -> Pad:
    return Pad(path=f"/dev/input/{node}", name=name, phys=f"usb-{node}",
               uniq="", vid=vid, pid=pid, syspath=f"/sys/class/input/{node}")


# The three adapters on the machine the reports came from.
STICK = pad("MAYFLASH Arcade Fightstick F300", 0x0079, 0x1830, "event20")
USBPAD = pad("USB GamePad", 0x0079, 0x1879, "event21")
CUBE = [pad("MAYFLASH GameCube Controller Adapter", 0x0079, 0x1843,
            f"event{22 + n}") for n in range(4)]
# A vid of 0x000d exercises zero padding: udev compares these as text, so a
# rule saying "d" matches nothing at all.
BLUETOOTH = pad("8BitDo SN30", 0x000d, 0x0f00, "event30")
# Some kernel joypads report no ids (some Bluetooth stacks, virtual devices).
NAMELESS = pad("Unknown Gamepad", 0x0000, 0x0000, "event31")

ALL_HARDWARE = [STICK, USBPAD, *CUBE]


class Udevadm:
    """Stand-in for hide._udevadm that records what would have been run."""

    def __init__(self, problem: str = "") -> None:
        self.calls: list[tuple[str, ...]] = []
        self.problem = problem

    def __call__(self, *args: str) -> str:
        self.calls.append(args)
        return self.problem


@contextlib.contextmanager
def sandbox(problem: str = ""):
    """Redirect both rules paths and udevadm into a temp directory."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        saved = (hide.RUNTIME_RULES_PATH, hide.RULES_PATH, hide._udevadm)
        hide.RUNTIME_RULES_PATH = root / "run/udev/rules.d/99-padmap.rules"
        hide.RULES_PATH = root / "etc/udev/rules.d/99-padmap.rules"
        recorder = Udevadm(problem)
        hide._udevadm = recorder            # type: ignore[assignment]
        # Belt and braces: if a future refactor makes these absolute again,
        # this test must not be what writes a rule to the live machine.
        for path in (hide.RUNTIME_RULES_PATH, hide.RULES_PATH):
            if not str(path).startswith(str(root)):
                fail(f"the test failed to redirect {path} out of the way")
        try:
            yield root, recorder
        finally:
            hide.RUNTIME_RULES_PATH, hide.RULES_PATH = saved[0], saved[1]
            hide._udevadm = saved[2]        # type: ignore[assignment]


def rule_lines(rules: str) -> list[str]:
    return [ln for ln in rules.splitlines()
            if ln.strip() and not ln.lstrip().startswith("#")]


def covered_pairs(rules: str) -> list[tuple[int, int]]:
    return [(int(v, 16), int(p, 16)) for v, p in RULE_RE.findall(rules)]


def check_targets_are_the_hardware() -> None:
    print("\nwhich pads the rules must cover:")

    # The reported bug exactly: rules generated from `[a.pad for a in
    # assignments]` while only the Fightstick held a slot.
    pairs = [(p.vid, p.pid) for p in hide.targets(ALL_HARDWARE, [STICK])]
    if (0x0079, 0x1843) not in pairs:
        fail("an adapter that is plugged in but unassigned gets no rule, so "
             "RetroArch enumerates its four ports and they take player slots "
             "2-5 -- the reported bug")
    if pairs != [(0x0079, 0x1830), (0x0079, 0x1879), (0x0079, 0x1843)]:
        fail(f"rules would cover {[(hex(v), hex(d)) for v, d in pairs]}, not "
             f"every adapter on the machine")
    print("  ok  every adapter present, assigned or not")

    # The destructive direction. Regenerating with one pad assigned must not
    # shrink the file: pads that were correctly hidden would come back.
    if len(hide.targets(ALL_HARDWARE, [])) != 3:
        fail("adapters were dropped when nothing was assigned -- running "
             "`padmap hide` at the wrong moment would un-hide the others")
    if len(hide.targets(ALL_HARDWARE, [CUBE[0]])) != 3:
        fail("regenerating with one controller assigned dropped the rules "
             "covering the rest, un-hiding pads that were hidden before")
    print("  ok  regenerating never shrinks the set")

    # Under sudo, XDG_RUNTIME_DIR is root's, so the assignment file usually
    # cannot be read at all -- `sudo padmap hide` must still cover everything.
    if len(hide.targets(ALL_HARDWARE, None)) != 3:
        fail("with no assignment readable (the normal case under sudo) the "
             "rules would cover nothing")
    print("  ok  no assignment at all still covers all three")

    # Four ports of one adapter are one rule, and one warning, not four.
    if len(hide.targets(CUBE, [])) != 1:
        fail(f"one adapter produced {len(hide.targets(CUBE, []))} rules")
    print("  ok  four ports of one adapter dedupe to one")


def check_targets_union_and_skips() -> None:
    print("\na pad that is assigned but missing from the scan:")
    # A pad can be assigned and absent from discovery -- claimed by the
    # daemon, or a scan that raced a replug. It still needs a rule.
    pairs = [(p.vid, p.pid) for p in hide.targets([STICK], [CUBE[0]])]
    if (0x0079, 0x1843) not in pairs:
        fail("an assigned pad missing from the scan got no rule, so the pad "
             "padmap is republishing right now stays visible to RetroArch")
    if pairs != [(0x0079, 0x1830), (0x0079, 0x1843)]:
        fail(f"the union came out as {pairs}")
    print("  ok  discovered and assigned are unioned, discovered first")

    print("\nand a pad with no vid/pid:")
    result = hide.targets([NAMELESS, STICK], [NAMELESS])
    if any(p.vid == 0 or p.pid == 0 for p in result):
        fail("a pad with no vid/pid was made a target; there is nothing safe "
             "to match on, and a rule matching 0000:0000 is a guess")
    if len(result) != 1:
        fail(f"the identifiable pad was lost alongside it ({result})")
    if hide.targets([NAMELESS], []) != []:
        fail("a pad with no ids produced a target on its own")
    print("  ok  skipped, and the pads beside it survive")


def check_generated_rules() -> None:
    print("\nthe rules text itself:")
    pads = hide.targets(ALL_HARDWARE + [BLUETOOTH], [])
    rules = hide.generate_rules(pads)
    lines = rule_lines(rules)

    # Two lines per adapter, not one: a USB match and a Bluetooth match.
    #
    # ATTRS{idVendor} resolves against a USB parent, and a pad arriving over
    # Bluetooth has none -- so for years that rule quietly covered nothing on
    # a wireless pad. A Switch Pro paired over Bluetooth was enumerated by
    # RetroArch right beside padmap's clone, both answering to 057e:2009, and
    # the game bound the wrong one. The second line matches the uhid path,
    # which carries the bus and the ids.
    if len(lines) != 8:
        fail(f"{len(lines)} rule lines for 4 adapters -- expected two each, a "
             f"USB match and a Bluetooth one. Either an adapter is unhidden, "
             f"one is listed twice, or a transport lost its rule:\n{rules}")
    usb = [ln for ln in lines if "ATTRS{idVendor}" in ln]
    bt = [ln for ln in lines if "uhid/0005:" in ln]
    if len(usb) != 4 or len(bt) != 4:
        fail(f"{len(usb)} USB rules and {len(bt)} Bluetooth rules for 4 "
             f"adapters; a pad hidden on one transport is still visible on "
             f"the other")
    for line in bt:
        if "/devices/virtual/input/" in line:
            fail(f"a Bluetooth rule could match padmap's own virtual pads, "
                 f"which would hide the very devices RetroArch is supposed "
                 f"to see: {line}")
    for line in lines:
        if 'SUBSYSTEM=="input"' not in line:
            fail(f"a rule does not match the input subsystem, so it applies "
                 f"to the wrong devices or to none: {line}")
        if 'ENV{ID_INPUT_JOYSTICK}=""' not in line:
            fail(f"a rule does not CLEAR ID_INPUT_JOYSTICK -- RetroArch's "
                 f"udev driver enumerates on exactly that property, so the "
                 f"pad stays visible: {line}")
    print(f"  ok  {len(lines)} lines, each SUBSYSTEM==\"input\" clearing "
          f"ID_INPUT_JOYSTICK")

    if 'ATTRS{idVendor}=="0079"' not in rules:
        fail("no rule matches vendor 0079 written as four hex digits; udev "
             "compares these as text, so the rule matches no device and the "
             "three MAYFLASH adapters stay visible to RetroArch")
    if 'ATTRS{idVendor}=="000d"' not in rules:
        fail("a vid was not zero-padded to four hex digits; udev compares "
             "these as text, so the rule would match nothing and the pad "
             "would stay visible to RetroArch")
    if 'ATTRS{idProduct}=="0f00"' not in rules:
        fail("a pid was not zero-padded to four hex digits")
    if covered_pairs(rules) != [(0x0079, 0x1830), (0x0079, 0x1879),
                                (0x0079, 0x1843), (0x000d, 0x0f00)]:
        fail(f"the wrong vid/pid pairs were emitted: {covered_pairs(rules)}")
    print("  ok  ids are four lower-case hex digits, in order")

    for expected in ("MAYFLASH Arcade Fightstick F300", "USB GamePad",
                     "MAYFLASH GameCube Controller Adapter", "8BitDo SN30"):
        if f"# {expected}" not in rules:
            fail(f"no comment names {expected!r}; someone reading this file "
                 f"to work out which controller went missing has nothing to "
                 f"go on")
    print("  ok  each rule is preceded by a comment naming the pad")

    for line in rules.splitlines():
        if line.strip() and not line.lstrip().startswith("#") \
                and not line.startswith("SUBSYSTEM=="):
            fail(f"a line udev cannot parse would make it reject the WHOLE "
                 f"file, un-hiding every adapter at once: {line!r}")
    if not rules.startswith("#") or not rules.endswith("\n"):
        fail("the rules file has no leading comment or no trailing newline")
    if "padmap" not in rules.splitlines()[0]:
        fail("the header does not say padmap generated this, so a stray "
             "rules file in /run has no explanation and no way back")
    print("  ok  every line is a comment or a rule, header names padmap")


def check_generate_rules_dedupes_and_skips() -> None:
    print("\ngenerating from the raw scan, four ports and an id-less pad:")
    rules = hide.generate_rules(CUBE + [NAMELESS])
    # One adapter, so one pair of rules: a USB match and a Bluetooth one. The
    # four ports still collapse to a single vid/pid -- that is what is being
    # checked here -- they simply produce two lines rather than one now.
    if len(rule_lines(rules)) != 2:
        fail(f"four ports of one adapter produced {len(rule_lines(rules))} "
             f"rule lines; expected exactly two, one per transport")
    if len(set(rule_lines(rules))) != 2:
        fail("the two rules for one adapter are identical, so a transport is "
             "covered twice and the other not at all")
    if covered_pairs(rules) != [(0x0079, 0x1843)]:
        fail(f"unexpected pairs: {covered_pairs(rules)}")
    if '"0000"' in rules:
        fail("a pad with no vid/pid got a rule matching 0000:0000, which is "
             "a guess at somebody else's hardware")
    if "skipped" not in rules or NAMELESS.name not in rules:
        fail("a pad that could not be hidden was dropped silently -- it will "
             "still appear in RetroArch and nothing will say why")
    print("  ok  one rule, and the id-less pad is named as skipped")


def check_unhidden_reads_the_installed_file() -> None:
    print("\nadapters the installed file does not cover:")
    with sandbox():
        # The file as it actually was: a Fightstick and a USB pad, written
        # before the GameCube adapter existed on this machine.
        hide.RUNTIME_RULES_PATH.parent.mkdir(parents=True, exist_ok=True)
        hide.RUNTIME_RULES_PATH.write_text(
            hide.generate_rules([STICK, USBPAD]))

        missing = hide.unhidden(ALL_HARDWARE)
        if [(p.vid, p.pid) for p in missing] != [(0x0079, 0x1843)]:
            fail(f"reported {[hex(p.pid) for p in missing]} as unhidden; the "
                 f"adapter added later is the one RetroArch still sees")
        if len(missing) != 1:
            fail(f"one adapter with four ports was reported {len(missing)} "
                 f"times, so the warning reads like four extra controllers")
        print("  ok  the pad added later is flagged, once")

        if hide.unhidden([STICK, USBPAD]):
            fail("a pad the installed rules already cover was reported, so "
                 "the warning cries wolf and gets ignored")
        if hide.unhidden([NAMELESS]):
            fail("a pad with no vid/pid was reported as unhidden -- there is "
                 "no rule that could ever cover it, so the warning would "
                 "never go away")
        print("  ok  covered pads and id-less pads are not reported")

        # It must read the file, not remember what padmap wrote. Someone
        # editing /run/udev/rules.d by hand, or a stale file surviving a
        # rebuild, is exactly the drift this exists to catch.
        hide.install(hide.generate_rules(ALL_HARDWARE))
        if not hide.RUNTIME_RULES_PATH.exists() or hide.RULES_PATH.exists():
            fail(f"install did not write {hide.RUNTIME_RULES_PATH}; that is "
                 f"the tmpfs udev reads and the only copy a reboot clears")
        if hide.unhidden(ALL_HARDWARE):
            fail("a pad is still reported unhidden right after installing "
                 "rules for it -- the generator and the checker disagree")
        hide.RUNTIME_RULES_PATH.write_text(hide.generate_rules([STICK]))
        missing = hide.unhidden(ALL_HARDWARE)
        if [(p.vid, p.pid) for p in missing] != [(0x0079, 0x1879),
                                                 (0x0079, 0x1843)]:
            fail(f"the file was edited behind padmap's back and unhidden "
                 f"reported {[hex(p.pid) for p in missing]} -- it is "
                 f"answering from memory, not from what udev applies")
        print("  ok  an edit behind padmap's back is seen")

        # Rules written by hand or by the NixOS module may use upper-case
        # hex; udev does not care, and neither may this.
        hide.RUNTIME_RULES_PATH.write_text(
            'SUBSYSTEM=="input", ATTRS{idVendor}=="0079", '
            'ATTRS{idProduct}=="184A", ENV{ID_INPUT_JOYSTICK}=""\n'
            'SUBSYSTEM=="input", ATTRS{idVendor}=="007D", '
            'ATTRS{idProduct}=="18BC", ENV{ID_INPUT_JOYSTICK}=""\n')
        upper_a = pad("Hand-written A", 0x0079, 0x184a, "event50")
        upper_b = pad("Hand-written B", 0x007d, 0x18bc, "event51")
        missing = hide.unhidden([upper_a, upper_b])
        if missing:
            fail(f"upper-case hex in a hand-written rule was not recognised, "
                 f"so padmap would nag about {missing[0].name} forever")
        print("  ok  upper-case hex ids count as covered")


def check_unhidden_file_locations() -> None:
    print("\nwhere the installed rules are looked for:")
    with sandbox():
        hide.RULES_PATH.parent.mkdir(parents=True, exist_ok=True)
        hide.RULES_PATH.write_text(hide.generate_rules(ALL_HARDWARE))
        if hide.unhidden(ALL_HARDWARE):
            fail(f"rules permanently installed in {hide.RULES_PATH} were "
                 f"ignored, so padmap nags about pads that are hidden")
        print("  ok  a permanent install in /etc is found")

        # /run wins: it is what a `sudo padmap hide` just wrote, and udev
        # reads it too. If /etc is consulted first, a fresh install that
        # covers everything is judged against a stale permanent file.
        hide.RUNTIME_RULES_PATH.parent.mkdir(parents=True, exist_ok=True)
        hide.RUNTIME_RULES_PATH.write_text(hide.generate_rules([STICK]))
        missing = hide.unhidden(ALL_HARDWARE)
        if len(missing) != 2:
            fail(f"with both files installed, {len(missing)} pads reported; "
                 f"the runtime file is what was installed last and must win")
        print("  ok  /run/udev/rules.d takes precedence over /etc")


def check_unhidden_survives_a_bad_file() -> None:
    print("\nno rules file, and files that make no sense:")
    with sandbox():
        if hide.unhidden(ALL_HARDWARE) != []:
            fail("with no rules installed at all, pads were reported as "
                 "unhidden -- hiding is optional, and every start-up would "
                 "print a warning about a step the user chose not to take")
        print("  ok  nothing installed means nothing to report")

        # Unreadable: a directory in its place, or a file with no permissions.
        hide.RUNTIME_RULES_PATH.mkdir(parents=True)
        if hide.unhidden(ALL_HARDWARE) != []:
            fail("an unreadable rules path was not tolerated; a warning is "
                 "not worth breaking start-up over")
        hide.RUNTIME_RULES_PATH.rmdir()
        print("  ok  an unreadable path is survivable")

        # Malformed: truncated ids, half a rule, prose. These cover nothing,
        # so everything is reported -- loudly wrong beats quietly covered.
        hide.RUNTIME_RULES_PATH.write_text(
            "this is not a udev rule\n"
            'SUBSYSTEM=="input", ATTRS{idVendor}=="79", '
            'ATTRS{idProduct}=="1843"\n'
            'ATTRS{idVendor}=="0079"\n')
        missing = hide.unhidden(ALL_HARDWARE)
        if len(missing) != 3:
            fail(f"a malformed rules file was read as covering "
                 f"{3 - len(missing)} pad(s); a truncated id matches no "
                 f"device, so claiming it is hidden hides the real problem")
        print("  ok  a malformed file covers nothing and does not crash")

        # An empty file is the same story and must not raise either.
        hide.RUNTIME_RULES_PATH.write_text("")
        if len(hide.unhidden(ALL_HARDWARE)) != 3:
            fail("an empty rules file was treated as covering something")
        print("  ok  an empty file covers nothing")
        # Known gap, deliberately not asserted: a rules file that is not
        # valid UTF-8 raises UnicodeDecodeError out of unhidden, which would
        # abort `padmap ensure-daemon` (cli.py calls it on every start).


def check_install_writes_and_reloads() -> None:
    print("\ninstalling the rules:")
    with sandbox() as (root, udevadm):
        rules = hide.generate_rules(hide.targets(ALL_HARDWARE, []))
        changed, messages = hide.install(rules)

        if not changed:
            fail(f"install reported no change on a first install ({messages})")
        if not hide.RUNTIME_RULES_PATH.exists():
            fail("install created no file, and its parent directory is not "
                 "there on a fresh boot -- /run/udev/rules.d is tmpfs")
        if hide.RUNTIME_RULES_PATH.read_text() != rules:
            fail("the installed file does not hold exactly the rules that "
                 "were generated")
        if hide.RULES_PATH.exists():
            fail(f"install wrote to {hide.RULES_PATH}; on NixOS that is a "
                 f"store symlink, and anywhere else it survives a reboot the "
                 f"user cannot undo by power-cycling")
        print("  ok  written to /run/udev/rules.d, /etc untouched")

        if ("control", "--reload-rules") not in udevadm.calls:
            fail(f"rules were written without `udevadm control "
                 f"--reload-rules` ({udevadm.calls}) -- udev keeps its rules "
                 f"in memory, so nothing changes until reboot and the pads "
                 f"stay visible after padmap says it hid them")
        if ("trigger", "--subsystem-match=input") not in udevadm.calls:
            fail(f"no `udevadm trigger --subsystem-match=input` "
                 f"({udevadm.calls}) -- reloading only affects devices added "
                 f"afterwards, so already-plugged pads stay visible until "
                 f"they are unplugged and back in")
        if udevadm.calls != [("control", "--reload-rules"),
                             ("trigger", "--subsystem-match=input")]:
            fail(f"udevadm was run as {udevadm.calls}; the reload must come "
                 f"before the trigger or the trigger re-applies the old rules")
        print("  ok  reloaded and triggered, in that order")

        if not any("wrote" in m for m in messages):
            fail(f"install said nothing about having written a file "
                 f"({messages})")
        if any("could not" in m or "failed" in m for m in messages):
            fail(f"a successful install reported a problem ({messages}); "
                 f"`padmap hide` keys its exit status off those words")
        print(f"  ok  reports: {'; '.join(messages)}")


def check_install_is_honest_about_doing_nothing() -> None:
    print("\ninstalling the same rules twice:")
    with sandbox() as (root, udevadm):
        rules = hide.generate_rules(hide.targets(ALL_HARDWARE, []))
        hide.install(rules)
        before = len(udevadm.calls)

        again, messages = hide.install(rules)
        if again:
            fail(f"reinstalling identical rules claimed to have changed "
                 f"something ({messages}); `padmap hide` then prints the "
                 f"'your controllers are invisible' warning every run")
        if not any("up to date" in m for m in messages):
            fail(f"no message says the file was already current ({messages})")
        if len(udevadm.calls) != before:
            fail("udev was reloaded for a file that did not change")
        print(f"  ok  no change, no reload, says {messages[0]!r}")

        # A stale file with the same pads in it is still a change, and must
        # be rewritten AND reloaded -- this is the path a version bump or a
        # hand edit takes.
        hide.RUNTIME_RULES_PATH.write_text("# something else entirely\n")
        changed, messages = hide.install(rules)
        if not changed:
            fail(f"a file whose contents differ was left alone ({messages})")
        if hide.RUNTIME_RULES_PATH.read_text() != rules:
            fail("a stale rules file was not overwritten")
        if ("control", "--reload-rules") not in udevadm.calls[before:]:
            fail("rules were rewritten without reloading udev")
        print("  ok  a differing file is rewritten and reloaded")

        # And new hardware is a change too, not "already up to date".
        more = hide.generate_rules(hide.targets(ALL_HARDWARE + [BLUETOOTH], []))
        changed, _ = hide.install(more)
        if not changed or hide.RUNTIME_RULES_PATH.read_text() != more:
            fail("adding a controller did not update the installed rules")
        print("  ok  new hardware updates the file")


def check_install_reports_failures() -> None:
    print("\nwhen the file cannot be written:")
    with sandbox() as (root, udevadm):
        # Root is required for /run/udev/rules.d; without it the write
        # fails. It must be reported, not raised: `padmap hide` prints the
        # messages and turns them into an exit status.
        blocker = root / "run"
        blocker.parent.mkdir(parents=True, exist_ok=True)
        blocker.write_text("not a directory")
        try:
            changed, messages = hide.install("# rules\n")
        except OSError as error:
            fail(f"install raised {error!r} instead of reporting it; "
                 f"`padmap hide` would traceback at the user")
        if changed:
            fail(f"a failed write reported success ({messages}), so padmap "
                 f"would claim the pads are hidden when they are not")
        if not any("could not write" in m for m in messages):
            fail(f"the failure was not described ({messages}); cli.cmd_hide "
                 f"looks for 'could not' to decide its exit status")
        if udevadm.calls:
            fail("udev was reloaded after a write that failed, which reads "
                 "like the install worked")
        print(f"  ok  reported, not raised: {messages[0][:60]}...")

    print("\nwhen udevadm itself fails:")
    with sandbox("udevadm control --reload-rules failed: Permission denied") \
            as (root, udevadm):
        rules = hide.generate_rules([STICK])
        changed, messages = hide.install(rules)
        if not changed:
            fail("the file was written, so the write must be reported")
        if not any("failed" in m for m in messages):
            fail(f"a udevadm failure was swallowed ({messages}) -- the file "
                 f"is on disk but udev never read it, so nothing is hidden "
                 f"and padmap says everything is fine. cli.cmd_hide keys its "
                 f"non-zero exit off the word 'failed'")
        if not any("Permission denied" in m for m in messages):
            fail(f"udevadm's own explanation was thrown away ({messages})")
        print(f"  ok  surfaced: {[m for m in messages if 'failed' in m][0]}")


def check_hint_and_snippet() -> None:
    print("\nthe printed instructions, for a machine without root here:")
    with sandbox():
        pads = hide.targets(ALL_HARDWARE + [NAMELESS], [])
        rules = hide.generate_rules(pads)
        saved = hide.is_nixos
        try:
            hide.is_nixos = lambda: False   # type: ignore[assignment]
            hint = hide.install_hint(rules, pads)
            for line in rule_lines(rules):
                if line not in hint:
                    fail(f"the pasteable script omits a rule, so following "
                         f"it leaves a pad visible: {line}")
            if str(hide.RUNTIME_RULES_PATH) not in hint:
                fail("the instructions never say where to write the rules")
            if "udevadm control --reload-rules" not in hint:
                fail("the instructions skip the reload, so following them "
                     "exactly changes nothing until reboot")
            if "trigger --subsystem-match=input" not in hint:
                fail("the instructions skip the trigger, so pads already "
                     "plugged in stay visible")
            if f"rm {hide.RUNTIME_RULES_PATH}" not in hint:
                fail("no way back is given -- with these rules installed and "
                     "padmap not running there are no controllers at all")
            if "invisible to RetroArch" not in hint:
                fail("the hint does not warn that these pads vanish entirely "
                     "when padmap is not running")
            if str(hide.RULES_PATH) not in hint:
                fail("no route to a permanent install is offered")
            print("  ok  rules, path, reload, trigger, undo and the caution")

            hide.is_nixos = lambda: True    # type: ignore[assignment]
            nixhint = hide.install_hint(rules, pads)
            if str(hide.RULES_PATH) in nixhint:
                fail("on NixOS the hint tells the user to write /etc/udev/"
                     "rules.d, which is a store symlink and cannot be written")
            if "programs.padmap" not in nixhint or "hideDevices" not in nixhint:
                fail("NixOS gets no declarative route, so the rules are lost "
                     "on the next reboot with no way to make them stick")
            for expected in ("0079:1830", "0079:1879", "0079:1843"):
                if expected not in nixhint:
                    fail(f"{expected} is missing from the NixOS snippet, so "
                         f"that adapter comes back after a rebuild")
            print("  ok  NixOS gets the module snippet instead")
        finally:
            hide.is_nixos = saved           # type: ignore[assignment]

    print("\nthe NixOS module snippet on its own:")
    snippet = hide.nix_module_snippet(CUBE + [STICK, NAMELESS])
    if snippet.count("0079:1843") != 1:
        fail(f"one adapter is listed {snippet.count('0079:1843')} times in "
             f"hideDevices")
    if "0079:1830" not in snippet:
        fail("an adapter is missing from hideDevices")
    if "0000:0000" in snippet:
        fail("a pad with no vid/pid was written into hideDevices, matching "
             "anything that reports no ids")
    if "enable = true" not in snippet or "hideDevices = [" not in snippet:
        fail(f"the snippet is not a usable NixOS module fragment:\n{snippet}")
    if hide.nix_module_snippet([]).count('"') != 0:
        fail("with no pads the snippet still lists a device")
    print("  ok  deduped, id-less pad skipped, valid fragment")


def check_the_reported_bug_end_to_end() -> None:
    print("\nthe whole chain, as the bug was reported:")
    with sandbox() as (root, udevadm):
        # What padmap used to do: rules from the current assignment.
        assigned_only = hide.generate_rules([STICK])
        hide.install(assigned_only)
        missing = hide.unhidden(ALL_HARDWARE)
        if len(missing) != 2:
            fail(f"rules built from the assignment left {len(missing)} pads "
                 f"unflagged; this check cannot prove the fix if the old bug "
                 f"is not visible to it")
        print(f"  ok  assignment-derived rules leave "
              f"{', '.join(f'{p.vid:04x}:{p.pid:04x}' for p in missing)} "
              f"visible, and unhidden says so")

        # What it does now: rules from the hardware.
        pads = hide.targets(ALL_HARDWARE, [STICK])
        changed, messages = hide.install(hide.generate_rules(pads))
        if not changed:
            fail(f"the corrected rules were not installed ({messages})")
        if hide.unhidden(ALL_HARDWARE):
            fail("a pad padmap republishes is still visible to RetroArch "
                 "after a full hide -- its ports will take player slots")
        if hide.unhidden(ALL_HARDWARE + [CUBE[1], CUBE[2]]):
            fail("the extra ports of a covered adapter were reported")
        if ("control", "--reload-rules") not in udevadm.calls:
            fail("udev was never reloaded across the whole chain")
        print("  ok  hardware-derived rules cover everything, udev reloaded")

        # And a controller bought tomorrow is flagged rather than ignored.
        newcomer = pad("8BitDo Pro 2", 0x2dc8, 0x6003, "event40")
        later = hide.unhidden(ALL_HARDWARE + [newcomer])
        if [(p.vid, p.pid) for p in later] != [(0x2dc8, 0x6003)]:
            fail(f"a controller connected after the rules were written was "
                 f"not flagged ({later}); that is the drift that produced "
                 f"four phantom RetroArch pads and went unnoticed")
        print("  ok  a pad added later is reported, not silently visible")


def main() -> int:
    check_targets_are_the_hardware()
    check_targets_union_and_skips()
    check_generated_rules()
    check_generate_rules_dedupes_and_skips()
    check_unhidden_reads_the_installed_file()
    check_unhidden_file_locations()
    check_unhidden_survives_a_bad_file()
    check_install_writes_and_reloads()
    check_install_is_honest_about_doing_nothing()
    check_install_reports_failures()
    check_hint_and_snippet()
    check_the_reported_bug_end_to_end()

    # Nothing above may have left the real paths redirected.
    if not str(hide.RUNTIME_RULES_PATH).startswith("/run/udev"):
        fail(f"hide.RUNTIME_RULES_PATH was left as {hide.RUNTIME_RULES_PATH}")
    if not str(hide.RULES_PATH).startswith("/etc/udev"):
        fail(f"hide.RULES_PATH was left as {hide.RULES_PATH}")

    print("\na rules file that is not valid UTF-8 does not break startup:")
    # Found by this file's first run: unhidden read the file with read_text(),
    # which raises UnicodeDecodeError -- not an OSError, so not caught.
    # `padmap ensure-daemon` calls unhidden() unconditionally on every start,
    # so a corrupt or binary 99-padmap.rules would traceback the whole start
    # path rather than being reported as covering nothing.
    with tempfile.TemporaryDirectory() as tmp:
        rules = Path(tmp) / "99-padmap.rules"
        rules.write_bytes(b"\xff\xfe\x00binary garbage\x80\x81")
        original = hide.RUNTIME_RULES_PATH
        hide.RUNTIME_RULES_PATH = rules
        try:
            sample = [pad("GameCube Adapter", 0x0079, 0x1843)]
            try:
                missing = hide.unhidden(sample)
            except Exception as error:                  # noqa: BLE001
                raise SystemExit(
                    f"FAIL: a non-UTF-8 rules file raised "
                    f"{type(error).__name__} -- ensure-daemon calls this on "
                    f"every start, so padmap would fail to start at all")
            if [p.pid for p in missing] != [0x1843]:
                raise SystemExit(
                    "FAIL: an unreadable rules file must be treated as "
                    "covering nothing, so the adapter is still reported")
        finally:
            hide.RUNTIME_RULES_PATH = original
    print("  ok  reported as covering nothing, and startup survives")

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
