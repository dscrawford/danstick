"""Show the library screen with favourites, with no daemon and no Pegasus.

preview_library.py cannot show any of this: its stub games have no `favorite`
property at all, which is deliberate -- it is how the library screen is kept
honest about surviving a front-end that does not implement one. So the stubs
grow a real, writable, notifying `favorite` here instead, and the screen is
driven the way a person would drive it.

    python3 tools/preview_favorites.py                       # the cover grid
    python3 tools/preview_favorites.py --noart               # the dense list
    python3 tools/preview_favorites.py --shot out.png        # write a picture
    python3 tools/preview_favorites.py --toggle --shot x.png # press the key first

What the pictures are for: whether a star is findable at TV distance, whether
the badge lands on the artwork rather than beside it, and whether the
favourites really are the rows at the top.
"""

import os
import sys

# Before PySide6, always: a preview that renders to a file must not depend on
# the caller having exported the platform plugin, and a preview that opens a
# window has to be able to override this.
if "--shot" in sys.argv:
    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from pathlib import Path  # noqa: E402

from PySide6.QtCore import (  # noqa: E402
    Property, QEvent, Qt, QTimer, QUrl, Signal, Slot,
)
from PySide6.QtCore import QCoreApplication  # noqa: E402
from PySide6.QtGui import QGuiApplication, QKeyEvent  # noqa: E402
from PySide6.QtQuick import QQuickView  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "tools"))
sys.path.insert(0, str(REPO / "src"))

from preview_library import (  # noqa: E402
    StubApi, StubCollection, StubGame, StubKeys, apply_art, fake_cover, take,
)

from padmap import pegasus  # noqa: E402
from padmap import titles as titles_mod  # noqa: E402

THEME = REPO / "pegasus" / "theme"


class FavGame(StubGame):
    """A stub game with the property Pegasus actually exposes.

    `favorite` is read/write with a change signal, exactly as model::Game
    declares it, so a binding on it updates the moment the theme assigns --
    which is the behaviour the star depends on.
    """

    favoriteChanged = Signal()

    def __init__(self, entry, favorite=False):
        super().__init__(entry)
        self._favorite = favorite

    def _get_favorite(self):
        return self._favorite

    def _set_favorite(self, value):
        if self._favorite == value:
            return
        self._favorite = value
        self.favoriteChanged.emit()

    favorite = Property(bool, _get_favorite, _set_favorite,
                        notify=favoriteChanged)


class FavKeys(StubKeys):
    """Pegasus binds Page Down / R2 to PAGE_DOWN; the theme favourites on it."""

    @Slot("QVariant", result=bool)
    def isPageDown(self, event):
        return self._is(event, (Qt.Key.Key_PageDown,))


def own(collection, games):
    """Parent the games to their collection, the way Pegasus does.

    model::ApiObject::setGameData calls `game->setParent(this)` on every game,
    which is what stops QML from taking ownership of them. A parentless QObject
    handed to QML from an invokable gets JavaScriptOwnership instead, and the
    engine's garbage collector is then free to delete it -- which it does, once
    something walks a collection big enough to trigger a collection. The
    library screen walks all 8302 arcade games looking for favourites, so
    without this the model quietly empties out halfway through the first
    screenshot and every row renders blank.
    """
    for game in games:
        game.setParent(collection)
    return collection


def load_collections(force_art, every, favorite_every, noart=False):
    """The real playlists when they exist, with some entries pre-favourited.

    Spread through the collection rather than clustered at the front, so a
    screenshot of the top of the list is evidence the reordering happened
    instead of evidence that the first rows were favourites already.
    """
    playlists = Path.home() / ".config" / "retroarch" / "playlists"
    titles = titles_mod.find_titles()
    out = []

    if playlists.is_dir():
        for lpl in sorted(playlists.glob("*.lpl")):
            collection = pegasus.read_playlist(lpl, titles)
            if collection is None or not collection.entries:
                continue
            apply_art(collection.entries, force_art, every)
            if noart:
                # The exported playlists carry thumbnail paths whether or not
                # the files exist, so "no art" cannot be had by leaving the
                # cover out -- the collection still claims to have one and the
                # screen picks the grid. This is the only way to look at the
                # list view, which is what this machine actually shows today.
                for entry in collection.entries:
                    entry.assets = {}
            games = [
                FavGame(entry, favorite=(n % favorite_every == favorite_every // 2))
                for n, entry in enumerate(collection.entries)
            ]
            out.append(own(StubCollection(collection.name, games), games))

    if out:
        return out

    from padmap.pegasus import Entry
    demo = [
        ("Arcade", ["Street Fighter II", "Donkey Kong", "1942", "Galaga",
                    "Bubble Bobble", "Metal Slug", "Rainbow Islands"]),
        ("Nintendo 64", ["Banjo-Kazooie", "The Legend of Zelda", "GoldenEye",
                         "Mario Kart 64", "Perfect Dark"]),
    ]
    for name, names in demo:
        entries = [Entry(title=t, path=f"/games/{t}.rom") for t in names]
        apply_art(entries, force_art, every)
        games = [FavGame(e, favorite=(n % favorite_every == favorite_every // 2))
                 for n, e in enumerate(entries)]
        out.append(own(StubCollection(name, games), games))
    return out


def press(view, key, text=""):
    for kind in (QEvent.Type.KeyPress, QEvent.Type.KeyRelease):
        QCoreApplication.sendEvent(
            view, QKeyEvent(kind, key, Qt.KeyboardModifier.NoModifier, text))


def main() -> int:
    argv = sys.argv[1:]
    argv, shot = take(argv, "--shot", "favorites.png")
    argv, tab = take(argv, "--tab", "0")
    argv, search = take(argv, "--search", "")
    argv, mix = take(argv, "--mix", "3")
    # One in this many games starts out a favourite. The default is chosen so
    # the arcade collection ends up with a plausible number rather than
    # hundreds -- a screen that is all stars says nothing about whether a star
    # stands out.
    argv, every = take(argv, "--every", "900")
    toggle = "--toggle" in argv

    app = QGuiApplication(sys.argv)

    art = None
    if "--art" in argv or mix is not None:
        art = fake_cover(Path("/tmp/padmap-preview-cover.png"))

    collections = load_collections(art, int(mix) if mix else 1,
                                   int(every) if every else 900,
                                   noart="--noart" in argv)
    api = StubApi(collections)
    # The collections belong to the api for the same reason the games belong
    # to their collection -- see own(). model::ApiObject parents both.
    for entry in collections:
        entry.setParent(api)
    # StubApi builds its own keys; swap them before QML reads the property,
    # which it does once because the property is constant.
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
    view.setTitle("padmap favourites preview")

    root = view.rootObject()
    root.setProperty("focus", True)
    if tab:
        root.setProperty("tab", int(tab))
    if search:
        root.setProperty("query", search)
        root.rebuild()

    view.show()
    # Key events go nowhere until the window is up and something has focus.
    app.processEvents()

    if toggle:
        # Down past the games that are favourites already, so the picture
        # shows one being marked in the middle of the list -- and shows that
        # the row it was on did not move out from under the cursor.
        for _ in range(12):
            press(view, Qt.Key.Key_Down)
        press(view, Qt.Key.Key_PageDown)

    if shot:
        def grab():
            # Covers decode on a worker thread; a grab taken immediately
            # catches a screen of empty tiles.
            view.grabWindow().save(shot)
            print(f"wrote {shot}")
            app.quit()

        QTimer.singleShot(1500, grab)
        return app.exec()

    print("q/e: switch console   arrows: move   page down: favourite   "
          "f: search   esc: quit")
    return app.exec()


if __name__ == "__main__":
    sys.exit(main())
