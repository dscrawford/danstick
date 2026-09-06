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

from PySide6.QtCore import Property, QObject, QUrl, Signal, Slot
from PySide6.QtGui import QGuiApplication
from PySide6.QtQml import QQmlComponent, QQmlEngine

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import layouts  # noqa: E402

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
        self.skip_calls = 0
        self._mapping_active = False
        self._choice_active = False

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
        return 0

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
        return [dict(layout) for layout in layouts.catalogue()]

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

    # -- driving the stub from Python -------------------------------------
    def set_state(self, state, players=None):
        self._state = state
        if players is not None:
            self._players = players
        self.stateChanged.emit()

    def set_phase(self, phase):
        self._phase = phase
        self.calibrationChanged.emit()


class StubKeys(QObject):
    @Slot("QVariant", result=bool)
    def isAccept(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isCancel(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isDetails(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isFilters(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isNextPage(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isPrevPage(self, event):
        return False

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

    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
