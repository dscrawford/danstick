import QtQuick 2.15

// Controller order screen, driven entirely by the padmap daemon through
// `api.padmap`. This file holds no assignment logic: it renders whatever
// state the daemon reports and sends four commands back.
FocusScope {
    id: root

    signal closed()

    Colors { id: colors }

    // Opening the screen starts a session, which grabs the pads. Leaving it
    // by any route has to cancel, or the pads stay grabbed and the rest of
    // Pegasus goes deaf.
    function open() {
        // Only start a session if one is not already running. This screen can
        // be opened either by the user pressing Details or by the daemon
        // already being in `assigning`; calling begin again in the second
        // case would restart the session and discard existing claims.
        root.offered = ({});
        root.claimedHere = ({});
        if (api.padmap.state !== "assigning")
            api.padmap.begin(4);

        // Deliberately does NOT offer setup here. `players` at this moment is
        // whatever the last state event left behind, which can describe the
        // *previous* session -- the daemon opens this screen by itself now
        // when it sees an unfamiliar controller, and it does that from a
        // state that already had players in it. Offering then asked the
        // daemon to calibrate a player the new session holds no claim for,
        // which it rightly refuses, leaving the overlay on "Starting..."
        // forever. Nothing is offered until a controller has actually
        // claimed a slot.
    }

    function leave() {
        api.padmap.cancel();
        root.closed();
    }

    // Players already offered setup this session, so skipping one does not
    // re-prompt on the next state event.
    property var offered: ({})

    // Players that claimed a slot *on this screen*, as player -> true.
    //
    // Configuration follows a press, and only a press. A player already in
    // `players` when the screen opens belongs to a session that is over --
    // the daemon opens this screen by itself now, from a state that already
    // had players in it -- and asking it to calibrate one produces "no
    // controller assigned to player N" and an overlay with nothing to show.
    property var claimedHere: ({})



    // Set once a controller has been configured for the first time, to hint
    // that button mapping exists without forcing anyone into it.
    property bool mappingSuggested: false

    Connections {
        target: api.padmap

        function onAccepted() {
            root.closed();
        }

        function onMappingFinished(stored) {
            // Accepting is what writes the RetroArch profile and the SDL
            // mapping, so it has to follow the capture rather than precede
            // it. Abandoning the wizard keeps the assignment but writes no
            // bindings.
            api.padmap.accept();
        }

        function onClaimed(player, name, node) {
            root.claimedHere[player] = true;
            // Try straight away, and again from the state event: the claim
            // signal and the state event carrying the new player arrive
            // separately and in no guaranteed order, so whichever lands
            // second is the one that finds a populated players list.
            root.maybeOfferSetup();
        }

        function onStateChanged() {
            root.maybeOfferSetup();
        }
    }

    // Offer calibration the first time a model of controller is seen. The
    // daemon decides what counts as "seen" -- it owns the profile store --
    // and reports it as `configured` on each player.
    function maybeOfferSetup() {
        if (calibration.player !== 0)
            return;
        if (api.padmap.state !== "assigning")
            return;

        var players = api.padmap.players;
        for (var i = 0; i < players.length; i++) {
            var entry = players[i];
            // Only a controller that pressed something on this screen. See
            // claimedHere.
            if (!root.claimedHere[entry.player])
                continue;
            if (entry.configured)
                continue;
            if (root.offered[entry.player])
                continue;

            root.offered[entry.player] = true;

            // First time this controller has been seen: ask which console it
            // is, then walk its buttons with padmap's own wizard. The daemon
            // reads the pad directly for both, so they work while the pads
            // are grabbed -- unlike Pegasus's Gamepad Editor, which reads
            // through SDL and therefore cannot run during a session at all.
            // That is still reachable deliberately with Next-page.
            //
            // The console is asked rather than assumed because the layout
            // decides which prompts the wizard shows: an N64 pad walked
            // through the generic gamepad is asked for an X, a Y and two
            // analogue triggers it does not have.
            api.padmap.chooseLayout(entry.player);
            return;
        }
    }

    Rectangle {
        anchors.fill: parent
        color: colors.background
    }

    Column {
        anchors.centerIn: parent
        spacing: 34
        width: parent.width

        Column {
            width: parent.width
            spacing: 8

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: "Controller Order"
                color: colors.text
                font.pixelSize: 30
                font.bold: true
            }

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: "Press and hold a button on each controller, "
                      + "in the order you want them assigned."
                color: colors.textDim
                font.pixelSize: 15
            }
        }

        Row {
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: 20

            Repeater {
                model: api.padmap.slotCount

                PlayerSlot {
                    property var claim: index < api.padmap.players.length
                                        ? api.padmap.players[index]
                                        : null

                    player: claim ? claim.player : index + 1
                    padName: claim ? claim.name : ""
                    padNode: claim ? claim.node : ""
                    icon: claim ? (claim.icon || "gamepad") : "gamepad"
                    claimed: claim !== null
                    // Claims are handed out in order, so a hold in flight
                    // always belongs to the first empty slot.
                    hold: index === api.padmap.players.length
                          ? api.padmap.hold : 0
                }
            }
        }

        Item {
            width: parent.width
            height: 54

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: parent.top
                text: {
                    if (!api.padmap.connected)
                        return "padmap daemon is not running — start it with `padmap serve`";
                    var n = api.padmap.players.length;
                    if (n === 0)
                        return "Hold a button to claim Player 1.";
                    if (n >= api.padmap.slotCount)
                        return "All slots filled. Hold again to continue.";
                    return "Player " + n + " set. Hold a button for Player "
                           + (n + 1) + ", or hold again to continue.";
                }
                color: api.padmap.connected ? colors.textDim : colors.danger
                font.pixelSize: 14
            }

            // Surfaced rather than acted on. Opening the button editor needs
            // the pads released, which ends the session -- so it has to be
            // the user's decision, made when they are finished assigning.
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: parent.top
                anchors.topMargin: 22
                visible: root.mappingSuggested
                text: "Buttons feel wrong? Prev-page maps them again — for "
                      + "every game, one console, or the game you just played."
                color: colors.textFaint
                font.pixelSize: 13
            }

            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: parent.bottom
                width: 260
                height: 6
                radius: 3
                color: colors.outline
                visible: api.padmap.confirmHold > 0

                Rectangle {
                    width: parent.width * api.padmap.confirmHold
                    height: parent.height
                    radius: parent.radius
                    color: colors.accent
                }
            }
        }
    }

    Text {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 22
        text: "hold again to continue     ·     Filters reset"
              + "     ·     Details recalibrate     ·     Prev-page map for…"
              // Only advertised once a slot is filled, since that is exactly
              // when the key does anything.
              + (api.padmap.players.length > 0
                 ? "     ·     1-" + api.padmap.players.length
                   + " start a controller over"
                 : "")
              + "     ·     Next-page gamepad editor     ·     Cancel back"
        color: colors.textFaint
        font.pixelSize: 13
    }

    // Name of a player's controller, for the overlay's subtitle.
    function padNameFor(player) {
        var players = api.padmap.players;
        for (var i = 0; i < players.length; i++)
            if (players[i].player === player)
                return players[i].name;
        return "";
    }

    MappingOverlay {
        id: buttonMapping
        anchors.fill: parent
        // One overlay for both steps: choosing a console flows straight into
        // mapping it, and the daemon switches between them without a gap.
        visible: api.padmap.mappingActive || api.padmap.layoutChoiceActive
        // Takes focus while open so Cancel reaches it rather than tearing
        // down the session underneath.
        focus: visible

        choosing: api.padmap.layoutChoiceActive
        choices: api.padmap.layoutChoices
        choiceIndex: api.padmap.layoutChoiceIndex
        chooseTitle: api.padmap.layoutChoiceTitle

        layout: api.padmap.mappingLayout
        player: choosing ? api.padmap.layoutChoicePlayer
                         : api.padmap.mappingPlayer
        padName: root.padNameFor(player)
        index: api.padmap.mappingIndex
        captured: api.padmap.mappingCaptured

        onSkipRequested: api.padmap.skipControl()
        onCancelRequested: api.padmap.configureEnd()
    }

    CalibrationOverlay {
        id: calibration
        anchors.fill: parent
        visible: player !== 0
        // Takes focus while open so its own key handling wins; the setup
        // screen's Cancel would otherwise tear down the session underneath it.
        focus: visible

        onFinished: {
            if (firstTime)
                root.mappingSuggested = true;
            player = 0;
            root.forceActiveFocus();
        }
    }

    Keys.onPressed: function (event) {
        if (api.keys.isCancel(event)) {
            event.accepted = true;
            root.leave();
        } else if (api.keys.isFilters(event)) {
            event.accepted = true;
            api.padmap.reset();
        } else if (api.keys.isNextPage(event)) {
            // Hand off to the frontend's own gamepad layout editor. It reads
            // through SDL, so padmap has to release its EVIOCGRAB first --
            // openGamepadEditor cancels the session for us. Setup therefore
            // ends here; press Details again from the library to resume.
            event.accepted = true;
            api.padmap.openGamepadEditor();
            root.closed();
        } else if (api.keys.isPrevPage(event)) {
            // Map (or re-map) the last controller to claim a slot, asking
            // what the mapping is *for* first. The only route to a mapping
            // that is not this controller's default -- and still the only
            // route to a different layout, since choosing "any game" leads
            // straight on to the console question.
            //
            // Deliberately not what the automatic first-run flow does. Someone
            // who has just plugged a controller in wants it to work, not to
            // be asked to think about scopes; that flow still goes straight
            // to "which controller is this?" and files the result as the
            // default. This is the deliberate route, for when the default
            // turns out not to be enough -- a GameCube pad that needs
            // different buttons for N64 games, say.
            event.accepted = true;
            var claimed = api.padmap.players;
            if (claimed.length > 0)
                api.padmap.chooseScope(claimed[claimed.length - 1].player);
        } else if (api.keys.isDetails(event)) {
            // Re-run setup for the most recently assigned controller, so a
            // wrong icon or a bad calibration is fixable without unplugging.
            event.accepted = true;
            var players = api.padmap.players;
            if (players.length > 0) {
                var last = players[players.length - 1];
                // Re-running by hand: calibration only. Use Next-page to
                // reach the button editor deliberately.
                calibration.start(last.player, last.name, false);
            }
        } else if (root.resetSlotFor(event) > 0) {
            // Number keys throw that slot's controller away and start the
            // wizard over. By slot number rather than "the last one claimed",
            // which every other shortcut here uses: with two controllers
            // assigned, "the last one" is precisely the ambiguity someone is
            // trying to resolve when they reach for this.
            event.accepted = true;
            api.padmap.forgetPad(root.resetSlotFor(event));
        }
    }

    /// Which player slot a key press asks to reset, or 0 for none.
    ///
    /// Only slots that actually hold a controller: forgetting an empty one is
    /// an error the daemon would have to reject, and a key that silently does
    /// nothing is worse than one that is simply not bound.
    ///
    /// Keyboard-only, and that is forced rather than chosen. The daemon holds
    /// EVIOCGRAB on every pad while this screen is open, so no controller
    /// input reaches the frontend at all -- the same constraint that stopped
    /// "press Select to skip" from ever working in the wizard.
    function resetSlotFor(event) {
        if (event.modifiers !== Qt.NoModifier)
            return 0;
        var slot = event.key - Qt.Key_0;
        if (slot < 1 || slot > api.padmap.slotCount)
            return 0;
        return slot <= api.padmap.players.length ? slot : 0;
    }
}
