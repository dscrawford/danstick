"""Would every QML file in the theme load, and does any of them shadow a global?

A theme that fails to load takes the whole front-end with it, so this is the
cheapest check here with the largest blast radius. The failure that prompted
it reached the user's screen:

    theme.qml:45:9: Signal parameter "console" hides global variable.

`console` is a QML global. A signal parameter, handler parameter or local
shadowing one is refused by the engine, and the refusal costs the whole file.

Two passes, because one of them cannot see the other's bugs.

**Compilation.** Every file is compiled through PySide6. That catches syntax
errors and unresolved types across the whole theme for almost no cost. It does
*not* catch the bug above, and that is not a guess: with the broken theme
built into the store and run through real Pegasus in `e2e_pegasus.py`, no
diagnostic appeared there either. PySide6 is Qt 6 and Pegasus is Qt 5, so a Qt
5 diagnostic is structurally invisible here -- but the engine that does emit it
did not emit it under the harness either, which leaves nothing that runs code
able to see this class of fault.

**Shadowed globals.** So it is linted for directly, by reading the text. Less
clever than an engine and it is the only thing that actually works: it fails on
the exact source that broke the theme, which is the bar a check has to clear.

    QT_QPA_PLATFORM=offscreen python3 tools/check_theme_loads.py
"""

import os
import re
import sys
from pathlib import Path

# Before PySide6 is imported, or Qt binds to whatever display is around.
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QUrl  # noqa: E402
from PySide6.QtGui import QGuiApplication  # noqa: E402
from PySide6.QtQml import QQmlComponent, QQmlEngine  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
THEME = REPO / "pegasus" / "theme"

# Names the QML engine puts in scope. Shadowing one is refused outright; this
# is not a style rule. Not exhaustive -- the JS built-ins a theme is plausibly
# tempted to name a variable after, plus QML's own two.
QML_GLOBALS = frozenset({
    "console", "Qt", "Math", "JSON", "Date", "String", "Number", "Boolean",
    "Array", "Object", "Function", "RegExp", "Error", "NaN", "Infinity",
    "undefined", "parseInt", "parseFloat", "isNaN",
})

# `signal foo(string console, string key)`
SIGNAL = re.compile(r"^\s*signal\s+\w+\s*\(([^)]*)\)")
# `onFoo: function (console, key) {`
HANDLER = re.compile(r"^\s*on[A-Z]\w*\s*:\s*function\s*\(([^)]*)\)")
# `function foo(console) {`
FUNCTION = re.compile(r"^\s*function\s+\w+\s*\(([^)]*)\)")
# `var console = ...`
LOCAL = re.compile(r"^\s*(?:var|let|const)\s+(\w+)\s*=")


def parameter_names(raw: str) -> list[str]:
    """Names out of a QML parameter list, typed (`string x`) or not."""
    names = []
    for part in raw.split(","):
        part = part.strip()
        if part:
            # `string console` -> console; `console` -> console.
            names.append(part.split()[-1])
    return names


def shadowed(path: Path) -> list[str]:
    problems = []
    for number, line in enumerate(path.read_text().splitlines(), 1):
        found: list[str] = []
        for pattern in (SIGNAL, HANDLER, FUNCTION):
            match = pattern.match(line)
            if match:
                found += parameter_names(match.group(1))
        match = LOCAL.match(line)
        if match:
            found.append(match.group(1))
        for name in found:
            if name in QML_GLOBALS:
                problems.append(
                    f"{path.name}:{number}: {name!r} shadows a QML global "
                    f"-- the engine refuses the file")
    return problems


def main() -> int:
    app = QGuiApplication(sys.argv)          # noqa: F841 -- required by Qt
    engine = QQmlEngine()
    engine.addImportPath(str(THEME))

    files = sorted(THEME.glob("*.qml"))
    if not files:
        raise SystemExit(f"FAIL: no QML found in {THEME}")

    print(f"checking {len(files)} QML file(s) from {THEME}:")
    failures: list[str] = []
    for path in files:
        problems = [
            error.toString()
            for error in QQmlComponent(
                engine, QUrl.fromLocalFile(str(path))).errors()
        ]
        problems += shadowed(path)
        if problems:
            failures += problems
            print(f"  FAIL  {path.name}")
            for problem in problems:
                print(f"          {problem}")
        else:
            print(f"  ok    {path.name}")

    if failures:
        raise SystemExit(
            f"\nFAIL: {len(failures)} problem(s). A theme that will not load "
            f"takes the whole front-end with it.")

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
