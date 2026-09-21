//! Real controllers, written down and executable.

/// One absolute axis, exactly as its driver declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Axis {
    pub code: u16,
    pub minimum: i32,
    pub maximum: i32,
    pub rest: i32,
    pub fuzz: i32,
    /// The deadzone the driver asks for.
    pub flat: i32,
}

impl Axis {
    const fn new(code: u16, minimum: i32, maximum: i32, rest: i32) -> Axis {
        Axis {
            code,
            minimum,
            maximum,
            rest,
            fuzz: 0,
            flat: 0,
        }
    }

    /// Steam's uinput mirror: xpad's stick, one short at the bottom.
    const fn steam_stick(code: u16) -> Axis {
        Axis {
            code,
            minimum: -32767,
            maximum: 32767,
            rest: 0,
            fuzz: 16,
            flat: 128,
        }
    }

    /// A Deck stick: hid-steam declares no fuzz and no deadzone at all.
    const fn deck_stick(code: u16) -> Axis {
        Axis {
            code,
            minimum: -32767,
            maximum: 32767,
            rest: 0,
            fuzz: 0,
            flat: 0,
        }
    }

    /// A Deck trackpad: a stick's range, with the fuzz a finger needs.
    const fn deck_pad(code: u16) -> Axis {
        Axis {
            code,
            minimum: -32767,
            maximum: 32767,
            rest: 0,
            fuzz: 256,
            flat: 0,
        }
    }

    const fn xpad_stick(code: u16) -> Axis {
        Axis {
            code,
            minimum: -32768,
            maximum: 32767,
            rest: 0,
            fuzz: 16,
            flat: 128,
        }
    }
}

/// One real controller model, as the kernel publishes it.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// The exact string the driver publishes (profiles match exactly).
    pub name: &'static str,
    pub vid: u16,
    pub pid: u16,
    pub bustype: u16,
    pub source: &'static str,
    pub buttons: &'static [(&'static str, u16)],
    pub axes: &'static [(&'static str, Axis)],
    /// Whether the driver emits `MSC_SCAN` before each key (hid-generic does, xpad doesn't).
    pub emits_scan: bool,
    /// Vocabulary name -> the HID usage its driver puts in `MSC_SCAN`.
    pub scancodes: &'static [(&'static str, u32)],
    pub dpad_is_hat: bool,
}

/// `BTN_THUMB`/`BTN_THUMB2`: a Steam Deck's trackpad clicks.
const BTN_THUMB: u16 = 0x121;
const BTN_THUMB2: u16 = 0x122;
/// `BTN_BASE`: the Deck's quick access ("...") button.
const BTN_BASE: u16 = 0x126;
const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;
const BTN_NORTH: u16 = 0x133;
const BTN_WEST: u16 = 0x134;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
/// `BTN_TL2`/`BTN_TR2`: a trigger pulled all the way, as a key.
const BTN_TL2: u16 = 0x138;
const BTN_TR2: u16 = 0x139;
const BTN_SELECT: u16 = 0x13A;
const BTN_START: u16 = 0x13B;
const BTN_MODE: u16 = 0x13C;
const BTN_THUMBL: u16 = 0x13D;
const BTN_THUMBR: u16 = 0x13E;
/// `KEY_RECORD`: a key code, not a BTN_.
const KEY_RECORD: u16 = 167;
/// `BTN_DPAD_*`: a d-pad reported as four keys rather than a hat.
const BTN_DPAD_UP: u16 = 0x220;
const BTN_DPAD_DOWN: u16 = 0x221;
const BTN_DPAD_LEFT: u16 = 0x222;
const BTN_DPAD_RIGHT: u16 = 0x223;
/// The Deck's four grips, on codes the kernel headers give no name to.
const DECK_GRIP_L4: u16 = 0x224;
const DECK_GRIP_R4: u16 = 0x225;
const DECK_GRIP_L5: u16 = 0x226;
const DECK_GRIP_R5: u16 = 0x227;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;
pub const ABS_HAT1X: u16 = 0x12;
pub const ABS_HAT1Y: u16 = 0x13;
pub const ABS_HAT2X: u16 = 0x14;
pub const ABS_HAT2Y: u16 = 0x15;

const BUS_USB: u16 = 0x03;

pub const XBOX_360: Fixture = Fixture {
    name: "Microsoft X-Box 360 pad",
    vid: 0x045E,
    pid: 0x028E,
    bustype: BUS_USB,
    source: "drivers/input/joystick/xpad.c: device table line 123, \
             xpad_common_btn/xpad_btn_pad line 411, xpad_set_up_abs line 1898",
    emits_scan: false,
    scancodes: &[],
    buttons: &[
        ("a", BTN_SOUTH),
        ("b", BTN_EAST),
        ("x", BTN_WEST),
        ("y", BTN_NORTH),
        ("l", BTN_TL),
        ("r", BTN_TR),
        ("select", BTN_SELECT),
        ("start", BTN_START),
        ("home", BTN_MODE),
        ("l3", BTN_THUMBL),
        ("r3", BTN_THUMBR),
    ],
    axes: &[
        ("lx", Axis::xpad_stick(ABS_X)),
        ("ly", Axis::xpad_stick(ABS_Y)),
        ("rx", Axis::xpad_stick(ABS_RX)),
        ("ry", Axis::xpad_stick(ABS_RY)),
        ("lt", Axis::new(ABS_Z, 0, 255, 0)),
        ("rt", Axis::new(ABS_RZ, 0, 255, 0)),
    ],
    dpad_is_hat: true,
};

/// Steam Input's virtual gamepad: what Steam publishes for an application it
/// launches while it is handling a controller on that application's behalf.
///
/// One physical press arrives here and on the pad it mirrors, which is why
/// discovery drops it (`pad::without_steam_mirrors`). It borrows xpad's name
/// and table with a trailing index, and is only ever told apart by id.
pub const STEAM_VIRTUAL: Fixture = Fixture {
    name: "Microsoft X-Box 360 pad 0",
    vid: 0x28DE,
    pid: 0x11FF,
    bustype: BUS_USB,
    source: "the id is icons::STEAM_VIRTUAL_ID (28de:11ff), Steam's uinput \
             gamepad since the Steam Input rewrite; the name and the button \
             table are xpad's Xbox 360 entry with Steam's slot index appended. \
             Recorded from Steam 1788652215 on a Steam Deck, 2026-09-21: the \
             buttons are xpad's eleven exactly, but the sticks stop at -32767 \
             where xpad's reach -32768",
    emits_scan: false,
    scancodes: &[],
    buttons: XBOX_360.buttons,
    axes: &[
        ("lx", Axis::steam_stick(ABS_X)),
        ("ly", Axis::steam_stick(ABS_Y)),
        ("rx", Axis::steam_stick(ABS_RX)),
        ("ry", Axis::steam_stick(ABS_RY)),
        ("lt", Axis::new(ABS_Z, 0, 255, 0)),
        ("rt", Axis::new(ABS_RZ, 0, 255, 0)),
    ],
    dpad_is_hat: true,
};

/// Same driver as 360, but triggers are 0..1023 instead of 0..255 (wrong by 4x if confused).
pub const XBOX_SERIES_X: Fixture = Fixture {
    name: "Microsoft Xbox Series S|X Controller",
    vid: 0x045E,
    pid: 0x0B12,
    bustype: BUS_USB,
    source: "drivers/input/joystick/xpad.c: device table line 134 \
             (MAP_SHARE_BUTTON|MAP_SHARE_OFFSET, XTYPE_XBOXONE), \
             xpad_set_up_abs line 1904",
    emits_scan: false,
    scancodes: &[],
    buttons: &[
        ("a", BTN_SOUTH),
        ("b", BTN_EAST),
        ("x", BTN_WEST),
        ("y", BTN_NORTH),
        ("l", BTN_TL),
        ("r", BTN_TR),
        ("select", BTN_SELECT),
        ("start", BTN_START),
        ("home", BTN_MODE),
        ("l3", BTN_THUMBL),
        ("r3", BTN_THUMBR),
        ("capture", KEY_RECORD),
    ],
    axes: &[
        ("lx", Axis::xpad_stick(ABS_X)),
        ("ly", Axis::xpad_stick(ABS_Y)),
        ("rx", Axis::xpad_stick(ABS_RX)),
        ("ry", Axis::xpad_stick(ABS_RY)),
        ("lt", Axis::new(ABS_Z, 0, 1023, 0)),
        ("rt", Axis::new(ABS_RZ, 0, 1023, 0)),
    ],
    dpad_is_hat: true,
};

pub const MAYFLASH_GAMECUBE: Fixture = Fixture {
    name: "mayflash MAYFLASH GameCube Controller Adapter",
    vid: 0x0079,
    pid: 0x1843,
    bustype: BUS_USB,
    source: "measured absinfo on the development machine -- ABS_RX rest=24, \
             ABS_RY rest=25, both of 0-255; hid-generic, so MSC_SCAN per \
             hid-input.c:1787",
    emits_scan: true,
    buttons: &[
        ("a", BTN_SOUTH),
        ("b", BTN_EAST),
        ("x", BTN_WEST),
        ("y", BTN_NORTH),
        ("l", BTN_TL),
        ("r", BTN_TR),
        ("start", BTN_START),
        ("select", BTN_SELECT),
    ],
    scancodes: &[
        ("a", 0x0009_0001),
        ("b", 0x0009_0002),
        ("x", 0x0009_0003),
        ("y", 0x0009_0004),
        ("l", 0x0009_0005),
        ("r", 0x0009_0006),
        ("select", 0x0009_0007),
        ("start", 0x0009_0008),
    ],
    axes: &[
        ("lx", Axis::new(ABS_X, 0, 255, 128)),
        ("ly", Axis::new(ABS_Y, 0, 255, 128)),
        ("cx", Axis::new(ABS_RZ, 0, 255, 128)),
        ("cy", Axis::new(ABS_Z, 0, 255, 131)),
        ("lt", Axis::new(ABS_RX, 0, 255, 24)),
        ("rt", Axis::new(ABS_RY, 0, 255, 25)),
    ],
    dpad_is_hat: true,
};

/// A Steam Deck's own controls, which are a pad only while Steam is not
/// holding the hidraw node: `hid-steam` withdraws this node for any client
/// that opens the device, and Steam is such a client.
///
/// It is not shaped like an Xbox pad in any of the four ways that matter.
/// The d-pad is four keys, and `ABS_HAT0X/Y` -- where a d-pad usually lives --
/// is the left trackpad. The triggers are `ABS_HAT2Y`/`ABS_HAT2X` of 0..32767,
/// not `ABS_Z`/`ABS_RZ` of 0..255, which the pad does not declare at all. And
/// X and Y arrive on each other's codes, because `hid-steam` writes `BTN_X`
/// for the west button and `BTN_X` *is* `BTN_NORTH`.
pub const STEAM_DECK: Fixture = Fixture {
    name: "Steam Deck",
    vid: 0x28DE,
    pid: 0x1205,
    bustype: BUS_USB,
    source: "captured from a Steam Deck (Galileo, SteamOS neptune \
             6.16.12-valve24.5) on 2026-09-21 with Steam stopped, by \
             EVIOCGBIT/EVIOCGABS on the `Steam Deck` node; the same table is \
             declared by drivers/hid/hid-steam.c steam_input_register under \
             STEAM_QUIRK_DECK. The grip order and the x/y swap are not pressed \
             but corroborated: SDL's built-in database has an entry for this \
             GUID, and tests/steam_deck.rs holds this fixture against it. The \
             grips are on 0x224..0x227 here, codes the kernel headers leave \
             unnamed; mainline v6.16 puts them on BTN_TRIGGER_HAPPY1..4 \
             (0x2C0..0x2C3), so this is SteamOS's hid-steam, not everyone's",
    emits_scan: false,
    scancodes: &[],
    buttons: &[
        ("a", BTN_SOUTH),
        ("b", BTN_EAST),
        // hid-steam writes BTN_X for the west button, and BTN_X is BTN_NORTH.
        ("x", BTN_NORTH),
        ("y", BTN_WEST),
        ("l", BTN_TL),
        ("r", BTN_TR),
        ("lt_click", BTN_TL2),
        ("rt_click", BTN_TR2),
        ("select", BTN_SELECT),
        ("start", BTN_START),
        ("home", BTN_MODE),
        ("l3", BTN_THUMBL),
        ("r3", BTN_THUMBR),
        ("up", BTN_DPAD_UP),
        ("down", BTN_DPAD_DOWN),
        ("left", BTN_DPAD_LEFT),
        ("right", BTN_DPAD_RIGHT),
        ("lpad_click", BTN_THUMB),
        ("rpad_click", BTN_THUMB2),
        ("quickaccess", BTN_BASE),
        ("l4", DECK_GRIP_L4),
        ("r4", DECK_GRIP_R4),
        ("l5", DECK_GRIP_L5),
        ("r5", DECK_GRIP_R5),
    ],
    axes: &[
        ("lx", Axis::deck_stick(ABS_X)),
        ("ly", Axis::deck_stick(ABS_Y)),
        ("rx", Axis::deck_stick(ABS_RX)),
        ("ry", Axis::deck_stick(ABS_RY)),
        ("lt", Axis::new(ABS_HAT2Y, 0, 32767, 0)),
        ("rt", Axis::new(ABS_HAT2X, 0, 32767, 0)),
        ("lpad_x", Axis::deck_pad(ABS_HAT0X)),
        ("lpad_y", Axis::deck_pad(ABS_HAT0Y)),
        ("rpad_x", Axis::deck_pad(ABS_HAT1X)),
        ("rpad_y", Axis::deck_pad(ABS_HAT1Y)),
    ],
    dpad_is_hat: false,
};

pub const EVERY: [&Fixture; 4] = [&XBOX_360, &XBOX_SERIES_X, &MAYFLASH_GAMECUBE, &STEAM_DECK];

pub const DPAD: [&str; 4] = ["up", "down", "left", "right"];

impl Fixture {
    pub fn controls(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .buttons
            .iter()
            .map(|(name, _)| *name)
            .chain(self.axes.iter().map(|(name, _)| *name))
            .collect();
        if self.dpad_is_hat {
            names.extend(DPAD);
        }
        names.sort_unstable();
        names.dedup();
        names
    }

    pub fn button(&self, control: &str) -> Option<u16> {
        self.buttons
            .iter()
            .find(|(name, _)| *name == control)
            .map(|(_, code)| *code)
    }

    pub fn axis(&self, control: &str) -> Option<Axis> {
        self.axes
            .iter()
            .find(|(name, _)| *name == control)
            .map(|(_, axis)| *axis)
    }

    pub fn scancode(&self, control: &str) -> Option<u32> {
        self.scancodes
            .iter()
            .find(|(name, _)| *name == control)
            .map(|(_, usage)| *usage)
    }

    /// This controller as discovery would report it, at `/dev/input/{event}`.
    pub fn pad(&self, event: &str) -> crate::pad::Pad {
        crate::pad::Pad {
            path: std::path::PathBuf::from(format!("/dev/input/{event}")),
            name: self.name.to_owned(),
            phys: String::new(),
            uniq: String::new(),
            vid: self.vid,
            pid: self.pid,
            syspath: std::path::PathBuf::from(format!("/sys/devices/virtual/input/{event}")),
            retroarch_visible: true,
            motion: None,
        }
    }

    /// Every key code this pad declares, sorted as the kernel reports them.
    pub fn key_codes(&self) -> Vec<u16> {
        let mut codes: Vec<u16> = self.buttons.iter().map(|(_, code)| *code).collect();
        codes.sort_unstable();
        codes
    }

    pub fn abs_codes(&self) -> Vec<u16> {
        let mut codes: Vec<u16> = self.axes.iter().map(|(_, axis)| axis.code).collect();
        if self.dpad_is_hat {
            codes.push(ABS_HAT0X);
            codes.push(ABS_HAT0Y);
        }
        codes.sort_unstable();
        codes
    }

    /// Four held directions as a hat (opposites cancel: hardware can't report both).
    pub fn hat_from(held: &[&str]) -> (i32, i32) {
        let has = |name: &str| held.contains(&name);
        (
            i32::from(has("right")) - i32::from(has("left")),
            i32::from(has("down")) - i32::from(has("up")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_fixture_is_coherent() {
        for fixture in EVERY {
            assert!(!fixture.name.is_empty());
            assert!(
                !fixture.source.is_empty(),
                "{}: a fixture nobody can check is a guess",
                fixture.name
            );
            assert!(fixture.vid != 0 && fixture.pid != 0, "{}", fixture.name);
            let names: BTreeSet<&str> = fixture.controls().into_iter().collect();
            assert_eq!(names.len(), fixture.controls().len(), "{}", fixture.name);
            for (name, _) in fixture.scancodes {
                assert!(
                    fixture.button(name).is_some(),
                    "{}: scancode for {name:?}, which it has no button for",
                    fixture.name
                );
            }
            if fixture.emits_scan {
                assert!(
                    !fixture.scancodes.is_empty(),
                    "{}: claims MSC_SCAN and names no usages",
                    fixture.name
                );
            }
            // Axes must have range and rest within that range.
            for (name, axis) in fixture.axes {
                assert!(
                    axis.maximum > axis.minimum,
                    "{}: {name} declares no travel",
                    fixture.name
                );
                assert!(
                    (axis.minimum..=axis.maximum).contains(&axis.rest),
                    "{}: {name} rests outside its own range",
                    fixture.name
                );
            }
        }
    }

    #[test]
    fn steams_mirror_is_an_xbox_pad_under_a_valve_id() {
        assert_eq!((STEAM_VIRTUAL.vid, STEAM_VIRTUAL.pid), (0x28DE, 0x11FF));
        assert_eq!(STEAM_VIRTUAL.buttons, XBOX_360.buttons);
        assert!(STEAM_VIRTUAL.name.starts_with(XBOX_360.name));
        let pad = STEAM_VIRTUAL.pad("event7");
        assert_eq!(pad.event(), "event7");
        assert_eq!((pad.vid, pad.pid), (0x28DE, 0x11FF));
    }

    #[test]
    fn steams_mirror_copies_xpads_table_but_stops_a_step_short_of_it() {
        // Recorded off Steam's uinput node: every axis but the bottom of a stick.
        assert_eq!(STEAM_VIRTUAL.axis("lt"), XBOX_360.axis("lt"));
        assert_eq!(STEAM_VIRTUAL.axis("rt"), XBOX_360.axis("rt"));
        for stick in ["lx", "ly", "rx", "ry"] {
            let mirror = STEAM_VIRTUAL.axis(stick).expect(stick);
            let xpad = XBOX_360.axis(stick).expect(stick);
            assert_eq!(mirror.minimum, -32767, "{stick}");
            assert_eq!(xpad.minimum, -32768, "{stick}");
            assert_eq!(
                (mirror.maximum, mirror.fuzz, mirror.flat),
                (xpad.maximum, xpad.fuzz, xpad.flat),
                "{stick}: only the floor differs"
            );
        }
    }

    #[test]
    fn a_deck_reports_x_and_y_on_each_others_codes() {
        // hid-steam writes BTN_X for the west button; BTN_X is BTN_NORTH.
        assert_eq!(STEAM_DECK.button("x"), Some(BTN_NORTH));
        assert_eq!(STEAM_DECK.button("y"), Some(BTN_WEST));
        assert_eq!(XBOX_360.button("x"), Some(BTN_WEST));
        assert_eq!(XBOX_360.button("y"), Some(BTN_NORTH));
        assert_eq!(STEAM_DECK.button("x"), XBOX_360.button("y"));
        assert_eq!(STEAM_DECK.button("y"), XBOX_360.button("x"));
        // A and B are not swapped, so this is not a whole-pad rotation.
        assert_eq!(STEAM_DECK.button("a"), XBOX_360.button("a"));
        assert_eq!(STEAM_DECK.button("b"), XBOX_360.button("b"));
    }

    #[test]
    fn a_decks_triggers_are_hat_two_and_it_has_no_abs_z_at_all() {
        let lt = STEAM_DECK.axis("lt").expect("lt");
        let rt = STEAM_DECK.axis("rt").expect("rt");
        assert_eq!((lt.code, lt.minimum, lt.maximum), (ABS_HAT2Y, 0, 32767));
        assert_eq!((rt.code, rt.minimum, rt.maximum), (ABS_HAT2X, 0, 32767));
        let codes = STEAM_DECK.abs_codes();
        assert!(!codes.contains(&ABS_Z), "a Deck declares no ABS_Z");
        assert!(!codes.contains(&ABS_RZ), "a Deck declares no ABS_RZ");
        // Read them as xpad's triggers and both are 128x too small.
        assert_eq!(XBOX_360.axis("lt").expect("lt").code, ABS_Z);
    }

    #[test]
    fn a_decks_hat_zero_is_a_trackpad_and_its_dpad_is_four_keys() {
        const { assert!(!STEAM_DECK.dpad_is_hat) };
        let pad = STEAM_DECK.axis("lpad_x").expect("lpad_x");
        assert_eq!(pad.code, ABS_HAT0X);
        assert_eq!((pad.minimum, pad.maximum), (-32767, 32767));
        assert_ne!(
            (pad.minimum, pad.maximum),
            (-1, 1),
            "a real hat is -1..1; this one is a finger"
        );
        for direction in DPAD {
            assert!(
                STEAM_DECK.button(direction).is_some(),
                "a Deck answers {direction} with a key"
            );
        }
        assert_eq!(STEAM_DECK.button("up"), Some(BTN_DPAD_UP));
    }

    #[test]
    fn a_decks_grips_are_on_codes_the_headers_do_not_name() {
        // SteamOS's hid-steam puts them at 0x224..0x227, in the gap after
        // BTN_DPAD_RIGHT; mainline v6.16 uses BTN_TRIGGER_HAPPY1..4.
        let grips: Vec<u16> = ["l4", "r4", "l5", "r5"]
            .iter()
            .map(|name| STEAM_DECK.button(name).expect("a grip"))
            .collect();
        assert_eq!(grips, vec![0x224, 0x225, 0x226, 0x227]);
        for code in &grips {
            assert!(*code > BTN_DPAD_RIGHT, "{code:#x} is past the named d-pad");
            assert!(*code < 0x2C0, "{code:#x} is not BTN_TRIGGER_HAPPY");
        }
    }

    #[test]
    fn the_gamecube_triggers_are_where_they_actually_are() {
        let lt = MAYFLASH_GAMECUBE.axis("lt").expect("lt");
        let rt = MAYFLASH_GAMECUBE.axis("rt").expect("rt");
        assert_eq!((lt.code, lt.rest), (ABS_RX, 24));
        assert_eq!((rt.code, rt.rest), (ABS_RY, 25));
        assert_eq!(MAYFLASH_GAMECUBE.axis("lx").expect("lx").rest, 128);
    }

    #[test]
    fn the_two_xbox_pads_differ_only_where_the_driver_says_they_do() {
        let three_sixty = XBOX_360.axis("lt").expect("lt");
        let series = XBOX_SERIES_X.axis("lt").expect("lt");
        assert_eq!(three_sixty.maximum, 255);
        assert_eq!(series.maximum, 1023, "XTYPE_XBOXONE is four times wider");
        assert_eq!(XBOX_360.axis("lx"), XBOX_SERIES_X.axis("lx"));
        assert_eq!(XBOX_SERIES_X.button("capture"), Some(KEY_RECORD)); // KEY_, not BTN_
        assert!(XBOX_360.button("capture").is_none());
    }

    #[test]
    fn only_the_hid_generic_pad_sends_a_scancode() {
        const { assert!(MAYFLASH_GAMECUBE.emits_scan) };
        const { assert!(!XBOX_360.emits_scan) };
        const { assert!(!XBOX_SERIES_X.emits_scan) };
    }

    #[test]
    fn opposite_directions_cancel_on_the_hat() {
        assert_eq!(Fixture::hat_from(&["right"]), (1, 0));
        assert_eq!(Fixture::hat_from(&["up"]), (0, -1));
        assert_eq!(Fixture::hat_from(&["left", "right"]), (0, 0));
        assert_eq!(Fixture::hat_from(&["down", "right"]), (1, 1));
        assert_eq!(Fixture::hat_from(&[]), (0, 0));
    }

    #[test]
    fn a_hat_pad_declares_the_hat_axes_it_reports_the_dpad_on() {
        for fixture in EVERY {
            if fixture.dpad_is_hat {
                let codes = fixture.abs_codes();
                assert!(codes.contains(&ABS_HAT0X), "{}", fixture.name);
                assert!(codes.contains(&ABS_HAT0Y), "{}", fixture.name);
                // Pad must answer to all four directions.
                for direction in DPAD {
                    assert!(
                        fixture.controls().contains(&direction),
                        "{}: no {direction}",
                        fixture.name
                    );
                }
            }
        }
    }
}
