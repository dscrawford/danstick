//! A Steam Deck's built-in controls, checked against SDL's own answer for them.
//!
//! The fixture in `fakepad` is a live recording; SDL's built-in database has an
//! entry for the same device, computed by people with the hardware. Holding the
//! two against each other says more than either alone: where they agree, the
//! recording is right, and where they disagree, the disagreement is danstick's
//! and is named here rather than discovered on a Deck.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use danstick_core::sdl::{self, AxisSpan};
use danstick_input::fakepad::{Fixture, STEAM_DECK, XBOX_360};

fn spans(fixture: &Fixture) -> BTreeMap<u16, AxisSpan> {
    fixture
        .axes
        .iter()
        .map(|(_, axis)| {
            (
                axis.code,
                AxisSpan::new(axis.minimum, axis.maximum, axis.rest),
            )
        })
        .collect()
}

fn guessed(fixture: &Fixture) -> BTreeMap<String, String> {
    let spans = spans(fixture);
    danstick_core::guess::guessed_fields(&fixture.key_codes(), &fixture.abs_codes(), Some(&spans))
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// The version byte the Deck's USB descriptor carries; part of the GUID.
const DECK_VERSION: u16 = 0x0110;

/// SDL's own line for the Deck, fetched once: initialising SDL from several
/// test threads at a time does not work.
fn sdl_deck() -> &'static BTreeMap<String, String> {
    static FIELDS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    FIELDS.get_or_init(|| {
        let guid = sdl::guid(
            STEAM_DECK.bustype,
            STEAM_DECK.vid,
            STEAM_DECK.pid,
            DECK_VERSION,
            STEAM_DECK.name,
        );
        let line = danstick_input::sdlprobe::builtin_mapping(&guid)
            .expect("SDL initialises")
            .unwrap_or_else(|| panic!("SDL has no entry for a Steam Deck ({guid})"));
        let (_, _, fields) = sdl::parse_line(&line).expect("SDL's own line parses");
        fields
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    })
}

/// Not a control: SDL's trailing `platform:Linux`.
const NOT_A_CONTROL: &str = "platform";

/// Controls danstick has no name for, so it can never bind them.
const BEYOND_DANSTICKS_VOCABULARY: [&str; 6] = [
    "misc1", "paddle1", "paddle2", "paddle3", "paddle4", "touchpad",
];

#[test]
fn sdl_knows_this_exact_device_so_the_fixture_is_checkable() {
    let fields = sdl_deck();
    assert!(!fields.is_empty());
    // Every button SDL names must exist on the fixture at that index.
    assert_eq!(fields.get("a").map(String::as_str), Some("b3"));
    assert_eq!(fields.get("b").map(String::as_str), Some("b4"));
}

#[test]
fn danstick_and_sdl_agree_about_a_deck_except_where_this_test_says_they_do_not() {
    let ours = guessed(&STEAM_DECK);
    let theirs = sdl_deck();

    let disagree: BTreeSet<&str> = ours
        .iter()
        .filter(|(name, value)| theirs.get(*name).is_some_and(|other| other != *value))
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        disagree,
        BTreeSet::from(["x", "y", "lefttrigger", "righttrigger"]),
        "danstick's guess drifted from SDL somewhere new"
    );

    // Everything danstick did not bind is a control it has no word for.
    let unbound: BTreeSet<&str> = theirs
        .keys()
        .filter(|name| !ours.contains_key(*name) && *name != NOT_A_CONTROL)
        .map(String::as_str)
        .collect();
    assert!(
        unbound
            .iter()
            .all(|name| BEYOND_DANSTICKS_VOCABULARY.contains(name)),
        "danstick left something bindable unbound: {unbound:?}"
    );
}

#[test]
fn a_decks_x_and_y_land_on_each_others_buttons() {
    let ours = guessed(&STEAM_DECK);
    let theirs = sdl_deck();
    // hid-steam writes BTN_X for the west button, and BTN_X is BTN_NORTH, so
    // danstick's positional reading of the codes comes out the wrong way round.
    assert_eq!(theirs.get("x").map(String::as_str), Some("b5"));
    assert_eq!(theirs.get("y").map(String::as_str), Some("b6"));
    assert_eq!(ours.get("x").map(String::as_str), Some("b6"));
    assert_eq!(ours.get("y").map(String::as_str), Some("b5"));
    assert_eq!(ours["x"], theirs["y"]);
    assert_eq!(ours["y"], theirs["x"]);
    // An Xbox pad, whose codes mean what they say, comes out the right way up.
    let xbox = guessed(&XBOX_360);
    let index = |code: u16| {
        format!(
            "b{}",
            XBOX_360
                .key_codes()
                .iter()
                .position(|candidate| *candidate == code)
                .expect("a code the pad has")
        )
    };
    assert_eq!(
        xbox["x"],
        index(0x134),
        "BTN_WEST is x on a pad that means it"
    );
    assert_eq!(xbox["y"], index(0x133), "BTN_NORTH is y");
}

#[test]
fn a_decks_dpad_comes_from_its_keys_and_never_from_the_trackpad_under_it() {
    let ours = guessed(&STEAM_DECK);
    let theirs = sdl_deck();
    for direction in ["dpup", "dpdown", "dpleft", "dpright"] {
        let bound = ours.get(direction).expect(direction);
        assert!(
            bound.starts_with('b'),
            "{direction} came from {bound}, which is not a key"
        );
        assert_eq!(bound, &theirs[direction], "{direction}");
    }
    // ABS_HAT0X/Y are declared, and are the left trackpad: -32767..32767.
    let codes = STEAM_DECK.abs_codes();
    assert!(codes.contains(&0x10) && codes.contains(&0x11));
    let hat = spans(&STEAM_DECK)[&0x10];
    assert!(!hat.is_hat_sized(), "a d-pad hat would be -1..1");

    // The same pipeline still reads a real hat as a d-pad.
    let xbox = guessed(&XBOX_360);
    assert_eq!(xbox["dpup"], "h0.1");
    assert!(
        !spans(&XBOX_360).contains_key(&0x10),
        "xpad declares no span for its hat; it is the d-pad"
    );
}

#[test]
fn a_decks_triggers_are_digital_because_sdls_analogue_answer_sits_on_hat_codes() {
    let ours = guessed(&STEAM_DECK);
    let theirs = sdl_deck();
    // SDL counts ABS_HAT2X/Y among the axes; danstick's axis numbering excludes
    // 0x10..0x18 outright, so it can only offer BTN_TL2/BTN_TR2.
    assert_eq!(theirs.get("lefttrigger").map(String::as_str), Some("a9"));
    assert_eq!(theirs.get("righttrigger").map(String::as_str), Some("a8"));
    assert_eq!(ours.get("lefttrigger").map(String::as_str), Some("b9"));
    assert_eq!(ours.get("righttrigger").map(String::as_str), Some("b10"));
    assert_eq!(STEAM_DECK.button("lt_click"), Some(0x138));
    assert_eq!(STEAM_DECK.button("rt_click"), Some(0x139));
}

#[test]
fn the_grips_are_sdls_four_paddles_in_the_order_the_fixture_names_them() {
    let theirs = sdl_deck();
    let index = |name: &str| {
        theirs[name]
            .trim_start_matches('b')
            .parse::<usize>()
            .expect("a button index")
    };
    let code = |index: usize| STEAM_DECK.key_codes()[index];
    // SDL's paddles are right-upper, left-upper, right-lower, left-lower.
    assert_eq!(code(index("paddle2")), STEAM_DECK.button("l4").expect("l4"));
    assert_eq!(code(index("paddle1")), STEAM_DECK.button("r4").expect("r4"));
    assert_eq!(code(index("paddle4")), STEAM_DECK.button("l5").expect("l5"));
    assert_eq!(code(index("paddle3")), STEAM_DECK.button("r5").expect("r5"));
    assert_eq!(
        code(index("misc1")),
        STEAM_DECK.button("quickaccess").expect("qam")
    );
}
