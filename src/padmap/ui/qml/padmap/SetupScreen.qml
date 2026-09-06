import QtQuick
import QtQuick.Shapes

// The setup screen proper, as an Item rather than a Window, so it can be
// pushed onto a StackView later and rendered standalone by tools/smoke_qml.py.
Item {
    id: root

    // Injected rather than referenced globally, so this stays renderable
    // against a stub model.
    required property var model

    Column {
        anchors.centerIn: parent
        spacing: 34
        width: parent.width

        Column {
            width: parent.width
            spacing: 8

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: qsTr("Controller Order")
                color: Theme.text
                font.pixelSize: 30
                font.bold: true
            }

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: qsTr("Press and hold a button on each controller, "
                           + "in the order you want them assigned.")
                color: Theme.textDim
                font.pixelSize: 15
            }
        }

        Row {
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: 20

            Repeater {
                model: root.model.slots

                PlayerSlot {
                    required property var modelData
                    required property int index

                    player: modelData.player
                    padName: modelData.name
                    padEvent: modelData.event
                    claimed: modelData.claimed
                    // The hold in flight always fills the first empty slot,
                    // since claims are handed out in order.
                    hold: index === root.model.claimedCount ? root.model.hold : 0
                }
            }
        }

        Item {
            width: parent.width
            height: 54

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: parent.top
                text: root.model.status
                color: Theme.textDim
                font.pixelSize: 14
            }

            // Confirm progress, visible only while a claimed pad is held.
            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: parent.bottom
                width: 260
                height: 6
                radius: 3
                color: Theme.outline
                visible: root.model.confirmHold > 0

                Rectangle {
                    width: parent.width * root.model.confirmHold
                    height: parent.height
                    radius: parent.radius
                    color: Theme.accent
                }
            }
        }
    }

    Text {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 22
        text: qsTr("hold again on an assigned controller to continue"
                   + "     ·     R to reset     ·     Esc to quit")
        color: Theme.textFaint
        font.pixelSize: 13
    }

    // Keyboard fallbacks. The controller path is the intended one, but a pad
    // that will not claim a slot leaves no other way out.
    focus: true
    Keys.onPressed: function (event) {
        if (event.key === Qt.Key_Escape) {
            Qt.quit();
        } else if (event.key === Qt.Key_R) {
            root.model.reset();
        } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            root.model.accept();
        }
    }
}
