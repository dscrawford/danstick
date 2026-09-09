"""Where do the library screen's keys go, and which games are marked broken?

Two halves, both invisible in a screenshot and neither covered until now.

The routing. `I` (Details) and `M` are the only two keys in the library that
leave the screen: one opens the Controller Order screen (S2), the other asks
to map a pad for the game under the cursor (S10). Everything about them is
order-sensitive. `M` is a raw key rather than one of Pegasus's named actions,
so it is checked *after* every rebindable `api.keys` gesture -- someone who
has bound Filters to `M` must get their search field, not a mapping wizard.
It is checked *before* Accept, so it maps rather than launching. And it has to
be inert while the search field is open, where `m` is a letter. Each of those
is one `if` in a chain of eight, and getting the order wrong loses a feature
silently, in a way only the person who rebound their keys would ever see.
Every branch also has to accept the event, or the key does its job *and*
moves the cursor in the grid underneath.

The marking. `x-mame-status` arrives in `game.extra`, and "preliminary" --
MAME's own grade behind its red "THIS GAME DOES NOT WORK" screen -- dims the
title and adds a "not working" pill. Everything else, including no grade at
all, has to read as fine: almost nothing outside arcade carries a grade, and
marking a whole console library as broken would be worse than saying nothing.
Checked in both shapes, because Pegasus stores every `x-` field as a
QStringList and hands the theme `["preliminary"]`, not `"preliminary"` -- the
difference that kept the marking from ever appearing on the real box.

Loaded the way the front-end loads it: the real Library.qml against a stub
`api`. No daemon, no Pegasus, and no controller touched.

    QT_QPA_PLATFORM=offscreen python3 tools/check_theme_routing.py
"""

import os
import sys
import tempfile

# Before PySide6 is imported, and not left to the caller: Qt otherwise binds
# to whatever display happens to be around, and the same check then passes or
# fails depending on where it was run from.
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

# Nothing in this file wants the real box's state, but the last section
# imports tools/preview_library.py to check that the preview stubs the shape
# production sends, and that pulls in padmap, which resolves its config and
# thumbnail paths off exactly these. Pointed at a scratch directory first, so
# a check can never read or write the live machine's configuration.
_SANDBOX = tempfile.mkdtemp(prefix="padmap-theme-routing-")
for _var in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_RUNTIME_DIR"):
    os.environ[_var] = _SANDBOX

from pathlib import Path  # noqa: E402

from PySide6.QtCore import (  # noqa: E402
    Property, QAbstractListModel, QByteArray, QCoreApplication, QEvent,
    QMetaObject, QModelIndex, QObject, Q_ARG, Q_RETURN_ARG, Qt, QUrl, Signal,
    Slot,
)
from PySide6.QtGui import QGuiApplication, QKeyEvent  # noqa: E402
from PySide6.QtQuick import QQuickView  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
THEME = REPO / "pegasus" / "theme"

DIRECT = Qt.ConnectionType.DirectConnection
NO_MOD = Qt.KeyboardModifier.NoModifier

# A real file, so the collections that stand in for an illustrated library
# are drawn as covers rather than as rows.
COVER = str(THEME / "icons" / "arcade.svg")


# -- the promise, and what it costs when it breaks --------------------------

def same(what: str, got, want, why: str = "") -> None:
    if got != want:
        raise SystemExit(
            f"FAIL: {what} -- got {got!r}, wanted {want!r}"
            + (f". {why}" if why else ""))
    print(f"  ok  {what}")


def truth(what: str, got, why: str = "") -> None:
    same(what, bool(got), True, why)


# -- stubs for the two objects Pegasus puts in front of the theme -----------
#
# Written here rather than imported from preview_library.py, because this file
# needs shapes that one does not have: extras carried as Pegasus's own string
# lists, a game with no `extra` at all, and key gestures that can be rebound
# in the middle of a run.

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


class BareGame(QObject):
    """A Pegasus Game with no `extra` map at all.

    Not a hypothetical: `extra` is the theme's only optional input, and a
    front-end -- or a collection file -- without the `x-` fields answers
    `undefined` for it. `driverStatus` guards for exactly that, and the guard
    has never been exercised.
    """

    favoriteChanged = Signal()

    def __init__(self, title, art=""):
        super().__init__()
        self._title = title
        self._assets = StubAssets(art)
        self._favorite = False
        self.launched = 0

    @Property(str, constant=True)
    def title(self):
        return self._title

    @Property(int, constant=True)
    def releaseYear(self):
        return 1997

    @Property(str, constant=True)
    def developer(self):
        return "Rare"

    @Property(QObject, constant=True)
    def assets(self):
        return self._assets

    def _get_favorite(self):
        return self._favorite

    def _set_favorite(self, value):
        self._favorite = value
        self.favoriteChanged.emit()

    # Writable, because the favourite key writes it and one of the routing
    # checks below rebinds that key onto `M`.
    favorite = Property(bool, _get_favorite, _set_favorite,
                        notify=favoriteChanged)

    @Slot()
    def launch(self):
        self.launched += 1


class StubGame(BareGame):
    """The usual game: `x-console`, `x-gamekey` and `x-mame-status` in `extra`."""

    def __init__(self, title, extra=None, art=""):
        super().__init__(title, art=art)
        self._extra = dict(extra or {})

    @Property("QVariantMap", constant=True)
    def extra(self):
        """Pegasus's `x-` passthrough: `x-console` arrives here as `console`."""
        return self._extra


class ObjectListModel(QAbstractListModel):
    """What Pegasus hands the theme: a model whose only role is `modelData`.

    Not a plain list. The theme deliberately never copies the games into a JS
    array, so it reads `count` and `get()` off the model itself.
    """

    countChanged = Signal()

    def __init__(self, items):
        super().__init__()
        self._items = items
        # Parented exactly as Pegasus parents them (`game->setParent(this)`).
        # Without a C++ parent QML takes ownership and the garbage collector
        # is free to delete a game mid-run, which shows up as blank rows and
        # searches that match nothing -- a bug in the harness that looks
        # exactly like a bug in the theme.
        for item in items:
            item.setParent(self)

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
        self.entries = games
        self._games = ObjectListModel(games)
        self._games.setParent(self)

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
    """Pegasus's key gestures, rebindable the way a user can rebind them.

    Every one of these is a *user setting* in the real front-end, which is the
    whole reason the library asks about all of them before its own raw `M`.
    `bind` starts on Pegasus's defaults and the checks below move `M` onto one
    gesture at a time.
    """

    def __init__(self):
        super().__init__()
        self.bind = {
            "accept": [Qt.Key.Key_Return, Qt.Key.Key_Enter],
            "cancel": [Qt.Key.Key_Escape],
            "details": [Qt.Key.Key_I],
            "filters": [Qt.Key.Key_F],
            "prevpage": [Qt.Key.Key_Q],
            "nextpage": [Qt.Key.Key_E],
            "pagedown": [Qt.Key.Key_PageDown],
        }

    def _is(self, event, name):
        return event.property("key") in self.bind[name]

    @Slot("QVariant", result=bool)
    def isAccept(self, event):
        return self._is(event, "accept")

    @Slot("QVariant", result=bool)
    def isCancel(self, event):
        return self._is(event, "cancel")

    @Slot("QVariant", result=bool)
    def isDetails(self, event):
        return self._is(event, "details")

    @Slot("QVariant", result=bool)
    def isFilters(self, event):
        return self._is(event, "filters")

    @Slot("QVariant", result=bool)
    def isPrevPage(self, event):
        return self._is(event, "prevpage")

    @Slot("QVariant", result=bool)
    def isNextPage(self, event):
        return self._is(event, "nextpage")

    @Slot("QVariant", result=bool)
    def isPageDown(self, event):
        return self._is(event, "pagedown")


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


class Watcher(QObject):
    """Everything the library asked the theme root to do."""

    def __init__(self, root):
        super().__init__()
        self.setups = 0
        self.mappings = []
        root.openControllerSetup.connect(self._setup)
        root.openMappingFor.connect(self._mapping)

    def _setup(self):
        self.setups += 1

    def _mapping(self, console, key, title):
        self.mappings.append((console, key, title))

    def take(self):
        """What has been asked for since the last look, and reset."""
        out = (self.setups, list(self.mappings))
        self.setups = 0
        self.mappings.clear()
        return out


# -- driving the screen -----------------------------------------------------

def settle(app, view):
    """Delegates are built when the scene renders, not when the model changes.

    Without the grab the view has a count and no items, and every check that
    reads a row silently passes on an empty list.
    """
    for _ in range(2):
        app.processEvents()
        view.grabWindow()
        app.processEvents()


def press(app, view, key, text=None, mods=NO_MOD):
    """One key the way a keyboard sends it; True if the screen took it.

    The text matters: search mode appends `event.text`, so an event built
    without it reports every letter as unhandled and the query stays empty --
    which looks exactly like a broken search rather than a broken harness.
    """
    if text is None:
        text = chr(key).lower() if Qt.Key.Key_A <= key <= Qt.Key.Key_Z else ""
    event = QKeyEvent(QEvent.Type.KeyPress, key, mods, text)
    QCoreApplication.sendEvent(view, event)
    taken = event.isAccepted()
    QCoreApplication.sendEvent(
        view, QKeyEvent(QEvent.Type.KeyRelease, key, mods, text))
    app.processEvents()
    return taken


def call(root, name, *args):
    """Call one of the theme's own functions and get its answer back."""
    return QMetaObject.invokeMethod(
        root, name, DIRECT, Q_RETURN_ARG("QVariant"),
        *[Q_ARG("QVariant", arg) for arg in args])


def go_to_tab(app, view, root, index):
    """Switch console the way the tab bar does, then let the rows appear."""
    call(root, "selectTab", index)
    settle(app, view)


def walk(item):
    yield item
    for child in item.childItems():
        yield from walk(child)


def rows(root):
    """The delegates currently on screen, whichever of the two views is live."""
    live = root.property("view")
    return [item for item in live.property("contentItem").childItems()
            if item.property("game") is not None]


def row_for(root, title):
    for item in rows(root):
        if item.property("game").title == title:
            return item
    have = [item.property("game").title for item in rows(root)]
    raise SystemExit(
        f"FAIL: {title!r} is not on screen, so nothing about how it is drawn "
        f"can be checked. On screen: {have}")


def label(item, text):
    """The one drawn label with this exact text, or None if there is none."""
    for node in walk(item):
        if node.property("text") == text:
            return node
    return None


def title_label(item, game_title):
    """The row's own title label, marked or not.

    Found by the title it contains rather than by id, because the marked form
    is the plain title with something added to it -- which is the thing being
    checked.
    """
    best = None
    for node in walk(item):
        text = node.property("text")
        if isinstance(text, str) and text.endswith(game_title):
            if best is None or len(text) >= len(best.property("text")):
                best = node
    if best is None:
        raise SystemExit(
            f"FAIL: nothing on screen draws the title {game_title!r}")
    return best


def art_of(item):
    for node in walk(item):
        if node.metaObject().indexOfProperty("dimmed") >= 0:
            return node
    raise SystemExit("FAIL: the tile has no artwork item that could be dimmed")


def footer(root):
    """The line at the bottom, which doubles as the transient message."""
    for node in walk(root):
        if node.property("text") is not None and node.parentItem() is root:
            return node
    raise SystemExit("FAIL: the screen has no key-hint line")


# -- the library the checks browse ------------------------------------------

def pegasus_extra(**fields):
    """The shape the real front-end delivers.

    `PegasusMetadata.cpp:444` builds a QStringList per `x-` key and inserts
    that into the game's extra map, so a theme reading `game.extra["console"]`
    is handed `["n64"]` and not `"n64"`.
    """
    return {key.replace("_", "-"): [value] for key, value in fields.items()}


N64, ARCADE, COVERS, UNKNOWN, EMPTY, REAL = range(6)


def build_api():
    collections = [
        StubCollection("Nintendo 64", [
            StubGame("GoldenEye 007 (USA)",
                     {"console": "n64", "gamekey": "n64/goldeneye-007-usa"}),
            StubGame("Banjo-Kazooie",
                     {"console": "n64", "gamekey": "n64/banjo-kazooie"}),
            # x-gamekey is omitted for an entry the exporter could compute no
            # key for. The console is still known, and a mapping for the
            # console is still worth offering.
            StubGame("Mystery Cart", {"console": "n64"}),
        ]),
        StubCollection("Arcade", [
            StubGame("Pac-Man", {"mame-status": "good", "console": "arcade",
                                 "gamekey": "arcade/puckman"}),
            StubGame("Cheeky Mouse", {"mame-status": "preliminary",
                                      "console": "arcade",
                                      "gamekey": "arcade/cheekyms"}),
            StubGame("Rally X", {"mame-status": "imperfect",
                                 "console": "arcade",
                                 "gamekey": "arcade/rallyx"}),
            # What nearly everything outside arcade looks like: no grade.
            StubGame("Super Mario Sunshine", {"console": "gamecube",
                                              "gamekey": "gamecube/sunshine"}),
            BareGame("Ancient Homebrew"),
        ]),
        StubCollection("Arcade Illustrated", [
            StubGame("Galaga", {"mame-status": "good", "console": "arcade",
                                "gamekey": "arcade/galaga"}, art=COVER),
            StubGame("Cosmic Guerilla",
                     {"mame-status": "preliminary", "console": "arcade",
                      "gamekey": "arcade/cosmicg"}, art=COVER),
        ]),
        StubCollection("Unknown Core", [
            # `pegasus.render` writes x-gamekey for every entry but omits
            # x-console when the collection's core is not one padmap knows.
            StubGame("Some Game", {"gamekey": "unknown/some-game"}),
            StubGame("Another Game", {"gamekey": "unknown/another-game"}),
        ]),
        StubCollection("Empty", []),
        StubCollection("As Pegasus Delivers It", [
            StubGame("Wave Race 64",
                     pegasus_extra(console="n64", gamekey="n64/wave-race-64")),
            StubGame("Cheeky Mouse II", pegasus_extra(mame_status="preliminary")),
            # The other grades in the same shape, so a fix for the list cannot
            # be a fix that marks everything: the whole arcade tab dimmed and
            # pilled would be worse than the bug it replaced.
            StubGame("Pac-Man II", pegasus_extra(mame_status="good",
                                                 console="arcade")),
            StubGame("Rally X II", pegasus_extra(mame_status="imperfect")),
            # A key that is present but carries nothing. `x-mame-status:` with
            # a blank value is one hand-edit away, and QStringList makes it an
            # empty list or a list holding an empty string rather than the
            # absent key the guard above was written for.
            StubGame("Blank Grade", {"mame-status": []}),
            StubGame("Empty Grade", {"mame-status": [""]}),
        ]),
    ]
    api = StubApi(collections)
    # The collections belong to the api for the same reason the games belong
    # to their collection: model::ApiObject parents both, and a parentless one
    # handed to QML is the engine's to delete -- which it does, several tabs
    # later, long after the code that caused it ran.
    for collection in collections:
        collection.setParent(api)
    return api, collections


def main() -> int:
    app = QGuiApplication(sys.argv)
    api, shelf = build_api()

    view = QQuickView()
    view.rootContext().setContextProperty("api", api)
    view.setSource(QUrl.fromLocalFile(str(THEME / "Library.qml")))
    if view.status() != QQuickView.Status.Ready:
        for error in view.errors():
            print("  QML error:", error.toString())
        raise SystemExit("FAIL: Library.qml did not load, so nothing below ran")
    view.setResizeMode(QQuickView.ResizeMode.SizeRootObjectToView)
    view.resize(1280, 720)

    root = view.rootObject()
    # theme.qml declares `Library { focus: true }`. Without it nothing has
    # activeFocus, no key is delivered anywhere, and every check below would
    # pass or fail for reasons that have nothing to do with what it tests.
    root.setProperty("focus", True)
    view.show()
    settle(app, view)

    watch = Watcher(root)
    keys = api._keys
    colors = next(child for child in root.findChildren(QObject)
                  if child.metaObject().indexOfProperty("danger") >= 0)
    danger = colors.property("danger")
    faint = colors.property("textFaint")
    plain = colors.property("text")

    # ---------------------------------------------------------------- S2 ---
    print("S2: Details opens the Controller Order screen")
    same("the screen comes up on the first collection", root.property("tab"), 0)
    taken = press(app, view, Qt.Key.Key_I)
    setups, mappings = watch.take()
    same("Details asks the theme to open it", setups, 1,
         "the only route from the library to the Controller Order screen")
    same("and asks for nothing else", mappings, [])
    truth("the key is taken, so the grid never sees it", taken,
          "an unaccepted Details would open the screen and move the cursor")

    print("\nS2: ...from the tab bar as well as from the games")
    press(app, view, Qt.Key.Key_Up)          # up out of the first row
    same("focus left the games",
         root.property("view").property("activeFocus"), False)
    press(app, view, Qt.Key.Key_I)
    setups, _ = watch.take()
    same("Details still opens it", setups, 1,
         "the key would work over a game and do nothing in the tab bar")
    press(app, view, Qt.Key.Key_Down)        # and back down to the games
    settle(app, view)
    truth("and the games have focus again",
          root.property("view").property("activeFocus"))

    # --------------------------------------------------------------- S10 ---
    print("\nS10: M asks to map a pad for the focused game")
    go_to_tab(app, view, root, N64)
    taken = press(app, view, Qt.Key.Key_M)
    setups, mappings = watch.take()
    same("one press asks once", len(mappings), 1,
         "one key press would open two wizards")
    same("it carries the console, the key and the title", mappings,
         [("n64", "n64/goldeneye-007-usa", "GoldenEye 007 (USA)")],
         "the mapping is filed under a scope nothing ever looks up")
    same("and it does not open the plain setup screen", setups, 0)
    truth("the key is taken", taken)
    same("the game is not launched by it", shelf[N64].entries[0].launched, 0,
         "M is checked before Accept precisely so that it maps, not launches")

    print("\nS10: it is the game under the cursor, not the first one")
    root.property("view").setProperty("currentIndex", 1)
    settle(app, view)
    press(app, view, Qt.Key.Key_M)
    _, mappings = watch.take()
    same("the second row is what gets mapped", mappings,
         [("n64", "n64/banjo-kazooie", "Banjo-Kazooie")],
         "a mapping made for whatever game happened to be first")

    print("\nS10: a game the exporter could compute no key for")
    root.property("view").setProperty("currentIndex", 2)
    settle(app, view)
    press(app, view, Qt.Key.Key_M)
    _, mappings = watch.take()
    same("the console mapping is still offered, with an empty key", mappings,
         [("n64", "", "Mystery Cart")],
         "a pad could not be mapped for a console because one game had no key")

    print("\nS10: the cover grid answers M the same way as the list")
    go_to_tab(app, view, root, COVERS)
    truth("this collection is drawn as covers", root.property("artMode"))
    press(app, view, Qt.Key.Key_M)
    _, mappings = watch.take()
    same("M maps the tile under the cursor", mappings,
         [("arcade", "arcade/galaga", "Galaga")],
         "the key works in one of the two views and not in the other")

    print("\nS10: a collection the exporter could not place")
    go_to_tab(app, view, root, UNKNOWN)
    taken = press(app, view, Qt.Key.Key_M)
    setups, mappings = watch.take()
    same("nothing is asked of the daemon", mappings, [],
         "the daemon would refuse it one screen later, with the pads grabbed")
    same("nor is the setup screen opened", setups, 0)
    same("the screen says what is missing", root.property("note"),
         "No console known for this game",
         "a key that silently does nothing reads as a broken key")
    same("and the message takes the place of the key hints",
         footer(root).property("text"), "No console known for this game")
    truth("the key is still taken", taken,
          "the refusal must not fall through to the grid underneath")

    print("\nS10: nothing under the cursor at all")
    go_to_tab(app, view, root, EMPTY)
    same("the collection is empty", root.property("shownCount"), 0)
    same("nothing is on the cursor",
         root.property("view").property("currentItem"), None)
    taken = press(app, view, Qt.Key.Key_M)
    setups, mappings = watch.take()
    same("M asks for nothing", (setups, mappings), (0, []))
    truth("and is still taken rather than falling through", taken)

    print("\nS10: a committed filter that matched nothing")
    go_to_tab(app, view, root, N64)
    press(app, view, Qt.Key.Key_F)
    for _ in range(3):
        press(app, view, Qt.Key.Key_Z)
    press(app, view, Qt.Key.Key_Return, text="\r")
    settle(app, view)
    same("the filter is committed and matches nothing",
         (root.property("searching"), root.property("shownCount")), (False, 0))
    press(app, view, Qt.Key.Key_M)
    setups, mappings = watch.take()
    same("M over an empty result asks for nothing", (setups, mappings), (0, []))

    # --------------------------------------------------- search is a mode ---
    print("\nS23: while the search field is open, M and I are letters")
    go_to_tab(app, view, root, N64)
    press(app, view, Qt.Key.Key_F)
    truth("Filters opened the search field", root.property("searching"))
    taken = press(app, view, Qt.Key.Key_M)
    setups, mappings = watch.take()
    same("M types an m", root.property("query"), "m",
         "a query with an m in it would open a mapping wizard mid-word")
    same("and asks for no mapping", mappings, [])
    truth("the letter is taken", taken)
    press(app, view, Qt.Key.Key_I)
    setups, mappings = watch.take()
    same("I types an i too", root.property("query"), "mi")
    same("and does not open the Controller Order screen", setups, 0,
         "typing 'mission' would throw the user onto another screen")
    press(app, view, Qt.Key.Key_Escape)
    settle(app, view)
    same("Escape leaves the mode and clears the filter",
         (root.property("searching"), root.property("query")), (False, ""))
    press(app, view, Qt.Key.Key_M)
    _, mappings = watch.take()
    same("and M maps again once the field is closed", len(mappings), 1)

    # ---------------------------------------------------------- modifiers --
    print("\nS10: M with a modifier on it is not this key")
    go_to_tab(app, view, root, N64)
    for name, mod in (("Ctrl", Qt.KeyboardModifier.ControlModifier),
                      ("Shift", Qt.KeyboardModifier.ShiftModifier),
                      ("Alt", Qt.KeyboardModifier.AltModifier)):
        taken = press(app, view, Qt.Key.Key_M, text="m", mods=mod)
        setups, mappings = watch.take()
        same(f"{name}+M asks for nothing", (setups, mappings), (0, []))
        same(f"{name}+M is left for whatever else wants it", taken, False)

    # -------------------------------------------------- the routing order --
    print("\nS10: a rebindable gesture bound to M wins over the raw key")
    # Every api.keys gesture is a user setting. The library asks about all of
    # them before its own M, so someone who has bound one to that letter gets
    # what they bound.
    go_to_tab(app, view, root, N64)
    keys.bind["filters"] = [Qt.Key.Key_M]
    press(app, view, Qt.Key.Key_M)
    _, mappings = watch.take()
    truth("Filters on M opens the search field", root.property("searching"),
          "the user's own binding would be shadowed by a raw key")
    same("and asks for no mapping", mappings, [])
    press(app, view, Qt.Key.Key_Escape)
    keys.bind["filters"] = [Qt.Key.Key_F]

    keys.bind["details"] = [Qt.Key.Key_M]
    press(app, view, Qt.Key.Key_M)
    setups, mappings = watch.take()
    same("Details on M opens the Controller Order screen", setups, 1)
    same("and asks for no mapping", mappings, [])
    keys.bind["details"] = [Qt.Key.Key_I]

    keys.bind["prevpage"] = [Qt.Key.Key_M]
    before = root.property("tab")
    press(app, view, Qt.Key.Key_M)
    settle(app, view)
    _, mappings = watch.take()
    same("Prev-page on M switches console", root.property("tab"),
         (before - 1) % root.property("collectionCount"))
    same("and asks for no mapping", mappings, [])
    keys.bind["prevpage"] = [Qt.Key.Key_Q]

    go_to_tab(app, view, root, N64)
    keys.bind["pagedown"] = [Qt.Key.Key_M]
    game = shelf[N64].entries[0]
    press(app, view, Qt.Key.Key_M)
    settle(app, view)
    _, mappings = watch.take()
    truth("the favourite key on M marks the game", game.favorite)
    same("and asks for no mapping", mappings, [])
    press(app, view, Qt.Key.Key_M)           # put it back
    settle(app, view)
    watch.take()
    same("unmarked again", game.favorite, False)
    keys.bind["pagedown"] = [Qt.Key.Key_PageDown]

    print("\nS10: ...but Accept does not, because M is asked about first")
    go_to_tab(app, view, root, N64)
    keys.bind["accept"] = [Qt.Key.Key_M]
    launched = game.launched
    press(app, view, Qt.Key.Key_M)
    _, mappings = watch.take()
    same("M still maps", len(mappings), 1,
         "binding Accept to M would silently cost the mapping key")
    same("and the game is not launched", game.launched, launched)
    keys.bind["accept"] = [Qt.Key.Key_Return, Qt.Key.Key_Enter]

    print("\nS23: a key the screen has no use for is left alone")
    taken = press(app, view, Qt.Key.Key_Backslash, text="\\")
    setups, mappings = watch.take()
    same("nothing happens", (setups, mappings), (0, []))
    same("and it is not accepted", taken, False,
         "if every key came back accepted, the checks above would prove "
         "nothing")

    print("\nS2/S10: both keys are named in the hints")
    # The transient note borrows the hint line for four seconds; the checks
    # above left one there, and this run is faster than the timer.
    root.setProperty("note", "")
    settle(app, view)
    hints = footer(root).property("text")
    truth("Details is offered by name", "Details controller order" in hints)
    truth("and the map key is named, since it cannot be named by action",
          "M map controller for this game" in hints,
          "a raw key nobody is told about has to be asked about")

    # --------------------------------------------------- the MAME marking --
    print("\nS23: MAME's driver grade, read off the game")
    good, broken, imperfect, ungraded, bare = shelf[ARCADE].entries
    same("a preliminary set reports its grade",
         call(root, "driverStatus", broken), "preliminary")
    truth("and is marked broken", call(root, "isBroken", broken),
          "MAME's own red 'THIS GAME DOES NOT WORK' set looks like any other")
    same("a good driver reports good", call(root, "driverStatus", good), "good")
    same("and is not marked", call(root, "isBroken", good), False,
         "a working game would be dimmed and pilled as broken")
    same("a game with no grade reports nothing",
         call(root, "driverStatus", ungraded), "")
    same("and is not marked", call(root, "isBroken", ungraded), False,
         "every console game would read as broken -- none carries a grade")
    same("a game with no extra map at all reports nothing",
         call(root, "driverStatus", bare), "")
    same("and is not marked", call(root, "isBroken", bare), False)
    same("nothing at all reports nothing", call(root, "driverStatus", None), "")
    same("and is not marked", call(root, "isBroken", None), False,
         "a delegate asks about its game before the model has seated one")
    # MAME's third grade. `titles.Title.working` counts it as working and
    # check_hostile_library.py pins that, so the theme has to agree: an
    # imperfect driver runs, it just has a flaw somewhere in it.
    same("an imperfect driver reports imperfect",
         call(root, "driverStatus", imperfect), "imperfect")
    same("and is deliberately not marked", call(root, "isBroken", imperfect),
         False, "a large part of the arcade library would read as unplayable")

    print("\nS23: the not-working pill on a row")
    go_to_tab(app, view, root, ARCADE)
    same("this collection is drawn as a list", root.property("artMode"), False)
    row = row_for(root, "Cheeky Mouse")
    pill = label(row, "not working")
    truth("the broken row carries a pill", pill is not None,
          "a dimmed title on its own is an unexplained puzzle")
    truth("it is on screen", pill.property("visible"))
    same("it says so in words", pill.property("text"), "not working",
         "an unlabelled glyph next to a dimmed title says nothing")
    same("in the danger colour", pill.property("color"), danger)
    truth("and it has a box around it", pill.parentItem().property("width") > 0)
    same("the broken title is dimmed",
         title_label(row, "Cheeky Mouse").property("color"), faint)

    print("\nS23: and every other row is left alone")
    for title in ("Pac-Man", "Rally X", "Super Mario Sunshine",
                  "Ancient Homebrew"):
        row = row_for(root, title)
        same(f"{title}: no pill",
             label(row, "not working").property("visible"), False)
        same(f"{title}: the pill takes no width either",
             label(row, "not working").parentItem().property("width"), 0.0,
             "the title would be indented around an invisible box")
        same(f"{title}: drawn under its own name",
             title_label(row, title).property("text"), title)
        same(f"{title}: at full strength",
             title_label(row, title).property("color"), plain)

    print("\nS23: the warning prefix on a cover")
    go_to_tab(app, view, root, COVERS)
    truth("this collection is drawn as covers", root.property("artMode"))
    tile = row_for(root, "Cosmic Guerilla")
    same("the broken tile is titled with a warning sign",
         title_label(tile, "Cosmic Guerilla").property("text"),
         "⚠ Cosmic Guerilla")
    same("in the danger colour",
         title_label(tile, "Cosmic Guerilla").property("color"), danger)
    truth("and the cover itself is dimmed", art_of(tile).property("dimmed"),
          "a set that does not run stays findable, so it is dimmed rather "
          "than hidden")
    tile = row_for(root, "Galaga")
    same("a working tile is titled with just its name",
         title_label(tile, "Galaga").property("text"), "Galaga")
    same("in the ordinary colour",
         title_label(tile, "Galaga").property("color"), plain)
    same("and its cover is not dimmed", art_of(tile).property("dimmed"), False)

    print("\nS23: the marking survives a search")
    go_to_tab(app, view, root, ARCADE)
    press(app, view, Qt.Key.Key_F)
    for key in (Qt.Key.Key_C, Qt.Key.Key_H, Qt.Key.Key_E):
        press(app, view, key)
    press(app, view, Qt.Key.Key_Return, text="\r")
    settle(app, view)
    same("the search found the broken set alone",
         (root.property("query"), root.property("shownCount")), ("che", 1))
    row = row_for(root, "Cheeky Mouse")
    truth("it is still marked in the results",
          label(row, "not working").property("visible"),
          "a game reached by searching would look like one that works")
    same("and still dimmed",
         title_label(row, "Cheeky Mouse").property("color"), faint)

    print("\nS23: ...and a switch to another console and back")
    go_to_tab(app, view, root, COVERS)
    go_to_tab(app, view, root, ARCADE)
    same("the query went with the tab", root.property("query"), "")
    truth("the pill is back with the whole collection",
          label(row_for(root, "Cheeky Mouse"), "not working").property("visible"))
    same("and the working set beside it still has none",
         label(row_for(root, "Pac-Man"), "not working").property("visible"),
         False)

    # ------------------------------------- the shape Pegasus really sends --
    print("\nS10/S23: the extra map as the real front-end delivers it")
    # PegasusMetadata.cpp:444 puts a QStringList in the extra map, so
    # `game.extra["console"]` is `["n64"]` in QML and not `"n64"`.
    go_to_tab(app, view, root, REAL)
    press(app, view, Qt.Key.Key_M)
    _, mappings = watch.take()
    same("M still names the console and the key", mappings,
         [("n64", "n64/wave-race-64", "Wave Race 64")],
         "openMappingFor declares both as `string`, so a one-entry list is "
         "flattened on the way out -- which is why S10 works at all")

    # The bug this half of the file was written to catch. Every assertion
    # above passes with `x-mame-status` carried as a bare string, which is
    # what preview_library.py used to build and what no front-end has ever
    # sent: PegasusMetadata.cpp stores each `x-` field as a QStringList, so
    # the theme is handed ["preliminary"] and `=== "preliminary"` is false.
    # For as long as that stood, no arcade set was marked in production while
    # every screenshot showed the pill.
    listed, good_l, imperfect_l, blank_l, empty_l = shelf[REAL].entries[1:]
    same("a preliminary grade delivered as a list still reads as the grade",
         call(root, "driverStatus", listed), "preliminary",
         "a list is what the front-end sends; a theme that only reads "
         "strings reads nothing")
    truth("and the set is marked broken",
          call(root, "isBroken", listed),
          "this is the production shape -- MAME's own 'THIS GAME DOES NOT "
          "WORK' set renders as a working game")
    same("a good grade delivered as a list reads as good",
         call(root, "driverStatus", good_l), "good")
    same("and is not marked", call(root, "isBroken", good_l), False,
         "reading the list wrongly the other way marks the whole arcade tab")
    same("an imperfect grade delivered as a list reads as imperfect",
         call(root, "driverStatus", imperfect_l), "imperfect")
    same("and is deliberately not marked",
         call(root, "isBroken", imperfect_l), False)
    same("an empty list reports nothing",
         call(root, "driverStatus", blank_l), "",
         "a blank x-mame-status line must read as ungraded, not as a crash")
    same("and is not marked", call(root, "isBroken", blank_l), False)
    same("a list holding an empty string reports nothing",
         call(root, "driverStatus", empty_l), "")
    same("and is not marked", call(root, "isBroken", empty_l), False)

    print("\nS23: and the row under the real shape is drawn as marked")
    same("this collection is drawn as a list", root.property("artMode"), False)
    row = row_for(root, "Cheeky Mouse II")
    truth("the pill is on screen",
          label(row, "not working").property("visible"),
          "the marking exists only if the user can see it")
    same("the title is dimmed",
         title_label(row, "Cheeky Mouse II").property("color"), faint)
    for title in ("Wave Race 64", "Pac-Man II", "Rally X II", "Blank Grade",
                  "Empty Grade"):
        row = row_for(root, title)
        same(f"{title}: no pill",
             label(row, "not working").property("visible"), False,
             "a playable game marked as not working is the same lie in "
             "reverse")
        same(f"{title}: at full strength",
             title_label(row, title).property("color"), plain)

    # ------------------------------------- the harness that hid it for years --
    print("\nS23: the preview stubs the shape the front-end really sends")
    # This is the reason the marking could be broken in production while every
    # screenshot showed it working: tools/preview_library.py handed the theme
    # a bare string. Pinned here rather than left to whoever next opens the
    # preview, because a harness that is more convenient than production is
    # a harness that lies, and nobody goes looking for that.
    sys.path.insert(0, str(REPO / "tools"))
    import preview_library  # noqa: E402

    from padmap.pegasus import Entry  # noqa: E402  (preview_library adds src/)

    previewed = preview_library.StubGame(
        Entry(title="Cheeky Mouse", path="/roms/cheekyms.zip",
              status="preliminary"))
    truth("the preview carries the grade as a list",
          isinstance(previewed.extra.get("mame-status"), list),
          "a preview that stubs a bare string shows a pill the real "
          "front-end never draws")
    truth("and the real theme marks the preview's own game",
          call(root, "isBroken", previewed),
          "what the preview shows and what Pegasus shows have to be the "
          "same screen")

    print("\nS14: exiting a game must not immediately launch another")
    # Reported: "when exiting a game, for some reason it immediately starts
    # another game as if an A input is stuck". Nothing grabs the virtual pads
    # exclusively, so while a game runs RetroArch and Pegasus read the SAME
    # pad -- every button pressed in-game also reaches the library sitting
    # behind it. Quitting with A still down handed the library an accept the
    # instant it came back.
    go_to_tab(app, view, root, N64)

    # The application becoming active is what returning from a game looks like
    # from in here -- the library never loses QML focus, because the game is a
    # separate process. This harness activated the app when it started, so the
    # guard is already disarmed, which is the wiring proving itself.
    if root.property("acceptArmed"):
        raise SystemExit(
            "FAIL: accept is armed even though the application only just "
            "became active. Coming back from a game would launch it again on "
            "the accept that quit it")
    print("  ok  activation disarmed accept without anything else being done")

    root.setProperty("acceptArmed", True)
    before = game.launched
    press(app, view, Qt.Key.Key_Return)
    same("a fresh press still launches", game.launched, before + 1,
         "the guard must not cost the ordinary case -- this is how a game is "
         "started at all")

    before = game.launched
    repeat = QKeyEvent(QEvent.Type.KeyPress, Qt.Key.Key_Return,
                       Qt.KeyboardModifier.NoModifier, "", True, 1)
    QCoreApplication.sendEvent(view, repeat)
    QCoreApplication.sendEvent(
        view, QKeyEvent(QEvent.Type.KeyRelease, Qt.Key.Key_Return,
                        Qt.KeyboardModifier.NoModifier, "", True, 1))
    app.processEvents()
    same("an auto-repeat does not launch", game.launched, before,
         "a held button relaunches the game it just came back from")
    if not repeat.isAccepted():
        raise SystemExit(
            "FAIL: the refused repeat was not accepted, so it falls through "
            "to the grid underneath and moves the selection instead")

    print("\n...and accept is disarmed while the screen settles:")
    root.setProperty("acceptArmed", False)
    before = game.launched
    press(app, view, Qt.Key.Key_Return)
    same("a press arriving while disarmed is refused", game.launched, before,
         "the accept that quit the game launches it again the moment Pegasus "
         "comes back")
    root.setProperty("acceptArmed", True)
    press(app, view, Qt.Key.Key_Return)
    same("and the next press works once rearmed", game.launched, before + 1,
         "the guard never rearms, so no game can be launched at all")

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
