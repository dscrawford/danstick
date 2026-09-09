import QtQuick 2.15

// Button mapping: a picture of the controller with an arrow at the control
// being mapped, filled in one at a time.
//
// Everything here is drawn from `layout` -- the body outline, where each
// control sits, what it is called. Nothing about any particular controller is
// written into this file, so a new layout (or a per-game one) is a data entry
// on the daemon side rather than another QML file to keep in step. That is
// also why the picture is drawn rather than an image: artwork and coordinates
// stored separately drift apart, and the drift is invisible until an arrow
// points at the wrong button.
FocusScope {
    id: root

    // Layout as the daemon describes it: {id, label, shapes[], controls[]}.
    property var layout: ({ id: "", label: "", shapes: [], controls: [] })
    property string padName: ""
    property int player: 0

    // Which control is being asked for, and what has been answered so far as
    // canonical name -> short description ("b3", "hat up").
    property int index: 0
    property var captured: ({})

    // Choosing which console the controller is, which comes before mapping it.
    //
    // The same overlay rather than a screen of its own, because the picture is
    // the point: the pad drawn while choosing is the pad the wizard will then
    // ask about, control for control, from one set of coordinates. A separate
    // list of console names could offer a console whose layout says something
    // different, and nothing would notice.
    property bool choosing: false
    // Options from the daemon, each {id, label, mapped, layout: <layout>}.
    // The theme holds no list of its own -- one here would silently fall
    // behind the daemon's, and a console (or a scope) added there would
    // simply never appear.
    //
    // `layout` is a whole layout, not a name, so the picture the user chooses
    // from is drawn from the very coordinates the wizard will point its arrow
    // at. It is separate from `id` because the two differ for a scope: the
    // entry "Nintendo 64 games" acts as `console:n64` and is drawn as an N64
    // pad.
    property var choices: []
    property int choiceIndex: 0
    // The question being asked, as the daemon words it. One picker mechanism
    // asks two things now; a title kept here would be a second place to
    // teach about a third.
    property string chooseTitle: ""

    signal skipRequested()
    signal cancelRequested()

    readonly property var shownLayout: {
        if (root.choosing && root.choices && root.choiceIndex >= 0
                && root.choiceIndex < root.choices.length) {
            // `.layout`, not the entry itself: an option is what is being
            // chosen, and the layout is only the picture of it.
            var picked = root.choices[root.choiceIndex];
            if (picked && picked.layout)
                return picked.layout;
        }
        return root.layout;
    }

    readonly property var controls: shownLayout.controls || []
    readonly property var current: !choosing && index >= 0 && index < controls.length
                                   ? controls[index] : null
    readonly property bool finished: !choosing && index >= controls.length

    Colors { id: colors }

    Rectangle {
        anchors.fill: parent
        color: colors.background
        opacity: 0.97
    }

    Column {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top
        anchors.topMargin: Math.round(parent.height * 0.06)
        spacing: 10
        width: parent.width

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: root.choosing
                  ? (root.chooseTitle || qsTr("Which controller is this?"))
                  : root.finished ? qsTr("All set")
                                  : qsTr("Set up your controller")
            color: colors.text
            font.pixelSize: 28
            font.bold: true
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: "Player " + root.player + "  ·  " + root.padName
                  + (root.choosing || !root.shownLayout.label
                     ? "" : "  ·  " + root.shownLayout.label)
            color: colors.textDim
            font.pixelSize: 15
        }
    }

    // -- the controller -----------------------------------------------------
    Item {
        id: picture

        // 2:1, matching the normalised coordinate space the layouts use.
        width: Math.min(parent.width * 0.72, parent.height * 1.28)
        height: width / 2
        anchors.centerIn: parent

        function px(v) { return v * picture.width }
        function py(v) { return v * picture.height }
        // Radii are fractions of *height*, not width. The canvas is 2:1, so
        // scaling them by width made every circle twice the intended size and
        // the body came out as a row of huge discs.
        function pr(v) { return v * picture.height }

        // Artwork, when the layout names some. Falls back to the drawn
        // shapes if the file is missing, so a layout can never end up with no
        // picture at all.
        Image {
            id: artwork
            anchors.fill: parent
            source: root.shownLayout.image ? Qt.resolvedUrl(root.shownLayout.image) : ""
            fillMode: Image.PreserveAspectFit
            visible: status === Image.Ready
            smooth: true
        }

        // Body. Rectangles and circles cover every layout so far; a polygon
        // would need a Canvas, and none of them has wanted one yet.
        Repeater {
            model: artwork.visible ? [] : (root.shownLayout.shapes || [])
            Rectangle {
                readonly property var s: modelData
                visible: s.kind === "rect" || s.kind === "circle"
                x: s.kind === "circle"
                   ? picture.px(s.points[0]) - width / 2
                   : picture.px(s.points[0])
                y: s.kind === "circle"
                   ? picture.py(s.points[1]) - height / 2
                   : picture.py(s.points[1])
                width: s.kind === "circle"
                       ? picture.pr(s.radius) * 2 : picture.px(s.points[2])
                height: s.kind === "circle"
                        ? picture.pr(s.radius) * 2 : picture.py(s.points[3])
                radius: s.kind === "circle"
                        ? width / 2 : picture.pr(s.radius)
                color: "transparent"
                border.width: 2
                border.color: colors.outline
            }
        }

        // Controls.
        Repeater {
            model: root.controls
            Item {
                readonly property var c: modelData
                readonly property bool isCurrent: !root.choosing && index === root.index
                readonly property bool isDone:
                    !root.choosing && root.captured[c.canonical] !== undefined

                // Shape follows `kind`, which every layout already carries.
                // Drawing everything as a circle made shoulder buttons look
                // like stray dots floating above the body instead of tabs on
                // its top edge.
                readonly property real unit: picture.pr(c.radius)

                x: picture.px(c.x) - width / 2
                y: picture.py(c.y) - height / 2
                width: c.kind === "shoulder" ? unit * 3.0 : unit * 2
                height: c.kind === "shoulder" ? unit * 1.3 : unit * 2

                Rectangle {
                    anchors.fill: parent
                    radius: parent.c.kind === "dpad"
                            ? Math.round(width * 0.28)
                            : height / 2
                    color: parent.isDone ? colors.accent : "transparent"
                    opacity: parent.isDone ? 0.85 : 1
                    border.width: 2
                    border.color: parent.isCurrent
                                  ? colors.accent
                                  : (parent.isDone ? colors.accent
                                                   : colors.textDim)
                }

                // The arrow. A ring alone reads as "selected"; people look for
                // something pointing.
                Canvas {
                    id: arrow
                    visible: parent.isCurrent
                    width: 46
                    height: 46
                    anchors.right: parent.left
                    anchors.rightMargin: 6
                    anchors.verticalCenter: parent.verticalCenter
                    onPaint: {
                        var ctx = getContext("2d");
                        ctx.reset();
                        ctx.fillStyle = colors.accent;
                        ctx.beginPath();
                        ctx.moveTo(width, height / 2);
                        ctx.lineTo(width - 16, height / 2 - 9);
                        ctx.lineTo(width - 16, height / 2 + 9);
                        ctx.closePath();
                        ctx.fill();
                        ctx.strokeStyle = colors.accent;
                        ctx.lineWidth = 3;
                        ctx.beginPath();
                        ctx.moveTo(0, height / 2);
                        ctx.lineTo(width - 15, height / 2);
                        ctx.stroke();
                    }

                    SequentialAnimation on anchors.rightMargin {
                        running: arrow.visible
                        loops: Animation.Infinite
                        NumberAnimation { from: 6; to: 16; duration: 480 }
                        NumberAnimation { from: 16; to: 6; duration: 480 }
                    }
                }
            }
        }
    }

    // -- prompt -------------------------------------------------------------
    Column {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Math.round(parent.height * 0.07)
        spacing: 12
        width: parent.width

        // The console strip. Every layout the daemon offers, in its order,
        // with the selected one lit. Shown as names beside the drawing rather
        // than only as a drawing, because two silhouettes can look alike at a
        // glance and "SNES" cannot be mistaken for "Arcade stick".
        Row {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: root.choosing
            spacing: 10

            Repeater {
                model: root.choices

                Rectangle {
                    readonly property bool isCurrent: index === root.choiceIndex
                    // Something is already recorded under this entry, so
                    // choosing it replaces that. Marked rather than hidden:
                    // re-mapping is the point of the picker, but it should
                    // not be a blind act.
                    readonly property bool isMapped: modelData.mapped === true
                    width: label.width + 28
                    height: 38
                    radius: colors.radius
                    color: isCurrent ? colors.accentDim : colors.surface
                    border.width: isCurrent ? 2 : 1
                    border.color: isCurrent ? colors.accent
                                            : (isMapped ? colors.accentDim
                                                        : colors.outline)

                    Behavior on color { ColorAnimation { duration: 120 } }

                    Text {
                        id: label
                        anchors.centerIn: parent
                        text: parent.isMapped
                              ? modelData.label + " ✓" : modelData.label
                        color: parent.isCurrent ? colors.text : colors.textDim
                        font.pixelSize: 15
                        font.bold: parent.isCurrent
                    }
                }
            }
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: !root.finished && !root.choosing
            text: root.current ? qsTr("Press ") + root.current.label : ""
            color: colors.text
            font.pixelSize: 30
            font.bold: true
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: !root.finished && !root.choosing
            text: (root.index + 1) + " / " + root.controls.length
            color: colors.textDim
            font.pixelSize: 15
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            horizontalAlignment: Text.AlignHCenter
            text: {
                if (root.choosing)
                    // Both gestures need nothing mapped, which is the whole
                    // constraint: the daemon holds the pads, so no controller
                    // input reaches this screen, and nothing is mapped yet
                    // for a named button to refer to.
                    //
                    // The same two gestures for both questions, deliberately.
                    // Whatever is being chosen, the way to choose it must not
                    // change -- there is nothing on screen to teach a second
                    // one with.
                    return qsTr("Push the stick or D-pad left and right to choose.\n"
                                + "Hold any button to continue.");
                if (root.finished)
                    return qsTr("Every control recorded.");
                return qsTr("Not on this controller? Hold any button to skip it.");
            }
            color: colors.textDim
            font.pixelSize: 14
        }
    }

    Keys.onPressed: function (event) {
        // The daemon watches the pad directly to advance -- both while
        // choosing and while mapping -- so presses arriving here can only have
        // come from a keyboard, and the pad's own input is already accounted
        // for. Only the deliberate exits are handled; everything else is
        // swallowed rather than reaching the screen underneath.
        //
        // Deliberately no keyboard navigation for the picker. The daemon owns
        // the selection, and a second index kept here would be the one the
        // user sees while the hold confirms the other. It costs nothing: this
        // is only ever reached by holding a button on the controller, so the
        // controller demonstrably works.
        // Logged because the comment above turned out to be wrong once
        // already, and nothing in either log said so. `event.text` is the
        // cheapest signal for where this came from: a keyboard press carries
        // the character, a gamepad button translated by the front-end does
        // not. If a cancel arrives here with empty text while a mapping is
        // active, the pad is reaching the UI and the daemon's pause is not
        // doing its job -- which is exactly the bug that kept being called
        // fixed. Reads in Pegasus's own lastrun.log.
        console.log("padmap-theme: MappingOverlay key=" + event.key
                    + " text=" + JSON.stringify(event.text)
                    + " repeat=" + event.isAutoRepeat
                    + " isCancel=" + api.keys.isCancel(event)
                    + " choosing=" + root.choosing
                    + " mappingActive=" + api.padmap.mappingActive);
        // Swallow anything the front-end synthesised from a gamepad.
        //
        // The comment above claims presses arriving here can only have come
        // from a keyboard. That was never true, and it is the whole bug: the
        // wizard asks the user to press B, Pegasus translates the pad's B into
        // a key event, `isCancel` matches it, and the step cancels itself with
        // the button it just requested. It cost several wrong fixes further
        // down the stack -- grabs, udev rules, pausing the republished clone --
        // none of which could work, because the front-end reads the pads
        // whatever padmap does. SDL does not honour ID_INPUT_JOYSTICK, so the
        // hide rules never applied to it at all.
        //
        // Telling the two apart is possible, and the log is what showed how:
        // a keyboard Escape arrives as Qt.Key_Escape (0x01000000), while the
        // front-end's gamepad buttons come through in its own 0x100000 block
        // with an empty `text`. Everything in that block is the pad, and the
        // pad's presses belong to the daemon while a wizard is open.
        //
        // Cancel from an actual keyboard still works, which is what keeps this
        // from being a trap: there is always a way out of the wizard.
        var fromGamepad = event.key >= 0x100000 && event.key < 0x1000000;
        if (fromGamepad) {
            event.accepted = true;
            return;
        }
        if (api.keys.isCancel(event)) {
            console.log("padmap-theme: MappingOverlay -> cancelRequested "
                        + "(configureEnd); THIS closes the wizard");
            root.cancelRequested();
        } else if (api.keys.isFilters(event) && !root.choosing) {
            root.skipRequested();
        }
        event.accepted = true;
    }
}
