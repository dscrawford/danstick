//! Discovery and identity for physical joypads.
//!
//! The central fact this module exists to work around: on multi-port adapters,
//! several ports can be completely indistinguishable. The four ports of a
//! Mayflash GameCube adapter share one USB interface and one HID device, and
//! report identical name, phys, uniq, vid, pid, version and properties. Only
//! their `inputN` ordinal differs, and that rides on a global counter which
//! libudev sorts as a string.
//!
//! So there is deliberately no `Pad::stable_key()`. There is no stable key.
//! Identity comes from the user pressing a button.
//!
//! The Python asked udev the same questions by running `udevadm info -q
//! property -n <node>` as a subprocess, once per input device. On this machine
//! that is 32 processes and 257-294ms, and it ran from the thread that forwards
//! controller events -- the measured cause of the tail in docs/LATENCY.md. The
//! answers come from libudev directly here, which is the same database the
//! subprocess was printing.

use std::path::{Path, PathBuf};

use log::debug;
use padmap_core::capability::{self, Mask};

/// Virtual pads we publish are tagged with this phys prefix so that discovery
/// never picks up our own output. Without it, restarting the daemon would grab
/// its own pads and republish them, one layer deeper each time.
pub const VIRTUAL_PHYS_PREFIX: &str = "padmap/";

/// Test escape hatch: restrict discovery to one device by name.
///
/// Without it an isolated test daemon still finds the machine's real
/// controllers and fights the live daemon for an exclusive grab on them.
pub const ENV_ONLY: &str = "PADMAP_ONLY_DEVICE";

/// Is this one of padmap's own clones?
///
/// Two tests, because either can be absent. The phys tag is the intended one,
/// and the name is the fallback for a clone whose phys could not be set --
/// which is every clone this crate creates, since evdev 0.13.2 encodes
/// `UI_SET_PHYS` with the wrong payload size and the kernel refuses it.
///
/// Getting this wrong is not a cosmetic bug: discovery that does not recognise
/// padmap's output grabs it and republishes it, one layer deeper on every
/// restart.
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
    /// False once the `padmap hide` udev rules have cleared ID_INPUT_JOYSTICK:
    /// padmap can still open and republish the pad, RetroArch cannot see it.
    pub retroarch_visible: bool,
}

impl Pad {
    /// `eventN`.
    pub fn event(&self) -> &str {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
    }

    /// The string RetroArch stores as this pad's phys.
    ///
    /// `udev_joypad.c:498-504` reads EVIOCGPHYS then appends EVIOCGUNIQ at
    /// `pad->phys+physlen` with no separator. Reproduced so diagnostics can
    /// show exactly what RetroArch would compare against.
    pub fn retroarch_id(&self) -> String {
        format!("{}{}", self.phys, self.uniq)
    }
}

/// Would `PADMAP_ONLY_DEVICE` let a pad with this name through?
///
/// The filter below answers the same question for a list that already exists.
/// This one is for a caller that has to decide *before* doing something --
/// `triton::slots` sends a feature report to each slot to find out whether a
/// controller is paired into it, and that is a write to real hardware, which
/// is exactly what the switch exists to prevent.
pub fn wanted_by_name(name: &str) -> bool {
    wanted_by(name, std::env::var(ENV_ONLY).ok().as_deref())
}

/// The same question, for a caller that already has the setting.
///
/// Separated so it can be tested without `std::env::set_var`, which is
/// process-wide: a test that set it raced every other test calling
/// `discover`, and the failure landed on whichever one lost rather than on
/// the one at fault. That exact bug was removed from `runtime.rs` a few
/// commits ago and walked straight back in here.
pub fn wanted_by(name: &str, only: Option<&str>) -> bool {
    match only {
        Some(only) if !only.is_empty() => name.contains(only),
        _ => true,
    }
}

/// What a caller wants out of [`discover`].
#[derive(Debug, Clone, Copy)]
pub struct Filter {
    /// Include padmap's own clones. Off by default, for the reason above.
    pub include_virtual: bool,
    /// Only the pads RetroArch can currently see, which is what pad-index
    /// prediction needs. The two differ exactly when the hide rules are
    /// installed.
    pub retroarch_only: bool,
    /// Include controllers padmap drives itself, which the kernel publishes
    /// no joypad for. On by default: they are controllers, and the whole
    /// point of driving them is that they behave like any other.
    ///
    /// Off for `retroarch_only`, truthfully -- RetroArch cannot see a device
    /// with no evdev node, and counting one would shift every pad index.
    pub include_undriven: bool,
}

impl Default for Filter {
    /// Written out rather than derived, because one field's default is not
    /// `false` and a derive would silently say it was.
    fn default() -> Self {
        Filter {
            include_virtual: false,
            retroarch_only: false,
            include_undriven: true,
        }
    }
}

/// Joypads, in the order RetroArch's udev driver would enumerate them.
///
/// libudev returns enumerate results sorted by syspath (verified against
/// `udevadm trigger --dry-run`), and RetroArch assigns each the first vacant
/// slot, so this ordering is what its pad indices would be.
pub fn discover(filter: Filter) -> std::io::Result<Vec<Pad>> {
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("input")?;

    let mut pads: Vec<Pad> = Vec::new();
    for device in enumerator.scan_devices()? {
        let Some(devnode) = device.devnode().map(Path::to_path_buf) else {
            continue;
        };
        if !devnode.to_string_lossy().starts_with("/dev/input/event") {
            continue;
        }

        // RetroArch's own filter, udev_joypad.c:1053.
        let visible = property(&device, "ID_INPUT_JOYSTICK").as_deref() == Some("1");
        // padmap must not use ID_INPUT_JOYSTICK for its own discovery, because
        // `padmap hide` deliberately clears it. Sharing the filter would mean
        // installing the hide rules made every controller invisible to padmap
        // too, so setup could never be run again -- unrecoverable without
        // hand-removing the rules.
        if !visible && !looks_like_joypad(&devnode) {
            continue;
        }
        if filter.retroarch_only && !visible {
            continue;
        }

        // The capability and identity attributes live on the parent `inputN`
        // directory, not on the `eventN` node.
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
        });
    }

    pads.sort_by(|left, right| left.syspath.cmp(&right.syspath));

    // A 2026 Steam Controller has no joypad node the loop above could find --
    // see `triton::slots`. Appended, not merged: the order above is
    // RetroArch's enumeration order and these are not in it, so putting one in
    // the middle would shift every pad below it.
    if filter.include_undriven && !filter.retroarch_only {
        pads.extend(crate::triton::slots(true));
    }

    if let Ok(only) = std::env::var(ENV_ONLY) {
        if !only.is_empty() {
            pads.retain(|pad| pad.name.contains(&only));
            debug!("{ENV_ONLY} is set: {} pad(s) after filtering", pads.len());
        }
    }
    Ok(pads)
}

/// Pads that no static identifier can tell apart.
///
/// Used to explain to the user why press-to-activate is required rather than
/// letting them think it is a stylistic choice.
pub fn ambiguous_groups(pads: &[Pad]) -> Vec<Vec<&Pad>> {
    /// Every static attribute a pad has. Four Mayflash ports agree on all five.
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

/// Is this a joypad by capability, regardless of how udev tagged it?
///
/// From sysfs first. Opening every input node to ask two questions means
/// closing every input node afterwards, and releasing a USB HID descriptor
/// takes about 11ms while the driver tears down its URB -- measured at 390ms
/// of a 400ms scan, in 36 calls to close. The bitmaps read here are the same
/// ones udev's `input_id` builtin reads to decide ID_INPUT_JOYSTICK.
fn looks_like_joypad(devnode: &Path) -> bool {
    match capability_verdict(devnode) {
        Some(verdict) => verdict,
        // The files were not there, or were not bitmaps. Ask the device, which
        // is what this used to do always.
        None => looks_like_joypad_by_opening(devnode),
    }
}

/// The sysfs answer, or `None` if it cannot be had.
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
///
/// `name`, `phys` and the id files sit on the `inputN` directory; a caller may
/// hand us either that or the `eventN` child, and both have to answer.
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
        // Reproducing udev_joypad.c exactly, including the absence of a
        // separator -- a diagnostic that inserts one compares against a string
        // RetroArch never stores.
        let pad = pad("x", "usb-0000:00:14.0-3/input0", "ab:cd", 0, 0, "event1");
        assert_eq!(pad.retroarch_id(), "usb-0000:00:14.0-3/input0ab:cd");
    }

    #[test]
    fn four_identical_adapter_ports_group_together() {
        // The Mayflash adapter: four ports, one USB interface, byte-identical
        // in every attribute the kernel exposes.
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
        // Either can be absent, and discovery that misses one grabs padmap's
        // own output and republishes it, one layer deeper on every restart.
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
    fn the_virtual_prefix_is_what_the_clones_are_published_under() {
        // If these two ever disagree, the daemon republishes its own output on
        // restart, one layer deeper each time.
        assert_eq!(VIRTUAL_PHYS_PREFIX, "padmap/");
        assert!(crate::clone::virtual_phys(1).starts_with(VIRTUAL_PHYS_PREFIX));
    }
}
