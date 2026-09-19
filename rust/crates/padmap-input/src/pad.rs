//! Discovery and identity for physical joypads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use log::debug;
use padmap_core::capability::{self, Mask};

pub const VIRTUAL_PHYS_PREFIX: &str = "padmap/";

/// Test escape hatch: restrict discovery to one device by name.
pub const ENV_ONLY: &str = "PADMAP_ONLY_DEVICE";

/// Is this one of padmap's own clones? Check both phys and name (phys may fail to set).
pub fn is_padmap_clone(name: &str, phys: &str) -> bool {
    phys.starts_with(VIRTUAL_PHYS_PREFIX) || name.starts_with(crate::clone::VIRTUAL_PREFIX)
}

/// A physical joypad node, as RetroArch's udev driver would see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pad {
    pub path: PathBuf,
    pub name: String,
    pub phys: String,
    pub uniq: String,
    pub vid: u16,
    pub pid: u16,
    pub syspath: PathBuf,
    /// Cleared by `padmap hide` udev rules so RetroArch cannot see it.
    pub retroarch_visible: bool,
    /// The controller's motion sensor (separate node, not republished by padmap).
    pub motion: Option<PathBuf>,
}

impl Pad {
    pub fn event(&self) -> &str {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
    }

    /// phys + uniq with no separator (matches udev_joypad.c:498-504).
    pub fn retroarch_id(&self) -> String {
        format!("{}{}", self.phys, self.uniq)
    }
}

/// Steam Input's virtual gamepad: a mirror of a pad Steam is driving, by id.
pub fn is_steam_virtual(pad: &Pad) -> bool {
    (pad.vid, pad.pid) == padmap_core::icons::STEAM_VIRTUAL_ID
}

pub const REASON_STEAM_MIRROR: &str =
    "Steam's virtual gamepad mirrors a controller padmap already reads";

/// A node discovery found and chose not to offer, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub pad: Pad,
    pub reason: &'static str,
}

/// What discovery found: the pads offered, and the nodes left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    pub pads: Vec<Pad>,
    pub dropped: Vec<Dropped>,
}

/// One physical controller is one pad: Steam's mirror goes when the pad it mirrors is here.
///
/// The mirror is kept when it is the only pad, since a controller Steam alone
/// can drive is still a controller.
pub fn without_steam_mirrors(pads: Vec<Pad>) -> Discovery {
    if !pads.iter().any(|pad| !is_steam_virtual(pad)) {
        return Discovery {
            pads,
            dropped: Vec::new(),
        };
    }
    let (mirrors, pads): (Vec<Pad>, Vec<Pad>) = pads.into_iter().partition(is_steam_virtual);
    Discovery {
        pads,
        dropped: mirrors
            .into_iter()
            .map(|pad| Dropped {
                pad,
                reason: REASON_STEAM_MIRROR,
            })
            .collect(),
    }
}

/// Would `PADMAP_ONLY_DEVICE` let a pad with this name through?
pub fn wanted_by_name(name: &str) -> bool {
    wanted_by(name, std::env::var(ENV_ONLY).ok().as_deref())
}

pub fn wanted_by(name: &str, only: Option<&str>) -> bool {
    match only {
        Some(only) if !only.is_empty() => name.contains(only),
        _ => true,
    }
}

/// What a caller wants out of [`discover`].
#[derive(Debug, Clone, Copy)]
pub struct Filter {
    pub include_virtual: bool,
    pub retroarch_only: bool,
    pub include_undriven: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            include_virtual: false,
            retroarch_only: false,
            include_undriven: true,
        }
    }
}

/// Joypads in RetroArch enumeration order (libudev sorts by syspath).
pub fn discover(filter: Filter) -> std::io::Result<Vec<Pad>> {
    discover_all(filter).map(|found| found.pads)
}

/// [`discover`], keeping what was dropped so a caller can say so.
pub fn discover_all(filter: Filter) -> std::io::Result<Discovery> {
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("input")?;

    let mut accelerometers: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut pads: Vec<Pad> = Vec::new();
    for device in enumerator.scan_devices()? {
        let Some(devnode) = device.devnode().map(Path::to_path_buf) else {
            continue;
        };
        if !devnode.to_string_lossy().starts_with("/dev/input/event") {
            continue;
        }

        if property(&device, "ID_INPUT_ACCELEROMETER").as_deref() == Some("1") {
            accelerometers.push((
                std::fs::canonicalize(device.syspath())
                    .unwrap_or_else(|_| device.syspath().to_path_buf()),
                devnode.clone(),
            ));
        }

        let visible = property(&device, "ID_INPUT_JOYSTICK").as_deref() == Some("1");
        // padmap must not use ID_INPUT_JOYSTICK; `padmap hide` deliberately clears it.
        if !visible && !looks_like_joypad(&devnode) {
            continue;
        }
        if filter.retroarch_only && !visible {
            continue;
        }

        let parent = device.parent();
        let owner = parent.as_ref().unwrap_or(&device);

        let phys = attribute(owner, "phys").unwrap_or_default();
        let name = attribute(owner, "name").unwrap_or_default();
        if !filter.include_virtual && is_padmap_clone(&name, &phys) {
            continue;
        }

        pads.push(Pad {
            syspath: std::fs::canonicalize(device.syspath())
                .unwrap_or_else(|_| device.syspath().to_path_buf()),
            name,
            phys,
            uniq: attribute(owner, "uniq").unwrap_or_default(),
            vid: hex_attribute(owner, "id/vendor"),
            pid: hex_attribute(owner, "id/product"),
            retroarch_visible: visible,
            path: devnode,
            motion: None,
        });
    }

    for pad in &mut pads {
        pad.motion = motion_sibling(&pad.syspath, &accelerometers).map(Path::to_path_buf);
    }

    pads.sort_by(|left, right| left.syspath.cmp(&right.syspath));

    if filter.include_undriven && !filter.retroarch_only {
        pads.extend(crate::triton::slots(true));
    }

    if let Ok(only) = std::env::var(ENV_ONLY) {
        if !only.is_empty() {
            pads.retain(|pad| pad.name.contains(&only));
            debug!("{ENV_ONLY} is set: {} pad(s) after filtering", pads.len());
        }
    }
    let found = without_steam_mirrors(pads);
    for dropped in &found.dropped {
        debug!(
            "{} ({}) left out: {}",
            dropped.pad.name,
            dropped.pad.event(),
            dropped.reason
        );
    }
    Ok(found)
}

/// Every padmap clone the kernel is publishing, by name.
pub fn clone_nodes() -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/input") else {
        return found;
    };
    for entry in entries.flatten() {
        let node = entry.file_name();
        let Some(node) = node.to_str() else { continue };
        if !node.starts_with("event") {
            continue;
        }
        let Ok(name) = std::fs::read_to_string(entry.path().join("device/name")) else {
            continue;
        };
        let name = name.trim();
        if name.starts_with(crate::clone::VIRTUAL_PREFIX) {
            found.insert(name.to_owned(), PathBuf::from("/dev/input").join(node));
        }
    }
    found
}

/// Motion sensor belonging to a pad: matched by longest shared syspath prefix (ancestor).
pub fn motion_sibling<'a>(
    pad: &Path,
    accelerometers: &'a [(PathBuf, PathBuf)],
) -> Option<&'a Path> {
    let mine = pad.parent()?;
    accelerometers
        .iter()
        .filter(|(syspath, _)| syspath.parent() == Some(mine) || syspath.starts_with(mine))
        .map(|(_, devnode)| devnode.as_path())
        .next()
}

/// Pads that no static identifier can tell apart (used for explaining press-to-activate requirement).
pub fn ambiguous_groups(pads: &[Pad]) -> Vec<Vec<&Pad>> {
    type Identity<'a> = (&'a str, &'a str, &'a str, u16, u16);

    let mut groups: Vec<(Identity<'_>, Vec<&Pad>)> = Vec::new();
    for pad in pads {
        let key = (
            pad.name.as_str(),
            pad.phys.as_str(),
            pad.uniq.as_str(),
            pad.vid,
            pad.pid,
        );
        match groups.iter_mut().find(|(candidate, _)| *candidate == key) {
            Some((_, members)) => members.push(pad),
            None => groups.push((key, vec![pad])),
        }
    }
    groups
        .into_iter()
        .map(|(_, members)| members)
        .filter(|group| group.len() > 1)
        .collect()
}

/// Is this a joypad by capability? Check sysfs first (read bitmaps like udev's input_id builtin).
fn looks_like_joypad(devnode: &Path) -> bool {
    match capability_verdict(devnode) {
        Some(verdict) => verdict,
        None => looks_like_joypad_by_opening(devnode),
    }
}

fn capability_verdict(devnode: &Path) -> Option<bool> {
    let name = devnode.file_name()?;
    let caps = Path::new("/sys/class/input")
        .join(name)
        .join("device/capabilities");
    let read = |leaf: &str| {
        std::fs::read_to_string(caps.join(leaf))
            .ok()
            .as_deref()
            .and_then(Mask::parse)
    };
    capability::joypad(read("abs").as_ref(), read("key").as_ref())
}

fn looks_like_joypad_by_opening(devnode: &Path) -> bool {
    let Ok(device) = evdev::Device::open(devnode) else {
        return false;
    };
    if device
        .supported_absolute_axes()
        .is_none_or(|axes| axes.iter().next().is_none())
    {
        return false;
    }
    device.supported_keys().is_some_and(|keys| {
        keys.iter()
            .any(|key| capability::BTN_JOYSTICK_RANGE.contains(&key.0))
    })
}

fn property(device: &udev::Device, name: &str) -> Option<String> {
    device
        .property_value(name)
        .map(|value| value.to_string_lossy().into_owned())
}

/// A sysfs attribute, walking up to the first parent that has it.
fn attribute(device: &udev::Device, name: &str) -> Option<String> {
    let mut current = Some(device.clone());
    for _ in 0..4 {
        let device = current?;
        if let Some(value) = device.attribute_value(name) {
            return Some(value.to_string_lossy().trim().to_owned());
        }
        current = device.parent();
    }
    None
}

fn hex_attribute(device: &udev::Device, name: &str) -> u16 {
    attribute(device, name)
        .and_then(|raw| u16::from_str_radix(raw.trim(), 16).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_pads_gyro_is_the_sibling_under_its_own_parent() {
        let hid = PathBuf::from("/sys/devices/pci0000:00/usb1/1-2/1-2:1.0/0003:057E:2009.0001");
        let pad = hid.join("input/input20/event18");
        let imu = hid.join("input/input21/event19");
        let accelerometers = vec![(imu.clone(), PathBuf::from("/dev/input/event19"))];
        assert_eq!(
            motion_sibling(&pad, &accelerometers),
            None,
            "input21 is not under input20, and must not be claimed by prefix alone"
        );

        let together = PathBuf::from("/sys/devices/pci0000:00/usb1/1-2/input/input20");
        let pad = together.join("event18");
        let accelerometers = vec![(
            together.join("event19"),
            PathBuf::from("/dev/input/event19"),
        )];
        assert_eq!(
            motion_sibling(&pad, &accelerometers).map(Path::to_path_buf),
            Some(PathBuf::from("/dev/input/event19"))
        );
    }

    #[test]
    fn a_gyro_on_a_different_device_is_not_claimed() {
        let mine = PathBuf::from("/sys/devices/usb1/1-2/input/input20");
        let theirs = PathBuf::from("/sys/devices/usb1/1-3/input/input30");
        let accelerometers = vec![(theirs.join("event31"), PathBuf::from("/dev/input/event31"))];
        assert_eq!(motion_sibling(&mine.join("event18"), &accelerometers), None);
    }

    #[test]
    fn a_pad_with_no_accelerometer_anywhere_has_none() {
        let pad = PathBuf::from("/sys/devices/usb1/1-2/input/input20/event18");
        assert_eq!(motion_sibling(&pad, &[]), None);
    }

    use super::*;

    fn pad(name: &str, phys: &str, uniq: &str, vid: u16, pid: u16, event: &str) -> Pad {
        Pad {
            path: PathBuf::from(format!("/dev/input/{event}")),
            name: name.to_owned(),
            phys: phys.to_owned(),
            uniq: uniq.to_owned(),
            vid,
            pid,
            syspath: PathBuf::from(format!("/sys/devices/{event}")),
            retroarch_visible: true,
            motion: None,
        }
    }

    #[test]
    fn the_event_name_is_the_basename_of_the_node() {
        assert_eq!(pad("x", "", "", 0, 0, "event12").event(), "event12");
    }

    #[test]
    fn a_pad_with_no_node_name_reports_an_empty_event_rather_than_panicking() {
        let mut orphan = pad("x", "", "", 0, 0, "event0");
        orphan.path = PathBuf::from("/");
        assert_eq!(orphan.event(), "");
    }

    #[test]
    fn the_retroarch_id_concatenates_phys_and_uniq_with_no_separator() {
        let pad = pad("x", "usb-0000:00:14.0-3/input0", "ab:cd", 0, 0, "event1");
        assert_eq!(pad.retroarch_id(), "usb-0000:00:14.0-3/input0ab:cd");
    }

    #[test]
    fn four_identical_adapter_ports_group_together() {
        let pads: Vec<Pad> = (0..4)
            .map(|index| {
                pad(
                    "MAYFLASH GameCube Adapter",
                    "usb-3/input0",
                    "",
                    0x0079,
                    0x1843,
                    &format!("event{index}"),
                )
            })
            .collect();
        let groups = ambiguous_groups(&pads);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 4);
    }

    #[test]
    fn pads_that_differ_in_any_attribute_are_not_grouped() {
        let pads = vec![
            pad("A", "p", "", 1, 2, "event0"),
            pad("B", "p", "", 1, 2, "event1"),
            pad("A", "q", "", 1, 2, "event2"),
            pad("A", "p", "u", 1, 2, "event3"),
            pad("A", "p", "", 9, 2, "event4"),
            pad("A", "p", "", 1, 9, "event5"),
        ];
        assert!(ambiguous_groups(&pads).is_empty());
    }

    #[test]
    fn a_lone_pad_is_not_a_group() {
        assert!(ambiguous_groups(&[pad("A", "p", "", 1, 2, "event0")]).is_empty());
        assert!(ambiguous_groups(&[]).is_empty());
    }

    #[test]
    fn a_clone_is_recognised_by_either_tag() {
        assert!(is_padmap_clone("padmap Player 1", "padmap/p1"));
        assert!(
            is_padmap_clone("padmap Player 1", ""),
            "phys can fail to set"
        );
        assert!(
            is_padmap_clone("", "padmap/p1"),
            "name is not guaranteed either"
        );
        assert!(!is_padmap_clone(
            "MAYFLASH GameCube Adapter",
            "usb-3/input0"
        ));
        assert!(!is_padmap_clone("padmap latency source", ""), "not a clone");
        assert!(!is_padmap_clone("", ""));
    }

    #[test]
    fn steams_mirror_goes_when_any_other_pad_is_here() {
        use crate::fakepad::{STEAM_VIRTUAL, XBOX_360};
        let found =
            without_steam_mirrors(vec![STEAM_VIRTUAL.pad("event25"), XBOX_360.pad("event24")]);
        assert_eq!(found.pads, vec![XBOX_360.pad("event24")]);
        assert_eq!(found.dropped.len(), 1);
        assert_eq!(found.dropped[0].pad, STEAM_VIRTUAL.pad("event25"));
        assert_eq!(found.dropped[0].reason, REASON_STEAM_MIRROR);
    }

    #[test]
    fn steams_mirror_goes_beside_a_puck_slot_too() {
        use crate::fakepad::STEAM_VIRTUAL;
        let puck = pad(
            "Steam Controller",
            "",
            "u/0003:28DE:1304.0007",
            0x28DE,
            0x1304,
            "event0",
        );
        let found = without_steam_mirrors(vec![puck.clone(), STEAM_VIRTUAL.pad("event25")]);
        assert_eq!(found.pads, vec![puck]);
        assert_eq!(found.dropped.len(), 1);
    }

    #[test]
    fn steams_mirror_stays_when_it_stands_alone() {
        use crate::fakepad::STEAM_VIRTUAL;
        let alone = vec![STEAM_VIRTUAL.pad("event25")];
        let found = without_steam_mirrors(alone.clone());
        assert_eq!(found.pads, alone);
        assert!(found.dropped.is_empty());

        let two = vec![STEAM_VIRTUAL.pad("event25"), STEAM_VIRTUAL.pad("event26")];
        let found = without_steam_mirrors(two.clone());
        assert_eq!(
            found.pads, two,
            "two mirrors and nothing else are still pads"
        );
        assert!(found.dropped.is_empty());
    }

    #[test]
    fn a_room_with_no_mirror_is_left_exactly_as_it_was() {
        use crate::fakepad::{MAYFLASH_GAMECUBE, XBOX_360};
        let pads = vec![XBOX_360.pad("event3"), MAYFLASH_GAMECUBE.pad("event4")];
        let found = without_steam_mirrors(pads.clone());
        assert_eq!(found.pads, pads);
        assert!(found.dropped.is_empty());
        assert!(without_steam_mirrors(Vec::new()).pads.is_empty());
    }

    #[test]
    fn the_mirror_is_known_by_its_id_and_not_by_the_name_it_borrows() {
        use crate::fakepad::{STEAM_VIRTUAL, XBOX_360};
        assert!(is_steam_virtual(&STEAM_VIRTUAL.pad("event1")));
        assert!(!is_steam_virtual(&XBOX_360.pad("event2")));
        assert_eq!(STEAM_VIRTUAL.name, XBOX_360.name.to_owned() + " 0");
    }

    #[test]
    fn the_virtual_prefix_is_what_the_clones_are_published_under() {
        assert_eq!(VIRTUAL_PHYS_PREFIX, "padmap/");
        assert!(crate::clone::virtual_phys(1).starts_with(VIRTUAL_PHYS_PREFIX));
    }
}
