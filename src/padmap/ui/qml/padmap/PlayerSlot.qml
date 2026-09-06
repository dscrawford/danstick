import QtQuick
import QtQuick.Shapes
import padmap

Item {
    id: root

    required property int player
    required property string padName
    required property string padEvent
    required property bool claimed
    // 0..1 fill for a hold in flight. Only the next unclaimed slot shows one.
    property real hold: 0

    implicitWidth: Theme.slotWidth
    implicitHeight: Theme.slotHeight

    Rectangle {
        id: card
        anchors.fill: parent
        radius: Theme.radius
        color: root.claimed ? Theme.surface : Theme.surfaceEmpty
        border.width: root.claimed ? 2 : 1
        border.color: root.claimed ? Theme.accent : Theme.outline

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
            text: qsTr("PLAYER %1").arg(root.player)
            color: root.claimed ? Theme.accent : Theme.textDim
            font.pixelSize: 15
            font.letterSpacing: 2
            font.bold: true
        }

        Item {
            id: indicator
            width: 84
            height: 84
            anchors.horizontalCenter: parent.horizontalCenter

            // Track
            Rectangle {
                anchors.fill: parent
                radius: width / 2
                color: "transparent"
                border.width: 3
                border.color: Theme.outline
                visible: !root.claimed
            }

            // Fill ring, drawn only while a hold is actually in flight.
            Shape {
                anchors.fill: parent
                visible: !root.claimed && root.hold > 0
                ShapePath {
                    strokeWidth: 4
                    strokeColor: Theme.accent
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

            // Claimed check
            Rectangle {
                anchors.fill: parent
                radius: width / 2
                color: Theme.accentDim
                visible: root.claimed
                Text {
                    anchors.centerIn: parent
                    text: "✓"
                    color: Theme.accent
                    font.pixelSize: 40
                }
            }
        }

        Text {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: root.claimed ? root.padName : qsTr("press and hold")
            color: root.claimed ? Theme.text : Theme.textDim
            font.pixelSize: root.claimed ? 14 : 13
            font.italic: !root.claimed
            wrapMode: Text.WordWrap
            maximumLineCount: 2
            elide: Text.ElideRight
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: root.padEvent
            color: Theme.textDim
            font.pixelSize: 11
            font.family: "monospace"
            visible: root.claimed
        }
    }
}
