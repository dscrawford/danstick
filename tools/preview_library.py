"""Show the library screen on screen, with no daemon and no Pegasus.

The point is to look at it. Tab bar spacing, cover proportions and the
no-artwork stand-in are all layout, and layout is only wrong in a way you can
see -- a plate whose initials collide with the title, or a tab strip that
wraps at 1080p, costs a rebuild and a front-end restart to find otherwise.

    python3 tools/preview_library.py                 # real playlists, real art
    python3 tools/preview_library.py --art           # pretend every game has a cover
    python3 tools/preview_library.py --shot out.png  # write a picture instead

Reads the exported collections when they exist so the counts and titles are
the real ones, including all 8302 arcade entries -- which is the only way to
find out whether scrolling the grid actually stays smooth.
"""

import sys
from pathlib import Path

from PySide6.QtCore import (
    Property, QAbstractListModel, QByteArray, QModelIndex, QObject, Qt,
    QTimer, QUrl, Signal, Slot,
)
from PySide6.QtGui import QGuiApplication
from PySide6.QtQuick import QQuickView

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import pegasus  # noqa: E402
from padmap import titles as titles_mod  # noqa: E402

THEME = REPO / "pegasus" / "theme"


class StubAssets(QObject):
    def __init__(self, box=""):
        super().__init__()
        self._box = box

    @Property(str, constant=True)
    def boxFront(self):
        return self._box

    @Property(str, constant=True)
    def titlescreen(self):
        return ""

    @Property(str, constant=True)
    def screenshot(self):
        return ""


class StubGame(QObject):
    def __init__(self, entry):
        super().__init__()
        self._entry = entry
        self._assets = StubAssets(entry.assets.get("boxFront", ""))
        self.launched = False

    @Property(str, constant=True)
    def title(self):
        return self._entry.title

    @Property(int, constant=True)
    def releaseYear(self):
        try:
            return int(self._entry.year)
        except ValueError:
            return 0

    @Property(str, constant=True)
    def developer(self):
        return self._entry.manufacturer

    @Property(QObject, constant=True)
    def assets(self):
        return self._assets

    @Slot()
    def launch(self):
        self.launched = True
        print(f"launch: {self._entry.title}")


class ObjectListModel(QAbstractListModel):
    """What Pegasus hands the theme: a model whose only role is `modelData`.

    Reimplemented rather than faked with a Python list because the theme
    deliberately avoids copying 8302 games into a JS array, and a plain list
    has no `count` or `get()`. Getting that wrong here would hide exactly the
    performance problem this preview exists to expose.
    """

    countChanged = Signal()

    def __init__(self, items):
        super().__init__()
        self._items = items

    def roleNames(self):
        return {Qt.ItemDataRole.UserRole: QByteArray(b"modelData")}

    def rowCount(self, parent=QModelIndex()):
        return len(self._items)

    def data(self, index, role=Qt.ItemDataRole.UserRole):
        if not index.isValid() or role != Qt.ItemDataRole.UserRole:
            return None
        return self._items[index.row()]

    @Property(int, notify=countChanged)
    def count(self):
        return len(self._items)

    @Slot(int, result=QObject)
    def get(self, row):
        return self._items[row] if 0 <= row < len(self._items) else None


class StubCollection(QObject):
    def __init__(self, name, games):
        super().__init__()
        self._name = name
        self._games = ObjectListModel(games)

    @Property(str, constant=True)
    def name(self):
        return self._name

    @Property(str, constant=True)
    def shortName(self):
        return self._name.lower()

    @Property(QObject, constant=True)
    def games(self):
        return self._games


class StubKeys(QObject):
    """Pegasus's default bindings, only the ones the library asks about."""

    def _is(self, event, keys):
        return event.property("key") in keys

    @Slot(QObject, result=bool)
    def isAccept(self, event):
        return self._is(event, (Qt.Key.Key_Return, Qt.Key.Key_Enter))

    @Slot(QObject, result=bool)
    def isCancel(self, event):
        return self._is(event, (Qt.Key.Key_Escape,))

    @Slot(QObject, result=bool)
    def isDetails(self, event):
        return self._is(event, (Qt.Key.Key_I,))

    @Slot(QObject, result=bool)
    def isFilters(self, event):
        return self._is(event, (Qt.Key.Key_F,))

    @Slot(QObject, result=bool)
    def isPrevPage(self, event):
        return self._is(event, (Qt.Key.Key_Q,))

    @Slot(QObject, result=bool)
    def isNextPage(self, event):
        return self._is(event, (Qt.Key.Key_E,))


class StubApi(QObject):
    gamedataReady = Signal()

    def __init__(self, collections):
        super().__init__()
        self._keys = StubKeys()
        self._collections = ObjectListModel(collections)

    @Property(QObject, constant=True)
    def keys(self):
        return self._keys

    @Property(QObject, constant=True)
    def collections(self):
        return self._collections


# A single tiny PNG stands in for a real thumbnail pack under --art. Drawing
# one is cheaper than depending on a downloaded pack that is not here, and the
# question --art answers is about layout, not about the picture.
def fake_cover(path: Path) -> str:
    from PySide6.QtGui import QColor, QLinearGradient, QPainter, QPixmap

    pixmap = QPixmap(300, 420)
    painter = QPainter(pixmap)
    gradient = QLinearGradient(0, 0, 300, 420)
    gradient.setColorAt(0, QColor("#4a5568"))
    gradient.setColorAt(1, QColor("#1f2430"))
    painter.fillRect(0, 0, 300, 420, gradient)
    painter.setPen(QColor("#c9ccd6"))
    painter.drawRect(10, 10, 279, 399)
    painter.end()
    pixmap.save(str(path))
    return str(path)


def apply_art(entries, cover: str | None, every: int) -> None:
    """Give some fraction of the entries a cover.

    `every == 1` is a fully illustrated collection; larger values fake a pack
    with holes in it, which is the realistic case -- arcade packs are keyed by
    MAME set name and miss the clones.
    """
    if not cover:
        return
    for n, entry in enumerate(entries):
        entry.assets = {"boxFront": cover} if n % every == 0 else {}


def load_collections(force_art: str | None, every: int = 1):
    playlists = Path.home() / ".config" / "retroarch" / "playlists"
    out = []
    # Same title table the export uses, so the arcade tab shows "10-Yard
    # Fight" rather than "10yard". Set names are short and uniform, and a
    # screen full of them hides how the layout copes with real titles.
    titles = titles_mod.find_titles()
    if playlists.is_dir():
        for lpl in sorted(playlists.glob("*.lpl")):
            collection = pegasus.read_playlist(lpl, titles)
            if collection is None or not collection.entries:
                continue
            apply_art(collection.entries, force_art, every)
            out.append(StubCollection(
                collection.name,
                [StubGame(e) for e in collection.entries],
            ))

    if out:
        return out

    # No playlists on this machine: still worth being able to look at the
    # screen, so fall back to something with the same shape.
    from padmap.pegasus import Entry
    demo = [
        ("Arcade", ["Street Fighter II", "Donkey Kong", "1942", "Galaga"]),
        ("Nintendo 64", ["Banjo-Kazooie", "The Legend of Zelda", "GoldenEye"]),
    ]
    for name, names in demo:
        entries = [Entry(title=t, path=f"/games/{t}.rom") for t in names]
        apply_art(entries, force_art, every)
        out.append(StubCollection(name, [StubGame(e) for e in entries]))
    return out


def take(argv: list[str], flag: str, fallback: str) -> tuple[list[str], str | None]:
    """Pull `--flag value` out of argv, returning what is left."""
    if flag not in argv:
        return argv, None
    at = argv.index(flag)
    value = argv[at + 1] if at + 1 < len(argv) else fallback
    return argv[:at] + argv[at + 2:], value


def main() -> int:
    argv = sys.argv[1:]
    argv, shot = take(argv, "--shot", "library.png")
    argv, tab = take(argv, "--tab", "0")
    argv, search = take(argv, "--search", "")
    argv, mix = take(argv, "--mix", "3")

    app = QGuiApplication(sys.argv)

    art = None
    if "--art" in argv or mix is not None:
        art = fake_cover(Path("/tmp/padmap-preview-cover.png"))

    api = StubApi(load_collections(art, int(mix) if mix else 1))

    view = QQuickView()
    view.rootContext().setContextProperty("api", api)
    view.setSource(QUrl.fromLocalFile(str(THEME / "Library.qml")))
    if view.status() != QQuickView.Status.Ready:
        for error in view.errors():
            print("  QML error:", error.toString())
        return 1

    view.setResizeMode(QQuickView.ResizeMode.SizeRootObjectToView)
    view.resize(1280, 720)
    view.setTitle("padmap library preview")

    root = view.rootObject()
    # theme.qml declares `Library { focus: true }`; without it nothing in the
    # screen has activeFocus and no key does anything.
    root.setProperty("focus", True)
    if tab:
        root.setProperty("tab", int(tab))
    if search:
        # Set through the property rather than by synthesising key presses:
        # the screenshot needs the *result*, and the input path is exercised
        # by using the preview interactively.
        root.setProperty("query", search)
        root.metaObject().invokeMethod(root, "rebuild")

    if shot:
        view.show()

        def grab():
            # Covers decode on a worker thread, so a grab taken the instant
            # the scene is up catches a screen of empty tiles.
            view.grabWindow().save(shot)
            print(f"wrote {shot}")
            app.quit()

        QTimer.singleShot(1500, grab)
        return app.exec()

    print("q/e: switch console   arrows: move   up from top row: tabs   "
          "f: search   esc: quit")
    view.show()
    return app.exec()


if __name__ == "__main__":
    sys.exit(main())
