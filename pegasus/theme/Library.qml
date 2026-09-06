import QtQuick 2.15
import QtQml.Models 2.15

// The library: one tab per collection, and either a cover grid or a list
// depending on whether that collection has any artwork.
//
// Two structural decisions worth knowing before editing.
//
// The unfiltered model is the collection's own `games` list, not a copy.
// Building a JS array of 8302 arcade entries costs a wrapper object per game
// every time the view changes; handing the ObjectListModel straight to the
// view costs nothing and keeps Qt's delegate recycling intact, so only the
// covers actually on screen are ever loaded. The array is built only while a
// search is active, where the result is small by construction. Favourites are
// floated to the top by permuting a DelegateModel, for the same reason: it
// moves ranges inside the model wrapper and never materialises the games.
//
// Focus moves between the tab bar and the games, rather than the tab bar
// being a thing you can only reach with a shoulder button. Left/right means
// "switch tab" up in the bar and "move the cursor" down in the grid, which is
// what a TV front-end is expected to do; the shoulder buttons switch tabs
// from either place for people who never leave the grid.
FocusScope {
    id: root

    signal openControllerSetup()

    property string query: ""
    property var filtered: []
    property int tab: 0

    // Search is a mode, not a default.
    //
    // Type-to-search cannot coexist with letter keybindings: Pegasus binds
    // Details to `I`, Filters to `F`, prev/next page to `Q`/`E`/`A`/`D`, so
    // typing any of those triggered an action instead of filtering. Requiring
    // an explicit entry key means letters are unambiguous in both modes.
    property bool searching: false

    // Favourites of the current view, and how many there are. Pegasus owns
    // the flag: `game.favorite` is a writable property on its Game objects and
    // the front-end mirrors every change into ~/.config/pegasus-frontend/
    // favorites.txt itself, so this screen only has to set it.
    property int favoriteCount: 0

    // Source-model indices of the favourites in each collection, keyed by tab.
    //
    // One pass over the collection is what finding them costs, and for arcade
    // that is 8302 property reads -- affordable once, not on every tab switch.
    // Nothing else changes the flag while the theme is up: the provider that
    // reads favorites.txt has finished long before gamedataReady fires, so the
    // cache can only go stale through this screen's own toggle, which drops
    // the one entry it invalidated.
    property var favCache: ({})

    // The same thing for a search result: indices into `filtered`.
    property var filteredFavorites: []

    // Transient line in place of the key hints, for the things that would
    // otherwise happen invisibly -- see toggleFavorite().
    property string note: ""

    readonly property int collectionCount: api.collections ? api.collections.count : 0
    readonly property var collection: collectionCount > 0 && tab < collectionCount
                                      ? api.collections.get(tab) : null
    readonly property var games: collection ? collection.games : null

    // The view's model. An ObjectListModel while browsing, a plain array
    // while searching -- both expose the game as `modelData` in a delegate,
    // so the delegates do not care which is in play.
    readonly property var shownModel: searching || query !== "" ? filtered : games
    readonly property int shownCount: searching || query !== ""
                                      ? filtered.length
                                      : (games ? games.count : 0)

    readonly property bool artMode: collectionHasArt(tab)

    // Whichever of the two views is live. Both exist at all times, so every
    // "the games" operation has to go through this rather than name one.
    readonly property var view: artMode ? grid : list

    Colors { id: colors }

    // Whether a collection is worth showing as covers, cached per tab.
    //
    // Sampled rather than counted in full. Thumbnails arrive as a per-system
    // pack, so the first entries are representative of the rest, and reading
    // `assets.boxFront` for all 8302 arcade games is a visible stall on every
    // tab switch. Partial coverage is not lost by guessing "list": rows carry
    // a cover too, just a small one.
    property var artCache: ({})

    function collectionHasArt(index) {
        if (index < 0 || index >= collectionCount)
            return false;
        if (artCache[index] !== undefined)
            return artCache[index];

        // Looked up by index rather than reusing `games`, so the answer
        // cached under a key is always the answer for that key. They agree
        // today because the only caller passes the current tab, and that is
        // exactly the kind of thing that stops being true quietly.
        var entries = api.collections.get(index).games;
        var limit = Math.min(entries.count, 400);
        var found = false;
        for (var i = 0; i < limit; i++) {
            if (entries.get(i).assets.boxFront != "") {
                found = true;
                break;
            }
        }
        artCache[index] = found;
        return found;
    }

    // MAME grades every driver, and "preliminary" is the grade behind its own
    // red "THIS GAME DOES NOT WORK" screen. `x-mame-status` carries it through
    // the collection file into `game.extra`; anything without a grade -- which
    // is nearly everything outside arcade -- must read as fine rather than as
    // broken.
    function driverStatus(game) {
        return (game && game.extra && game.extra["mame-status"]) || "";
    }

    function isBroken(game) {
        return driverStatus(game) === "preliminary";
    }

    function favoriteIndices(index) {
        if (index < 0 || index >= collectionCount)
            return [];
        if (favCache[index] !== undefined)
            return favCache[index];

        var entries = api.collections.get(index).games;
        var out = [];
        // A front-end that does not implement the flag answers undefined for
        // every game, and asking 8302 of them costs the same as asking one.
        // Sampling the first entry is enough to know there is nothing to find.
        if (entries.count > 0 && entries.get(0).favorite === undefined) {
            favCache[index] = out;
            return out;
        }
        for (var i = 0; i < entries.count; i++) {
            // Compared against true rather than taken as truthy: `favorite` is
            // absent on some entries of a mixed model, and that has to mean
            // "not a favourite" rather than a crash.
            if (entries.get(i).favorite === true)
                out.push(i);
        }
        favCache[index] = out;
        return out;
    }

    // Favourites to the top of whatever is being shown.
    //
    // The order lives in the DelegateModel, not in a sorted list of games:
    // `items.move` shuffles ranges inside the model wrapper, so the 8302-entry
    // ObjectListModel is still the model and no game is copied anywhere. It
    // costs one move per favourite -- a handful -- against the length of the
    // library.
    //
    // Clearing `model` first is what makes it repeatable. Moves are
    // cumulative, so a second pass over a group that is already permuted
    // treats source indices as if they were visual ones and scrambles the
    // order it was asked to produce; assigning null returns the group to
    // source order, which measured as free even on the arcade collection.
    function floatFavorites(order, indices) {
        order.model = null;
        order.model = shownModel;
        for (var k = 0; k < indices.length; k++)
            order.items.move(indices[k], k, 1);
    }

    // Both views, always. The hidden one becomes visible the moment a tab with
    // different art coverage is selected, and it has to already be in order.
    function applyOrder() {
        var indices = searching || query !== ""
                      ? filteredFavorites : favoriteIndices(tab);
        favoriteCount = indices.length;
        floatFavorites(gridOrder, indices);
        floatFavorites(listOrder, indices);
    }

    // Marking a game does not move it.
    //
    // Sorting on the spot means the row under the cursor leaves while you are
    // looking at it: either the cursor follows the game to the top and you
    // lose your place in the alphabet, or it stays on an index that is now a
    // different game. Both are worse than waiting. So the star appears at
    // once and the order is applied the next time the tab is opened -- which
    // is when "favourites first" is worth something, and the one moment where
    // moving things is not disorienting because the view is starting fresh
    // anyway.
    function toggleFavorite() {
        var game = view.currentItem ? view.currentItem.game : null;
        if (!game)
            return;

        var now = game.favorite !== true;
        game.favorite = now;

        // The cached scan for this collection is stale now. Dropping it is
        // the whole update: it is rebuilt on the next tab entry, which is also
        // when the new order is applied.
        delete favCache[tab];
        favoriteCount += now ? 1 : -1;

        say(now ? "★ Added to favourites — moves to the top next time you open "
                  + collection.name
                : "Removed from favourites");
    }

    // R2 on a pad, Page Down on a keyboard.
    //
    // Everything nearer to hand is spoken for: Accept launches, Cancel clears
    // the filter, Details opens the controller order screen, Filters searches,
    // L1/R1 change console. That leaves Pegasus's two trigger bindings, and a
    // trigger is a good home for something that changes state -- it is not a
    // button you brush past while moving around a grid.
    //
    // The predicate is checked for existence first because api.keys grew its
    // set over releases: calling one that is not there is a TypeError on the
    // first key press, which takes the entire screen down rather than just
    // losing the feature.
    function isFavoriteKey(event) {
        if (api.keys.isPageDown)
            return api.keys.isPageDown(event);
        return event.key === Qt.Key_PageDown;
    }

    function say(text) {
        note = text;
        noteTimer.restart();
    }

    Timer {
        id: noteTimer
        interval: 4000
        onTriggered: root.note = ""
    }

    function beginSearch() {
        searching = true;
    }

    function endSearch(clear) {
        searching = false;
        if (clear && query !== "") {
            query = "";
            rebuild();
        }
    }

    function rebuild() {
        var out = [];
        var favs = [];
        if (games && query !== "") {
            var needle = query.toLowerCase();
            for (var i = 0; i < games.count; i++) {
                var game = games.get(i);
                if (game.title.toLowerCase().indexOf(needle) >= 0) {
                    if (game.favorite === true)
                        favs.push(out.length);
                    out.push(game);
                }
            }
        }
        // Favourites first, because assigning `filtered` is itself what tells
        // the views to re-seat, and by then the indices have to be right.
        filteredFavorites = favs;
        filtered = out;

        // Before the cursor is placed, not after: re-seating the model resets
        // the views' currentIndex, so an index set first would not survive.
        applyOrder();

        // Both, not just the live one: the other keeps whatever index it was
        // left on, and it becomes live again the moment a tab with different
        // art coverage is selected.
        var at = shownCount > 0 ? 0 : -1;
        grid.currentIndex = at;
        list.currentIndex = at;
    }

    function selectTab(index) {
        if (collectionCount === 0)
            return;
        // Wraps, because a tab bar you can get stuck at the end of is worse
        // on a controller than one that cycles.
        tab = (index + collectionCount) % collectionCount;
        // A query is about the collection it was typed in; carrying it across
        // tabs mostly produced an empty screen and a hunt for why.
        query = "";
        searching = false;
        rebuild();
        grid.positionViewAtBeginning();
        list.positionViewAtBeginning();
    }

    function launchCurrent() {
        if (view.currentItem && view.currentItem.game)
            view.currentItem.game.launch();
    }

    // Focus has to be pushed down twice: to the FocusScope holding the two
    // views, and then to whichever of them is live. Setting only the inner
    // one leaves it with focus but no activeFocus, and the grid silently
    // stops answering the d-pad.
    function focusGames() {
        content.focus = true;
        if (artMode)
            grid.focus = true;
        else
            list.focus = true;
    }

    onArtModeChanged: if (content.activeFocus) focusGames()

    // The views' models are seated by hand rather than bound, so something has
    // to notice when the thing to show changes. Doing it here rather than only
    // in rebuild() keeps `tab` usable as a plain property -- the preview and
    // the checks set it directly, and so would anything else that drives this
    // screen from outside.
    onShownModelChanged: applyOrder()

    Component.onCompleted: {
        rebuild();
        focusGames();
    }

    Connections {
        target: api
        function onGamedataReady() {
            // Collections arrive after the first paint, so anything cached
            // from the empty model is meaningless now. This is also the point
            // where the favourites read out of favorites.txt exist, so the
            // scan below is the first one that can find any.
            root.artCache = ({});
            root.favCache = ({});
            root.rebuild();
        }
    }

    Rectangle {
        anchors.fill: parent
        color: colors.background
    }

    Column {
        anchors.fill: parent
        anchors.margins: 36
        spacing: 16

        Row {
            width: parent.width
            spacing: 16

            Text {
                text: "Library"
                color: colors.text
                font.pixelSize: 28
                font.bold: true
                anchors.verticalCenter: parent.verticalCenter
            }

            Rectangle {
                width: 360
                height: 40
                radius: 8
                color: root.searching ? colors.surface : colors.surfaceEmpty
                border.width: root.searching ? 2 : 1
                border.color: root.searching ? colors.accent
                              : (root.query === "" ? colors.outline : colors.accentDim)
                anchors.verticalCenter: parent.verticalCenter

                Behavior on border.color { ColorAnimation { duration: 120 } }

                Text {
                    anchors.left: parent.left
                    anchors.leftMargin: 14
                    anchors.right: caret.left
                    anchors.verticalCenter: parent.verticalCenter
                    elide: Text.ElideLeft
                    text: {
                        if (root.query !== "")
                            return root.query;
                        return root.searching ? "" : "Filters to search";
                    }
                    color: root.query === "" ? colors.textFaint : colors.text
                    font.pixelSize: 15
                    font.italic: root.query === ""
                }

                // Only blinks while the field is actually taking input, so it
                // is obvious which mode you are in.
                Rectangle {
                    id: caret
                    width: 2
                    height: 20
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.right: parent.right
                    anchors.rightMargin: 14
                    color: colors.accent
                    visible: root.searching

                    SequentialAnimation on opacity {
                        running: root.searching
                        loops: Animation.Infinite
                        NumberAnimation { to: 0; duration: 500 }
                        NumberAnimation { to: 1; duration: 500 }
                    }
                }
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: root.query === ""
                      ? root.shownCount + " games"
                      : root.shownCount + " / " + (root.games ? root.games.count : 0)
                color: colors.textDim
                font.pixelSize: 14
            }

            // How many of them are favourites. Free -- the number is a
            // by-product of the scan that put them at the top.
            Text {
                anchors.verticalCenter: parent.verticalCenter
                visible: root.favoriteCount > 0
                text: "★ " + root.favoriteCount
                color: colors.accent
                font.pixelSize: 14
                font.bold: true
            }
        }

        // -- tab bar ------------------------------------------------------
        FocusScope {
            id: tabBar
            width: parent.width
            height: 44

            Row {
                id: tabs
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                spacing: 10

                Repeater {
                    model: api.collections

                    Rectangle {
                        readonly property bool active: index === root.tab

                        width: label.implicitWidth + 34
                        height: 38
                        radius: 19
                        color: active ? colors.surface : colors.surfaceEmpty
                        border.width: active ? 2 : 1
                        border.color: {
                            if (active && tabBar.activeFocus)
                                return colors.accent;
                            return active ? colors.accentDim : colors.outline;
                        }

                        Behavior on border.color { ColorAnimation { duration: 120 } }

                        Text {
                            id: label
                            anchors.centerIn: parent
                            text: modelData.name
                            color: active ? colors.text : colors.textDim
                            font.pixelSize: 15
                            font.bold: active
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: {
                                root.selectTab(index);
                                tabBar.focus = true;
                            }
                        }
                    }
                }
            }

            // Only meaningful while the bar has focus; in the grid the
            // shoulder hint in the footer covers it.
            Text {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                visible: tabBar.activeFocus
                text: "left / right to switch     ·     down for games"
                color: colors.textFaint
                font.pixelSize: 13
            }

            Keys.onPressed: function (event) {
                if (event.key === Qt.Key_Left) {
                    event.accepted = true;
                    root.selectTab(root.tab - 1);
                } else if (event.key === Qt.Key_Right) {
                    event.accepted = true;
                    root.selectTab(root.tab + 1);
                } else if (event.key === Qt.Key_Down
                           || api.keys.isAccept(event)) {
                    event.accepted = true;
                    root.focusGames();
                }
            }
        }

        // -- games --------------------------------------------------------
        //
        // Both views are instantiated and only one is shown. Swapping a
        // Loader's component instead would rebuild the view on every tab
        // change, losing the scroll position and re-deciding focus each time.
        FocusScope {
            id: content
            width: parent.width
            // Stops short of the bottom so the key hints below are not
            // overlapped by whatever row happens to be half-scrolled there.
            height: parent.height - y - 26

            GridView {
                id: grid
                anchors.fill: parent
                visible: root.artMode
                clip: true

                // The model is seated by applyOrder() rather than bound, so
                // that clearing it -- the only way back to source order -- is
                // not immediately undone by the binding.
                model: DelegateModel {
                    id: gridOrder
                    delegate: gridTile
                }

                // Whole columns only, so the last one is not clipped by the
                // right edge at whatever resolution the TV happens to be.
                readonly property int columns: Math.max(3, Math.floor(width / 190))
                cellWidth: Math.floor(width / columns)
                cellHeight: Math.floor(cellWidth * 1.42)

                // Enough to fill roughly a screen ahead, so a stick-held
                // scroll has covers ready without decoding the whole library.
                cacheBuffer: cellHeight * 3

                Keys.onPressed: function (event) {
                    // Up out of the first row is how the tab bar is reached
                    // without a shoulder button.
                    if (event.key === Qt.Key_Up && currentIndex < columns) {
                        event.accepted = true;
                        tabBar.focus = true;
                    }
                }
            }

            ListView {
                id: list
                anchors.fill: parent
                visible: !root.artMode
                clip: true
                model: DelegateModel {
                    id: listOrder
                    delegate: listRow
                }
                cacheBuffer: 400

                Keys.onPressed: function (event) {
                    if (event.key === Qt.Key_Up && currentIndex <= 0) {
                        event.accepted = true;
                        tabBar.focus = true;
                    }
                }
            }

            Text {
                anchors.centerIn: parent
                width: parent.width * 0.7
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                visible: root.shownCount === 0
                text: {
                    if (root.collectionCount === 0)
                        return "No collections found. Run `padmap export-pegasus`, "
                             + "then check ~/.config/pegasus-frontend/game_dirs.txt";
                    if (root.query !== "")
                        return "Nothing in " + root.collection.name
                             + " matches \"" + root.query + "\"";
                    return "This collection is empty.";
                }
                color: colors.textDim
                font.pixelSize: 16
            }
        }
    }

    Component {
        id: gridTile

        Item {
            property var game: modelData
            readonly property bool current: GridView.isCurrentItem

            width: grid.cellWidth
            height: grid.cellHeight

            Rectangle {
                anchors.fill: parent
                anchors.margins: 6
                radius: colors.radius
                color: colors.surface
                border.width: parent.current ? 2 : 1
                border.color: parent.current && grid.activeFocus
                              ? colors.accent
                              : (parent.current ? colors.accentDim : colors.outline)

                Behavior on border.color { ColorAnimation { duration: 120 } }

                Column {
                    anchors.fill: parent
                    anchors.margins: 10
                    spacing: 8

                    GameArt {
                        width: parent.width
                        height: parent.height - 44
                        title: modelData.title
                        art: modelData.assets.boxFront || modelData.assets.titlescreen
                             || modelData.assets.screenshot || ""
                        // On the cover rather than on the tile: the cover is
                        // letterboxed inside the tile, and a badge in the
                        // tile's corner floats in dead space next to a narrow
                        // one instead of sitting on the picture.
                        favorite: modelData.favorite === true
                        // Same reasoning as the badge: dim the picture rather
                        // than hide the tile, so a set that does not run is
                        // still findable.
                        dimmed: root.isBroken(modelData)
                    }

                    Text {
                        width: parent.width
                        text: root.isBroken(modelData)
                              ? qsTr("⚠ ") + modelData.title : modelData.title
                        color: root.isBroken(modelData)
                               ? colors.danger : colors.text
                        font.pixelSize: 13
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                        maximumLineCount: 2
                        elide: Text.ElideRight
                    }
                }
            }
        }
    }

    Component {
        id: listRow

        Item {
            property var game: modelData
            readonly property bool current: ListView.isCurrentItem

            width: list.width
            // Dense on purpose. This view exists because a collection has no
            // covers, and the compensation for losing the pictures is seeing
            // three times as many titles at once.
            height: 54

            Rectangle {
                anchors.fill: parent
                anchors.rightMargin: 10
                anchors.bottomMargin: 3
                radius: colors.radius - 6
                color: parent.current ? colors.surface : "transparent"
                border.width: parent.current ? 2 : 0
                border.color: list.activeFocus ? colors.accent : colors.accentDim

                Behavior on border.color { ColorAnimation { duration: 120 } }
            }

            // Kept even in the no-art view. A collection with partial
            // coverage still shows what it has, and with none at all the
            // initials give the eye something to track down a list of 8302
            // otherwise identical rows.
            GameArt {
                id: chip
                anchors.left: parent.left
                anchors.leftMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                width: 34
                height: 42
                title: modelData.title
                art: modelData.assets.boxFront || modelData.assets.titlescreen
                     || modelData.assets.screenshot || ""
            }

            // A column of its own, kept even for the rows that have no star,
            // so the titles stay aligned and the stars line up into something
            // you can find with your eye from across the room instead of a
            // mark that shifts row to row.
            Text {
                id: star
                anchors.left: chip.right
                anchors.leftMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                width: 20
                text: "★"
                color: colors.accent
                font.pixelSize: 19
                visible: modelData.favorite === true
            }

            Text {
                id: rowTitle
                anchors.left: star.right
                anchors.leftMargin: 6
                anchors.right: broken.left
                anchors.rightMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                text: modelData.title
                // Dimmed rather than hidden. A MAME set that does not run is
                // still worth seeing -- it is why the ROM is there -- but it
                // should not read the same as one that works.
                color: root.isBroken(modelData) ? colors.textFaint : colors.text
                font.pixelSize: 17
                elide: Text.ElideRight
            }

            // Words, not an icon: "does not work" is the whole message, and
            // an unlabelled glyph next to a dimmed title is a puzzle.
            Rectangle {
                id: broken
                visible: root.isBroken(modelData)
                anchors.right: meta.left
                anchors.rightMargin: 20
                anchors.verticalCenter: parent.verticalCenter
                // Zero-width when absent, so the title simply extends into
                // the space instead of the two of them anchoring to each
                // other conditionally.
                width: visible ? brokenLabel.implicitWidth + 14 : 0
                height: 22
                radius: 4
                color: "transparent"
                border.width: 1
                border.color: colors.danger

                Text {
                    id: brokenLabel
                    anchors.centerIn: parent
                    text: qsTr("not working")
                    color: colors.danger
                    font.pixelSize: 12
                }
            }

            // Right-aligned rather than stacked under the title: it keeps the
            // rows one line tall, and year and maker line up into columns you
            // can scan down instead of reading per row.
            Text {
                id: meta
                anchors.right: parent.right
                anchors.rightMargin: 32
                anchors.verticalCenter: parent.verticalCenter
                width: Math.min(implicitWidth, list.width * 0.34)
                horizontalAlignment: Text.AlignRight
                // Both fields are frequently absent outside arcade, so the
                // separator has to be conditional or the row reads as " · ".
                text: {
                    var year = modelData.releaseYear > 0
                               ? String(modelData.releaseYear) : "";
                    var maker = modelData.developer || "";
                    if (year && maker)
                        return year + "   ·   " + maker;
                    return year || maker;
                }
                color: colors.textFaint
                font.pixelSize: 14
                elide: Text.ElideRight
            }
        }
    }

    Text {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 14
        // Doubles as the place transient messages appear. It is the line the
        // eye already goes to for "what did that key do", and borrowing it
        // avoids a second floating panel that could land on top of the one
        // theme.qml puts here.
        text: {
            if (root.note !== "")
                return root.note;
            if (root.searching)
                return "type to filter     ·     Enter to keep it     ·     Esc to clear";
            return "Accept launch     ·     L1/R1 switch console"
                 + "     ·     Filters search     ·     R2 favourite"
                 + "     ·     Details controller order";
        }
        color: root.note !== "" ? colors.accent : colors.textFaint
        font.pixelSize: 13

        Behavior on color { ColorAnimation { duration: 120 } }
    }

    // Fallback handler: keys the focused view did not accept arrive here, so
    // the shoulder buttons and the search mode work from the tab bar and from
    // either list without being wired into both.
    Keys.onPressed: function (event) {
        // While searching, letters are text and nothing else. Only Escape and
        // Enter get out, so a query containing "i" or "f" cannot fire an
        // action mid-word.
        if (root.searching) {
            if (event.key === Qt.Key_Escape) {
                event.accepted = true;
                root.endSearch(true);
            } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                event.accepted = true;
                root.endSearch(false);   // keep the filter, return to the grid
            } else if (event.key === Qt.Key_Backspace) {
                event.accepted = true;
                root.query = root.query.slice(0, -1);
                root.rebuild();
            } else if (event.text.length === 1 && event.text >= " ") {
                event.accepted = true;
                root.query += event.text;
                root.rebuild();
            }
            return;
        }

        if (api.keys.isPrevPage(event)) {
            event.accepted = true;
            root.selectTab(root.tab - 1);
            return;
        }
        if (api.keys.isNextPage(event)) {
            event.accepted = true;
            root.selectTab(root.tab + 1);
            return;
        }
        if (api.keys.isFilters(event)) {
            event.accepted = true;
            root.beginSearch();
            return;
        }
        if (api.keys.isDetails(event)) {
            event.accepted = true;
            root.openControllerSetup();
            return;
        }
        if (root.isFavoriteKey(event)) {
            event.accepted = true;
            root.toggleFavorite();
            return;
        }
        if (api.keys.isAccept(event)) {
            event.accepted = true;
            root.launchCurrent();
            return;
        }
        if (api.keys.isCancel(event) && root.query !== "") {
            // Clear an active filter before Cancel means anything else.
            event.accepted = true;
            root.endSearch(true);
        }
    }
}
