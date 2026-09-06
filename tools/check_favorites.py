"""Does marking a game a favourite behave, and does the order stay still?

Three things here are invisible in a screenshot and are where this feature
breaks.

The write has to land on Pegasus's own property: `favorite` on model::Game is
read/write with a change signal, and the front-end mirrors it into
~/.config/pegasus-frontend/favorites.txt itself. If the theme sets anything
else the star appears and nothing is ever saved -- so the stub game here has
exactly that property and the checks assert the object was really written to.

The cursor has to stay where it was. Sorting the moment a game is marked pulls
the row out from under you; the checks below pin the opposite behaviour, that
nothing moves until the tab is opened again.

The scan that finds the favourites walks all 8302 arcade games, so it has to
happen once per collection and not once per tab switch. The model here counts
its own reads, which is the only way to see that from outside.

    python3 tools/check_favorites.py
"""

import os
import sys

# Before PySide6 is imported, and not left to the caller: the same check run
# with and without this differed, which is a worse trap than the bugs it looks
# for.
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from pathlib import Path  # noqa: E402

from PySide6.QtCore import (  # noqa: E402
    QCoreApplication, QEvent, QObject, Qt, QUrl, Slot,
)
from PySide6.QtGui import QGuiApplication, QKeyEvent  # noqa: E402
from PySide6.QtQuick import QQuickView  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "tools"))
sys.path.insert(0, str(REPO / "src"))

from preview_favorites import FavGame, FavKeys  # noqa: E402
from preview_library import ObjectListModel, StubApi, StubCollection, fake_cover  # noqa: E402

from padmap.pegasus import Entry  # noqa: E402

THEME = REPO / "pegasus" / "theme"

FAILURES: list[str] = []


def check(name: str, got, want) -> None:
    if got == want:
        print(f"  ok    {name}")
    else:
        print(f"  FAIL  {name}: got {got!r}, wanted {want!r}")
        FAILURES.append(name)


class CountingModel(ObjectListModel):
    """Counts the reads the theme makes, so caching can be checked at all."""

    def __init__(self, items):
        super().__init__(items)
        self.reads = 0

    @Slot(int, result=QObject)
    def get(self, row):
        self.reads += 1
        return super().get(row)


def collection(name, count, favorites, art=""):
    games = [
        FavGame(Entry(title=f"{name} {i:04d}", path=f"/games/{name}/{i}.rom",
                      assets={"boxFront": art} if art else {}),
                favorite=(i in favorites))
        for i in range(count)
    ]
    out = StubCollection(name, games)
    # Pegasus parents every Game to its ApiObject, which is what keeps QML
    # from taking ownership of them and letting the garbage collector delete
    # them out from under the model once a scan has touched all 8302.
    for game in games:
        game.setParent(out)
    out._games = CountingModel(games)
    return out


def press(view, key, text=None):
    """One key, the way a keyboard sends it.

    The text matters: search mode appends `event.text`, so an event built
    without it looks like a broken search rather than a broken test.
    """
    if text is None:
        text = chr(key).lower() if Qt.Key.Key_A <= key <= Qt.Key.Key_Z else ""
    for kind in (QEvent.Type.KeyPress, QEvent.Type.KeyRelease):
        QCoreApplication.sendEvent(
            view, QKeyEvent(kind, key, Qt.KeyboardModifier.NoModifier, text))


def settle(app, view):
    """Delegates are created when the scene is rendered, not when it changes.

    Without the grab, the view has a count and no items, and every check that
    reads the visible order silently passes on an empty list.
    """
    for _ in range(2):
        app.processEvents()
        view.grabWindow()
        app.processEvents()


def visible_rows(root):
    """The delegates on screen, in the order they are laid out in."""
    view = root.property("view")
    found = []
    for item in view.property("contentItem").childItems():
        game = item.property("game")
        if game is not None:
            found.append((round(item.property("y")), round(item.property("x")), item))
    found.sort(key=lambda entry: (entry[0], entry[1]))
    return [item for _, _, item in found]


def visible_titles(root, count):
    return [item.property("game").title for item in visible_rows(root)[:count]]


def starred(item):
    """Whether this delegate is showing the favourite marker.

    Searched for by its glyph rather than by id: the list draws the star in
    the row itself and the grid draws it inside GameArt, and the point of the
    check is that both end up with one.
    """
    if item.property("text") == "★" and item.property("visible"):
        return True
    return any(starred(child) for child in item.childItems())


def main() -> int:
    app = QGuiApplication(sys.argv)

    cover = fake_cover(Path("/tmp/padmap-check-cover.png"))
    arcade = collection("Arcade", 300, {7, 100, 250})
    cube = collection("GameCube", 12, {5}, art=cover)
    n64 = collection("N64", 8, set())

    api = StubApi([arcade, cube, n64])
    # And the collections belong to the api for the same reason the games
    # belong to their collection: model::ApiObject parents both, and a
    # parentless one handed to QML is the engine's to delete. Without this the
    # third tab visited comes up empty, long after the code that caused it ran.
    for entry in (arcade, cube, n64):
        entry.setParent(api)
    # Pegasus binds PAGE_DOWN to Page Down and R2; the stub Keys in
    # preview_library predates the favourite key and does not answer for it.
    api._keys = FavKeys()

    view = QQuickView()
    view.rootContext().setContextProperty("api", api)
    view.setSource(QUrl.fromLocalFile(str(THEME / "Library.qml")))
    if view.status() != QQuickView.Status.Ready:
        for error in view.errors():
            print("  QML error:", error.toString())
        return 1
    view.setResizeMode(QQuickView.ResizeMode.SizeRootObjectToView)
    view.resize(1280, 720)

    root = view.rootObject()
    # theme.qml declares `Library { focus: true }`; without it nothing has
    # activeFocus and no key is delivered anywhere.
    root.setProperty("focus", True)
    view.show()
    settle(app, view)

    print("favourites first")
    check("the collection's favourites are counted",
          root.property("favoriteCount"), 3)
    check("they are the first rows, in collection order",
          visible_titles(root, 4),
          ["Arcade 0007", "Arcade 0100", "Arcade 0250", "Arcade 0000"])
    check("and the rest keep their order behind them",
          visible_titles(root, 6)[3:],
          ["Arcade 0000", "Arcade 0001", "Arcade 0002"])
    check("the favourites are the marked rows",
          [starred(item) for item in visible_rows(root)[:4]],
          [True, True, True, False])

    print("the browse model is still the collection's own")
    # The whole point of reordering inside a DelegateModel rather than sorting
    # a list of games: 8302 entries must never be copied into an array.
    check("shownModel is the ObjectListModel, not a copy",
          root.property("shownModel") is arcade.games, True)
    check("and it still has every game", root.property("shownCount"), 300)

    print("scanning for them is not repeated")
    reads = arcade.games.reads
    press(view, Qt.Key.Key_E)          # to GameCube
    press(view, Qt.Key.Key_Q)          # and back
    settle(app, view)
    check("the tab is back where it started", root.property("tab"), 0)
    # A rescan would be another 300 reads. The handful that remain are the
    # sampling collectionHasArt does, which is capped and cached separately.
    check("returning to a tab does not walk the collection again",
          arcade.games.reads - reads < 20, True)

    print("marking a game")
    for _ in range(5):
        press(view, Qt.Key.Key_Down)
    settle(app, view)
    before = root.property("view").property("currentIndex")
    marked = root.property("view").property("currentItem").property("game")
    check("the cursor is on an unmarked game", marked.favorite, False)

    press(view, Qt.Key.Key_PageDown)
    settle(app, view)
    check("the key writes Pegasus's own property", marked.favorite, True)
    check("the count goes up", root.property("favoriteCount"), 4)
    check("something says so", root.property("note") != "", True)

    print("and nothing moves while you are looking at it")
    check("the cursor is on the same row",
          root.property("view").property("currentIndex"), before)
    check("which is still the same game",
          root.property("view").property("currentItem").property("game") is marked,
          True)
    check("the top of the list is untouched",
          visible_titles(root, 3),
          ["Arcade 0007", "Arcade 0100", "Arcade 0250"])
    check("the row it is on shows the star now",
          starred(root.property("view").property("currentItem")), True)

    print("it moves up the next time the tab is opened")
    reads = arcade.games.reads
    press(view, Qt.Key.Key_E)
    press(view, Qt.Key.Key_Q)
    settle(app, view)
    check("a toggle does make the next visit rescan",
          arcade.games.reads - reads > 200, True)
    check("the marked game is now at the top",
          marked.title in visible_titles(root, 4), True)
    check("in collection order with the others",
          visible_titles(root, 4),
          ["Arcade 0002", "Arcade 0007", "Arcade 0100", "Arcade 0250"])

    print("unmarking puts it back")
    settle(app, view)
    press(view, Qt.Key.Key_PageDown)   # cursor is on the first row: Arcade 0005
    settle(app, view)
    check("the property is cleared", marked.favorite, False)
    press(view, Qt.Key.Key_E)
    press(view, Qt.Key.Key_Q)
    settle(app, view)
    check("and it is out of the top block on the next visit",
          visible_titles(root, 4),
          ["Arcade 0007", "Arcade 0100", "Arcade 0250", "Arcade 0000"])

    print("the key does not collide")
    launched = [game for game in [marked] if game.launched]
    check("marking does not launch anything", launched, [])
    press(view, Qt.Key.Key_F)
    press(view, Qt.Key.Key_PageDown)
    settle(app, view)
    # Page Down is a key, not a letter: it must neither type nor toggle while
    # a query is being written.
    check("it does nothing at all while searching",
          root.property("query"), "")
    press(view, Qt.Key.Key_Escape)
    settle(app, view)
    check("nothing was marked by it", root.property("favoriteCount"), 3)

    print("search results are ordered too")
    press(view, Qt.Key.Key_F)
    for key in (Qt.Key.Key_0, Qt.Key.Key_0):
        press(view, key, text="0")
    press(view, Qt.Key.Key_Return, text="\r")
    settle(app, view)
    # "00" matches Arcade 0000-0099 plus the 0100/0200 decades; the favourites
    # among them have to lead.
    check("the matches are filtered", root.property("shownCount") < 300, True)
    check("favourites lead the results",
          visible_titles(root, 2), ["Arcade 0007", "Arcade 0100"])
    press(view, Qt.Key.Key_Escape)
    settle(app, view)

    print("the cover grid does it too")
    press(view, Qt.Key.Key_E)
    settle(app, view)
    check("this collection is shown as covers", root.property("artMode"), True)
    check("its favourite is the first tile",
          visible_titles(root, 2), ["GameCube 0005", "GameCube 0000"])
    check("and the tile is badged",
          [starred(item) for item in visible_rows(root)[:2]], [True, False])

    print("a collection with no favourites is left alone")
    press(view, Qt.Key.Key_E)
    settle(app, view)
    check("nothing is marked", root.property("favoriteCount"), 0)
    check("and the order is the collection's",
          visible_titles(root, 3), ["N64 0000", "N64 0001", "N64 0002"])

    print()
    if FAILURES:
        print(f"{len(FAILURES)} failure(s): {', '.join(FAILURES)}")
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
