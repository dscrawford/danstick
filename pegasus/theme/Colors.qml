import QtQuick 2.15

// Instantiated per-component rather than registered as a qmldir singleton.
// A theme is loaded as a plain QML directory and singleton registration is
// the fiddliest part of that to get right; these are constants, so extra
// instances cost nothing and remove a whole class of load failure.
QtObject {
    readonly property color background: "#1b1b1f"
    readonly property color surface: "#26262c"
    readonly property color surfaceEmpty: "#202026"
    readonly property color outline: "#3a3a44"
    readonly property color accent: "#00b8c4"
    readonly property color accentDim: "#0b5f66"
    readonly property color text: "#f2f2f5"
    readonly property color textDim: "#9a9aa8"
    readonly property color textFaint: "#6d6d7c"
    readonly property color danger: "#e2725b"

    readonly property int radius: 14

    // Stand-in covers for games with no artwork, which on a fresh machine is
    // every game. Muted and low-contrast on purpose: a wall of saturated
    // tiles reads as an error state, and the title printed on top has to stay
    // legible. Picked per game from the title, so a game keeps its colour
    // between sessions instead of shuffling on every load.
    readonly property var plates: [
        "#2f3540", "#33303e", "#2c3a3a", "#3a3330",
        "#303845", "#38323a", "#2e3a33", "#3a3733"
    ]

    function plate(title) {
        var hash = 0;
        for (var i = 0; i < title.length; i++)
            hash = (hash * 31 + title.charCodeAt(i)) & 0x7fffffff;
        return plates[hash % plates.length];
    }

    // Two letters carry a surprising amount of a title: "SM" for Super Mario
    // Sunshine, "BK" for Banjo-Kazooie.
    //
    // Only words that start capitalised or numeric count. Taking the first
    // two words outright gave "LO" for "Legend of Zelda, The" -- the
    // connectives are exactly the words that carry no identity. Articles are
    // dropped for the same reason, and because "TH" for every "The ..."
    // defeats the point.
    function initials(title) {
        var words = title.replace(/[^A-Za-z0-9 ]/g, " ").split(" ");
        var out = "";
        for (var i = 0; i < words.length && out.length < 2; i++) {
            var word = words[i];
            if (word === "" || ["The", "A", "An"].indexOf(word) >= 0)
                continue;
            var first = word[0];
            if (first < "0" || (first > "9" && first < "A") || first > "Z")
                continue;
            out += first;
        }
        return out || title.slice(0, 1).toUpperCase() || "?";
    }
}
