//! A controller's keyboard and mouse are padmap's to hold while it is seated.
//!
//! A Steam Controller Puck is, in hardware, four keyboards and four mice
//! until something turns lizard mode off; an Xbox pad over Bluetooth carries
//! `Keyboard` and `Mouse` collections the kernel exposes as their own nodes.
//! A front-end takes the keyboard and the mouse on purpose and cannot tell
//! these from a real one. While a pad is seated, or is a candidate for a seat,
//! its siblings are grabbed as well as its joystick, and released with it.
//!
//! A sibling is known by `uniq` -- the device's own address -- or, for Valve
//! hardware, by vendor, since the Puck's lizard nodes carry no uniq. Never by
//! `phys`: over Bluetooth every device on an adapter reports the adapter's
//! address, and BlueZ's own media-control keyboard shares it with the pad.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use evdev::{KeyCode, RelativeAxisCode};
use log::{debug, info, warn};

use crate::pad::Pad;

pub const VALVE_VID: u16 = 0x28de;

/// A keyboard or mouse node, with what ties it to a controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub path: PathBuf,
    pub name: String,
    pub uniq: String,
    pub vid: u16,
    /// Types letters, or moves a pointer; nothing else is worth holding.
    pub keyboard: bool,
    pub mouse: bool,
}

/// The nodes a seated `pad` should hold beside its joystick.
pub fn of(pad: &Pad, nodes: &[Node]) -> Vec<PathBuf> {
    nodes
        .iter()
        .filter(|node| node.path != pad.path && (node.keyboard || node.mouse))
        .filter(|node| {
            (!pad.uniq.is_empty() && node.uniq == pad.uniq)
                || (pad.vid == VALVE_VID && node.vid == VALVE_VID)
        })
        .map(|node| node.path.clone())
        .collect()
}

/// Every keyboard and mouse node on the machine, by opening each to ask what it does.
pub fn scan() -> Vec<Node> {
    let Ok(mut enumerator) = udev::Enumerator::new() else {
        return Vec::new();
    };
    if enumerator.match_subsystem("input").is_err() {
        return Vec::new();
    }
    let Ok(devices) = enumerator.scan_devices() else {
        return Vec::new();
    };
    let only = std::env::var(crate::pad::ENV_ONLY).unwrap_or_default();
    let mut out = Vec::new();
    for device in devices {
        let Some(path) = device.devnode().map(Path::to_path_buf) else {
            continue;
        };
        if !path.to_string_lossy().starts_with("/dev/input/event") {
            continue;
        }
        let parent = device.parent();
        let owner = parent.as_ref().unwrap_or(&device);
        let name = crate::pad::attribute(owner, "name").unwrap_or_default();
        if !only.is_empty() && !name.contains(&only) {
            continue;
        }
        let Ok(opened) = evdev::Device::open(&path) else {
            continue;
        };
        let keyboard = opened
            .supported_keys()
            .is_some_and(|keys| keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_ENTER));
        let mouse = opened
            .supported_relative_axes()
            .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_X))
            || opened
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::BTN_LEFT));
        if !keyboard && !mouse {
            continue;
        }
        out.push(Node {
            path,
            name,
            uniq: crate::pad::attribute(owner, "uniq").unwrap_or_default(),
            vid: crate::pad::hex_attribute(owner, "id/vendor"),
            keyboard,
            mouse,
        });
    }
    out
}

/// The nodes currently held, open and grabbed; dropping one releases it.
#[derive(Debug, Default)]
pub struct Held {
    open: BTreeMap<PathBuf, evdev::Device>,
}

impl Held {
    pub fn paths(&self) -> Vec<&Path> {
        self.open.keys().map(PathBuf::as_path).collect()
    }

    /// Hold exactly `wanted`: grab what is new, let go of what is not wanted.
    pub fn sync(&mut self, wanted: &BTreeSet<PathBuf>) {
        let stale: Vec<PathBuf> = self
            .open
            .keys()
            .filter(|path| !wanted.contains(*path))
            .cloned()
            .collect();
        for path in stale {
            self.open.remove(&path);
            info!("released {}", path.display());
        }
        for path in wanted {
            if self.open.contains_key(path) {
                continue;
            }
            match evdev::Device::open(path) {
                Ok(mut device) => match device.grab() {
                    Ok(()) => {
                        info!(
                            "holding {} ({})",
                            path.display(),
                            device.name().unwrap_or("?")
                        );
                        self.open.insert(path.clone(), device);
                    }
                    Err(error) => warn!("could not hold {}: {error}", path.display()),
                },
                Err(error) => debug!("could not open {} to hold it: {error}", path.display()),
            }
        }
    }

    pub fn release_all(&mut self) {
        self.sync(&BTreeSet::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(path: &str, name: &str, uniq: &str, vid: u16) -> Pad {
        Pad {
            path: PathBuf::from(path),
            name: name.to_owned(),
            phys: "b8:27:eb:00:00:01".to_owned(),
            uniq: uniq.to_owned(),
            vid,
            pid: 0x028e,
            syspath: PathBuf::new(),
            retroarch_visible: true,
            motion: None,
        }
    }

    fn node(path: &str, name: &str, uniq: &str, vid: u16, keyboard: bool, mouse: bool) -> Node {
        Node {
            path: PathBuf::from(path),
            name: name.to_owned(),
            uniq: uniq.to_owned(),
            vid,
            keyboard,
            mouse,
        }
    }

    #[test]
    fn an_xbox_pads_keyboard_and_mouse_share_its_uniq_and_are_held() {
        let xbox = pad(
            "/dev/input/event258",
            "Xbox Wireless Controller",
            "0c:35:26:75:f6:2b",
            0x045e,
        );
        let nodes = [
            node(
                "/dev/input/event259",
                "Xbox Wireless Controller Keyboard",
                "0c:35:26:75:f6:2b",
                0x045e,
                true,
                false,
            ),
            node(
                "/dev/input/event260",
                "Xbox Wireless Controller Mouse",
                "0c:35:26:75:f6:2b",
                0x045e,
                false,
                true,
            ),
            node(
                "/dev/input/event261",
                "Xbox Wireless Controller Consumer Control",
                "0c:35:26:75:f6:2b",
                0x045e,
                false,
                false,
            ),
        ];
        assert_eq!(
            of(&xbox, &nodes),
            [
                PathBuf::from("/dev/input/event259"),
                PathBuf::from("/dev/input/event260")
            ],
            "consumer control types nothing and moves nothing"
        );
    }

    #[test]
    fn bluez_media_keys_share_the_adapters_phys_and_are_somebody_elses() {
        let xbox = pad(
            "/dev/input/event258",
            "Xbox Wireless Controller",
            "0c:35:26:75:f6:2b",
            0x045e,
        );
        let phone = node(
            "/dev/input/event3",
            "nixos #1 (MCS)",
            "a4:c3:f0:11:22:33",
            0x0000,
            true,
            false,
        );
        assert!(of(&xbox, &[phone]).is_empty());
    }

    #[test]
    fn the_pucks_lizard_keyboards_carry_no_uniq_and_are_known_by_vendor() {
        let puck = pad(
            "/dev/input/event9",
            "Valve Software Steam Controller",
            "",
            VALVE_VID,
        );
        let nodes = [
            node(
                "/dev/input/event1",
                "Valve Software Steam Controller Puck Keyboard",
                "",
                VALVE_VID,
                true,
                false,
            ),
            node(
                "/dev/input/event2",
                "Valve Software Steam Controller Puck Mouse",
                "",
                VALVE_VID,
                false,
                true,
            ),
            node(
                "/dev/input/event0",
                "AT Translated Set 2 keyboard",
                "",
                0x0001,
                true,
                false,
            ),
        ];
        assert_eq!(
            of(&puck, &nodes).len(),
            2,
            "the laptop's own keyboard stays"
        );
    }

    #[test]
    fn a_pad_with_no_uniq_and_no_vendor_rule_holds_nothing() {
        let usb = pad("/dev/input/event5", "Generic USB Joystick", "", 0x0079);
        let nodes = [node(
            "/dev/input/event6",
            "Generic USB Keyboard",
            "",
            0x0079,
            true,
            false,
        )];
        assert!(of(&usb, &nodes).is_empty(), "an empty uniq matches nothing");
    }

    #[test]
    fn the_pads_own_node_is_never_a_sibling() {
        let xbox = pad(
            "/dev/input/event258",
            "Xbox Wireless Controller",
            "u",
            0x045e,
        );
        let itself = node(
            "/dev/input/event258",
            "Xbox Wireless Controller",
            "u",
            0x045e,
            true,
            true,
        );
        assert!(of(&xbox, &[itself]).is_empty());
    }
}
