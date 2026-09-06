"""Load the QML with a stub model, so it can be checked without grabbing pads.

Instantiating the real SetupModel would EVIOCGRAB every controller, which
steals input from whatever session is running. This exercises the QML -- the
part that actually breaks on a typo -- against a fake exposing the same
properties.

    QT_QPA_PLATFORM=offscreen python3 tools/smoke_qml.py
"""

import sys
from pathlib import Path

from PySide6.QtCore import Property, QObject, QUrl, Signal, Slot
from PySide6.QtGui import QGuiApplication
from PySide6.QtQml import QQmlApplicationEngine

QML_DIR = Path(__file__).resolve().parent.parent / "src" / "padmap" / "ui" / "qml"


class StubModel(QObject):
    slotsChanged = Signal()
    holdChanged = Signal()
    confirmHoldChanged = Signal()
    statusChanged = Signal()
    accepted = Signal()

    @Property("QVariantList", notify=slotsChanged)
    def slots(self):
        return [
            {"player": 1, "name": "USB GamePad", "event": "event24", "claimed": True},
            {"player": 2, "name": "Fightstick F300", "event": "event23", "claimed": True},
            {"player": 3, "name": "", "event": "", "claimed": False},
            {"player": 4, "name": "", "event": "", "claimed": False},
        ]

    @Property(int, notify=slotsChanged)
    def claimedCount(self):
        return 2

    @Property(float, notify=holdChanged)
    def hold(self):
        return 0.45

    @Property(float, notify=confirmHoldChanged)
    def confirmHold(self):
        return 0.0

    @Property(str, notify=statusChanged)
    def status(self):
        return "Player 2 set. Hold a button for Player 3."

    @Slot()
    def reset(self):
        pass

    @Slot()
    def accept(self):
        pass


def main() -> int:
    app = QGuiApplication(sys.argv)
    engine = QQmlApplicationEngine()
    engine.addImportPath(str(QML_DIR))
    model = StubModel()
    engine.rootContext().setContextProperty("setup", model)

    warnings: list[str] = []
    engine.warnings.connect(
        lambda errs: warnings.extend(str(e.toString()) for e in errs)
    )

    engine.load(QUrl.fromLocalFile(str(QML_DIR / "Main.qml")))

    # Connecting to `warnings` suppresses Qt's own stderr output, so these
    # have to be printed on every path or failures arrive with no detail.
    if warnings:
        print("QML warnings:")
        for warning in warnings:
            print(f"  {warning}")

    if not engine.rootObjects():
        print("FAIL: QML did not load")
        return 1
    if warnings:
        return 1

    root = engine.rootObjects()[0]
    print(f"OK: loaded {root.property('title')}")
    print(f"    size {root.property('width')}x{root.property('height')}")

    del app
    return 0


def render(out: str) -> int:
    """Render SetupScreen.qml to a PNG, for eyeballing the layout headlessly.

    Uses QQuickView on the screen Item rather than the Window in Main.qml:
    QQmlApplicationEngine hands its root back typed as QWindow, which has no
    grabWindow, and recasting it does not work because shiboken returns the
    cached wrapper. A QQuickView we construct here is a QQuickWindow already.
    """
    from PySide6.QtCore import QEventLoop, QTimer
    from PySide6.QtGui import QColor
    from PySide6.QtQuick import QQuickView

    app = QGuiApplication(sys.argv)
    view = QQuickView()
    view.engine().addImportPath(str(QML_DIR))
    model = StubModel()
    view.rootContext().setContextProperty("stub", model)
    view.setInitialProperties({"model": model})
    view.setColor(QColor("#1b1b1f"))
    view.setResizeMode(QQuickView.ResizeMode.SizeRootObjectToView)
    view.resize(1000, 620)
    view.setSource(QUrl.fromLocalFile(str(QML_DIR / "padmap" / "SetupScreen.qml")))

    if view.status() == QQuickView.Status.Error:
        for error in view.errors():
            print(f"  {error.toString()}")
        print("FAIL: SetupScreen.qml did not load")
        return 1

    view.show()
    # Let the scene graph run a few frames; grabbing immediately returns an
    # empty surface.
    loop = QEventLoop()
    QTimer.singleShot(700, loop.quit)
    loop.exec()

    image = view.grabWindow()
    if image.isNull():
        print("FAIL: grabWindow returned a null image")
        return 1
    if not image.save(out):
        print(f"FAIL: could not write {out}")
        return 1
    print(f"wrote {out}")
    del app
    return 0


if __name__ == "__main__":
    sys.exit(render(sys.argv[1]) if len(sys.argv) > 1 else main())
