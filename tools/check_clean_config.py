#!/usr/bin/env python3
"""S21: `padmap clean-config` rewriting the user's own retroarch.cfg.

`retroarch.clean_user_config` is the only code in padmap that writes to
~/.config/retroarch/retroarch.cfg. Everything else padmap generates lives in
files padmap owns, or is handed to RetroArch for a single launch through
`--appendconfig`. This one function edits a 3000-line file the user did not
write by hand but which is nonetheless theirs, and it exists because
`config_save_on_exit` used to persist padmap's launch override into it:

  * a stale `input_player3_joypad_index` equal to an assigned player's index,
    so one controller drove two ports -- what "four players in an N64 game"
    actually was;
  * padmap-named `input_playerN_reserved_device` reservations holding slots
    for virtual pads that are no longer republished;
  * per-player `_btn`/`_axis` binds, which outrank the autoconfig profile
    (`joykey = (bind_joykey != NO_BTN) ? bind_joykey : autobind_joykey;`), so
    a pad reports as "configured in port 1" while its buttons do nothing.

The promise is therefore not about a return value, it is about the bytes
RetroArch reads next time. So every scenario below asserts on the rewritten
*text*: which lines changed, and -- at least as important -- that every other
byte of the user's file survived. A cleaner that is too eager damages the one
file padmap admits is not its own, and the backup is the only undo.

Scenarios cover the exact key families it rewrites and the near-miss keys it
must not, idempotency, the backup (including when one already exists),
--dry-run, and the shapes a real config comes in: CRLF, no trailing newline,
duplicate keys, odd spacing around `=`, comments, values containing `=` or a
quote, plus a file that is empty, not valid UTF-8, read-only, or a directory.

Nothing here touches real user state or hardware: XDG_RUNTIME_DIR,
XDG_CONFIG_HOME, XDG_DATA_HOME and PADMAP_PROFILE_DIR are redirected into a
temp tree before padmap is imported, `devices.discover` and `server.Assigner`
are replaced so no command can reach the live daemon's controllers, no device
is opened and no uinput node is created. Only `clean_user_config` and
`cli.cmd_clean_config` are exercised, and only against files in that tree.

Lines marked "gap:" are defects this file found and reports rather than fixes;
each names its reproduction. Everything else is asserted.
"""

from __future__ import annotations

import contextlib
import io
import os
import shutil
import sys
import tempfile
from pathlib import Path

# Before importing padmap: retroarch.CONFIG_DIR and several other modules read
# the environment at import time, and nothing here should be able to reach
# real user state even by accident.
_SANDBOX = Path(tempfile.mkdtemp(prefix="check-clean-config-"))
os.environ["XDG_RUNTIME_DIR"] = str(_SANDBOX / "run")
os.environ["XDG_CONFIG_HOME"] = str(_SANDBOX / "config")
os.environ["XDG_DATA_HOME"] = str(_SANDBOX / "data")
os.environ["PADMAP_PROFILE_DIR"] = str(_SANDBOX / "devices")
os.environ["RETROARCH_CONFIG_DIR"] = str(_SANDBOX / "retroarch")
for _sub in ("run/padmap", "config", "data", "devices", "retroarch", "cfg"):
    (_SANDBOX / _sub).mkdir(parents=True, exist_ok=True)

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from padmap import cli, devices, retroarch, server  # noqa: E402

# A live daemon owns the controllers on this machine. Nothing below dispatches
# a command that would reach them, but neuter the two entry points anyway so a
# future edit here cannot start a real session.
devices.discover = lambda *a, **k: []           # type: ignore[assignment]
server.Assigner = None                          # type: ignore[assignment]


# -- reporting ---------------------------------------------------------------

CHECKS = 0


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def heading(text: str) -> None:
    print(f"\n{text}:")


def ok(text: str) -> None:
    global CHECKS
    CHECKS += 1
    print(f"  ok  {text}")


def gap(text: str) -> None:
    print(f"  gap: {text}")


def note(text: str) -> None:
    print(f"  --  {text}")


# -- fixtures ----------------------------------------------------------------

CFG = _SANDBOX / "cfg"

BACKUP_SUFFIX = ".padmap-backup"


def fresh(name: str = "retroarch.cfg") -> Path:
    """A clean path under the sandbox, with any previous backup removed."""
    path = CFG / name
    for stale in (path, Path(str(path) + BACKUP_SUFFIX)):
        if stale.is_dir():
            shutil.rmtree(stale)
        elif stale.exists() or stale.is_symlink():
            stale.chmod(0o644)
            stale.unlink()
    return path


def backup_of(path: Path) -> Path:
    return Path(str(path) + BACKUP_SUFFIX)


def quiet(call):
    with contextlib.redirect_stdout(io.StringIO()) as out:
        result = call()
    return result, out.getvalue()


# The shape a leaked config really has, with one member of every key family
# clean_user_config knows about and a decoy beside each of them.
LEAKY = (
    '# RetroArch config -- not hand written, but still the user\'s.\n'
    'audio_driver = "alsa"\n'
    'input_libretro_device_p1 = "1"\n'
    'input_player1_a = "x"\n'
    'input_player1_a_btn = "1"\n'
    'input_player1_analog_dpad_mode = "1"\n'
    'input_player1_b_btn = "nul"\n'
    'input_player1_device_reservation_type = "2"\n'
    'input_player1_joypad_index = "0"\n'
    'input_player1_l_x_plus_axis = "+0"\n'
    'input_player1_mouse_index = "3"\n'
    'input_player1_reserved_device = "padmap Player 1"\n'
    'input_player2_device_reservation_type = "2"\n'
    'input_player2_joypad_index = "0"\n'
    'input_player2_reserved_device = "8BitDo Pro 2"\n'
    'input_player3_joypad_index = "0"\n'
    'savefile_directory = "/roms/save=data"\n'
    'system_directory = "/roms/sys"\n'
    'video_driver = "gl"\n'
)

# Exactly the lines that must change, and to what. Everything else in LEAKY is
# either a key padmap never wrote or a value that is already correct.
LEAKY_EDITS = (
    ('input_player1_a_btn = "1"',
     'input_player1_a_btn = "nul"'),
    ('input_player1_device_reservation_type = "2"',
     'input_player1_device_reservation_type = "0"'),
    ('input_player1_l_x_plus_axis = "+0"',
     'input_player1_l_x_plus_axis = "nul"'),
    ('input_player1_reserved_device = "padmap Player 1"',
     'input_player1_reserved_device = ""'),
    ('input_player2_joypad_index = "0"',
     'input_player2_joypad_index = "1"'),
    ('input_player3_joypad_index = "0"',
     'input_player3_joypad_index = "2"'),
)

LEAKY_CHANGES = [
    'input_player1_a_btn: "1" -> "nul"',
    'input_player1_device_reservation_type: "2" -> "0"',
    'input_player1_l_x_plus_axis: "+0" -> "nul"',
    'input_player1_reserved_device: "padmap Player 1" -> ""',
    'input_player2_joypad_index: "0" -> "1"',
    'input_player3_joypad_index: "0" -> "2"',
]


def leaky_cleaned() -> str:
    text = LEAKY
    for before, after in LEAKY_EDITS:
        if before not in text:
            fail(f"the fixture no longer contains {before!r}; this test is "
                 f"asserting against a config it does not have")
        text = text.replace(before, after)
    return text


# -- S21: the symptom that made this command exist ---------------------------

def check_stale_joypad_index() -> None:
    heading("S21: the stale joypad_index that made one pad drive two ports")

    path = fresh()
    path.write_text('input_player1_joypad_index = "0"\n'
                    'input_player3_joypad_index = "0"\n')
    changes, backup = retroarch.clean_user_config(path)

    if 'input_player3_joypad_index = "2"\n' not in path.read_text():
        fail("clean-config left input_player3_joypad_index pointing at pad 0 "
             "while player 1 also uses pad 0: that one controller still "
             "drives two ports, which is what four players in an N64 game "
             "looked like")
    ok("a stale input_player3_joypad_index is reset to RetroArch's own N-1")

    if 'input_player1_joypad_index = "0"\n' not in path.read_text():
        fail("clean-config rewrote input_player1_joypad_index, whose value "
             "already was RetroArch's default")
    ok("a joypad_index that already holds N-1 is left as it is")

    if changes != ['input_player3_joypad_index: "0" -> "2"']:
        fail(f"clean-config reported {changes!r}: the report the user reads "
             f"must name the one line it touched and no other")
    ok("the change is reported as 'key: old -> new' for the one line touched")

    if backup is None or not backup.exists():
        fail("clean-config rewrote retroarch.cfg without taking a backup")
    ok("a run that changes something takes a backup")


def check_every_key_family() -> None:
    heading("S21: exactly which keys are rewritten, over a realistic config")

    path = fresh()
    path.write_text(LEAKY)
    changes, _ = retroarch.clean_user_config(path)
    after = path.read_text()

    if changes != LEAKY_CHANGES:
        fail(f"clean-config reported {changes!r}, not {LEAKY_CHANGES!r}: the "
             f"set of keys it claims to fix has moved")
    ok("all four key families are reported: joypad_index, reserved_device, "
       "device_reservation_type and the per-player binds")

    if after != leaky_cleaned():
        fail("the rewritten retroarch.cfg is not the original with exactly "
             "the six leaked lines edited; padmap has changed a line in the "
             "user's config that it did not report changing")
    ok("the rewritten file is the original with exactly the reported lines "
       "edited, byte for byte")

    for line in ('audio_driver = "alsa"',
                 'savefile_directory = "/roms/save=data"',
                 'system_directory = "/roms/sys"',
                 'video_driver = "gl"',
                 "# RetroArch config -- not hand written, but still the "
                 "user's."):
        if f"{line}\n" not in after:
            fail(f"clean-config lost the user's own line {line!r}")
    ok("unrelated settings and the leading comment survive untouched")


# Named here rather than read from retroarch.PLAYER_BINDS on purpose. The bug
# in FINDINGS was a bind family nobody had a name for being left behind, and a
# fixture built from the source's own list cannot notice one going missing
# from it -- it would just stop asking about it.
EVERY_BIND = (
    "b", "y", "select", "start", "up", "down", "left", "right",
    "a", "x", "l", "r", "l2", "r2", "l3", "r3",
    "l_x_plus", "l_x_minus", "l_y_plus", "l_y_minus",
    "r_x_plus", "r_x_minus", "r_y_plus", "r_y_minus",
)


def check_binds_become_nul() -> None:
    heading("S21: per-player binds outrank autoconfig, so all of them are "
            "handed back")

    path = fresh()
    lines = []
    for bind in EVERY_BIND:
        lines.append(f'input_player1_{bind}_btn = "7"')
        lines.append(f'input_player1_{bind}_axis = "+1"')
    path.write_text("\n".join(lines) + "\n")
    changes, _ = retroarch.clean_user_config(path)
    after = path.read_text()

    missed = sorted({line.split(" = ")[0] for line in after.splitlines()
                     if line.endswith(('"7"', '"+1"'))})
    if missed:
        fail(f"{len(missed)} per-player bind(s) kept their old value: "
             f"{missed[:4]}. RetroArch only falls back to padmap's autoconfig "
             f"profile when a bind is nul, so each of these buttons reports "
             f"as configured in port 1 and does nothing")
    expected = 2 * len(EVERY_BIND)
    if len(changes) != expected:
        fail(f"clean-config reported {len(changes)} of {expected} per-player "
             f"binds cleared")
    ok(f"all {expected} per-player _btn and _axis binds are set to nul")

    path.write_text('input_player1_start_btn = "nul"\n')
    changes, backup = retroarch.clean_user_config(path)
    if changes or backup is not None:
        fail(f"clean-config reported {changes!r} for a bind already at nul: a "
             f"no-op run must not claim a change or take a backup")
    ok("a bind already at nul is not reported as a change")


def check_reservations() -> None:
    heading("S21: padmap-named reservations are released, the user's own is "
            "not")

    path = fresh()
    path.write_text(
        'input_player1_device_reservation_type = "2"\n'
        'input_player1_reserved_device = "padmap Player 1"\n'
        'input_player2_device_reservation_type = "2"\n'
        'input_player2_reserved_device = "8BitDo Pro 2"\n')
    retroarch.clean_user_config(path)
    after = path.read_text()

    if 'input_player1_reserved_device = ""\n' not in after:
        fail("a reservation naming a padmap virtual pad was left in place: "
             "the slot stays held for a pad that is no longer republished")
    ok("a reservation naming a padmap virtual pad is cleared")

    if 'input_player1_device_reservation_type = "0"\n' not in after:
        fail("the reservation type beside a cleared padmap device was left at "
             "RESERVED, so the slot is held for nothing at all")
    ok("the reservation type beside it is reset to NONE")

    if 'input_player2_reserved_device = "8BitDo Pro 2"\n' not in after:
        fail("clean-config cleared a reservation the user made for their own "
             "controller by name")
    if 'input_player2_device_reservation_type = "2"\n' not in after:
        fail("clean-config reset the type of a reservation the user made for "
             "their own controller")
    ok("a reservation the user made for a real pad by name survives, type and "
       "all")

    # The type line sorts before the device line in RetroArch's alphabetical
    # output, so the two cannot be judged one line at a time.
    path.write_text(
        'input_player4_device_reservation_type = "2"\n'
        'zzz_last_key = "1"\n'
        'input_player4_reserved_device = "padmap Player 4"\n')
    retroarch.clean_user_config(path)
    if 'input_player4_device_reservation_type = "0"\n' not in path.read_text():
        fail("the reservation type was judged before its device name was "
             "read, so a padmap reservation whose type line comes first keeps "
             "holding the slot")
    ok("type and device name are judged together whichever order they appear")

    path.write_text('input_player5_device_reservation_type = "2"\n')
    retroarch.clean_user_config(path)
    if 'input_player5_device_reservation_type = "0"\n' not in path.read_text():
        fail("a RESERVED type with no reserved_device line at all was left "
             "set, holding the slot for a device with no name")
    ok("a RESERVED type with no device name is reset to NONE")

    path.write_text('input_player6_device_reservation_type = "0"\n'
                    'input_player6_reserved_device = ""\n')
    changes, backup = retroarch.clean_user_config(path)
    if changes or backup is not None:
        fail(f"clean-config reported {changes!r} for an already-empty "
             f"reservation pair")
    ok("an already-empty reservation pair is not reported as a change")


def check_keys_left_alone() -> None:
    heading("S21: near-miss keys padmap never wrote are left alone")

    decoys = (
        # Never written to retroarch.cfg by RetroArch; a rule for it could
        # only damage an .rmp file someone pointed --config at.
        'input_libretro_device_p1 = "1"',
        # Keyboard binds, not joypad binds: no _btn/_axis suffix.
        'input_player1_a = "x"',
        'input_player1_start = "enter"',
        # Neighbours of the joypad_index rule. Every value here is deliberately
        # NOT what the rule would rewrite it to, so an over-eager match shows
        # up as a change rather than a coincidence.
        'input_player1_mouse_index = "3"',
        'input_player2_mouse_index = "5"',
        'input_player1_analog_dpad_mode = "1"',
        'input_player1_joypad_index_backup = "3"',
        'input_joypad_index = "3"',
        'input_playerX_joypad_index = "3"',
        # Neighbours of the bind rule.
        'input_player1_b_mbtn = "1"',
        'input_player1_gun_trigger_btn = "1"',
        'input_player1_a_btn_old = "1"',
        # Neighbours of the reservation rules.
        'input_player1_reserved_device_name = "padmap Player 1"',
        'input_reserved_device = "padmap Player 1"',
        # A commented-out leak must not be resurrected or edited.
        '# input_player3_joypad_index = "0"',
        # Global settings that merely mention a player.
        'input_max_users = "4"',
        'config_save_on_exit = "true"',
    )
    path = fresh()
    original = "\n".join(decoys) + "\n"
    path.write_text(original)
    changes, backup = retroarch.clean_user_config(path)

    if changes:
        fail(f"clean-config rewrote keys padmap never wrote: {changes!r}. "
             f"Rewriting a setting the user chose is damage, not cleaning")
    if path.read_text() != original:
        fail("clean-config rewrote a config it reported no changes for")
    if backup is not None:
        fail("clean-config took a backup of a file it did not change")
    ok(f"all {len(decoys)} near-miss keys survive byte for byte, with no "
       f"backup taken")

    if backup_of(path).exists():
        fail("clean-config left a .padmap-backup beside a file it never "
             "rewrote")
    ok("no stray backup file is written for a clean config")


# -- S21: idempotency and the backup -----------------------------------------

def check_idempotent() -> None:
    heading("S21: running clean-config twice reports nothing the second time")

    path = fresh()
    path.write_text(LEAKY)
    first, _ = retroarch.clean_user_config(path)
    after_first = path.read_bytes()

    second, backup = retroarch.clean_user_config(path)
    if second:
        fail(f"a second clean-config run reported {second!r}: the values it "
             f"writes are not the values it considers clean, so the command "
             f"never settles and every run takes another backup")
    ok("the second run reports no changes")

    if backup is not None:
        fail("the second run took a backup of a file it did not change")
    ok("the second run takes no backup")

    if path.read_bytes() != after_first:
        fail("the second run rewrote the file it had already cleaned")
    ok("the second run leaves the file byte for byte identical")

    if len(first) != len(LEAKY_CHANGES):
        fail("the first run of the idempotency check did not clean the "
             "fixture, so the second run proves nothing")
    ok("the first run really did have something to clean")


def check_backup_contents() -> None:
    heading("S21: the backup is the file as it was, at the documented path")

    path = fresh()
    path.write_text(LEAKY)
    changes, backup = retroarch.clean_user_config(path)

    if backup != CFG / ("retroarch.cfg" + BACKUP_SUFFIX):
        fail(f"the backup went to {backup}, not retroarch.cfg.padmap-backup: "
             f"the path S21 tells the user to look for")
    ok("the backup is written to retroarch.cfg.padmap-backup")

    if backup.read_text() != LEAKY:
        fail("the backup is not the config as it was before the rewrite, so "
             "the one undo padmap offers does not undo")
    ok("the backup is the pre-run config, byte for byte")

    if path.read_text() == LEAKY:
        fail("clean-config took a backup and then did not rewrite anything")
    ok("the rewrite and the backup are different files")

    # A backup path that cannot be written must not leave the config half done.
    path = fresh("blocked.cfg")
    path.write_text(LEAKY)
    blocked = backup_of(path)
    blocked.mkdir()
    try:
        retroarch.clean_user_config(path)
    except OSError:
        pass
    else:
        fail("clean-config claimed success when it could not write its backup")
    if path.read_text() != LEAKY:
        fail("clean-config rewrote retroarch.cfg after failing to back it up: "
             "the user's config was changed with no way back")
    ok("a backup that cannot be written aborts before the config is touched")
    shutil.rmtree(blocked)


def check_backup_already_exists() -> None:
    heading("S21: a second cleaning when a backup is already there")

    path = fresh()
    original = LEAKY.replace('audio_driver = "alsa"',
                             'audio_driver = "alsa"\n'
                             'user_note = "the config I started with"')
    path.write_text(original)
    retroarch.clean_user_config(path)
    if backup_of(path).read_text() != original:
        fail("the first backup is not the original config")
    ok("the first run backs up the original")

    # RetroArch leaks again -- a second session, and the user has since
    # dropped their note.
    relapse = LEAKY
    path.write_text(relapse)
    changes, backup = retroarch.clean_user_config(path)
    if not changes:
        fail("clean-config found nothing to clean in a freshly leaked config, "
             "so this scenario tests nothing")
    ok("a config that leaked again is cleaned again")

    if backup is None or backup != backup_of(path):
        fail(f"the second run wrote its backup to {backup}, not the "
             f"documented .padmap-backup path")
    if backup.read_text() != relapse:
        fail("the backup after the second run is not that run's input, so it "
             "cannot undo that run either")
    ok("the backup always holds the input of the most recent rewrite")

    if 'user_note = "the config I started with"' not in backup.read_text():
        gap("clean_user_config writes its backup with a plain "
            "backup.write_text(), so a second run silently overwrites the "
            "first run's backup. The copy of the config as it was before "
            "padmap ever touched it -- the only thing that makes the first "
            "rewrite undoable -- is destroyed with no warning and no second "
            "slot. Repro: run `padmap clean-config` on a leaked "
            "retroarch.cfg, let RetroArch leak again, run it a second time; "
            "retroarch.cfg.padmap-backup now holds the already-cleaned-once "
            "config and the original is gone. A refusal, a numbered backup or "
            "a warning would all keep it.")
    else:
        ok("an existing backup is preserved rather than overwritten")


def check_dry_run() -> None:
    heading("S21: --dry-run reports the same changes and writes nothing")

    path = fresh()
    path.write_text(LEAKY)
    before = path.read_bytes()
    changes, backup = retroarch.clean_user_config(path, dry_run=True)

    if changes != LEAKY_CHANGES:
        fail(f"a dry run reported {changes!r}, not the {len(LEAKY_CHANGES)} "
             f"changes the real run makes: a preview that does not match what "
             f"will happen is worse than no preview")
    ok("a dry run reports exactly what the real run would change")

    if path.read_bytes() != before:
        fail("a dry run rewrote retroarch.cfg")
    ok("a dry run leaves retroarch.cfg byte for byte")

    if backup is not None or backup_of(path).exists():
        fail("a dry run took a backup, so it did write to the user's config "
             "directory after all")
    ok("a dry run writes no backup file")

    real, _ = retroarch.clean_user_config(path)
    if real != changes:
        fail(f"the real run changed {real!r} after the dry run promised "
             f"{changes!r}")
    ok("the real run afterwards makes exactly the previewed changes")


# -- S21: the shapes a real config comes in ----------------------------------

def check_crlf() -> None:
    heading("S21: a config with CRLF line endings")

    path = fresh()
    text = ('video_driver = "gl"\r\n'
            'input_player3_joypad_index = "0"\r\n'
            'audio_driver = "alsa"\r\n')
    path.write_bytes(text.encode())
    changes, _ = retroarch.clean_user_config(path)
    after = path.read_bytes()

    if changes != ['input_player3_joypad_index: "0" -> "2"']:
        fail(f"clean-config did not recognise a leaked key on a CRLF line: "
             f"{changes!r}")
    ok("a leaked key is recognised on a CRLF line")

    if b'input_player3_joypad_index = "2"' not in after:
        fail("the leaked key was reported as changed but not written")
    ok("the leaked key really is reset to N-1 on disk")

    expected = text.replace('_index = "0"', '_index = "2"').encode()
    if after == expected:
        ok("every CRLF ending survives, on the changed line and the others")
    else:
        gap("clean_user_config reads with read_text() and writes with "
            "write_text(), both in universal-newlines mode, so every CRLF in "
            "the file becomes a bare LF on the way out. A config with Windows "
            "line endings -- one shared from a dual-boot install, or "
            "hand-edited on Windows -- is rewritten end to end by a command "
            "that reported changing one line, and the backup is written from "
            "the same translated text so it cannot put the endings back. "
            "Repro: write a retroarch.cfg with \\r\\n endings and one "
            "input_player3_joypad_index line, run `padmap clean-config`; the "
            "file and retroarch.cfg.padmap-backup both come back pure LF. "
            "newline='' on the read and the write, or reading and writing "
            "bytes, would keep them.")

    if backup_of(path).read_bytes() == text.encode():
        ok("the backup of a CRLF config is the original bytes")
    else:
        note("the backup shares the translation above, so it cannot undo it")

    # Whatever it does to the endings, it must not lose or duplicate a line.
    if after.replace(b"\r\n", b"\n").count(b"\n") != 3:
        fail(f"clean-config changed how many lines the config has: {after!r}")
    ok("no line is lost, duplicated or split by the rewrite")


def check_no_trailing_newline() -> None:
    heading("S21: a config whose last line has no newline")

    path = fresh()
    text = 'video_driver = "gl"\ninput_player3_joypad_index = "0"'
    path.write_text(text)
    retroarch.clean_user_config(path)
    after = path.read_text()

    if after.endswith("\n"):
        fail("clean-config added a trailing newline to a config that had "
             "none, so the file differs beyond the line it reported")
    if after != 'video_driver = "gl"\ninput_player3_joypad_index = "2"':
        fail(f"the unterminated last line was not cleaned correctly: "
             f"{after!r}")
    ok("a leaked key on an unterminated last line is cleaned and no newline "
       "is added")


def check_duplicate_keys() -> None:
    heading("S21: a config with the same key twice")

    path = fresh()
    path.write_text('input_player3_joypad_index = "0"\n'
                    'video_driver = "gl"\n'
                    'input_player3_joypad_index = "0"\n')
    changes, _ = retroarch.clean_user_config(path)
    after = path.read_text()

    if after.count('input_player3_joypad_index = "2"') != 2:
        fail(f"clean-config cleaned only one of two duplicate "
             f"input_player3_joypad_index lines: {after!r}. Which duplicate "
             f"RetroArch honours is its business, so a cleaner that leaves "
             f"either one can leave the stale value in force")
    ok("every occurrence of a duplicated leaked key is cleaned")

    if len(changes) != 2:
        fail(f"clean-config reported {len(changes)} changes for two rewritten "
             f"lines, so the report undercounts what it did")
    ok("both rewrites are reported")

    path.write_text('input_player3_reserved_device = "padmap Player 3"\n'
                    'input_player3_reserved_device = "padmap Player 3"\n')
    retroarch.clean_user_config(path)
    if path.read_text() != ('input_player3_reserved_device = ""\n'
                            'input_player3_reserved_device = ""\n'):
        fail("a duplicated padmap reservation was only half released")
    ok("a duplicated padmap reservation is released on both lines")


def check_spacing_and_comments() -> None:
    heading("S21: odd spacing around '=', comments and blank lines")

    path = fresh()
    text = ('\n'
            '# a comment padmap must not touch\n'
            '   input_player3_joypad_index   =   "0"   \n'
            'input_player4_joypad_index="0"\n'
            '\t input_player5_joypad_index\t=\t"0"\n'
            '\n'
            '   # an indented comment\n')
    path.write_text(text)
    changes, _ = retroarch.clean_user_config(path)
    after = path.read_text()

    expected = text.replace('   =   "0"   ', '   =   "2"   ') \
                   .replace('_index="0"', '_index="3"') \
                   .replace('\t=\t"0"', '\t=\t"4"')
    if after != expected:
        fail(f"clean-config did not preserve the spacing around '=': "
             f"{after!r} != {expected!r}. RetroArch rewrites this file too, "
             f"and reflowing lines the user can see is a change padmap did "
             f"not report")
    ok("indent, the spacing around '=' and the trailing whitespace are all "
       "preserved on rewritten lines")

    if len(changes) != 3:
        fail(f"clean-config found {len(changes)} of 3 leaked keys written "
             f"with unusual spacing")
    ok("a leaked key is recognised with no spaces, extra spaces or tabs "
       "around '='")

    if "# a comment padmap must not touch\n" not in after:
        fail("clean-config lost a comment")
    if after.count("\n\n") != text.count("\n\n"):
        fail("clean-config changed the blank lines in the user's config")
    ok("comments and blank lines survive")


def check_awkward_values() -> None:
    heading("S21: values containing '=' or a quote")

    path = fresh()
    text = ('savefile_directory = "/roms/save=data"\n'
            'netplay_password = "he said "hi""\n'
            'video_shader = "shaders/a=b/c.slangp"\n'
            'input_player3_joypad_index = "0=0"\n')
    path.write_text(text)
    changes, _ = retroarch.clean_user_config(path)
    after = path.read_text()

    for line in ('savefile_directory = "/roms/save=data"',
                 'netplay_password = "he said "hi""',
                 'video_shader = "shaders/a=b/c.slangp"'):
        if f"{line}\n" not in after:
            fail(f"clean-config damaged the user's value {line!r}: a path or "
                 f"password containing '=' or a quote is not padmap's to "
                 f"reinterpret")
    ok("unrelated values containing '=' or a quote survive byte for byte")

    if 'input_player3_joypad_index = "0=0"\n' in after:
        gap("a leaked joypad_index whose value contains '=' was left alone")
    elif 'input_player3_joypad_index = "2"\n' not in after:
        fail(f"a leaked joypad_index with an odd value was rewritten to "
             f"something other than N-1: {after!r}")
    else:
        ok("a leaked joypad_index is reset to N-1 whatever nonsense its value "
           "held")

    if len(changes) != 1:
        fail(f"clean-config reported {changes!r} over values containing '=' "
             f"and quotes: it matched a line it has no rule for")
    ok("only the one leaked line is reported")


def check_parser_is_stricter_than_retroarch() -> None:
    heading("S21: leaked values RetroArch honours but clean-config does not "
            "match")

    # config_file.c reads an unquoted value up to whitespace, and stops a
    # quoted one at its closing quote, ignoring the rest of the line. padmap's
    # _SETTING regex requires quotes and nothing but whitespace after them.
    path = fresh()
    text = ('input_player3_joypad_index = 0\n'
            'input_player4_joypad_index = "0" # left over from padmap\n')
    path.write_text(text)
    changes, backup = retroarch.clean_user_config(path)

    if path.read_text() != text:
        fail(f"clean-config half-rewrote a line it does not fully match, "
             f"which is worse than skipping it: {path.read_text()!r}")
    ok("a line clean-config does not match is left exactly as it was")

    if changes:
        gap("clean-config now rewrites unquoted or comment-trailed values; "
            "turn the assertion below into the real one")
    else:
        gap("clean_user_config's _SETTING regex is stricter than RetroArch's "
            "own config parser: it requires a double-quoted value with "
            "nothing but whitespace after the closing quote. RetroArch's "
            "config_file.c accepts an unquoted value (read up to whitespace) "
            "and ignores anything after a quoted value's closing quote. So "
            "`input_player3_joypad_index = 0` and "
            "`input_player4_joypad_index = \"0\" # note` are both honoured by "
            "RetroArch and both invisible to clean-config -- and the command "
            "then prints \"has no padmap leftovers\", which reads as "
            "all-clear while the exact stale index of S21 is still in force. "
            "Repro: put `input_player3_joypad_index = 0` in a retroarch.cfg "
            "and run `padmap clean-config`; it exits 0 reporting nothing.")

    if backup is not None:
        fail("clean-config took a backup for a file it reported no changes to")
    ok("no backup is taken when nothing is reported")


# -- S21: files that are not a readable config -------------------------------

def check_empty_file() -> None:
    heading("S21: an empty retroarch.cfg")

    path = fresh()
    path.write_text("")
    changes, backup = retroarch.clean_user_config(path)

    if changes:
        fail(f"clean-config reported {changes!r} in an empty file")
    if backup is not None or backup_of(path).exists():
        fail("clean-config backed up an empty file it never rewrote")
    if path.read_bytes() != b"":
        fail("clean-config wrote to an empty retroarch.cfg")
    ok("an empty config has nothing to clean, is not rewritten and is not "
       "backed up")

    path.write_text("   \n\n\t\n")
    changes, backup = retroarch.clean_user_config(path)
    if changes or backup is not None or path.read_text() != "   \n\n\t\n":
        fail("clean-config touched a whitespace-only config")
    ok("a whitespace-only config is left alone")


def check_not_utf8() -> None:
    heading("S21: a retroarch.cfg that is not valid UTF-8")

    path = fresh()
    raw = (b'system_directory = "/roms/caf\xe9"\n'
           b'input_player3_joypad_index = "0"\n')
    path.write_bytes(raw)
    changes, backup = retroarch.clean_user_config(path)

    if changes != ['input_player3_joypad_index: "0" -> "2"']:
        fail(f"clean-config failed to clean a leaked key in a config holding "
             f"a non-UTF-8 byte: {changes!r}. A latin-1 ROM path in one line "
             f"must not stop the stale index in another being fixed")
    ok("a leaked key is still cleaned in a config holding a non-UTF-8 byte")

    after = path.read_bytes()
    if b'input_player3_joypad_index = "2"' not in after:
        fail("the leaked line was reported as changed but not written")
    ok("the leaked line really is rewritten on disk")

    if b"/roms/caf\xe9" in after:
        ok("the non-UTF-8 byte in the user's own line survives the rewrite")
    else:
        gap("clean_user_config reads the config with read_text(errors="
            "'replace') and then writes BOTH the rewritten file and the "
            "backup from that replaced text, so any byte that is not UTF-8 -- "
            "a latin-1 ROM path in system_directory, say -- becomes U+FFFD in "
            "the config RetroArch reads next time AND in the backup taken to "
            "make the rewrite undoable. The one file padmap admits is the "
            "user's is damaged in a way padmap's own backup cannot restore, "
            "and RetroArch then cannot find the directory. Repro: write "
            "b'system_directory = \"/roms/caf\\xe9\"' beside an "
            "input_player3_joypad_index line and run `padmap clean-config`; "
            "the byte is 0xef 0xbf 0xbd in both files afterwards. Reading and "
            "writing bytes, or errors='surrogateescape', would keep it.")

    if backup is not None and backup.read_bytes() == raw:
        ok("the backup of a non-UTF-8 config is the original bytes")
    else:
        note("the backup shares the damage above, so it cannot undo it")

    if backup is None:
        fail("clean-config rewrote a config without backing it up")
    ok("a non-UTF-8 config is still backed up before being rewritten")


def check_read_only_file() -> None:
    heading("S21: a retroarch.cfg the user cannot write")

    if os.geteuid() == 0:
        note("running as root, so a read-only file is still writable; skipped")
        return

    path = fresh()
    path.write_text(LEAKY)
    path.chmod(0o444)
    try:
        try:
            retroarch.clean_user_config(path)
        except OSError:
            pass
        except Exception as error:  # noqa: BLE001
            fail(f"clean-config raised {type(error).__name__}: {error} for a "
                 f"read-only retroarch.cfg. The caller catches OSError, so "
                 f"anything else escapes `padmap clean-config` as a traceback")
        else:
            fail("clean-config claimed to have rewritten a read-only config")
        ok("a read-only retroarch.cfg raises an OSError the caller can catch")

        if path.read_text() != LEAKY:
            fail("a read-only retroarch.cfg was rewritten anyway")
        ok("a read-only retroarch.cfg is left exactly as it was")

        args = type("Args", (), {"config": str(path), "dry_run": False})()
        code, output = quiet(lambda: cli.cmd_clean_config(args))
        if code != 1:
            fail(f"padmap clean-config returned {code} for a config it could "
                 f"not write: a failure the user must see, not a success")
        if str(path) not in output:
            fail(f"padmap clean-config did not name the file it could not "
                 f"rewrite: {output!r}")
        ok("padmap clean-config reports the failure and names the file")

        if backup_of(path).exists():
            gap("clean_user_config writes the backup before the rewrite and "
                "does not clean it up when the rewrite fails, so a run that "
                "changed nothing still leaves a retroarch.cfg.padmap-backup "
                "behind in the user's config directory -- next to a config "
                "that is not the one it backs up a copy of, which is exactly "
                "the confusion the backup exists to avoid. Repro: chmod 444 a "
                "leaked retroarch.cfg and run `padmap clean-config`; it "
                "reports a failure and the .padmap-backup is there.")
        else:
            ok("a failed rewrite leaves no stray backup file behind")
    finally:
        path.chmod(0o644)


def check_directory_and_missing() -> None:
    heading("S21: a retroarch.cfg that is a directory, or is not there")

    path = fresh("as-a-dir.cfg")
    path.mkdir()
    try:
        retroarch.clean_user_config(path)
    except OSError:
        pass
    except Exception as error:  # noqa: BLE001
        fail(f"clean-config raised {type(error).__name__}: {error} for a "
             f"config path that is a directory; the caller only catches "
             f"OSError, so this escapes as a traceback")
    else:
        fail("clean-config claimed to have cleaned a directory")
    ok("a config path that is a directory raises a catchable OSError")

    args = type("Args", (), {"config": str(path), "dry_run": False})()
    code, output = quiet(lambda: cli.cmd_clean_config(args))
    if code != 1:
        fail(f"padmap clean-config returned {code} for a config path that is "
             f"a directory")
    if not output.strip():
        fail("padmap clean-config said nothing about a config it could not "
             "read")
    ok("padmap clean-config reports a directory as a failure, with a message")
    shutil.rmtree(path)

    missing = fresh("not-there.cfg")
    args = type("Args", (), {"config": str(missing), "dry_run": False})()
    code, output = quiet(lambda: cli.cmd_clean_config(args))
    if code != 1 or str(missing) not in output:
        fail(f"padmap clean-config returned {code} for a config that does not "
             f"exist, saying {output!r}: S21 requires the file to exist and "
             f"the user needs to be told which one padmap looked for")
    ok("a config that does not exist is reported as a failure, by name")

    if missing.exists() or backup_of(missing).exists():
        fail("padmap clean-config created a retroarch.cfg that was not there")
    ok("nothing is created for a config that does not exist")


# -- S21: the command the user actually runs ---------------------------------

def check_cli_end_to_end() -> None:
    heading("S21: `padmap clean-config` and `--dry-run` end to end")

    path = fresh()
    path.write_text(LEAKY)
    args = type("Args", (), {"config": str(path), "dry_run": True})()
    code, output = quiet(lambda: cli.cmd_clean_config(args))

    if code != 0:
        fail(f"padmap clean-config --dry-run returned {code} over a config it "
             f"could read")
    if path.read_text() != LEAKY:
        fail("padmap clean-config --dry-run rewrote retroarch.cfg")
    ok("padmap clean-config --dry-run succeeds and writes nothing")

    for change in LEAKY_CHANGES:
        if change not in output:
            fail(f"padmap clean-config --dry-run did not print {change!r}, so "
                 f"the user cannot see what it would do before it does it")
    if "Would change" not in output or "Dry run" not in output:
        fail(f"a dry run did not say it was a dry run: {output!r}")
    ok("every change is listed and the output says nothing was written")

    args = type("Args", (), {"config": str(path), "dry_run": False})()
    code, output = quiet(lambda: cli.cmd_clean_config(args))
    if code != 0:
        fail(f"padmap clean-config returned {code} over a leaked config")
    if path.read_text() != leaky_cleaned():
        fail("padmap clean-config did not produce the cleaned config")
    ok("padmap clean-config rewrites the config it previewed")

    if str(backup_of(path)) not in output:
        fail(f"padmap clean-config did not tell the user where the backup "
             f"went: {output!r}")
    ok("the output names the backup path")

    code, output = quiet(lambda: cli.cmd_clean_config(args))
    if code != 0 or "no padmap leftovers" not in output:
        fail(f"a second `padmap clean-config` returned {code} saying "
             f"{output!r}: an already-clean config is a success with nothing "
             f"to say")
    ok("a second run reports an already-clean config as a success")


def main() -> int:
    check_stale_joypad_index()
    check_every_key_family()
    check_binds_become_nul()
    check_reservations()
    check_keys_left_alone()
    check_idempotent()
    check_backup_contents()
    check_backup_already_exists()
    check_dry_run()
    check_crlf()
    check_no_trailing_newline()
    check_duplicate_keys()
    check_spacing_and_comments()
    check_awkward_values()
    check_parser_is_stricter_than_retroarch()
    check_empty_file()
    check_not_utf8()
    check_read_only_file()
    check_directory_and_missing()
    check_cli_end_to_end()

    print(f"\n{CHECKS} checks over S21 clean-config")
    print("all checks passed")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    finally:
        shutil.rmtree(_SANDBOX, ignore_errors=True)
