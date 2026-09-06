import QtQuick 2.15

// Box art for one game, or a stand-in when there is none.
//
// Shared by the grid and the list so the two views cannot disagree about what
// "has art" means, and so the no-art case is designed once. On this machine
// RetroArch's thumbnail tree is empty, so the stand-in is not an edge case --
// it is what every entry renders as until a pack is downloaded.
Item {
    id: root

    property string title: ""
    property url art: ""
    property int titleSize: 15
    property bool favorite: false

    // Decoding is deferred to the Image, but the *decision* is here: an
    // Image with an empty source still occupies a texture slot and reports a
    // Null status the delegate would have to special-case.
    readonly property bool hasArt: art != "" && image.status === Image.Ready

    Colors { id: colors }

    // What the picture actually occupies, which is not the item: a cover is
    // letterboxed by PreserveAspectFit and the stand-in plate is narrower
    // still. The favourite badge is placed against these so it sits on the
    // corner of the artwork rather than out in the tile's margin.
    readonly property real artWidth: hasArt ? image.paintedWidth : plate.width
    readonly property real artHeight: hasArt ? image.paintedHeight : plate.height

    Rectangle {
        id: plate
        anchors.centerIn: parent
        // Shaped like a box art, not like the slot. The Image below is
        // letterboxed by PreserveAspectFit, so a plate that filled the slot
        // sat visibly wider than its neighbours in a partly illustrated
        // collection -- the mismatch, not the missing picture, was what made
        // the grid look broken.
        width: Math.min(parent.width, parent.height * 0.72)
        height: parent.height
        radius: colors.radius - 6
        color: colors.plate(root.title)
        visible: !root.hasArt

        // A faint bevel keeps the plate from reading as a hole in the tile.
        Rectangle {
            anchors.fill: parent
            radius: parent.radius
            color: "transparent"
            border.width: 1
            border.color: colors.outline
        }

        Text {
            anchors.centerIn: parent
            text: colors.initials(root.title)
            color: colors.textDim
            font.pixelSize: Math.max(13, Math.min(parent.width, parent.height) * 0.3)
            font.bold: true
            // Slightly transparent so it reads as a placeholder mark rather
            // than as content someone chose to put there.
            opacity: 0.5
        }
    }

    Image {
        id: image
        anchors.fill: parent
        fillMode: Image.PreserveAspectFit
        // Off the render thread: a grid scrolled with a stick would otherwise
        // stall for every cover that comes into view.
        asynchronous: true
        // Bounds the decoded size. Boxart packs ship images far larger than
        // any tile here, and 8302 of them decoded at full resolution is the
        // difference between a working library and an out-of-memory kill.
        //
        // Two buckets rather than a factor of `height`: sourceSize is part of
        // the cache key, so a value that tracks the item size re-decodes the
        // whole visible screen on every window resize, and the list chip and
        // the grid tile stop sharing a cache entry for the same picture.
        sourceSize.height: height > 120 ? 512 : 128
        source: root.art
        visible: root.hasArt
    }

    // Opaque disc rather than a bare glyph: the star has to survive landing on
    // a light patch of somebody's box art, and at the distance a TV is viewed
    // from a thin outlined mark is the first thing to disappear.
    Rectangle {
        readonly property real size: Math.max(20, Math.min(32, root.artWidth * 0.24))

        visible: root.favorite
        width: size
        height: size
        radius: size / 2
        color: colors.background
        border.width: 1
        border.color: colors.accentDim
        // Half off the corner, so it is legible against the picture without
        // covering the part of a cover anyone looks at.
        x: (root.width + root.artWidth) / 2 - size * 0.7
        y: (root.height - root.artHeight) / 2 - size * 0.3

        Text {
            anchors.centerIn: parent
            text: "★"
            color: colors.accent
            font.pixelSize: parent.size * 0.66
        }
    }
}
