"""Entry point for the padmap setup screen."""

from __future__ import annotations

import sys
from pathlib import Path

from PySide6.QtCore import QUrl
from PySide6.QtGui import QGuiApplication
from PySide6.QtQml import QQmlApplicationEngine

from .setup_model import SetupModel

QML_DIR = Path(__file__).parent / "qml"


def run_setup(players: int = 4) -> list:
    """Show the setup screen. Returns the assignments, or [] if cancelled.

    The QML lives next to this file rather than in a Qt resource bundle so it
    stays editable in place; the import path is registered explicitly because
    the `padmap` QML module is that same directory.
    """
    app = QGuiApplication.instance() or QGuiApplication(sys.argv)

    model = SetupModel(players=players)
    engine = QQmlApplicationEngine()
    engine.addImportPath(str(QML_DIR))
    engine.rootContext().setContextProperty("setup", model)

    accepted: list = []

    def on_accepted() -> None:
        accepted.extend(model.assignments())
        engine.quit()

    model.accepted.connect(on_accepted)

    engine.load(QUrl.fromLocalFile(str(QML_DIR / "Main.qml")))
    if not engine.rootObjects():
        model.close()
        raise RuntimeError(f"failed to load QML from {QML_DIR}")

    try:
        app.exec()
    finally:
        model.close()
    return accepted
