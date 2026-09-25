//! Discovery and identity for physical joypads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use danstick_core::capability::{self, Mask};
use log::debug;

pub const VIRTUAL_PHYS_PREFIX: &str = "danstick/";

/// Test escape hatch: restrict discovery to one device by name.
pub const ENV_ONLY: &str = "DANSTICK_ONLY_DEVICE";

/// Is this one of danstick's own clones? Check both phys and name (phys may fail to set).
pub fn is_danstick_clone(name: &str, phys: &str) -> bool {
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
    /// Cleared by `danstick hide` udev rules so RetroArch cannot see it.
    pub retroarch_visible: bool,
    /// The controller's motion sensor (separate node, not republished by danstick).
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
    (pad.vid, pad.pid) == danstick_core::icons::STEAM_VIRTUAL_ID
}

pub const REASON_STEAM_MIRROR: &str =
    "Steam's virtual gamepad mirrors a controller danstick already reads";

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

/// One physical controller is one pad: Steam's mirror goes when the pad it
/// mirrors is one danstick reads, and stays when it is the only way to reach one.
///
/// `unreadable` counts controllers Steam drives that danstick cannot read itself
/// -- the Deck's own controls in Game Mode, which Steam holds -- each reachable
/// only through its mirror. Steam mirrors at most one per pad danstick reads and
/// one per clone it can wrap (`clones`), so a surplus over both is kept too.
/// Kept lowest slot first: Steam numbers them in the order it opened the
/// controllers, and it opens the Deck's first. One mirror too many shows a pad
/// twice; one too few hides somebody's controller.
pub fn without_steam_mirrors(pads: Vec<Pad>, unreadable: usize, clones: usize) -> Discovery {
    let mirrors: Vec<&Pad> = pads.iter().filter(|pad| is_steam_virtual(pad)).collect();
    let readable = pads.len() - mirrors.len();
    if readable + clones == 0 {
        return Discovery {
            pads,
            dropped: Vec::new(),
        };
    }
    let keep = mirrors
        .len()
        .saturating_sub(readable + clones)
        .max(unreadable)
        .min(mirrors.len());
    let mut by_slot = mirrors.clone();
    by_slot.sort_by_key(|pad| (steam_slot(pad), pad.syspath.clone()));
    let kept: Vec<PathBuf> = by_slot
        .iter()
        .take(keep)
        .map(|pad| pad.path.clone())
        .collect();
    let (pads, dropped): (Vec<Pad>, Vec<Pad>) = pads
        .into_iter()
        .partition(|pad| !is_steam_virtual(pad) || kept.contains(&pad.path));
    Discovery {
        pads,
        dropped: dropped
            .into_iter()
            .map(|pad| Dropped {
                pad,
                reason: REASON_STEAM_MIRROR,
            })
            .collect(),
    }
}

/// Steam's slot for a mirror, from the index it appends to the name.
fn steam_slot(pad: &Pad) -> u32 {
    pad.name
        .rsplit(' ')
        .next()
        .and_then(|slot| slot.parse().ok())
        .unwrap_or(u32::MAX)
}

/// A HID device's `HID_ID` property -- `bus:vendor:product`, eight hex digits
/// each -- as vendor and product.
pub fn hid_id(text: &str) -> Option<(u16, u16)> {
    let mut parts = text.split(':');
    let _bus = parts.next()?;
    let vendor = u32::from_str_radix(parts.next()?, 16).ok()?;
    let product = u32::from_str_radix(parts.next()?, 16).ok()?;
    Some((u16::try_from(vendor).ok()?, u16::try_from(product).ok()?))
}

/// The Steam Deck's own controls, as the HID device Steam holds.
const STEAM_DECK_ID: (u16, u16) = (0x28DE, 0x1205);

/// How many controllers Steam drives that danstick reads no gamepad node for: a
/// Deck whose hidraw is there when no `Steam Deck` event node is. In Game Mode
/// Steam holds the hidraw, hid-steam publishes nothing, and the Deck exists
/// only as a Steam mirror (docs/STEAM-DECK.md).
fn steam_driven_unreadable(pads: &[Pad]) -> usize {
    if pads.iter().any(|pad| (pad.vid, pad.pid) == STEAM_DECK_ID) {
        return 0;
    }
    let Ok(mut enumerator) = udev::Enumerator::new() else {
        return 0;
    };
    if enumerator.match_subsystem("hidraw").is_err() {
        return 0;
    }
    let Ok(devices) = enumerator.scan_devices() else {
        return 0;
    };
    let decks = devices
        .filter(|device| {
            device
                .parent_with_subsystem("hid")
                .ok()
                .flatten()
                .and_then(|hid| {
                    hid.property_value("HID_ID")
                        .and_then(|id| hid_id(&id.to_string_lossy()))
                })
                == Some(STEAM_DECK_ID)
        })
        .count();
    // The Deck's controls publish more than one HID interface; it is one Deck.
    decks.min(1)
}

/// Would `DANSTICK_ONLY_DEVICE` let a pad with this name through?
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
    let mut wrappable_clones = 0;
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
        // danstick must not use ID_INPUT_JOYSTICK; `danstick hide` deliberately clears it.
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
        if !filter.include_virtual && is_danstick_clone(&name, &phys) {
            // Steam wraps a clone as it wraps any pad, unless it is Steam's own kind.
            let id = (
                hex_attribute(owner, "id/vendor"),
                hex_attribute(owner, "id/product"),
            );
            if id != danstick_core::icons::STEAM_VIRTUAL_ID {
                wrappable_clones += 1;
            }
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

    let scoped = std::env::var(ENV_ONLY).is_ok_and(|only| !only.is_empty());
    if let Ok(only) = std::env::var(ENV_ONLY) {
        if !only.is_empty() {
            pads.retain(|pad| pad.name.contains(&only));
            debug!("{ENV_ONLY} is set: {} pad(s) after filtering", pads.len());
        }
    }
    // A scoped discovery sees only its own pads, and a Deck or a clone
    // elsewhere on the machine is none of its business.
    let (unreadable, clones) = if scoped {
        (0, 0)
    } else {
        (steam_driven_unreadable(&pads), wrappable_clones)
    };
    let found = without_steam_mirrors(pads, unreadable, clones);
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

/// Every danstick clone the kernel is publishing, by name.
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
pub(crate) fn attribute(device: &udev::Device, name: &str) -> Option<String> {
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

pub(crate) fn hex_attribute(device: &udev::Device, name: &str) -> u16 {
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
        assert!(is_danstick_clone("danstick Player 1", "danstick/p1"));
        assert!(
            is_danstick_clone("danstick Player 1", ""),
            "phys can fail to set"
        );
        assert!(
            is_danstick_clone("", "danstick/p1"),
            "name is not guaranteed either"
        );
        assert!(!is_danstick_clone(
            "MAYFLASH GameCube Adapter",
            "usb-3/input0"
        ));
        assert!(
            !is_danstick_clone("danstick latency source", ""),
            "not a clone"
        );
        assert!(!is_danstick_clone("", ""));
    }

    #[test]
    fn steams_mirror_goes_when_any_other_pad_is_here() {
        use crate::fakepad::{STEAM_VIRTUAL, XBOX_360};
        let found = without_steam_mirrors(
            vec![STEAM_VIRTUAL.pad("event25"), XBOX_360.pad("event24")],
            0,
            0,
        );
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
        let found = without_steam_mirrors(vec![puck.clone(), STEAM_VIRTUAL.pad("event25")], 0, 0);
        assert_eq!(found.pads, vec![puck]);
        assert_eq!(found.dropped.len(), 1);
    }

    #[test]
    fn steams_mirror_stays_when_it_stands_alone() {
        use crate::fakepad::STEAM_VIRTUAL;
        let alone = vec![STEAM_VIRTUAL.pad("event25")];
        let found = without_steam_mirrors(alone.clone(), 0, 0);
        assert_eq!(found.pads, alone);
        assert!(found.dropped.is_empty());

        let two = vec![STEAM_VIRTUAL.pad("event25"), STEAM_VIRTUAL.pad("event26")];
        let found = without_steam_mirrors(two.clone(), 0, 0);
        assert_eq!(
            found.pads, two,
            "two mirrors and nothing else are still pads"
        );
        assert!(found.dropped.is_empty());
    }

    fn mirror(slot: u32, event: &str) -> Pad {
        let mut pad = crate::fakepad::STEAM_VIRTUAL.pad(event);
        pad.name = format!("Microsoft X-Box 360 pad {slot}");
        pad
    }

    #[test]
    fn the_decks_mirror_stays_beside_a_pad_danstick_reads() {
        use crate::fakepad::XBOX_360;
        // The Deck in Game Mode with an Xbox pad: Steam mirrors both, and only
        // the Xbox pad is readable. Listed out of slot order on purpose.
        let pads = vec![
            mirror(1, "event18"),
            XBOX_360.pad("event11"),
            mirror(0, "event10"),
        ];
        let found = without_steam_mirrors(pads, 1, 0);
        assert_eq!(
            found.pads,
            vec![XBOX_360.pad("event11"), mirror(0, "event10")],
            "the Deck's mirror, slot 0, went with the Xbox pad's"
        );
        assert_eq!(found.dropped.len(), 1);
        assert_eq!(found.dropped[0].pad, mirror(1, "event18"));
    }

    #[test]
    fn more_mirrors_than_pads_danstick_reads_keeps_the_surplus() {
        use crate::fakepad::XBOX_360;
        // Steam mirrors at most one per pad danstick reads, so a mirror over
        // that count is a controller danstick cannot see any other way.
        let pads = vec![
            mirror(0, "event10"),
            mirror(1, "event18"),
            XBOX_360.pad("event11"),
        ];
        let found = without_steam_mirrors(pads, 0, 0);
        assert_eq!(
            found.pads,
            vec![mirror(0, "event10"), XBOX_360.pad("event11")]
        );
    }

    #[test]
    fn the_decks_mirror_stays_even_when_steam_mirrors_nothing_else() {
        use crate::fakepad::XBOX_360;
        // Steam Input off for the Xbox pad: one mirror, one pad, and the count
        // alone would hide the Deck again. Knowing the Deck is there keeps it.
        let pads = vec![mirror(0, "event10"), XBOX_360.pad("event11")];
        let found = without_steam_mirrors(pads.clone(), 1, 0);
        assert_eq!(found.pads, pads);
        assert!(found.dropped.is_empty());
        // Without that knowledge it is the mirror of the pad danstick reads.
        assert_eq!(without_steam_mirrors(pads, 0, 0).pads.len(), 1);
    }

    #[test]
    fn a_mirror_of_dansticks_own_clone_is_not_a_controller() {
        use crate::fakepad::XBOX_360;
        // The Deck in Game Mode with four fixed slots and an Xbox pad: Steam
        // wraps the Deck (slot 0), the Xbox pad and all four 360 clones.
        let pads: Vec<Pad> = (0..6)
            .map(|slot| mirror(slot, &format!("event{}", 20 + slot)))
            .chain([XBOX_360.pad("event11")])
            .collect();
        let found = without_steam_mirrors(pads.clone(), 1, 4);
        assert_eq!(
            found.pads,
            vec![mirror(0, "event20"), XBOX_360.pad("event11")],
            "one entry per controller, none for a clone's mirror"
        );
        assert_eq!(found.dropped.len(), 5);
        assert_eq!(
            without_steam_mirrors(pads, 1, 0).pads.len(),
            6,
            "counting only the pads it reads, the clones' mirrors pass as controllers"
        );
    }

    #[test]
    fn mirrors_of_clones_alone_are_nobody() {
        // Fixed slots stand before anybody plugs in: Steam's pads are all the clones'.
        let pads: Vec<Pad> = (1..=4)
            .map(|slot| mirror(slot, &format!("event{}", 20 + slot)))
            .collect();
        assert!(without_steam_mirrors(pads, 0, 4).pads.is_empty());
    }

    #[test]
    fn clones_with_no_mirror_about_change_nothing() {
        use crate::fakepad::XBOX_360;
        let pads = vec![XBOX_360.pad("event11")];
        let found = without_steam_mirrors(pads.clone(), 0, 4);
        assert_eq!(found.pads, pads);
        assert!(found.dropped.is_empty());
    }

    #[test]
    fn more_clones_counted_than_mirrors_seen_drops_them_all() {
        let pads: Vec<Pad> = (1..=4)
            .map(|slot| mirror(slot, &format!("event{}", 20 + slot)))
            .collect();
        assert!(without_steam_mirrors(pads, 0, 99).pads.is_empty());
    }

    #[test]
    fn a_deck_known_to_be_there_keeps_a_mirror_even_if_steam_has_not_made_its_yet() {
        // The count cannot tell whose it keeps; seating's timing can (`echo`).
        let pads: Vec<Pad> = (1..=4)
            .map(|slot| mirror(slot, &format!("event{}", 20 + slot)))
            .collect();
        assert_eq!(
            without_steam_mirrors(pads, 1, 4).pads,
            vec![mirror(1, "event21")]
        );
    }

    #[test]
    fn a_hid_id_is_read_as_bus_vendor_product() {
        assert_eq!(hid_id("0003:000028DE:00001205"), Some((0x28DE, 0x1205)));
        assert_eq!(hid_id("0005:0000045E:00000B13"), Some((0x045E, 0x0B13)));
        assert_eq!(hid_id(""), None);
        assert_eq!(hid_id("0003:28DE"), None);
        assert_eq!(hid_id("0003:0000ZZZZ:00001205"), None);
    }

    #[test]
    fn a_room_with_no_mirror_is_left_exactly_as_it_was() {
        use crate::fakepad::{MAYFLASH_GAMECUBE, XBOX_360};
        let pads = vec![XBOX_360.pad("event3"), MAYFLASH_GAMECUBE.pad("event4")];
        let found = without_steam_mirrors(pads.clone(), 0, 0);
        assert_eq!(found.pads, pads);
        assert!(found.dropped.is_empty());
        assert!(without_steam_mirrors(Vec::new(), 0, 0).pads.is_empty());
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
        assert_eq!(VIRTUAL_PHYS_PREFIX, "danstick/");
        assert!(crate::clone::virtual_phys(1).starts_with(VIRTUAL_PHYS_PREFIX));
    }
}
