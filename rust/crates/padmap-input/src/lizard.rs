//! Controllers the kernel has, but not as controllers.
//!
//! Some pads power up pretending to be a mouse and a keyboard -- Valve calls it
//! lizard mode -- and put their real gamepad state on a *vendor-defined* HID
//! interface that only a driver written for that vendor can read. Until
//! something sends the command that leaves lizard mode, the machine has a
//! keyboard where a controller should be.
//!
//! The kernel ships `hid-steam` to do exactly that, and it matches on product
//! id. A model newer than the driver's table falls through to `hid-generic`,
//! which faithfully exposes the mouse and the keyboard and cannot know that the
//! fifth interface is a gamepad. Nothing reports an error: the device enumerates
//! perfectly, and simply is not a controller.
//!
//! Measured on this machine, kernel 6.18.44, with a Steam Controller Puck:
//!
//! ```text
//! hid-steam claims  28de:1102  28de:1142  28de:1205
//! the Puck is       28de:1304                    -> hid-generic
//! four interfaces   05 01 09 02 ...  Generic Desktop / Mouse
//! one interface     06 00 ff 09 02 ... vendor page 0xFF00, report 0x42
//! ```
//!
//! The shape is: a vendor-defined interface, sibling keyboard and mouse nodes,
//! and no joypad. That is lizard mode -- and it is also, exactly, a wireless
//! keyboard-and-mouse receiver. Both were on this machine when this was
//! written: a Logitech Unifying receiver matches every part of it, and so does
//! an RGB lighting controller if the keyboard test is left out.
//!
//! There is no way to tell them apart from outside, so this does not try. It
//! reports only devices whose vendor has a kernel driver for this family --
//! somewhere there is a remedy to offer. A warning about a device nobody can
//! help with is not a diagnosis; it is noise that teaches the user to ignore
//! the one that matters.

use std::path::{Path, PathBuf};

/// A controller the kernel is not treating as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dormant {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    /// The driver actually bound, usually `hid-generic`.
    pub driver: String,
    /// The vendor-defined interface's hidraw node, if it has one.
    pub hidraw: Option<PathBuf>,
}

/// Vendors whose controllers do this and whose driver matches on product id.
///
/// Valve only. `hid-steam` claims 28de:1102, :1142 and :1205, and a model newer
/// than that table -- the Puck is 28de:1304 -- gets nothing.
const KNOWN_FAMILIES: [(u16, &str); 1] = [(0x28DE, "hid-steam")];

fn has_kernel_driver(vid: u16) -> bool {
    KNOWN_FAMILIES
        .iter()
        .any(|(candidate, _)| *candidate == vid)
}

impl Dormant {
    /// The kernel driver that drives this vendor's other models.
    pub fn kernel_driver(&self) -> Option<&'static str> {
        KNOWN_FAMILIES
            .iter()
            .find(|(vid, _)| *vid == self.vid)
            .map(|(_, driver)| *driver)
    }

    /// What to run. Two lines, because the first is not persistent.
    pub fn remedy(&self) -> Option<String> {
        let driver = self.kernel_driver()?;
        let short = driver.trim_start_matches("hid-");
        Some(format!(
            "sudo modprobe {driver}\n  \
             echo \"0003 {:04X} {:04X}\" | sudo tee /sys/bus/hid/drivers/{short}/new_id",
            self.vid, self.pid
        ))
    }
}

/// A report descriptor whose first item selects a vendor-defined usage page.
///
/// `06 xx FF` is Usage Page (vendor-defined); anything in that range is a
/// private protocol, which is exactly what a generic driver cannot read.
pub fn is_vendor_interface(descriptor: &[u8]) -> bool {
    descriptor.len() >= 3 && descriptor[0] == 0x06 && descriptor[2] == 0xFF
}

/// Scan for controllers that are present but not being driven as controllers.
pub fn dormant() -> Vec<Dormant> {
    let Ok(mut enumerator) = udev::Enumerator::new() else {
        return Vec::new();
    };
    if enumerator.match_subsystem("hid").is_err() {
        return Vec::new();
    }
    let Ok(devices) = enumerator.scan_devices() else {
        return Vec::new();
    };

    let mut found: Vec<Dormant> = Vec::new();
    for device in devices {
        let syspath = device.syspath().to_path_buf();
        let descriptor = std::fs::read(syspath.join("report_descriptor")).unwrap_or_default();
        if !is_vendor_interface(&descriptor) {
            continue;
        }
        let driver = device
            .property_value("DRIVER")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        // A device a real driver has claimed is somebody's problem already.
        if driver != "hid-generic" {
            continue;
        }
        let Some((vid, pid)) = hid_ids(&device) else {
            continue;
        };
        // ...one that already offers a joypad needs nothing said about it...
        if has_joypad(&syspath) {
            continue;
        }
        // ...and a device with no keyboard or mouse is not in lizard mode; it
        // is something that was never a controller.
        if !looks_like_lizard_mode(&syspath) {
            continue;
        }
        if found.iter().any(|seen| (seen.vid, seen.pid) == (vid, pid)) {
            continue;
        }
        // Only where there is something to do about it. See the module note:
        // the shape matches keyboard receivers too, and cannot be narrowed
        // further from outside.
        if !has_kernel_driver(vid) {
            continue;
        }
        found.push(Dormant {
            name: device
                .property_value("HID_NAME")
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default(),
            vid,
            pid,
            driver,
            hidraw: first_hidraw(&syspath),
        });
    }
    found
}

/// `HID_ID=0003:000028DE:00001304` -> (0x28DE, 0x1304).
fn hid_ids(device: &udev::Device) -> Option<(u16, u16)> {
    let raw = device
        .property_value("HID_ID")?
        .to_string_lossy()
        .into_owned();
    let mut parts = raw.split(':');
    let _bus = parts.next()?;
    let vid = u16::from_str_radix(parts.next()?.trim_start_matches('0'), 16).ok()?;
    let pid = u16::from_str_radix(parts.next()?.trim_start_matches('0'), 16).ok()?;
    Some((vid, pid))
}

/// Does this HID device, or any sibling of it, offer a joypad?
///
/// Asked of the whole USB device rather than the one interface, because the
/// gamepad and the mouse are different interfaces of one controller.
fn has_joypad(syspath: &Path) -> bool {
    let Some(usb_device) = syspath
        .ancestors()
        .find(|path| path.join("idVendor").is_file())
    else {
        return false;
    };
    let mut stack = vec![usb_device.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("js") {
                return true;
            }
            if name.starts_with("input") || name.starts_with("event") || name.contains(':') {
                stack.push(path);
            }
        }
    }
    false
}

/// Does this device present as a keyboard or a mouse?
///
/// What lizard mode looks like from outside, and the test that separates a
/// controller nobody is driving from a vendor HID device that was never one.
fn looks_like_lizard_mode(syspath: &Path) -> bool {
    let Some(usb_device) = syspath
        .ancestors()
        .find(|path| path.join("idVendor").is_file())
    else {
        return false;
    };
    let mut stack = vec![usb_device.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("input") {
                if let Ok(label) = std::fs::read_to_string(path.join("name")) {
                    let label = label.to_lowercase();
                    if label.contains("keyboard") || label.contains("mouse") {
                        return true;
                    }
                }
            }
            if name.starts_with("input") || name.contains(':') {
                stack.push(path);
            }
        }
    }
    false
}

fn first_hidraw(syspath: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(syspath.join("hidraw")).ok()?;
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("hidraw"))
        .collect();
    names.sort();
    names.first().map(|name| PathBuf::from("/dev").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vendor_defined_page_is_recognised() {
        // The Puck's gamepad interface, verbatim from this machine.
        let descriptor = [0x06, 0x00, 0xff, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x42];
        assert!(is_vendor_interface(&descriptor));
    }

    #[test]
    fn a_generic_desktop_page_is_not_a_vendor_interface() {
        // The Puck's four lizard-mode interfaces: Generic Desktop / Mouse.
        let descriptor = [0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x40];
        assert!(!is_vendor_interface(&descriptor));
    }

    #[test]
    fn a_short_or_empty_descriptor_is_not_mistaken_for_one() {
        // sysfs hands back an empty read for a node that has gone.
        assert!(!is_vendor_interface(&[]));
        assert!(!is_vendor_interface(&[0x06]));
        assert!(!is_vendor_interface(&[0x06, 0x00]));
    }

    #[test]
    fn the_whole_vendor_page_range_counts_not_just_ff00() {
        // 0xFF00..0xFFFF are all vendor-defined.
        assert!(is_vendor_interface(&[0x06, 0x00, 0xff]));
        assert!(is_vendor_interface(&[0x06, 0xa0, 0xff]));
        assert!(!is_vendor_interface(&[0x06, 0x00, 0xfe]));
    }

    #[test]
    fn the_remedy_names_the_driver_and_the_exact_id() {
        let puck = Dormant {
            name: "Valve Software Steam Controller Puck".to_owned(),
            vid: 0x28DE,
            pid: 0x1304,
            driver: "hid-generic".to_owned(),
            hidraw: None,
        };
        let remedy = puck.remedy().expect("Valve has a kernel driver");
        assert!(remedy.contains("modprobe hid-steam"));
        // Uppercase hex and the bus, because that is the format new_id parses.
        assert!(remedy.contains("0003 28DE 1304"), "{remedy}");
        assert!(remedy.contains("/sys/bus/hid/drivers/steam/new_id"));
    }

    #[test]
    fn a_vendor_with_no_kernel_driver_gets_no_remedy_invented_for_it() {
        // Naming a driver that does not exist sends someone to a dead end.
        let other = Dormant {
            name: "Some Other Pad".to_owned(),
            vid: 0x1234,
            pid: 0x5678,
            driver: "hid-generic".to_owned(),
            hidraw: None,
        };
        assert_eq!(other.kernel_driver(), None);
        assert_eq!(other.remedy(), None);
    }

    #[test]
    fn only_devices_with_a_remedy_are_reported() {
        // The shape alone matches a Logitech Unifying receiver and an RGB
        // lighting controller, both of which were on the machine this was
        // written on. A warning nobody can act on is noise.
        for device in dormant() {
            assert!(
                device.kernel_driver().is_some(),
                "{} ({:04x}:{:04x}) was reported with no remedy to offer",
                device.name,
                device.vid,
                device.pid
            );
        }
    }

    #[test]
    fn every_known_family_can_produce_a_remedy() {
        for (vid, driver) in KNOWN_FAMILIES {
            let device = Dormant {
                name: String::new(),
                vid,
                pid: 0x9999,
                driver: "hid-generic".to_owned(),
                hidraw: None,
            };
            let remedy = device.remedy().expect("a known family has a remedy");
            assert!(remedy.contains(driver), "{remedy}");
        }
    }

    #[test]
    fn scanning_a_real_machine_does_not_fail() {
        // Whatever is plugged in, this must answer rather than raise: it runs
        // inside `list`, which has to work on a machine with no controllers.
        let _ = dormant();
    }
}
