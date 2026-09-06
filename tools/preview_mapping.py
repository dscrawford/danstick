"""Show the mapping overlay on screen, with no daemon and no controllers.

The point is to look at it. Layout coordinates are the artwork -- if an arrow
points at the wrong place, or a d-pad sits inside a grip, that is visible in a
second here and only after a rebuild, a daemon restart and a real button hold
otherwise.

    python3 tools/preview_mapping.py            # step through every layout
    python3 tools/preview_mapping.py n64        # just one
    python3 tools/preview_mapping.py --shot out.png   # write a picture instead
    python3 tools/preview_mapping.py --choose   # the console picker instead

Left/right or space steps through controls, tab switches layout, escape quits.
In --choose mode left/right move the selection, which is what the daemon does
when the stick is pushed.
"""

import sys
from pathlib import Path

from PySide6.QtCore import Property, QObject, Qt, QTimer, QUrl, Signal, Slot
from PySide6.QtGui import QGuiApplication
from PySide6.QtQuick import QQuickView

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))

from padmap import capture, layouts  # noqa: E402

THEME = REPO / "pegasus" / "theme"


class StubKeys(QObject):
    """Only what the overlay touches."""

    @Slot("QVariant", result=bool)
    def isCancel(self, event):
        return False

    @Slot("QVariant", result=bool)
    def isFilters(self, event):
        return False


class StubApi(QObject):
    def __init__(self):
        super().__init__()
        self._keys = StubKeys()

    @Property(QObject, constant=True)
    def keys(self):
        return self._keys


def main() -> int:
    argv = sys.argv[1:]
    choosing = "--choose" in argv
    # The same picker, asking what a mapping is *for* instead of which
    # controller this is. Worth rendering separately: the entries are scopes
    # while the picture is a console's layout, and the whole risk in sharing
    # one overlay is those two coming apart -- which a screenshot settles and
    # a passing test does not.
    scoping = "--scopes" in argv
    argv = [a for a in argv if a not in ("--choose", "--scopes")]
    shot = None
    if "--shot" in argv:
        at = argv.index("--shot")
        shot = argv[at + 1] if at + 1 < len(argv) else "mapping.png"
        # Drop both, or the filename is taken for a layout name.
        argv = argv[:at] + argv[at + 2:]
    args = [a for a in argv if not a.startswith("-")]

    order = args or list(layouts.ALL)
    unknown = [name for name in order if name not in layouts.ALL]
    if unknown:
        print(f"unknown layout(s): {', '.join(unknown)}")
        print(f"available: {', '.join(layouts.ALL)}")
        return 1

    app = QGuiApplication(sys.argv)
    view = QQuickView()
    api = StubApi()
    view.rootContext().setContextProperty("api", api)
    view.setSource(QUrl.fromLocalFile(str(THEME / "MappingOverlay.qml")))
    if view.status() != QQuickView.Status.Ready:
        for error in view.errors():
            print("  QML error:", error.toString())
        return 1

    root = view.rootObject()
    view.setResizeMode(QQuickView.ResizeMode.SizeRootObjectToView)
    view.resize(1100, 700)

    # `preview_mapping.py --choose n64` opens on that console.
    state = {"layout": 0, "control": 0,
             "choice": layouts.index_of(args[0]) if args else 0}

    # Built through the daemon's own option builders, not hand-written here.
    # A preview that constructs its own payload can look perfect while the
    # daemon sends something else entirely.
    if scoping:
        options = [
            option.to_json()
            for option in capture.scope_options(
                scopes={"console:n64"}, default_layout="gamecube",
                last_game=("n64", "n64/super-mario-64", "Super Mario 64"))
        ]
        title = "What is this mapping for?"
    else:
        options = [option.to_json() for option in capture.layout_options()]
        title = "Which controller is this?"

    def show():
        layout = layouts.get(order[state["layout"]])
        if choosing or scoping:
            # Exactly what the daemon sends: options carrying whole layouts,
            # and an index into them. The overlay draws the highlighted one's
            # layout.
            root.setProperty("choosing", True)
            root.setProperty("choices", options)
            root.setProperty("chooseTitle", title)
            root.setProperty("choiceIndex", state["choice"] % len(options))
            root.setProperty("padName", "Preview Pad")
            root.setProperty("player", 1)
            chosen = options[state["choice"] % len(options)]
            view.setTitle(f"padmap picker — {chosen['label']}")
            return
        root.setProperty("layout", layout.to_json())
        root.setProperty("padName", "Preview Pad")
        root.setProperty("player", 1)
        root.setProperty("index", state["control"])
        # Everything before the current control counts as answered, so the
        # filled-in look is visible too rather than only the arrow.
        done = {
            control.canonical: "b%d" % n
            for n, control in enumerate(layout.controls[:state["control"]])
        }
        root.setProperty("captured", done)
        view.setTitle(f"padmap mapping — {layout.label} "
                      f"({state['control'] + 1}/{len(layout.controls)})")

    def step(delta):
        if choosing or scoping:
            state["choice"] = (state["choice"] + delta) % len(options)
            show()
            return
        layout = layouts.get(order[state["layout"]])
        state["control"] += delta
        if state["control"] >= len(layout.controls):
            state["control"] = 0
            state["layout"] = (state["layout"] + 1) % len(order)
        elif state["control"] < 0:
            state["control"] = 0
        show()

    show()

    if shot:
        # Give the scene a moment to lay out before grabbing it, or the
        # picture is captured half-built.
        view.show()

        def grab():
            view.grabWindow().save(shot)
            print(f"wrote {shot}")
            app.quit()

        QTimer.singleShot(1200, grab)
        return app.exec()

    class Filter(QObject):
        def eventFilter(self, obj, event):
            if event.type() == event.Type.KeyPress:
                key = event.key()
                if key == Qt.Key.Key_Escape:
                    app.quit()
                elif key in (Qt.Key.Key_Right, Qt.Key.Key_Space):
                    step(1)
                elif key == Qt.Key.Key_Left:
                    step(-1)
                elif key == Qt.Key.Key_Tab:
                    state["layout"] = (state["layout"] + 1) % len(order)
                    state["control"] = 0
                    show()
                return True
            return False

    filt = Filter()
    view.installEventFilter(filt)

    print("left/right or space: next control   tab: next layout   esc: quit")
    view.show()
    return app.exec()


if __name__ == "__main__":
    sys.exit(main())
