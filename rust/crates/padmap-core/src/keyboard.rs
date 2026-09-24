//! The keyboard takes the first port no pad holds, in each emulator's own keys.
//!
//! padmap binds pads. A person with no pad still has the keyboard, and every
//! emulator padmap writes for either binds it to port 1 by default (RetroArch,
//! Dolphin, Ryujinx) or not at all (ares, Cemu). Seating a pad on port 1 used
//! to silently take the keyboard's port with it. Now the keyboard moves to the
//! first free port, so it is always somebody's, and never the same somebody as
//! a pad.
//!
//! Where an emulator has a keyboard layout of its own, that layout is written
//! (see `docs/KEYBOARD.md` for the tables). Where it has none, padmap's layout
//! is: arrows for direction (d-pad and left stick both, so it is right whatever
//! the system calls its primary direction), Z/X/A/S for south/east/west/north,
//! Q/W bumpers, E/R triggers, Enter start, right Shift select, I/J/K/L right
//! stick, B/N stick clicks.

/// What the seat is called, in `claim` and in `state`'s `players[]`.
///
/// The person at the keyboard has the mouse under their other hand, and the
/// seat carries both: the keys in every emulator, and the pointer wherever an
/// emulator has one for a port.
pub const SEAT_NAME: &str = "Keyboard and Mouse";

/// The seat's icon, drawn by the front-end; not one of `icons::ICON_NAMES`,
/// which are a pad's.
pub const SEAT_ICON: &str = "keyboard-mouse";

/// What `state.players[]` and `claim` say of the keyboard's seat.
pub fn seat_state(player: u32) -> crate::state::PlayerState {
    crate::state::PlayerState {
        player,
        name: SEAT_NAME.to_owned(),
        node: String::new(),
        icon: SEAT_ICON.to_owned(),
        configured: true,
        mappings: Vec::new(),
        published: false,
        keyboard: true,
        mouse: true,
    }
}

/// The lowest port in `1..=max` that no seated player holds.
pub fn first_free(players: &[u32], max: u32) -> Option<u32> {
    (1..=max).find(|port| !players.contains(port))
}

/// The keyboard's port in an emulator with `max` ports: its seat when it holds
/// one (and only if that seat is a port this emulator has), else the first
/// port no pad holds. A seat pins the keyboard ahead of pads seated later; it
/// never falls back, because "player 6 is the keyboard" is not "player 2 is".
pub fn port(seat: Option<u32>, players: &[u32], max: u32) -> Option<u32> {
    match seat {
        Some(seat) => ((1..=max).contains(&seat) && !players.contains(&seat)).then_some(seat),
        None => first_free(players, max),
    }
}

#[cfg(test)]
mod tests {
    use super::first_free;

    #[test]
    fn the_seat_is_the_keyboard_and_the_mouse_and_says_so() {
        let state = super::seat_state(2);
        assert_eq!(state.player, 2);
        assert_eq!(state.name, "Keyboard and Mouse");
        assert_eq!(state.icon, "keyboard-mouse");
        assert!(state.keyboard, "a front-end keying on keyboard still works");
        assert!(state.mouse, "the mouse is this seat's too");
        assert!(state.node.is_empty(), "no device is read");
        assert!(state.configured && !state.published);
    }

    #[test]
    fn the_keyboard_takes_the_lowest_port_nobody_holds() {
        assert_eq!(first_free(&[], 4), Some(1));
        assert_eq!(first_free(&[1], 4), Some(2));
        assert_eq!(first_free(&[2, 3], 4), Some(1));
        assert_eq!(first_free(&[1, 2, 4], 4), Some(3));
    }

    #[test]
    fn a_seat_pins_the_keyboard_and_a_seat_beyond_the_ports_is_nothing() {
        use super::port;
        assert_eq!(port(Some(1), &[2, 3], 4), Some(1));
        assert_eq!(port(Some(3), &[1], 4), Some(3), "not moved down to 2");
        assert_eq!(port(Some(6), &[1], 4), None, "no sixth port here");
        assert_eq!(port(Some(2), &[1, 2], 4), None, "a pad holds it");
        assert_eq!(port(None, &[1], 4), Some(2));
    }

    #[test]
    fn a_full_table_has_no_seat_for_the_keyboard() {
        assert_eq!(first_free(&[1, 2, 3, 4], 4), None);
        assert_eq!(first_free(&[3, 1, 4, 2, 9], 4), None);
    }
}
