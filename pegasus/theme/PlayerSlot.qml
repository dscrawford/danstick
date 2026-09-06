import QtQuick 2.15
import QtQuick.Shapes 1.15

// One player slot. Deliberately avoids `required property`: theme QML is
// parsed by whatever Qt the Pegasus build used, and plain properties with
// defaults behave identically here without depending on 5.15+ semantics.
Item {
    id: root

    property int player: 1
    property string padName: ""
    property string padNode: ""
    property bool claimed: false
    // Icon name from the daemon; see padmap/icons.py. Falls back to the
    // generic pad so an unrecognised device still renders something.
    property string icon: "gamepad"
    // 0..1 fill for a hold in flight. Only the next unclaimed slot shows one.
    property real hold: 0

    implicitWidth: 190
    implicitHeight: 250

    Colors { id: colors }

    Rectangle {
        anchors.fill: parent
        radius: colors.radius
        color: root.claimed ? colors.surface : colors.surfaceEmpty
        border.width: root.claimed ? 2 : 1
        border.color: root.claimed ? colors.accent : colors.outline

        Behavior on border.color { ColorAnimation { duration: 180 } }
        Behavior on color { ColorAnimation { duration: 180 } }

        scale: root.claimed ? 1.0 : 0.97
        Behavior on scale {
            NumberAnimation { duration: 220; easing.type: Easing.OutBack }
        }
    }

    Column {
        anchors.centerIn: parent
        spacing: 16
        width: parent.width - 24

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: "PLAYER " + root.player
            color: root.claimed ? colors.accent : colors.textDim
            font.pixelSize: 15
            font.letterSpacing: 2
            font.bold: true
        }

        Item {
            id: indicator
            width: 84
            height: 84
            anchors.horizontalCenter: parent.horizontalCenter

            Rectangle {
                anchors.fill: parent
                radius: width / 2
                color: "transparent"
                border.width: 3
                border.color: colors.outline
                visible: !root.claimed
            }

            Shape {
                anchors.fill: parent
                visible: !root.claimed && root.hold > 0
                ShapePath {
                    strokeWidth: 4
                    strokeColor: colors.accent
                    fillColor: "transparent"
                    capStyle: ShapePath.RoundCap
                    PathAngleArc {
                        centerX: indicator.width / 2
                        centerY: indicator.height / 2
                        radiusX: indicator.width / 2 - 2
                        radiusY: indicator.height / 2 - 2
                        startAngle: -90
                        sweepAngle: 360 * root.hold
                    }
                }
            }

            // Claimed: show which controller took the slot. More useful than
            // a checkmark when several pads look alike in the name column.
            Rectangle {
                anchors.fill: parent
                radius: width / 2
                color: colors.accentDim
                visible: root.claimed

                Image {
                    anchors.centerIn: parent
                    width: 54
                    height: 54
                    // sourceSize keeps the SVG sharp: without it Qt rasterises
                    // at the natural 64px and scales the bitmap.
                    sourceSize.width: 54
                    sourceSize.height: 54
                    fillMode: Image.PreserveAspectFit
                    source: "icons/" + (root.icon || "gamepad") + ".svg"
                }
            }
        }

        Text {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: root.claimed ? root.padName : "press and hold"
            color: root.claimed ? colors.text : colors.textDim
            font.pixelSize: root.claimed ? 14 : 13
            font.italic: !root.claimed
            wrapMode: Text.WordWrap
            maximumLineCount: 2
            elide: Text.ElideRight
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: root.padNode
            color: colors.textFaint
            font.pixelSize: 11
            font.family: "monospace"
            visible: root.claimed
        }
    }
}
