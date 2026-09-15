#!/usr/bin/env python3
"""Files padmap reads but did not write, and the start-up it must not stop.

One failure, found repeatedly, always the same shape: a command dies
completely over something small and local to it.

  * **Not-UTF-8 text.** `Path.read_text()` raises `UnicodeDecodeError` on a
    stray byte, and that is a `ValueError`, not an `OSError` -- so every
    `except OSError` guard in this codebase missed it. It has been found in
    five places (`hide.unhidden`, the profile store, `Server._load_prompted`,
    `hide.install`, `cli._forget_prompted`), which is why there is now one
    helper, `padmap.safeio.read_text`, rather than five separate fixes. The
    user-visible cost was real: `sudo padmap hide` tracebacked instead of
    overwriting the corrupt rules file, and `padmap forget` -- the command
    you run *because* something is already broken -- tracebacked too.

And the answer `ensure-daemon` exists to give, which the wrappers run as
    padmap ensure-daemon || echo continuing without a current daemon
so a wrong answer there is a session with no daemon, no virtual pads and no
controllers at all.

Nothing here touches the real system: XDG_RUNTIME_DIR, XDG_CONFIG_HOME and
XDG_DATA_HOME are redirected before padmap is imported, both udev rules paths
and udevadm are replaced, `devices.discover` returns nothing, and the daemon
lookup is stubbed -- no real pad is opened, no real daemon is spoken to, and
no rule is written to /run.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import os
import re
import sys
import tempfile
from pathlib import Path

# Before importing padmap: nothing here may reach real user state, and
# nothing may reach the daemon that owns the controllers on this machine.
_SANDBOX = tempfile.mkdtemp(prefix="check-safe-reads-")
os.environ["XDG_RUNTIME_DIR"] = _SANDBOX
os.environ["XDG_CONFIG_HOME"] = _SANDBOX
os.environ["XDG_DATA_HOME"] = _SANDBOX

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import cli, devices, hide, protocol, safeio  # noqa: E402
from padmap.devices import Pad  # noqa: E402

# One stray byte is all it takes; 0xff cannot start a UTF-8 sequence.
BAD_BYTE = b"\xff"


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def pad(name: str, vid: int, pid: int, node: str = "event9") -> Pad:
    return Pad(path=f"/dev/input/{node}", name=name, phys=f"usb-{node}",
               uniq="", vid=vid, pid=pid, syspath=f"/sys/class/input/{node}")


STICK = pad("MAYFLASH Arcade Fightstick F300", 0x0079, 0x1830, "event20")
CUBE = pad("MAYFLASH GameCube Controller Adapter", 0x0079, 0x1843, "event22")


@contextlib.contextmanager
def rules_sandbox():
    """Redirect both rules paths and udevadm into a temp directory."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        saved = (hide.RUNTIME_RULES_PATH, hide.RULES_PATH, hide._udevadm)
        hide.RUNTIME_RULES_PATH = root / "run/udev/rules.d/99-padmap.rules"
        hide.RULES_PATH = root / "etc/udev/rules.d/99-padmap.rules"
        hide._udevadm = lambda *args: ""     # type: ignore[assignment]
        for path in (hide.RUNTIME_RULES_PATH, hide.RULES_PATH):
            # A rule written to the real /run would hide this machine's pads
            # from RetroArch. If a refactor ever un-redirects these, stop.
            if not str(path).startswith(str(root)):
                fail(f"the test failed to redirect {path} out of the way")
        try:
            yield root
        finally:
            hide.RUNTIME_RULES_PATH, hide.RULES_PATH = saved[0], saved[1]
            hide._udevadm = saved[2]         # type: ignore[assignment]


# --------------------------------------------------------------------------
# The helper itself.


def check_safeio_reads_what_it_can() -> None:
    print("\nreading a file padmap does not own:")
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)

        good = root / "good.txt"
        good.write_text("0079:1830\n0079:1843\n")
        if safeio.read_text(good) != "0079:1830\n0079:1843\n":
            fail("a perfectly good file did not read back unchanged")
        print("  ok  an ordinary file reads back unchanged")

        damaged = root / "damaged.txt"
        damaged.write_bytes(b"0079:1830\n" + BAD_BYTE + b"\n0079:1843\n")
        try:
            text = safeio.read_text(damaged)
        except Exception as error:
            fail(f"the helper written to close this hole raised "
                 f"{type(error).__name__} on one undecodable byte, which is "
                 f"the hole: {error}")
        if text is None:
            fail("a file with one bad byte read as unreadable")
        if "0079:1830" not in text or "0079:1843" not in text:
            fail("one undecodable byte cost the whole file; the good lines "
                 "around it are what padmap was reading the file for")
        print("  ok  a damaged byte costs a character, not the file")

        missing = root / "nothing-here.txt"
        if safeio.read_text(missing) != "":
            fail("a missing file did not read as empty")
        if safeio.read_text(missing, default=None) is not None:
            fail("default=None did not report a missing file as missing; "
                 "hide.unhidden needs to tell 'no rules installed' from "
                 "'rules that cover nothing'")
        print("  ok  a missing file is empty, or None when asked")

        # A directory, and a file with no read permission: both OSError, and
        # both must answer with the default rather than raise.
        (root / "adir").mkdir()
        if safeio.read_text(root / "adir", default=None) is not None:
            fail("a directory in place of a file was not reported unreadable")
        locked = root / "locked.txt"
        locked.write_text("secret\n")
        locked.chmod(0o000)
        try:
            if os.geteuid() != 0:
                if safeio.read_text(locked, default=None) is not None:
                    fail("a file that cannot be opened was not reported "
                         "unreadable")
        finally:
            locked.chmod(0o600)
        print("  ok  a directory or an unreadable file answers with the "
              "default")

        empty = root / "empty.txt"
        empty.write_text("")
        if safeio.read_text(empty, default=None) != "":
            fail("an empty file read as absent; a rules file that exists but "
                 "is empty covers no controllers, which is not the same as "
                 "no rules being installed at all")
        print("  ok  an empty file is empty, not absent")


def check_hide_has_one_way_of_reading() -> None:
    print("\nhow hide.py reads the installed rules:")
    source = (Path(__file__).resolve().parent.parent
              / "src/padmap/hide.py").read_text()
    direct = re.compile(r"(?<!safeio)\.read_(?:text|bytes)\(")
    offenders = [line.strip() for line in source.splitlines()
                 if direct.search(line.split("#")[0])]
    if offenders:
        fail(f"hide.py reads a file directly again ({offenders[0]!r}); this "
             f"decode hole shipped five times because each site guarded it "
             f"its own way, so the rules file is read through padmap.safeio "
             f"and nowhere else")
    print("  ok  the rules file is read through safeio, once")


# --------------------------------------------------------------------------
# BUG 37: sudo padmap hide, on a rules file that is not UTF-8.


def check_hide_install_overwrites_a_corrupt_file() -> None:
    print("\n`sudo padmap hide` over a rules file that is not UTF-8:")
    with rules_sandbox():
        hide.RUNTIME_RULES_PATH.parent.mkdir(parents=True, exist_ok=True)
        # printf '\xff' | sudo tee /run/udev/rules.d/99-padmap.rules
        hide.RUNTIME_RULES_PATH.write_bytes(BAD_BYTE + b" not utf-8 at all\n")

        rules = hide.generate_rules([STICK, CUBE])
        try:
            changed, messages = hide.install(rules)
        except Exception as error:
            fail(f"`sudo padmap hide` raised {type(error).__name__} on a "
                 f"rules file that is not UTF-8, so the one command that "
                 f"would have replaced the corrupt file is the command that "
                 f"cannot run: {error}")
        if not changed:
            fail("installing over a corrupt rules file reported no change, "
                 "so padmap says the pads are hidden while udev is applying "
                 "whatever that file used to be")
        if hide.RUNTIME_RULES_PATH.read_text() != rules:
            fail(f"the corrupt file was not replaced by the generated rules "
                 f"({messages}); the physical adapters stay visible and "
                 f"RetroArch keeps counting them as extra controllers")
        print("  ok  a corrupt rules file is replaced, not tracebacked over")

        again, _ = hide.install(rules)
        if again:
            fail("reinstalling identical rules over the repaired file "
                 "claimed to have changed something")
        print("  ok  and the repaired file is then recognised as current")


def check_unhidden_survives_a_file_that_is_not_utf8() -> None:
    print("\nthe start-up warning, over a rules file that is not UTF-8:")
    with rules_sandbox():
        hide.RUNTIME_RULES_PATH.parent.mkdir(parents=True, exist_ok=True)
        hide.RUNTIME_RULES_PATH.write_bytes(BAD_BYTE)
        try:
            missing = hide.unhidden([STICK, CUBE])
        except Exception as error:
            fail(f"unhidden raised {type(error).__name__} on a rules file "
                 f"that is not UTF-8; ensure-daemon calls it on every start, "
                 f"so the front-end would start with no daemon at all: "
                 f"{error}")
        if [(p.vid, p.pid) for p in missing] != [(0x0079, 0x1830),
                                                 (0x0079, 0x1843)]:
            fail(f"a rules file that cannot be parsed was read as covering "
                 f"{2 - len(missing)} adapter(s); a file udev cannot apply "
                 f"hides nothing, and saying otherwise buries the problem")
        print("  ok  an undecodable rules file covers nothing and warns")

        # A rule that is intact apart from one bad byte elsewhere still
        # counts: replacement decoding is what keeps the good lines.
        hide.RUNTIME_RULES_PATH.write_bytes(
            b"# " + BAD_BYTE + b"\n" + hide.generate_rules([STICK]).encode())
        missing = hide.unhidden([STICK, CUBE])
        if [(p.vid, p.pid) for p in missing] != [(0x0079, 0x1843)]:
            fail("one bad byte in a comment threw away the rules below it, "
                 "so padmap warns about a controller that really is hidden")
        print("  ok  a bad byte costs its own line, not the rules below it")


# --------------------------------------------------------------------------
# BUG 38: padmap forget, on a `prompted` file that is not UTF-8.


@contextlib.contextmanager
def prompted(content: bytes):
    """Put exact bytes in the runtime `prompted` file for the test."""
    path = protocol.prompted_path()
    if not str(path).startswith(_SANDBOX):
        fail(f"the prompted file resolved to {path}, outside the sandbox; "
             f"this test must never touch the running daemon's state")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    try:
        yield path
    finally:
        path.unlink(missing_ok=True)


def check_forget_survives_a_prompted_file_that_is_not_utf8() -> None:
    print("\n`padmap forget`, over a `prompted` file that is not UTF-8:")
    # printf '\xff' > $XDG_RUNTIME_DIR/padmap/prompted
    with prompted(BAD_BYTE) as path:
        try:
            cleared = cli._forget_prompted(None)
        except Exception as error:
            fail(f"`padmap forget` raised {type(error).__name__} on a "
                 f"`prompted` file that is not UTF-8. That file lives in "
                 f"XDG_RUNTIME_DIR where anything may write it, and forget "
                 f"is the command run to recover: {error}")
        if path.exists():
            fail("`padmap forget --all` left the damaged `prompted` file in "
                 "place, so the controllers it names are still never offered "
                 "setup and the user has no way to clear them")
        if cleared < 1:
            fail(f"forget reported clearing {cleared} record(s) while "
                 f"removing the file; saying 'nothing to forget' after "
                 f"deleting something sends the user away misinformed")
        print("  ok  a damaged `prompted` file is cleared and counted")

    # One bad byte among real signatures: the good ones must still be
    # matchable, or forgetting a named controller silently does nothing.
    body = b"usb-0079:1830\n" + BAD_BYTE + b"\nusb-0079:1843\n"
    with prompted(body) as path:
        cleared = cli._forget_prompted({"usb-0079:1830"})
        if cleared != 1:
            fail(f"forgetting one controller out of a damaged file cleared "
                 f"{cleared} record(s); the undamaged lines must still be "
                 f"matchable by signature")
        left = path.read_bytes()
        if b"usb-0079:1830" in left or b"usb-0079:1843" not in left:
            fail("forget removed the wrong records from a damaged file: the "
                 "named controller is still suppressed and another one is "
                 "now offered setup it never asked for")
        print("  ok  the good lines around it are still matched, and kept")

    with prompted(b"") as path:
        if cli._forget_prompted(None) != 0:
            fail("forget claimed to have cleared records from an empty file")
        print("  ok  an empty file clears nothing")


# --------------------------------------------------------------------------
# ensure-daemon: the answer it exists to give.


@contextlib.contextmanager
def ensure_daemon_world(state: dict | None):
    """cmd_ensure_daemon with no hardware, no daemon and no real rules.

    A live daemon owns the controllers on this machine. `_daemon_state` is
    replaced so nothing is ever sent to it, and `_spawn_daemon`/`_stop_daemon`
    raise, so a test that accidentally takes the restart path says so instead
    of starting a second daemon on top of the real one.
    """
    def never(*args, **kwargs):
        fail("the test reached the daemon start/stop path, which must never "
             "run against the daemon that owns this machine's controllers")

    saved = (cli._daemon_state, cli._spawn_daemon, cli._stop_daemon,
             devices.discover)
    cli._daemon_state = lambda: state           # type: ignore[assignment]
    cli._spawn_daemon = never                   # type: ignore[assignment]
    cli._stop_daemon = never                    # type: ignore[assignment]
    devices.discover = lambda *a, **k: []       # type: ignore[assignment]
    with rules_sandbox():
        try:
            yield
        finally:
            (cli._daemon_state, cli._spawn_daemon,     # type: ignore[assignment]
             cli._stop_daemon, devices.discover) = saved


def run_ensure_daemon() -> tuple[int, str]:
    """cmd_ensure_daemon --check, with its output captured."""
    args = argparse.Namespace(check=True, timeout=1.0)
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = cli.cmd_ensure_daemon(args)
    return code, out.getvalue()


def check_ensure_daemon_still_reports_a_stale_daemon() -> None:
    print("\n`padmap ensure-daemon --check`, on a daemon it must not trust:")
    with ensure_daemon_world({"build": "an-older-build"}):
        code, output = run_ensure_daemon()
        if code == 0 or "older code" not in output:
            fail(f"a daemon running older code was reported as current "
                 f"({code}); that is the whole point of the command, and it "
                 f"is how a fixed controller-port bug went on reproducing: "
                 f"{output!r}")
        print("  ok  a stale daemon is still reported")

    with ensure_daemon_world(None):
        code, output = run_ensure_daemon()
        if code == 0 or "no daemon running" not in output:
            fail(f"with no daemon running, --check reported success: "
                 f"{output!r}")
        print("  ok  and no daemon at all is still reported")


def main() -> None:
    print("files padmap does not own, and the start-up they must not stop")
    check_safeio_reads_what_it_can()
    check_hide_install_overwrites_a_corrupt_file()
    check_unhidden_survives_a_file_that_is_not_utf8()
    # After the behaviour, not before it: the source check is a guard against
    # the sixth instance, and a failure there must not mask what actually
    # breaks for the user.
    check_hide_has_one_way_of_reading()
    check_forget_survives_a_prompted_file_that_is_not_utf8()
    check_ensure_daemon_still_reports_a_stale_daemon()
    print("\nall checks passed")


if __name__ == "__main__":
    main()
