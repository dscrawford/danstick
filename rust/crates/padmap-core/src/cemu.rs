//! Cemu controller profiles.
//!
//! Cemu binds a player to a controller by **SDL GUID**, in
//! `~/.config/Cemu/controllerProfiles/controllerN.xml`, and padmap already
//! computes that GUID for every virtual pad. So making a pad work in Cemu is
//! writing one small file per player, and the user never opens its input
//! settings.
//!
//! The mapping table is a constant, which is worth explaining because it looks
//! like it should be per-controller. `<button>` is an **SDL gamepad** id, not
//! an evdev code -- and padmap's clone is already presented to SDL as a
//! standard gamepad by the mapping line padmap writes for it. So by the time
//! Cemu sees the pad, A is SDL button 0 whatever the user physically pressed
//! during capture. Translating the capture again here would be translating it
//! twice.
//!
//! Verified against a `controller0.xml` Cemu itself wrote on the machine this
//! was developed on: every `<mapping>`/`<button>` pair below matches it.
//!
//! Two traps, both from Cemu's source:
//!
//! * **The `uuid` prefix is not a device index.** It is the ordinal among
//!   devices sharing that GUID (`SDLControllerProvider::get_index`), so it is
//!   always `0` for padmap -- each player's GUID embeds a CRC of its own name
//!   and is therefore unique.
//! * **Cemu reads no `gamecontrollerdb.txt`.** It only sees devices SDL
//!   already recognises as gamepads, so padmap's mapping has to reach it
//!   through `SDL_GAMECONTROLLERCONFIG` in the environment. Writing the
//!   profile alone is not enough. See [`CONFIG_ENV`].
//!
//! # Motion, and why player one is a GamePad
//!
//! A real Wii U Pro Controller has no gyroscope, and Cemu's `ProController`
//! has no motion code to match -- `VPADController` and `WPADController` have
//! it and `ProController.cpp` does not contain the word. So a Wii U game that
//! uses motion reads the **GamePad**, and a padmap that writes Pro Controller
//! for everybody has no way to deliver a gyro at all.
//!
//! Player one therefore gets `<type>Wii U GamePad</type>`, which is also
//! Cemu's own default and what some games require outright. Cemu emulates
//! exactly one, so players two and up stay Pro Controllers. The two use
//! *different button id tables* -- see [`GAMEPAD_MAPPING`] -- which is the
//! trap this file already warned about.
//!
//! Motion itself arrives as a **second `<controller>` element** on the same
//! emulated controller. `InputManager::load` iterates `select_nodes(
//! "controller")` and `EmulatedController::get_motion_data` returns the first
//! one whose `<motion>` is true, so a DSU entry beside the SDL entry gives one
//! emulated pad whose buttons come from the clone and whose gyro comes from
//! padmap's DSU server.

/// The environment variable that carries padmap's mapping into Cemu.
///
/// Cemu has no mapping-database file of its own -- a device SDL does not
/// already recognise as a gamepad simply does not appear in its list -- so
/// this is the only way in. SDL reads it directly.
pub const CONFIG_ENV: &str = "SDL_GAMECONTROLLERCONFIG";

/// Cemu allows eight.
pub const MAX_PLAYERS: u32 = 8;

/// `ProController::ButtonId`, from Cemu's `src/input/emulated/ProController.h`.
///
/// Note this is *not* `VPADController::ButtonId`, which numbers the same
/// controls differently and puts Home at 27 -- so the table is only correct
/// for `<type>Wii U Pro Controller</type>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WiiU {
    A = 1,
    B = 2,
    X = 3,
    Y = 4,
    L = 5,
    R = 6,
    Zl = 7,
    Zr = 8,
    Plus = 9,
    Minus = 10,
    Up = 12,
    Down = 13,
    Left = 14,
    Right = 15,
    StickL = 16,
    StickR = 17,
    StickLUp = 18,
    StickLDown = 19,
    StickLLeft = 20,
    StickLRight = 21,
    StickRUp = 22,
    StickRDown = 23,
    StickRLeft = 24,
    StickRRight = 25,
}

/// `VPADController::ButtonId`, from Cemu's `src/input/emulated/VPADController.h`.
///
/// Identical to [`WiiU`] through `Minus`, and then one lower for everything
/// after it -- the Pro table skips 11 and this one does not. Written out
/// rather than derived from the other by subtraction, because "the same but
/// one less from here on" is a relationship nobody can check against Cemu's
/// header at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GamePad {
    A = 1,
    B = 2,
    X = 3,
    Y = 4,
    L = 5,
    R = 6,
    Zl = 7,
    Zr = 8,
    Plus = 9,
    Minus = 10,
    Up = 11,
    Down = 12,
    Left = 13,
    Right = 14,
    StickL = 15,
    StickR = 16,
    StickLUp = 17,
    StickLDown = 18,
    StickLLeft = 19,
    StickLRight = 20,
    StickRUp = 21,
    StickRDown = 22,
    StickRLeft = 23,
    StickRRight = 24,
}

/// Cemu's `Buttons2` ids for the SDL backend, `src/input/api/Controller.h`.
///
/// 0..20 are literally `SDL_GamepadButton`. 38 upwards are synthetic axis
/// directions: left stick is "Axis", right stick is "Rotation", and the two
/// triggers are "Trigger" -- which is why the right stick's ids are not
/// adjacent to the left's.
mod sdl_id {
    pub const SOUTH: u8 = 0;
    pub const EAST: u8 = 1;
    pub const WEST: u8 = 2;
    pub const NORTH: u8 = 3;
    pub const BACK: u8 = 4;
    pub const START: u8 = 6;
    pub const LEFT_STICK: u8 = 7;
    pub const RIGHT_STICK: u8 = 8;
    pub const LEFT_SHOULDER: u8 = 9;
    pub const RIGHT_SHOULDER: u8 = 10;
    pub const DPAD_UP: u8 = 11;
    pub const DPAD_DOWN: u8 = 12;
    pub const DPAD_LEFT: u8 = 13;
    pub const DPAD_RIGHT: u8 = 14;
    pub const AXIS_X_POS: u8 = 38;
    pub const AXIS_Y_POS: u8 = 39;
    pub const ROTATION_X_POS: u8 = 40;
    pub const ROTATION_Y_POS: u8 = 41;
    pub const TRIGGER_X_POS: u8 = 42;
    pub const TRIGGER_Y_POS: u8 = 43;
    pub const AXIS_X_NEG: u8 = 44;
    pub const AXIS_Y_NEG: u8 = 45;
    pub const ROTATION_X_NEG: u8 = 46;
    pub const ROTATION_Y_NEG: u8 = 47;
}

/// Wii U control -> SDL gamepad id.
///
/// A is SDL *East* and B is SDL *South*: Nintendo's labels are mirrored
/// against everyone else's, and Cemu maps by label. Getting this the obvious
/// way round swaps A and B in every Wii U game, which is the kind of wrong
/// that feels like the emulator's fault.
pub const MAPPING: [(WiiU, u8); 24] = [
    (WiiU::A, sdl_id::EAST),
    (WiiU::B, sdl_id::SOUTH),
    (WiiU::X, sdl_id::NORTH),
    (WiiU::Y, sdl_id::WEST),
    (WiiU::L, sdl_id::LEFT_SHOULDER),
    (WiiU::R, sdl_id::RIGHT_SHOULDER),
    (WiiU::Zl, sdl_id::TRIGGER_X_POS),
    (WiiU::Zr, sdl_id::TRIGGER_Y_POS),
    (WiiU::Plus, sdl_id::START),
    (WiiU::Minus, sdl_id::BACK),
    (WiiU::Up, sdl_id::DPAD_UP),
    (WiiU::Down, sdl_id::DPAD_DOWN),
    (WiiU::Left, sdl_id::DPAD_LEFT),
    (WiiU::Right, sdl_id::DPAD_RIGHT),
    (WiiU::StickL, sdl_id::LEFT_STICK),
    (WiiU::StickR, sdl_id::RIGHT_STICK),
    (WiiU::StickLUp, sdl_id::AXIS_Y_NEG),
    (WiiU::StickLDown, sdl_id::AXIS_Y_POS),
    (WiiU::StickLLeft, sdl_id::AXIS_X_NEG),
    (WiiU::StickLRight, sdl_id::AXIS_X_POS),
    (WiiU::StickRUp, sdl_id::ROTATION_Y_NEG),
    (WiiU::StickRDown, sdl_id::ROTATION_Y_POS),
    (WiiU::StickRLeft, sdl_id::ROTATION_X_NEG),
    (WiiU::StickRRight, sdl_id::ROTATION_X_POS),
];

/// The GamePad's bindings, in the same order and to the same SDL ids.
///
/// Only the left column differs from [`MAPPING`]: the right-hand side is what
/// SDL calls the button, and that does not change with what Cemu emulates.
pub const GAMEPAD_MAPPING: [(GamePad, u8); 24] = [
    (GamePad::A, sdl_id::EAST),
    (GamePad::B, sdl_id::SOUTH),
    (GamePad::X, sdl_id::NORTH),
    (GamePad::Y, sdl_id::WEST),
    (GamePad::L, sdl_id::LEFT_SHOULDER),
    (GamePad::R, sdl_id::RIGHT_SHOULDER),
    (GamePad::Zl, sdl_id::TRIGGER_X_POS),
    (GamePad::Zr, sdl_id::TRIGGER_Y_POS),
    (GamePad::Plus, sdl_id::START),
    (GamePad::Minus, sdl_id::BACK),
    (GamePad::Up, sdl_id::DPAD_UP),
    (GamePad::Down, sdl_id::DPAD_DOWN),
    (GamePad::Left, sdl_id::DPAD_LEFT),
    (GamePad::Right, sdl_id::DPAD_RIGHT),
    (GamePad::StickL, sdl_id::LEFT_STICK),
    (GamePad::StickR, sdl_id::RIGHT_STICK),
    (GamePad::StickLUp, sdl_id::AXIS_Y_NEG),
    (GamePad::StickLDown, sdl_id::AXIS_Y_POS),
    (GamePad::StickLLeft, sdl_id::AXIS_X_NEG),
    (GamePad::StickLRight, sdl_id::AXIS_X_POS),
    (GamePad::StickRUp, sdl_id::ROTATION_Y_NEG),
    (GamePad::StickRDown, sdl_id::ROTATION_Y_POS),
    (GamePad::StickRLeft, sdl_id::ROTATION_X_NEG),
    (GamePad::StickRRight, sdl_id::ROTATION_X_POS),
];

/// What Cemu emulates for a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emulated {
    /// `Wii U GamePad`. The only one with motion, and Cemu emulates one.
    GamePad,
    /// `Wii U Pro Controller`.
    Pro,
}

impl Emulated {
    /// What goes in `<type>`.
    pub fn tag(self) -> &'static str {
        match self {
            Emulated::GamePad => "Wii U GamePad",
            Emulated::Pro => "Wii U Pro Controller",
        }
    }

    /// The bindings, as `(control id, SDL id)`.
    pub fn mapping(self) -> Vec<(u8, u8)> {
        match self {
            Emulated::GamePad => GAMEPAD_MAPPING
                .iter()
                .map(|(control, button)| (*control as u8, *button))
                .collect(),
            Emulated::Pro => MAPPING
                .iter()
                .map(|(control, button)| (*control as u8, *button))
                .collect(),
        }
    }
}

/// Player one is the GamePad; everybody else is a Pro Controller.
///
/// Cemu emulates exactly one GamePad, and it is the only emulated controller
/// with motion -- so this is both the constraint and the reason.
pub fn emulated_for(player: u32) -> Emulated {
    if player == 1 {
        Emulated::GamePad
    } else {
        Emulated::Pro
    }
}

/// `controllerN.xml` for one player.
///
/// `player` is 1-based, as padmap counts; Cemu's filename is 0-based, which
/// [`profile_filename`] handles. It also decides what is emulated and which
/// DSU slot the motion comes from.
pub fn profile(player: u32, guid: &str, display_name: &str) -> String {
    let emulated = emulated_for(player);
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <emulated_controller>\n\
         \t<type>{}</type>\n\
         \t<controller>\n\
         \t\t<api>SDLController</api>\n",
        emulated.tag()
    );
    // Always `0_`: each player's GUID embeds a CRC of its own name, so no two
    // padmap pads share one and none is ever the second holder.
    out.push_str(&format!("\t\t<uuid>0_{guid}</uuid>\n"));
    out.push_str(&format!(
        "\t\t<display_name>{}</display_name>\n",
        escape(display_name)
    ));
    out.push_str(
        "\t\t<motion>false</motion>\n\
         \t\t<rumble>0</rumble>\n",
    );
    // Cemu's own defaults, written out rather than left absent: a profile that
    // omits them still loads, but the next time Cemu saves it they appear, and
    // a file that changes on its own looks like something went wrong.
    for group in ["axis", "rotation", "trigger"] {
        out.push_str(&format!(
            "\t\t<{group}>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</{group}>\n"
        ));
    }
    out.push_str("\t\t<mappings>\n");
    for (control, button) in emulated.mapping() {
        out.push_str(&format!(
            "\t\t\t<entry>\n\t\t\t\t<mapping>{control}</mapping>\n\t\t\t\t<button>{button}</button>\n\t\t\t</entry>\n"
        ));
    }
    out.push_str("\t\t</mappings>\n\t</controller>\n");
    // DSU has four slots. DSUController's constructor throws for an index
    // past them, and Cemu skips the whole node with a log line on every load.
    if (1..=crate::dsu::MAX_SLOTS as u32).contains(&player) {
        out.push_str(&motion_controller(player, display_name));
    }
    out.push_str("</emulated_controller>\n");
    out
}

/// A second `<controller>` that supplies nothing but motion.
///
/// `<uuid>` is the DSU slot as a bare integer -- `ControllerFactory` parses it
/// with `ConvertString<uint32>`, so the `0_` prefix the SDL entry needs would
/// be a parse failure here and the whole controller would be skipped with one
/// line in Cemu's log.
///
/// No `<mappings>`: buttons come from the clone, and a DSU entry that also
/// bound buttons would give every press twice.
fn motion_controller(player: u32, display_name: &str) -> String {
    format!(
        "\t<controller>\n\
         \t\t<api>DSUController</api>\n\
         \t\t<uuid>{slot}</uuid>\n\
         \t\t<display_name>{name} motion</display_name>\n\
         \t\t<motion>true</motion>\n\
         \t\t<rumble>0</rumble>\n\
         \t\t<ip>{host}</ip>\n\
         \t\t<port>{port}</port>\n\
         \t</controller>\n",
        slot = player.saturating_sub(1),
        name = escape(display_name),
        host = crate::dsu::HOST,
        port = crate::dsu::PORT,
    )
}

/// `controller0.xml` for player 1, and so on.
pub fn profile_filename(player: u32) -> String {
    format!("controller{}.xml", player.saturating_sub(1))
}

/// XML-escape a device name.
///
/// padmap names its pads, so this is never load-bearing today -- but the name
/// reaches a file another program parses, and a `&` in it would make Cemu fail
/// to read the whole profile rather than one field.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
