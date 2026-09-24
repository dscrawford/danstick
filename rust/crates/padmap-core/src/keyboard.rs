//! The keyboard takes the first port no pad holds, in each emulator's own keys.
//!
//! Every emulator padmap writes for either binds the keyboard to port 1 by
//! default (RetroArch, Dolphin, Ryujinx) or not at all (ares, Cemu), so seating
//! a pad on port 1 used to take the keyboard's port with it. The per-emulator
//! key tables are in `docs/KEYBOARD.md`.

use std::collections::BTreeMap;

/// What the seat is called in `claim` and `state`: one person with both hands
/// busy, so the seat carries the keys and the pointer alike.
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

/// `KEY_SPACE`, the only key read, so typing cannot take a seat by accident.
pub const SEAT_KEY: u16 = 57;

/// What a [`Hold`] tick produced, shaped like the pads' [`crate::assign::Tick`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HoldTick {
    /// How far through the hold, while one is running.
    pub progress: Option<f64>,
    /// The hold ran its length: seat the keyboard.
    pub claimed: bool,
    /// A hold that was filling stopped without claiming.
    pub released: bool,
}

/// A held space bar, timed like a pad's hold on a button. Never grabbed, so a
/// character still jumps in the game while somebody joins.
///
/// One per keyboard: the same keyboard often has two nodes, and a release on
/// one must not cancel a fill running on the other.
#[derive(Debug, Clone, Default)]
pub struct Hold {
    down: BTreeMap<u64, f64>,
    filling: bool,
}

impl Hold {
    /// Offer one event from the keyboard `device` hashes to. Autorepeat
    /// (value 2) is not an edge.
    pub fn feed(&mut self, device: u64, kind: u16, code: u16, value: i32, now: f64) {
        if kind != crate::capture::EV_KEY || code != SEAT_KEY {
            return;
        }
        if value == 1 {
            self.down.entry(device).or_insert(now);
        } else if value == 0 {
            self.down.remove(&device);
        }
    }

    /// Advance the timer; a claim clears the hold, so it cannot seat twice.
    pub fn tick(&mut self, now: f64, hold_seconds: f64) -> HoldTick {
        // The keyboard held longest: two people cannot both be the keyboard,
        // so the earliest press is the one filling.
        let Some(started) = self.down.values().copied().reduce(f64::min) else {
            let released = std::mem::take(&mut self.filling);
            return HoldTick {
                released,
                ..HoldTick::default()
            };
        };
        let elapsed = now - started;
        if elapsed >= hold_seconds {
            self.down.clear();
            self.filling = false;
            return HoldTick {
                claimed: true,
                ..HoldTick::default()
            };
        }
        self.filling = true;
        let fraction = if hold_seconds > 0.0 {
            (elapsed / hold_seconds).clamp(0.0, 1.0)
        } else {
            0.0
        };
        HoldTick {
            progress: Some(fraction),
            ..HoldTick::default()
        }
    }

    /// Drop a hold in flight: seating closed, or the seat is taken.
    pub fn reset(&mut self) {
        *self = Hold::default();
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
    fn a_held_space_bar_fills_and_then_claims_once() {
        use super::{Hold, SEAT_KEY};
        let mut hold = Hold::default();
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 1, 0.0);

        let tick = hold.tick(0.75, 1.5);
        assert_eq!(tick.progress, Some(0.5), "half way through");
        assert!(!tick.claimed && !tick.released);

        let tick = hold.tick(1.5, 1.5);
        assert!(tick.claimed, "the hold ran its length");
        assert_eq!(tick.progress, None);

        // Still held: the claim cleared it, so nobody is seated twice.
        let tick = hold.tick(9.0, 1.5);
        assert!(!tick.claimed && !tick.released && tick.progress.is_none());
    }

    #[test]
    fn letting_go_half_way_releases_and_claims_nothing() {
        use super::{Hold, SEAT_KEY};
        let mut hold = Hold::default();
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 1, 0.0);
        assert_eq!(hold.tick(0.5, 1.5).progress, Some(0.5f64 / 1.5));
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 0, 0.6);

        let tick = hold.tick(0.7, 1.5);
        assert!(tick.released, "a fill that stopped is a fill that stopped");
        assert!(!tick.claimed);
        // And only once.
        assert!(!hold.tick(0.8, 1.5).released);
    }

    #[test]
    fn nothing_but_the_space_bar_is_read() {
        use super::{Hold, SEAT_KEY};
        let mut hold = Hold::default();
        // A letter, and an EV_ABS that happens to carry the same code.
        hold.feed(1, crate::capture::EV_KEY, 30, 1, 0.0);
        hold.feed(1, 0x03, SEAT_KEY, 1, 0.0);
        assert!(
            hold.tick(9.0, 1.5).progress.is_none(),
            "something else held"
        );

        // Autorepeat is not a fresh press: the hold keeps its own start time.
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 1, 1.0);
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 2, 1.4);
        assert!(hold.tick(2.5, 1.5).claimed, "autorepeat restarted the hold");
    }

    #[test]
    fn a_release_on_one_keyboard_leaves_a_hold_running_on_another() {
        use super::{Hold, SEAT_KEY};
        // The same keyboard often has two nodes, so this is the ordinary case
        // and not a two-people one.
        let mut hold = Hold::default();
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 1, 0.0);
        hold.feed(2, crate::capture::EV_KEY, SEAT_KEY, 1, 0.2);
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 0, 0.3);

        let tick = hold.tick(0.5, 1.5);
        assert!(
            tick.progress.is_some() && !tick.released,
            "one node letting go ended the other's fill: {tick:?}"
        );
        // The fill is the surviving press's own, from when it went down.
        assert!(!hold.tick(1.6, 1.5).claimed, "it claimed on node 1's clock");
        assert!(hold.tick(1.75, 1.5).claimed, "node 2's hold never finished");
    }

    #[test]
    fn a_hold_of_no_length_claims_at_once_and_a_nonsense_one_never_does() {
        use super::{Hold, SEAT_KEY};
        let mut hold = Hold::default();
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 1, 5.0);
        assert!(hold.tick(5.0, 0.0).claimed, "zero means now");

        let mut hold = Hold::default();
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 1, 0.0);
        let tick = hold.tick(10.0, f64::NAN);
        assert!(!tick.claimed, "a NaN length seated somebody: {tick:?}");
        assert_eq!(tick.progress, Some(0.0));
    }

    #[test]
    fn a_fresh_press_after_a_claim_fills_from_nothing() {
        use super::{Hold, SEAT_KEY};
        let mut hold = Hold::default();
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 1, 0.0);
        assert!(hold.tick(1.5, 1.5).claimed);
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 0, 1.6);
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 1, 2.0);
        let tick = hold.tick(2.2, 1.5);
        assert!(
            tick.progress.is_some_and(|f| (f - 0.2 / 1.5).abs() < 1e-9),
            "the second hold carried the first one's time: {tick:?}"
        );
    }

    #[test]
    fn a_reset_drops_a_hold_in_flight() {
        use super::{Hold, SEAT_KEY};
        let mut hold = Hold::default();
        hold.feed(1, crate::capture::EV_KEY, SEAT_KEY, 1, 0.0);
        hold.reset();
        assert!(hold.tick(9.0, 1.5).progress.is_none());
        assert!(!hold.tick(9.0, 1.5).claimed);
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
