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

const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;
const BTN_NORTH: u16 = 0x133;
const BTN_WEST: u16 = 0x134;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
const BTN_SELECT: u16 = 0x13A;
const BTN_START: u16 = 0x13B;
const BTN_MODE: u16 = 0x13C;
const BTN_THUMBL: u16 = 0x13D;
const BTN_THUMBR: u16 = 0x13E;
/// `KEY_RECORD`: a key code, not a BTN_.
const KEY_RECORD: u16 = 167;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

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
             A live recording beside a puck is still owed (docs/requests/\
             one-controller-one-pad.md)",
    emits_scan: false,
    scancodes: &[],
    buttons: XBOX_360.buttons,
    axes: XBOX_360.axes,
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

pub const EVERY: [&Fixture; 3] = [&XBOX_360, &XBOX_SERIES_X, &MAYFLASH_GAMECUBE];

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
        assert_eq!(STEAM_VIRTUAL.axes, XBOX_360.axes);
        assert!(STEAM_VIRTUAL.name.starts_with(XBOX_360.name));
        let pad = STEAM_VIRTUAL.pad("event7");
        assert_eq!(pad.event(), "event7");
        assert_eq!((pad.vid, pad.pid), (0x28DE, 0x11FF));
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
