"""Does the library screen answer the keys a controller actually sends?

Layout is checked by looking at it (tools/preview_library.py). Key routing is
not visible in a screenshot, and it is where this screen breaks: focus has to
travel between the tab bar and two sibling views, and the shoulder buttons
have to keep working from all three. A wrong `focus` assignment leaves an item
holding focus without activeFocus, which looks completely normal and silently
stops answering the d-pad.

    QT_QPA_PLATFORM=offscreen python3 tools/check_library.py
"""

import os
import sys

# Pick the offscreen platform before PySide6 is imported, or Qt
# binds to whatever display happens to be around. Run without it,
# this harness reported two spurious failures -- a check that
# depends on an environment variable the caller must remember is a
# worse trap than the bugs it is looking for.
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
from pathlib import Path

from PySide6.QtCore import QCoreApplication, QEvent, Qt, QUrl
from PySide6.QtGui import QGuiApplication, QKeyEvent
from PySide6.QtQuick import QQuickView

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "tools"))
sys.path.insert(0, str(REPO / "src"))

from preview_library import StubApi, StubCollection, StubGame  # noqa: E402

from padmap.pegasus import Entry  # noqa: E402

THEME = REPO / "pegasus" / "theme"

FAILURES: list[str] = []


def check(name: str, got, want) -> None:
    if got == want:
        print(f"  ok    {name}")
    else:
        print(f"  FAIL  {name}: got {got!r}, wanted {want!r}")
        FAILURES.append(name)


def collection(name, titles, art=""):
    entries = [
        Entry(title=t, path=f"/games/{t}.rom",
              assets={"boxFront": art} if art else {})
        for t in titles
    ]
    return StubCollection(name, [StubGame(e) for e in entries])


def press(view, key, text=None):
    """Send one key the way a keyboard would.

    The text matters: search mode appends `event.text`, so an event built
    without it reports every letter as unhandled and the query stays empty --
    which looks exactly like a broken search rather than a broken test.
    """
    if text is None:
        text = chr(key).lower() if Qt.Key.Key_A <= key <= Qt.Key.Key_Z else ""
    for kind in (QEvent.Type.KeyPress, QEvent.Type.KeyRelease):
        QCoreApplication.sendEvent(
            view,
            QKeyEvent(kind, key, Qt.KeyboardModifier.NoModifier, text),
        )


def main() -> int:
    app = QGuiApplication(sys.argv)

    # One collection with art and one without, so the grid/list switch is
    # exercised rather than assumed.
    api = StubApi([
        collection("Arcade", ["Pac-Man", "Galaga", "1942", "Donkey Kong"]),
        collection("GameCube", ["Super Mario Sunshine", "Animal Crossing"],
                   art=str(REPO / "pegasus" / "theme" / "icons")),
        collection("Nintendo 64", ["Banjo-Kazooie", "GoldenEye"]),
    ])

    view = QQuickView()
    view.rootContext().setContextProperty("api", api)
    view.setSource(QUrl.fromLocalFile(str(THEME / "Library.qml")))
    if view.status() != QQuickView.Status.Ready:
        for error in view.errors():
            print("  QML error:", error.toString())
        return 1
    view.setResizeMode(QQuickView.ResizeMode.SizeRootObjectToView)
    view.resize(1280, 720)

    # theme.qml declares `Library { focus: true }`. Without it nothing in the
    # screen has activeFocus, no key is delivered anywhere, and every check
    # below passes or fails for reasons that have nothing to do with what it
    # is testing.
    root = view.rootObject()
    root.setProperty("focus", True)

    view.show()
    app.processEvents()

    print("tabs")
    check("starts on the first collection", root.property("tab"), 0)
    check("counts the collections", root.property("collectionCount"), 3)
    # Q and E are Pegasus's prev/next page, which is L1/R1 on a pad.
    press(view, Qt.Key.Key_E)
    check("R1 moves to the next tab", root.property("tab"), 1)
    press(view, Qt.Key.Key_Q)
    check("L1 moves back", root.property("tab"), 0)
    press(view, Qt.Key.Key_Q)
    check("L1 wraps to the last tab", root.property("tab"), 2)
    press(view, Qt.Key.Key_E)
    check("R1 wraps to the first", root.property("tab"), 0)

    print("views")
    check("a collection with no art lists", root.property("artMode"), False)
    root.setProperty("tab", 1)
    app.processEvents()
    check("a collection with art grids", root.property("artMode"), True)
    root.setProperty("tab", 0)
    app.processEvents()

    print("focus")
    # The screen must come up ready to move the cursor, not needing a click.
    view.setProperty("__unused", 0)
    check("the games have focus at startup",
          root.property("view").property("activeFocus"), True)
    press(view, Qt.Key.Key_Up)
    check("up from the first row reaches the tabs",
          root.property("view").property("activeFocus"), False)
    press(view, Qt.Key.Key_Right)
    check("right in the tab bar switches tab", root.property("tab"), 1)
    press(view, Qt.Key.Key_Down)
    check("down returns to the games",
          root.property("view").property("activeFocus"), True)

    print("search")
    root.setProperty("tab", 2)
    app.processEvents()
    press(view, Qt.Key.Key_F)
    check("Filters enters search", root.property("searching"), True)
    for key in (Qt.Key.Key_B, Qt.Key.Key_A, Qt.Key.Key_N):
        press(view, key)
    # "b", "a" and "n" are all bound to something by Pegasus outside search
    # mode; inside it they have to be plain text.
    check("letters are text while searching", root.property("query"), "ban")
    check("the filter applies to this collection only",
          root.property("shownCount"), 1)
    press(view, Qt.Key.Key_Escape)
    check("Escape clears the filter", root.property("query"), "")
    check("Escape leaves search mode", root.property("searching"), False)

    print("a committed filter belongs to its tab")
    press(view, Qt.Key.Key_F)
    press(view, Qt.Key.Key_Z)
    check("query typed", root.property("query"), "z")
    # Enter commits and leaves search mode. Until it does, "e" is still a
    # letter -- pressing R1 mid-word must type, not change console.
    press(view, Qt.Key.Key_E)
    check("R1 is a letter while still searching", root.property("query"), "ze")
    press(view, Qt.Key.Key_Return, text="\r")
    check("Enter keeps the filter and exits the mode",
          root.property("searching"), False)
    check("the filter survived", root.property("query"), "ze")
    press(view, Qt.Key.Key_E)
    # A query typed for one console mostly matches nothing in the next, and
    # the empty screen reads as a broken tab rather than as a live filter.
    check("switching tab now drops the query", root.property("query"), "")

    print()
    if FAILURES:
        print(f"{len(FAILURES)} failure(s): {', '.join(FAILURES)}")
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
