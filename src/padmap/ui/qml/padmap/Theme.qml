pragma Singleton
import QtQuick

QtObject {
    readonly property color background: "#1b1b1f"
    readonly property color surface: "#26262c"
    readonly property color surfaceEmpty: "#202026"
    readonly property color outline: "#3a3a44"
    readonly property color accent: "#00b8c4"
    readonly property color accentDim: "#0b5f66"
    readonly property color text: "#f2f2f5"
    readonly property color textDim: "#9a9aa8"
    // For the keybinding footer. `outline` is a border colour and is far too
    // low-contrast against `background` to read as text.
    readonly property color textFaint: "#6d6d7c"

    readonly property int radius: 14
    readonly property int slotWidth: 190
    readonly property int slotHeight: 250
}
