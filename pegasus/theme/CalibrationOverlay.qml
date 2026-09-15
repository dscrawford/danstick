import QtQuick 2.15

// First-run setup for a controller padmap has not seen before.
//
// Three steps in one overlay, because they are one conversation from the
// user's side: offer -> measure -> pick an icon. All the measurement lives in
// the daemon; this only renders phases and sends four commands.
FocusScope {
    id: root

    // 0 when idle, otherwise the player whose controller is being set up.
    property int player: 0
    property string padName: ""

    // "measuring" | "icon"
    //
    // There is deliberately no confirmation step. The button that claimed the
    // slot is often still travelling when this opens, and an "A to continue"
    // prompt just eats it -- the user sees setup skip itself. Starting
    // immediately means a stray press can only land on a step that ignores it.
    property string step: "measuring"

    signal finished()

    // Every icon padmap knows, in picking order. Must match icons.ICON_NAMES;
    // tools/check_icons.py fails if it does not. It had already fallen behind
    // by two -- "switch" and "genesis" existed, shipped an SVG each, and could
    // not be chosen here -- which is the failure mode a second copy of a list
    // always has.
    readonly property var iconChoices: [
        "gamepad", "arcade", "n64", "gamecube", "snes",
        "playstation", "xbox", "steam", "switch", "genesis", "wheel",
    ]
    property int iconIndex: 0

    Colors { id: colors }

    // Non-empty while a failure is being shown instead of a phase.
    property string problem: ""

    function start(playerNumber, name, isFirstTime) {
        player = playerNumber;
        padName = name;
        iconIndex = 0;
        root.firstTime = isFirstTime === true;
        step = "measuring";
        problem = "";
        api.padmap.calibrate(playerNumber);
        watchdog.restart();
    }

    // Give up rather than sit on "Starting..." forever.
    //
    // The daemon refuses to calibrate a player it holds no claim for, and
    // replies with an error. Nothing here listened for that, so the overlay
    // waited for a phase that was never coming and the screen was simply
    // stuck -- with the pads grabbed, so there was no way out either.
    function fail(message) {
        watchdog.stop();
        problem = message;
        step = "problem";
    }

    Timer {
        id: watchdog
        // Comfortably longer than the daemon takes to answer, short enough
        // that a wedged overlay does not look like a frozen machine.
        interval: 4000
        onTriggered: {
            if (root.player !== 0 && root.phase === "")
                root.fail(qsTr("The controller did not respond."));
        }
    }

    // The daemon owns the phase sequence; this only renders it. Phases are
    // await_rest -> rest -> await_reach -> reach -> icon, and the daemon stays
    // modal (ignoring claims and the confirm gesture) for all of them.
    readonly property string phase: api.padmap.calibrationPhase

    Connections {
        target: api.padmap

        function onCalibrationChanged() {
            if (api.padmap.calibrationPhase !== "")
                watchdog.stop();
            root.step = (api.padmap.calibrationPhase === "icon")
                        ? "icon" : "measuring";
        }

        function onErrorReported(message) {
            // Only while waiting to start: once a phase is running the
            // daemon owns the flow, and an unrelated error should not tear
            // a measurement down.
            if (root.player !== 0 && root.phase === "")
                root.fail(message);
        }

        function onCalibrationFinished(player) {
            // Deliberately does NOT chain into the button-mapping editor.
            //
            // That editor records through SDL, so opening it means releasing
            // the EVIOCGRAB -- which cancels the assignment session. Doing
            // that automatically destroyed the setup the user had just
            // completed: slots emptied, daemon back to idle, nothing
            // assigned. Mapping stays an explicit action (Next-page).
            root.firstTime = false;
            root.finished();
        }
    }

    // Dim whatever is behind rather than replacing it: the player slots stay
    // visible, so it is clear which controller this is about.
    Rectangle {
        anchors.fill: parent
        color: "#d0000000"
    }

    Column {
        anchors.centerIn: parent
        spacing: 24
        width: parent.width

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: root.step === "icon"
                  ? qsTr("Which controller is this?")
                  : root.step === "problem"
                    ? qsTr("Setup could not start")
                    : qsTr("New controller")
            color: colors.text
            font.pixelSize: 26
            font.bold: true
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: "Player " + root.player + "  ·  " + root.padName
            color: colors.textDim
            font.pixelSize: 15
        }

        // -- could not start ------------------------------------------------
        Column {
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: 14
            visible: root.step === "problem"

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 560
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                color: colors.text
                font.pixelSize: 19
                text: root.problem
            }

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                color: colors.textDim
                font.pixelSize: 14
                text: qsTr("Press any button to go back and hold a button "
                           + "on the controller you want to set up.")
            }
        }

        // -- measuring ------------------------------------------------------
        Column {
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: 18
            visible: root.step === "measuring"

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 560
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                color: colors.text
                font.pixelSize: 19
                text: {
                    switch (root.phase) {
                    case "await_rest":
                        return qsTr("Let go of the sticks.\n"
                                    + "Press any button when you are ready.");
                    case "rest":
                        return qsTr("Hold still...");
                    case "await_reach":
                        return qsTr("Now move every stick in full circles, "
                                    + "all the way to the edges.\n"
                                    + "Press any button to begin.");
                    case "reach":
                        return qsTr("Keep going — reach every edge.\n"
                                    + "Press any button when done.");
                    }
                    return qsTr("Starting...");
                }
            }

            // Shown only while sampling. During an await phase there is
            // nothing to measure yet, and a bar sitting at zero reads as if
            // something has stalled.
            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 420
                height: 10
                radius: 5
                color: colors.outline
                visible: root.phase === "rest" || root.phase === "reach"

                Rectangle {
                    width: parent.width * api.padmap.calibrationProgress
                    height: parent.height
                    radius: parent.radius
                    color: root.phase === "reach" ? colors.accent : colors.textDim
                    Behavior on width { NumberAnimation { duration: 60 } }
                }
            }

            // During the reach phase the bar is coverage, not time: it says
            // whether the circles are actually reaching the edges, which is
            // what decides whether the calibration is any good.
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                visible: root.phase === "reach"
                text: Math.round(api.padmap.calibrationProgress * 100)
                      + qsTr("% of range covered")
                color: colors.textDim
                font.pixelSize: 13
            }
        }

        // -- icon -----------------------------------------------------------
        Grid {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: root.step === "icon"
            columns: 4
            spacing: 14

            Repeater {
                model: root.iconChoices

                Rectangle {
                    width: 104
                    height: 104
                    radius: colors.radius
                    color: index === root.iconIndex ? colors.accentDim : colors.surface
                    border.width: index === root.iconIndex ? 2 : 1
                    border.color: index === root.iconIndex ? colors.accent : colors.outline

                    Behavior on color { ColorAnimation { duration: 120 } }

                    Image {
                        anchors.centerIn: parent
                        width: 56; height: 56
                        sourceSize.width: 56
                        sourceSize.height: 56
                        fillMode: Image.PreserveAspectFit
                        source: "icons/" + modelData + ".svg"
                        opacity: index === root.iconIndex ? 1.0 : 0.55
                    }

                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            root.iconIndex = index;
                            root.confirmIcon();
                        }
                    }
                }
            }
        }
    }

    Text {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 26
        visible: root.step === "icon"
        text: qsTr("left/right to choose     ·     A to confirm")
        color: colors.textFaint
        font.pixelSize: 13
    }

    // True while configuring a controller for the first time. Used only to
    // surface a hint about button mapping -- see onCalibrationFinished for
    // why this does not open the editor itself.
    property bool firstTime: false

    // set_icon is the last step of the daemon's modal flow, so it also ends
    // it. Do not call finished() here -- the daemon replies with phase "done"
    // and the calibrationFinished signal closes this.
    function confirmIcon() {
        api.padmap.setIcon(root.player, root.iconChoices[root.iconIndex]);
    }

    function abort() {
        api.padmap.configureEnd();
        root.finished();
    }

    Keys.onPressed: function (event) {
        if (root.step === "problem") {
            // Any button dismisses it. There is nothing to configure and the
            // pads are still grabbed, so leaving the user in here with only
            // a specific key that works is how a screen becomes a trap.
            event.accepted = true;
            root.problem = "";
            root.finished();
            return;
        }

        if (root.step === "icon") {
            if (api.keys.isLeft(event)) {
                event.accepted = true;
                root.iconIndex = (root.iconIndex - 1 + root.iconChoices.length)
                                 % root.iconChoices.length;
            } else if (api.keys.isRight(event)) {
                event.accepted = true;
                root.iconIndex = (root.iconIndex + 1) % root.iconChoices.length;
            } else if (api.keys.isAccept(event)) {
                event.accepted = true;
                root.confirmIcon();
            } else if (api.keys.isCancel(event)) {
                event.accepted = true;
                root.abort();
            }
            return;
        }

        // While measuring, swallow everything except an explicit cancel. The
        // daemon reads the pads directly to advance phases, so button presses
        // here are already accounted for -- letting them reach the setup
        // screen underneath is what made setup skip itself.
        if (api.keys.isCancel(event))
            root.abort();
        event.accepted = true;
    }
}
