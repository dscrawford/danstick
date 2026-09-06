import QtQuick
import QtQuick.Window
import padmap

Window {
    id: window
    visible: true
    width: 1000
    height: 620
    title: qsTr("padmap — controller setup")
    color: Theme.background

    SetupScreen {
        anchors.fill: parent
        model: setup
    }
}
