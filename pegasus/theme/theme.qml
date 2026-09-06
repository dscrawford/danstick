import QtQuick 2.15

// Theme root. Pegasus requires this to be a FocusScope holding active focus.
//
// Two screens: the game library, and the padmap controller order screen
// reached with the Details key. The setup screen grabs the controllers while
// it is open, so focus and lifetime are managed carefully -- leaving it by
// any route cancels the session.
FocusScope {
    id: root

    focus: true

    Colors { id: colors }

    Rectangle {
        anchors.fill: parent
        color: colors.background
    }

    Library {
        id: library
        anchors.fill: parent
        focus: true
        visible: !setupLoader.active

        onOpenControllerSetup: {
            if (!api.padmap.connected) {
                toast.show("padmap daemon is not running — start it with `padmap serve`");
                return;
            }
            setupLoader.active = true;
        }
    }

    // The daemon is authoritative about whether a session is open, so follow
    // it rather than assuming this theme is the only thing that can start
    // one. Anything else driving the daemon -- padctl, a second front-end,
    // the test harness -- now surfaces the screen too, and the flow becomes
    // testable without someone physically pressing a key.
    Connections {
        target: api.padmap
        function onStateChanged() {
            if (api.padmap.state === "assigning" && !setupLoader.active)
                setupLoader.active = true;
        }
    }

    Loader {
        id: setupLoader
        anchors.fill: parent
        active: false
        focus: active

        sourceComponent: ControllerSetup {
            focus: true
            Component.onCompleted: open()
            onClosed: {
                setupLoader.active = false;
                library.focus = true;
            }
        }
    }

    // Small transient message, used when the daemon is unreachable. Better
    // than silently doing nothing when the Details key is pressed.
    Rectangle {
        id: toast

        function show(text) {
            label.text = text;
            opacity = 1;
            timer.restart();
        }

        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 60
        width: label.implicitWidth + 32
        height: 44
        radius: 8
        color: colors.surface
        border.width: 1
        border.color: colors.danger
        opacity: 0
        visible: opacity > 0

        Behavior on opacity { NumberAnimation { duration: 200 } }

        Text {
            id: label
            anchors.centerIn: parent
            color: colors.text
            font.pixelSize: 14
        }

        Timer {
            id: timer
            interval: 4000
            onTriggered: toast.opacity = 0
        }
    }
}
