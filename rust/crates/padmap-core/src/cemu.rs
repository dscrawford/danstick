//! Cemu profiles: SDL GUID binding, motion-aware (player 1 is GamePad for gyro).

/// Environment variable to reach Cemu (only SDL-recognized gamepads appear).
pub const CONFIG_ENV: &str = "SDL_GAMECONTROLLERCONFIG";

pub const MAX_PLAYERS: u32 = 8;

/// ProController::ButtonId; differs from VPADController.
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

/// VPADController::ButtonId (numbering differs from WiiU after Minus).
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

/// SDL backend button ids (0-20 literal, 38+ synthetic axes).
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

/// Nintendo labels mirrored (A->East, B->South) to match Cemu's label-based mapping.
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

/// GamePad bindings (left column differs from MAPPING, SDL ids same).
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

/// Cemu controller type (GamePad for motion, Pro otherwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emulated {
    GamePad,
    Pro,
}

impl Emulated {
    pub fn tag(self) -> &'static str {
        match self {
            Emulated::GamePad => "Wii U GamePad",
            Emulated::Pro => "Wii U Pro Controller",
        }
    }

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

/// Player 1 gets GamePad (motion); others get Pro Controller.
pub fn emulated_for(player: u32) -> Emulated {
    if player == 1 {
        Emulated::GamePad
    } else {
        Emulated::Pro
    }
}

/// Generate controllerN.xml for one player (1-based).
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
    out.push_str(&format!("\t\t<uuid>0_{}</uuid>\n", escape(guid)));
    out.push_str(&format!(
        "\t\t<display_name>{}</display_name>\n",
        escape(display_name)
    ));
    out.push_str(
        "\t\t<motion>false</motion>\n\
         \t\t<rumble>0</rumble>\n",
    );
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
    if (1..=crate::dsu::MAX_SLOTS as u32).contains(&player) {
        out.push_str(&motion_controller(player, display_name));
    }
    out.push_str("</emulated_controller>\n");
    out
}

/// Second controller for motion only (DSU slot, no button mappings).
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

/// Marks a keyboard profile as padmap's, so a stale one can be told from the user's.
pub const KEYBOARD_MARKER: &str = "<!-- padmap keyboard: the first port no pad holds -->";

/// GDK keysyms: on Linux Cemu stores the raw wx keycode, which is the keysym of
/// the key's *output*, so letters are the lowercase ones.
mod keysym {
    pub const Z: u32 = 122;
    pub const X: u32 = 120;
    pub const A: u32 = 97;
    pub const S: u32 = 115;
    pub const Q: u32 = 113;
    pub const W: u32 = 119;
    pub const E: u32 = 101;
    pub const R: u32 = 114;
    pub const I: u32 = 105;
    pub const J: u32 = 106;
    pub const K: u32 = 107;
    pub const L: u32 = 108;
    pub const B: u32 = 98;
    pub const N: u32 = 110;
    pub const RETURN: u32 = 0xff0d;
    pub const SHIFT_R: u32 = 0xffe2;
    pub const UP: u32 = 0xff52;
    pub const DOWN: u32 = 0xff54;
    pub const LEFT: u32 = 0xff51;
    pub const RIGHT: u32 = 0xff53;
}

/// padmap's keyboard layout on the Wii U's controls, by Nintendo label:
/// A is east and B is south, as on the pad tables above.
fn keyboard_mapping(emulated: Emulated) -> Vec<(u8, u32)> {
    use keysym as k;
    match emulated {
        Emulated::GamePad => vec![
            (GamePad::A as u8, k::X),
            (GamePad::B as u8, k::Z),
            (GamePad::X as u8, k::S),
            (GamePad::Y as u8, k::A),
            (GamePad::L as u8, k::Q),
            (GamePad::R as u8, k::W),
            (GamePad::Zl as u8, k::E),
            (GamePad::Zr as u8, k::R),
            (GamePad::Plus as u8, k::RETURN),
            (GamePad::Minus as u8, k::SHIFT_R),
            (GamePad::Up as u8, k::UP),
            (GamePad::Down as u8, k::DOWN),
            (GamePad::Left as u8, k::LEFT),
            (GamePad::Right as u8, k::RIGHT),
            (GamePad::StickL as u8, k::B),
            (GamePad::StickR as u8, k::N),
            (GamePad::StickLUp as u8, k::UP),
            (GamePad::StickLDown as u8, k::DOWN),
            (GamePad::StickLLeft as u8, k::LEFT),
            (GamePad::StickLRight as u8, k::RIGHT),
            (GamePad::StickRUp as u8, k::I),
            (GamePad::StickRDown as u8, k::K),
            (GamePad::StickRLeft as u8, k::J),
            (GamePad::StickRRight as u8, k::L),
        ],
        Emulated::Pro => vec![
            (WiiU::A as u8, k::X),
            (WiiU::B as u8, k::Z),
            (WiiU::X as u8, k::S),
            (WiiU::Y as u8, k::A),
            (WiiU::L as u8, k::Q),
            (WiiU::R as u8, k::W),
            (WiiU::Zl as u8, k::E),
            (WiiU::Zr as u8, k::R),
            (WiiU::Plus as u8, k::RETURN),
            (WiiU::Minus as u8, k::SHIFT_R),
            (WiiU::Up as u8, k::UP),
            (WiiU::Down as u8, k::DOWN),
            (WiiU::Left as u8, k::LEFT),
            (WiiU::Right as u8, k::RIGHT),
            (WiiU::StickL as u8, k::B),
            (WiiU::StickR as u8, k::N),
            (WiiU::StickLUp as u8, k::UP),
            (WiiU::StickLDown as u8, k::DOWN),
            (WiiU::StickLLeft as u8, k::LEFT),
            (WiiU::StickLRight as u8, k::RIGHT),
            (WiiU::StickRUp as u8, k::I),
            (WiiU::StickRDown as u8, k::K),
            (WiiU::StickRLeft as u8, k::J),
            (WiiU::StickRRight as u8, k::L),
        ],
    }
}

/// controllerN.xml for the keyboard on one player: GamePad on player 1, Pro after.
pub fn keyboard_profile(player: u32) -> String {
    let emulated = emulated_for(player);
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         {KEYBOARD_MARKER}\n\
         <emulated_controller>\n\
         \t<type>{}</type>\n\
         \t<profile>Keyboard</profile>\n\
         \t<controller>\n\
         \t\t<api>Keyboard</api>\n\
         \t\t<uuid>keyboard</uuid>\n\
         \t\t<display_name>Keyboard</display_name>\n\
         \t\t<motion>false</motion>\n\
         \t\t<rumble>0</rumble>\n",
        emulated.tag()
    );
    for group in ["axis", "rotation", "trigger"] {
        out.push_str(&format!(
            "\t\t<{group}>\n\t\t\t<deadzone>0.25</deadzone>\n\t\t\t<range>1</range>\n\t\t</{group}>\n"
        ));
    }
    out.push_str("\t\t<mappings>\n");
    for (control, key) in keyboard_mapping(emulated) {
        out.push_str(&format!(
            "\t\t\t<entry>\n\t\t\t\t<mapping>{control}</mapping>\n\t\t\t\t<button>{key}</button>\n\t\t\t</entry>\n"
        ));
    }
    out.push_str("\t\t</mappings>\n\t</controller>\n</emulated_controller>\n");
    out
}

/// Whether a profile on disk is padmap's keyboard, as opposed to one the user made.
pub fn is_keyboard_profile(text: &str) -> bool {
    text.contains(KEYBOARD_MARKER)
}

/// Filename controllerN.xml for player (1-based -> 0-based).
pub fn profile_filename(player: u32) -> String {
    format!("controller{}.xml", player.saturating_sub(1))
}

/// XML-escape for device name (& < > ").
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_guid_cannot_close_the_element_it_is_written_into() {
        let xml = super::profile(1, "x</uuid><rumble>1", "Pad");
        assert!(!xml.contains("<rumble>1"), "{xml}");
        assert!(
            xml.contains("<uuid>0_x&lt;/uuid&gt;&lt;rumble&gt;1</uuid>"),
            "{xml}"
        );
    }
}
