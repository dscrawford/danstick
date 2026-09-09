"""When may the controller setup screen offer configuration?

Two bugs from a real session, both in the theme rather than the daemon:

  * Configuration was offered the instant the screen opened, before anything
    had been pressed. `players` at that moment describes whatever session came
    last -- and the daemon now opens this screen by itself, from a state that
    already had players in it. So it asked to calibrate a player the new
    session held no claim for.
  * The daemon rightly refused, and nothing listened for the error, so the
    overlay sat on "Starting..." forever with the pads still grabbed. No way
    forward and no way out.

Loads the real theme QML against a stub `api`, so the logic can be checked
without a daemon, a front-end, or grabbing a single controller.

    QT_QPA_PLATFORM=offscreen python3 tools/check_theme_setup.py
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

from PySide6.QtCore import (Property, QCoreApplication, QEvent, QObject, Qt, QUrl, Signal,  # noqa: E402
                            Slot)
from PySide6.QtGui import QGuiApplication, QKeyEvent
from PySide6.QtQml import QQmlComponent, QQmlEngine

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import capture, layouts  # noqa: E402

THEME = REPO / "pegasus" / "theme"


class StubPadmap(QObject):
    connectedChanged = Signal()
    stateChanged = Signal()
    holdChanged = Signal()
    confirmHoldChanged = Signal()
    launchConfigChanged = Signal()
    calibrationChanged = Signal()
    calibrationFinished = Signal(int)
    mappingChanged = Signal()
    mappingFinished = Signal(bool)
    layoutChoiceChanged = Signal()
    claimed = Signal(int, str, str)
    accepted = Signal()
    errorReported = Signal(str)
    gamepadEditorRequested = Signal()

    def __init__(self):
        super().__init__()
        self._state = "idle"
        self._players = []
        self._phase = ""
        self.calibrate_calls = []
        self.begin_calls = []
        self.accept_calls = 0
        self.editor_calls = 0
        self.map_calls = []
        self.choose_calls = []
        self.scope_calls = []
        self.forget_calls = []
        self.map_for_game_calls = []
        self.skip_calls = 0
        self._mapping_active = False
        self._mapping_player = 0
        self._choice_active = False
        self._choice_kind = ""
        self._choice_title = ""
        self._options = []

    # -- properties the theme reads --------------------------------------
    @Property(bool, notify=connectedChanged)
    def connected(self):
        return True

    @Property(str, notify=stateChanged)
    def state(self):
        return self._state

    @Property(int, notify=stateChanged)
    def slotCount(self):
        return 4

    @Property("QVariantList", notify=stateChanged)
    def players(self):
        return self._players

    @Property(float, notify=holdChanged)
    def hold(self):
        return 0.0

    @Property(float, notify=confirmHoldChanged)
    def confirmHold(self):
        return 0.0

    @Property(str, notify=launchConfigChanged)
    def launchConfig(self):
        return ""

    @Property(str, notify=calibrationChanged)
    def calibrationPhase(self):
        return self._phase

    @Property(float, notify=calibrationChanged)
    def calibrationProgress(self):
        return 0.0

    @Property(int, notify=calibrationChanged)
    def calibrationPlayer(self):
        return 0

    @Property(bool, notify=mappingChanged)
    def mappingActive(self):
        return self._mapping_active

    @Property("QVariantMap", notify=mappingChanged)
    def mappingLayout(self):
        return {}

    @Property(int, notify=mappingChanged)
    def mappingIndex(self):
        return 0

    @Property(int, notify=mappingChanged)
    def mappingTotal(self):
        return 0

    @Property(int, notify=mappingChanged)
    def mappingPlayer(self):
        return self._mapping_player

    @Property(str, notify=mappingChanged)
    def mappingLabel(self):
        return ""

    @Property("QVariantMap", notify=mappingChanged)
    def mappingCaptured(self):
        return {}

    @Property(bool, notify=layoutChoiceChanged)
    def layoutChoiceActive(self):
        return self._choice_active

    @Property("QVariantList", notify=layoutChoiceChanged)
    def layoutChoices(self):
        # Built from the daemon's own option builder rather than from a list
        # written here, so the shape the theme is fed is the shape the daemon
        # actually sends -- the whole point of the stub is that it cannot
        # quietly disagree with the thing it stands in for.
        return [option.to_json() for option in self._options]

    @Property(str, notify=layoutChoiceChanged)
    def layoutChoiceKind(self):
        return self._choice_kind

    @Property(str, notify=layoutChoiceChanged)
    def layoutChoiceTitle(self):
        return self._choice_title

    @Property(int, notify=layoutChoiceChanged)
    def layoutChoiceIndex(self):
        return 0

    @Property(int, notify=layoutChoiceChanged)
    def layoutChoicePlayer(self):
        return self.choose_calls[-1] if self.choose_calls else 0

    # -- commands ---------------------------------------------------------
    @Slot(int)
    def begin(self, players):
        self.begin_calls.append(players)

    @Slot()
    def reset(self):
        pass

    @Slot()
    def accept(self):
        self.accept_calls += 1

    @Slot()
    def cancel(self):
        pass

    @Slot(int)
    def calibrate(self, player):
        self.calibrate_calls.append(player)

    @Slot(int)
    def chooseLayout(self, player):
        self.choose_calls.append(player)
        self._choice_active = True
        self._choice_kind = capture.KIND_LAYOUT
        self._choice_title = "Which controller is this?"
        self._options = capture.layout_options()
        self.layoutChoiceChanged.emit()

    @Slot(int)
    def chooseScope(self, player):
        self.scope_calls.append(player)
        self._choice_active = True
        self._choice_kind = capture.KIND_SCOPE
        self._choice_title = "What is this mapping for?"
        self._options = capture.scope_options(
            scopes=set(), default_layout="n64",
            recent=[("n64", "n64/super-mario-64", "Super Mario 64")])
        self.layoutChoiceChanged.emit()

    # Two decorators, because QML calls this with one argument and the real
    # C++ side has a default parameter. A single two-argument slot is simply
    # never matched, and the call vanishes without an error.
    @Slot(int)
    @Slot(int, str)
    def startMapping(self, player, layout=""):
        self.map_calls.append(player)
        self._mapping_active = True
        self.mappingChanged.emit()

    @Slot()
    def skipControl(self):
        self.skip_calls += 1

    @Slot(int, str)
    def setIcon(self, player, icon):
        pass

    @Slot()
    def configureEnd(self):
        pass

    @Slot()
    def openGamepadEditor(self):
        self.editor_calls += 1

    @Slot(int)
    def forgetPad(self, player):
        self.forget_calls.append(player)

    @Slot(int, str, str, str)
    def mapForGame(self, player, console, key, title):
        self.map_for_game_calls.append((player, console, key, title))

    # -- driving the stub from Python -------------------------------------
    def set_state(self, state, players=None):
        self._state = state
        if players is not None:
            self._players = players
        self.stateChanged.emit()

    def finish_mapping(self, player, stored=True):
        """The daemon's final mapping event: player still set, done=True."""
        self._mapping_player = player
        self._mapping_active = False
        self.mappingChanged.emit()
        self.mappingFinished.emit(stored)

    def set_phase(self, phase):
        self._phase = phase
        self.calibrationChanged.emit()


class StubKeys(QObject):
    """Answers False to everything, unless a test says which key it is sending.

    The theme asks `api.keys.isPrevPage(event)` rather than looking at key
    codes, so a harness cannot press a key without standing in for that
    question too. `wanted` is the one gesture currently being sent.
    """

    def __init__(self):
        super().__init__()
        self.wanted = ""

    @Slot("QVariant", result=bool)
    def isAccept(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isCancel(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isDetails(self, event):
        return self.wanted == "details"

    @Slot("QVariant", result=bool)
    def isFilters(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isNextPage(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isPrevPage(self, event):
        return self.wanted == "prevpage"

    @Slot("QVariant", result=bool)
    def isLeft(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isRight(self, event):
        return False


class StubApi(QObject):
    def __init__(self):
        super().__init__()
        self._padmap = StubPadmap()
        self._keys = StubKeys()

    @Property(QObject, constant=True)
    def padmap(self):
        return self._padmap

    @Property(QObject, constant=True)
    def keys(self):
        return self._keys


def player(number, name, configured):
    return {"player": number, "name": name, "node": "event90",
            "icon": "gamepad", "configured": configured}


def load(engine, api):
    engine.rootContext().setContextProperty("api", api)
    component = QQmlComponent(engine, QUrl.fromLocalFile(
        str(THEME / "ControllerSetup.qml")))
    obj = component.create()
    if obj is None:
        for error in component.errors():
            print("  QML error:", error.toString())
        raise SystemExit("FAIL: ControllerSetup.qml did not load")
    # Without this the engine owns it, and Python drops the last reference
    # the moment create() returns -- every later property read then fails
    # with "Internal C++ object already deleted".
    QQmlEngine.setObjectOwnership(obj, QQmlEngine.ObjectOwnership.CppOwnership)
    # A C++ parent as well: the component is a local here, and letting it go
    # takes the object it created with it.
    obj.setParent(engine)
    _alive.extend([component, obj])
    return obj


# Kept out of scope of the garbage collector for the run's duration.
_alive: list = []


def drop_setups(app):
    """Delete every screen built so far, so call counts mean one screen.

    Each ControllerSetup stays connected to the same stub `api`, so a signal
    emitted for one check reaches the screens left over from earlier ones too.
    Property reads are unaffected -- those are per object -- but anything that
    counts calls (accept, calibrate) otherwise measures the whole pile, and a
    check that cannot tell one screen from five is not measuring what it says.
    """
    for obj in _alive:
        if hasattr(obj, "deleteLater"):
            obj.deleteLater()
    _alive.clear()
    # processEvents() alone does not run deferred deletions, so the screens
    # would stay alive and connected while looking as though they had gone --
    # which is worse than not trying, because the counts then quietly include
    # them.
    QCoreApplication.sendPostedEvents(None, QEvent.Type.DeferredDelete)
    app.processEvents()


def mapping_overlay_of(setup):
    """The overlay that draws the controller: picker and wizard in one."""
    for child in setup.findChildren(QObject):
        if child.property("choosing") is not None and \
                child.property("shownLayout") is not None:
            return child
    raise SystemExit("FAIL: could not find the mapping overlay")


def overlay_of(setup):
    for child in setup.findChildren(QObject):
        if child.property("padName") is not None and \
                child.property("iconChoices") is not None:
            return child
    raise SystemExit("FAIL: could not find the calibration overlay")


def main() -> int:
    app = QGuiApplication(sys.argv)
    engine = QQmlEngine()
    api = StubApi()
    pad = api._padmap

    # The state the daemon's auto-open leaves behind: a session is already
    # assigning, and `players` still describes the one before it, whose
    # controller has never been configured.
    pad.set_state("assigning", [player(1, "Old Session Pad", False)])

    setup = load(engine, api)
    setup.setProperty("focus", True)
    app.processEvents()

    print("opening the screen with a stale, unconfigured player listed:")
    if pad.calibrate_calls:
        raise SystemExit(
            f"FAIL: offered configuration before any press "
            f"(calibrate{pad.calibrate_calls})")
    overlay = overlay_of(setup)
    if overlay.property("player") != 0:
        raise SystemExit("FAIL: the overlay opened before any press")
    print("  ok  nothing is offered, and no overlay")

    print("\na state event arrives with the same stale player:")
    pad.set_state("assigning", [player(1, "Old Session Pad", False)])
    app.processEvents()
    if pad.calibrate_calls:
        raise SystemExit("FAIL: a state event alone triggered configuration")
    print("  ok  still nothing -- a press is what counts")

    print("\na player appears in the list without ever claiming here:")
    # e.g. the daemon restored assignments, or a previous session's list.
    pad.set_state("assigning", [player(3, "Never Pressed Anything", False)])
    app.processEvents()
    if pad.calibrate_calls:
        raise SystemExit(
            f"FAIL: offered setup for a player that never pressed "
            f"({pad.calibrate_calls})")
    print("  ok  ignored -- configuration follows a press, nothing else")

    print("\nnow a first-time controller actually claims a slot:")
    pad.claimed.emit(1, "Brand New Pad", "event91")
    pad.set_state("assigning", [player(1, "Brand New Pad", False)])
    app.processEvents()
    if pad.choose_calls != [1]:
        raise SystemExit(
            f"FAIL: expected the layout picker to open for player 1, got "
            f"{pad.choose_calls}")
    if pad.map_calls:
        raise SystemExit(
            f"FAIL: started mapping without asking which console it is -- "
            f"that is how an N64 pad gets asked for an X it does not have "
            f"({pad.map_calls})")
    if pad.accept_calls:
        raise SystemExit(
            "FAIL: accepted before the buttons were captured -- the profile "
            "would be written with no bindings in it")
    if pad.calibrate_calls:
        raise SystemExit(
            f"FAIL: ran the Calibration Overlay; it is out of the way now "
            f"({pad.calibrate_calls})")
    print("  ok  the console picker opens, nothing written yet")

    print("\nthe picker draws the console it is offering:")
    overlay_shown = mapping_overlay_of(setup)
    if not overlay_shown.property("visible"):
        raise SystemExit("FAIL: the picker is not on screen")
    if not overlay_shown.property("choosing"):
        raise SystemExit("FAIL: the overlay is not in choose mode")
    shown = overlay_shown.property("shownLayout")
    offered = [entry["id"] for entry in pad.layoutChoices]
    if not shown or shown.get("id") != offered[0]:
        raise SystemExit(
            f"FAIL: nothing drawn for the highlighted console "
            f"(shown={shown and shown.get('id')!r}, offered={offered})")
    if [name for name in ("n64", "gamecube", "snes", "arcade")
            if name not in offered]:
        raise SystemExit(
            f"FAIL: the picker is missing consoles the daemon offers: "
            f"{offered}")
    print(f"  ok  offers {', '.join(offered)}, drawing {shown['id']}")

    print("\nPrev-page asks what a mapping is FOR, not which console:")
    # The deliberate re-map route. It must not be the layout picker any more:
    # that question cannot express "this pad, but only for N64 games", which
    # is the whole reported need.
    api._keys.wanted = "prevpage"
    setup.forceActiveFocus()
    QGuiApplication.sendEvent(
        setup, QKeyEvent(QEvent.Type.KeyPress, Qt.Key.Key_PageUp,
                         Qt.KeyboardModifier.NoModifier))
    app.processEvents()
    api._keys.wanted = ""
    if pad.scope_calls != [1]:
        raise SystemExit(
            f"FAIL: Prev-page did not open the scope picker ({pad.scope_calls}; "
            f"layout picker calls {pad.choose_calls})")
    print(f"  ok  chooseScope({pad.scope_calls[0]})")

    print("\nand the overlay draws the scope's console, not its scope string:")
    overlay_shown = mapping_overlay_of(setup)
    if not overlay_shown.property("choosing"):
        raise SystemExit("FAIL: the overlay is not in choose mode")
    if overlay_shown.property("chooseTitle") != "What is this mapping for?":
        raise SystemExit(
            f"FAIL: the heading is the daemon's, or should be "
            f"({overlay_shown.property('chooseTitle')!r})")
    entries = pad.layoutChoices
    n64 = next(i for i, e in enumerate(entries) if e["id"] == "console:n64")
    overlay_shown.setProperty("choiceIndex", n64)
    app.processEvents()
    shown = overlay_shown.property("shownLayout")
    if not shown or shown.get("id") != "n64":
        raise SystemExit(
            f"FAIL: the N64 scope draws {shown and shown.get('id')!r}. The "
            f"picture and the pad the wizard asks about must be the same one.")
    if not any(e["id"].startswith("game:") for e in entries):
        raise SystemExit("FAIL: the game last played is not offered")
    print(f"  ok  {len(entries)} scopes, 'console:n64' drawn as the N64 pad")

    # Back to where the rest of this test expects to be.
    pad._choice_active = False
    pad._options = []
    pad.layoutChoiceChanged.emit()
    app.processEvents()

    print("\nthe capture finishes:")
    pad.mappingFinished.emit(True)
    app.processEvents()
    if pad.accept_calls != 1:
        raise SystemExit(
            f"FAIL: expected the session to be accepted once the capture was "
            f"done -- that is what writes the profile and the SDL mapping "
            f"({pad.accept_calls})")
    print("  ok  session accepted, which is what writes both files")

    print("\na second controller claims, already configured:")
    pad.accept_calls = 0
    pad.editor_calls = 0
    pad.choose_calls.clear()
    pad.map_calls.clear()
    pad.calibrate_calls.clear()
    pad.claimed.emit(2, "Known Pad", "event92")
    pad.set_state("assigning", [player(1, "Brand New Pad", False),
                                player(2, "Known Pad", True)])
    app.processEvents()
    if (pad.choose_calls or pad.map_calls or pad.accept_calls
            or pad.calibrate_calls):
        raise SystemExit(
            "FAIL: a controller that has been set up before triggered setup")
    print("  ok  left alone -- it has been set up before")

    # The Calibration Overlay is now only reached deliberately (Details on a
    # controller whose sticks need measuring), but it can still be asked to
    # calibrate a player the daemon holds no claim for -- which is what used
    # to leave it on "Starting..." forever with the pads grabbed.
    print("\nthe Calibration Overlay is opened and the daemon refuses:")
    overlay.setProperty("player", 1)
    overlay.setProperty("step", "measuring")
    pad.errorReported.emit("no controller assigned to player 1")
    app.processEvents()
    step = overlay.property("step")
    if step != "problem":
        raise SystemExit(
            f"FAIL: overlay stayed on step {step!r} -- it is stuck again")
    if not overlay.property("problem"):
        raise SystemExit("FAIL: no message shown for the failure")
    print(f"  ok  shows {overlay.property('problem')!r} instead of hanging")

    print("\nan error once measuring has started is not a reason to bail:")
    pad.calibrate_calls.clear()
    overlay.setProperty("player", 2)
    overlay.setProperty("step", "measuring")
    pad.set_phase("rest")
    pad.errorReported.emit("something unrelated")
    app.processEvents()
    if overlay.property("step") == "problem":
        raise SystemExit("FAIL: tore down a measurement in progress")
    print("  ok  measurement left alone")

    print("\nthe keyboard reset, which is the only kind that can work:")
    # The daemon holds EVIOCGRAB for the whole session, so the front-end sees
    # no controller input at all while this screen is open -- a pad gesture
    # could not reach this. By slot number rather than "the last one claimed":
    # with two controllers assigned, "the last one" is exactly the ambiguity
    # someone is trying to resolve when they reach for it.
    pad.forget_calls.clear()
    pad.set_state("assigning", [player(1, "First Pad", True),
                                player(2, "Second Pad", True)])
    setup.setProperty("focus", True)
    app.processEvents()

    for key, slot in ((Qt.Key.Key_2, 2), (Qt.Key.Key_1, 1)):
        QGuiApplication.sendEvent(
            setup, QKeyEvent(QEvent.Type.KeyPress, key,
                             Qt.KeyboardModifier.NoModifier))
        app.processEvents()
    if pad.forget_calls != [2, 1]:
        raise SystemExit(
            f"FAIL: number keys reset {pad.forget_calls}, wanted [2, 1] -- "
            f"the key must name the slot, not the most recent claim")
    print(f"  ok  forgetPad{tuple(pad.forget_calls)} from pressing 2 then 1")

    print("\n...but not a slot that holds no controller:")
    pad.forget_calls.clear()
    QGuiApplication.sendEvent(
        setup, QKeyEvent(QEvent.Type.KeyPress, Qt.Key.Key_3,
                         Qt.KeyboardModifier.NoModifier))
    app.processEvents()
    if pad.forget_calls:
        raise SystemExit(
            f"FAIL: reset an empty slot ({pad.forget_calls}) -- the daemon "
            f"would only reject it, and a key that fails silently is worse "
            f"than one that is not bound")
    print("  ok  slot 3 is empty, so 3 does nothing")

    print("\n...and a modified press is left for whatever else wants it:")
    pad.forget_calls.clear()
    QGuiApplication.sendEvent(
        setup, QKeyEvent(QEvent.Type.KeyPress, Qt.Key.Key_1,
                         Qt.KeyboardModifier.ControlModifier))
    app.processEvents()
    if pad.forget_calls:
        raise SystemExit(
            f"FAIL: Ctrl+1 was treated as a reset ({pad.forget_calls})")
    print("  ok  Ctrl+1 ignored")

    print("\nmapping a pad for the game the library was sitting on:")
    # The flow asked for: a key on a focused game, then a controller select,
    # then "console or this game?" -- with both already known because the
    # library knew them. The claim *is* the controller select: whichever pad
    # presses a button is the one configured, which needs nothing mapped and
    # works on a pad padmap has never seen.
    pad.map_for_game_calls.clear()
    setup = load(engine, api)
    setup.setProperty("pendingGame", {
        "console": "n64", "key": "n64/goldeneye-007-usa",
        "title": "GoldenEye 007 (USA)"})
    setup.setProperty("focus", True)
    pad.set_state("assigning", [])
    app.processEvents()

    if pad.map_for_game_calls:
        raise SystemExit(
            f"FAIL: mapping started before any pad claimed a slot "
            f"({pad.map_for_game_calls}) -- there is no controller select then")

    # The case the claim gate actually exists for: the daemon opens this
    # screen by itself, from a state that already lists players belonging to
    # a session that is over. Configuring one of those means configuring
    # whichever pad happened to be player 1 last time, not the pad the user
    # is holding -- and nobody pressed anything to say so.
    pad.set_state("assigning", [player(1, "Pad From Last Session", True)])
    app.processEvents()
    if pad.map_for_game_calls:
        raise SystemExit(
            f"FAIL: mapped a player left over from an earlier session "
            f"({pad.map_for_game_calls}) -- the claim is the controller "
            f"select, so it has to follow a press")

    pad.set_state("assigning", [player(2, "GameCube Pad", True)])
    pad.claimed.emit(2, "GameCube Pad", "event27")
    app.processEvents()

    if pad.map_for_game_calls != [
            (2, "n64", "n64/goldeneye-007-usa", "GoldenEye 007 (USA)")]:
        raise SystemExit(
            f"FAIL: {pad.map_for_game_calls}, wanted one call for player 2 "
            f"carrying the console and key the exporter computed")
    print(f"  ok  mapForGame{pad.map_for_game_calls[0]}")

    print("\n...once, though the claim and the state event both fire:")
    pad.claimed.emit(2, "GameCube Pad", "event27")
    pad.set_state("assigning", [player(2, "GameCube Pad", True)])
    app.processEvents()
    if len(pad.map_for_game_calls) != 1:
        raise SystemExit(
            f"FAIL: {len(pad.map_for_game_calls)} wizards started for one "
            f"press ({pad.map_for_game_calls})")
    print("  ok  one press, one wizard")

    print("\nand with no game pending it behaves as it always did:")
    pad.map_for_game_calls.clear()
    plain = load(engine, api)
    plain.setProperty("focus", True)
    pad.set_state("assigning", [player(1, "Some Pad", True)])
    pad.claimed.emit(1, "Some Pad", "event24")
    app.processEvents()
    if pad.map_for_game_calls:
        raise SystemExit(
            f"FAIL: a plain setup opened a game mapping ({pad.map_for_game_calls})")
    print("  ok  no mapping started")

    print("\nfinishing the wizard measures the sticks before accepting:")
    # Reported: an analog stick behaving as though it were only off or full.
    # Nothing had ever been calibrated, so axes were scaled against the range
    # the adapter declares rather than the one the stick reaches. Measuring is
    # part of configuring a controller, so the wizard does it rather than
    # leaving it as an errand nobody knows to run.
    drop_setups(app)
    pad.calibrate_calls.clear()
    pad.accept_calls = 0
    setup = load(engine, api)
    setup.setProperty("focus", True)
    pad.set_state("assigning", [player(2, "GameCube Pad", True)])
    app.processEvents()

    pad.finish_mapping(2, stored=True)
    app.processEvents()

    if pad.accept_calls:
        raise SystemExit(
            "FAIL: accepted straight after the wizard. Accept ends the "
            "session and releases the pads, so calibration would then be "
            "measuring a controller nobody is holding")
    if pad.calibrate_calls != [2]:
        raise SystemExit(
            f"FAIL: calibrate{pad.calibrate_calls}, wanted [2] -- the sticks "
            f"are never measured and the analog range stays wrong")
    print(f"  ok  calibrate({pad.calibrate_calls[0]}), and no accept yet")

    print("\n...and accepting follows the calibration, not the wizard:")
    overlay = overlay_of(setup)
    overlay.setProperty("player", 2)
    pad.calibrationFinished.emit(2)
    app.processEvents()
    if pad.accept_calls != 1:
        raise SystemExit(
            f"FAIL: {pad.accept_calls} accepts after calibration finished -- "
            f"nothing writes the RetroArch profile or the SDL mapping")
    print("  ok  accepted once, after the measurement")

    print("\nan abandoned wizard still accepts, and measures nothing:")
    drop_setups(app)
    pad.calibrate_calls.clear()
    pad.accept_calls = 0
    plain = load(engine, api)
    plain.setProperty("focus", True)
    pad.set_state("assigning", [player(1, "Some Pad", True)])
    pad.finish_mapping(1, stored=False)
    app.processEvents()
    if pad.calibrate_calls:
        raise SystemExit(
            f"FAIL: calibrated after a run that stored nothing "
            f"({pad.calibrate_calls})")
    if pad.accept_calls != 1:
        raise SystemExit(f"FAIL: {pad.accept_calls} accepts, wanted 1")
    print("  ok  accepted, nothing measured")

    print("\nasking to calibrate before anything claimed says what is missing:")
    # The key did nothing at all until a controller had claimed a slot, which
    # reads as a broken key rather than as a missing step.
    drop_setups(app)
    pad.calibrate_calls.clear()
    setup = load(engine, api)
    setup.setProperty("focus", True)
    pad.set_state("assigning", [])
    app.processEvents()

    api._keys.wanted = "details"
    QGuiApplication.sendEvent(
        setup, QKeyEvent(QEvent.Type.KeyPress, Qt.Key.Key_I,
                         Qt.KeyboardModifier.NoModifier))
    app.processEvents()
    api._keys.wanted = ""

    if pad.calibrate_calls:
        raise SystemExit(
            f"FAIL: asked the daemon to calibrate nothing ({pad.calibrate_calls})")
    note = setup.property("note")
    if not note or "Hold a button" not in note:
        raise SystemExit(
            f"FAIL: said nothing ({note!r}) -- a key that silently does "
            f"nothing is indistinguishable from a broken one")
    print(f"  ok  {note!r}")

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
