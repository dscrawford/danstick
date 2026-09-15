"""Reading a text file padmap does not own.

Every file padmap reads but did not write -- the udev rules in /run, the
`prompted` list in XDG_RUNTIME_DIR, a stored profile, another program's config --
can be absent, unreadable, or full of bytes that are not UTF-8. The first two
of those raise `OSError` and every reader guards for it. The third raises
`UnicodeDecodeError`, which is a `ValueError`, so `except OSError` misses it
entirely and the exception escapes into whatever called it.

That exact hole has now been found five times in five separate places:
`hide.unhidden`, the profile store, `Server._load_prompted`, `hide.install`
and `cli._forget_prompted`. Each cost a whole command or a whole start-up over
one bad byte: `padmap serve` could not start, `sudo padmap hide` tracebacked,
`padmap forget` -- the recovery command, run precisely when things are already
broken -- tracebacked too. Five instances of one mistake is not five bugs; it
is a missing helper, so this is it. A sixth turned up while converting the
last two -- reading a file another program owns -- which is
the argument in one line. New code reading a file padmap does not control
should call this rather than `Path.read_text()`.

`errors="replace"` rather than a stricter decode because these files are read
to find something in them -- a vid/pid pair, a signature, a directory. A
damaged byte should cost that one character, not the ninety-nine good lines
around it.
"""

from __future__ import annotations

from pathlib import Path


def read_text(path: Path | str, default: str | None = "") -> str | None:
    """The file's text, or `default` when it cannot be read at all.

    Missing, a directory, no permission, mid-removal -- all `OSError`, all
    answered with `default`. Bytes that are not UTF-8 are decoded with
    `errors="replace"`, so a damaged file still reads as text.

    Pass `default=None` when "no file" and "an empty file" must be told
    apart: `hide.unhidden` falls through to the next rules path only when
    there was nothing to read, but an installed file that happens to be empty
    genuinely covers no controllers.
    """
    try:
        return Path(path).read_bytes().decode("utf-8", "replace")
    except OSError:
        return default
