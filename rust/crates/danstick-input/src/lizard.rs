//! Controllers the kernel has but not as controllers.

use std::path::{Path, PathBuf};

/// A controller not being driven as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dormant {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub driver: String,
    pub channels: Vec<PathBuf>,
}

/// Kernel driver support for a late-arriving model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LateModel {
    pub vid: u16,
    pub pid: u16,
    pub model: &'static str,
    pub receiver: bool,
    pub module: &'static str,
    pub since: (u32, u32),
}

const LATE_MODELS: &[LateModel] = &[
    LateModel {
        vid: 0x28DE,
        pid: 0x1302,
        model: "a 2026 Steam Controller, wired",
        receiver: false,
        module: "hid-steam",
        since: (7, 3),
    },
    LateModel {
        vid: 0x28DE,
        pid: 0x1303,
        model: "a 2026 Steam Controller over Bluetooth",
        receiver: false,
        module: "hid-steam",
        since: (7, 3),
    },
    LateModel {
        vid: 0x28DE,
        pid: 0x1304,
        model: "a four-slot wireless receiver for 2026 Steam Controllers",
        receiver: true,
        module: "hid-steam",
        since: (7, 3),
    },
    LateModel {
        vid: 0x28DE,
        pid: 0x1305,
        model: "a Steam Machine's built-in receiver for 2026 Steam Controllers",
        receiver: true,
        module: "hid-steam",
        since: (7, 3),
    },
];

impl Dormant {
    /// The table entry for this model, if there is one.
    pub fn late_model(&self) -> Option<&'static LateModel> {
        LATE_MODELS
            .iter()
            .find(|entry| (entry.vid, entry.pid) == (self.vid, self.pid))
    }
}

impl LateModel {
    /// Is `running` new enough that `module` knows this id?
    pub fn supported_by(&self, running: (u32, u32)) -> bool {
        running >= self.since
    }

    pub fn remedy(&self, running: (u32, u32)) -> String {
        if self.supported_by(running) {
            return format!(
                "This kernel ({}.{}) has a hid-steam that knows this model, so \
                 it only\nneeds loading:\n  sudo modprobe {}",
                running.0, running.1, self.module
            );
        }
        let mut out = String::new();
        out.push_str(&format!(
            "{} learned {:04X}:{:04X} in Linux {}.{}; this kernel is {}.{}.\n",
            self.module, self.vid, self.pid, self.since.0, self.since.1, running.0, running.1
        ));
        out.push_str("danstick drives it itself over hidraw -- see triton.py and\n");
        out.push_str("docs/STEAM-CONTROLLER.md -- so this is a note, not a fault.\n");
        out.push_str("A newer kernel would hand it to hid-steam instead.\n\n");
        out.push_str("Force-binding the running driver does not work, and this\n");
        out.push_str("used to say that and an upgrade were the only options.\n");
        out.push_str("Writing the id to\n");
        out.push_str(&format!("  /sys/bus/hid/drivers/{}/new_id\n", self.module));
        out.push_str("does bind it, but before that release steam_raw_event opens\n");
        out.push_str("with\n");
        out.push_str("  if (size != 64 || data[0] != 1 || data[1] != 0)\n");
        out.push_str("          return 0;\n");
        out.push_str("and every report this model sends is 54 bytes or fewer, under\n");
        out.push_str("its own report id. They are all dropped, in silence: the\n");
        out.push_str("driver would be attached and the pad still dead.");
        out
    }
}

/// Where the kernel publishes its own version.
const OSRELEASE: &str = "/proc/sys/kernel/osrelease";

/// The running kernel as `(major, minor)`.
pub fn running_kernel() -> Option<(u32, u32)> {
    parse_kernel(&std::fs::read_to_string(OSRELEASE).ok()?)
}

/// Parse kernel version: take only major and minor, not patch.
fn parse_kernel(release: &str) -> Option<(u32, u32)> {
    let mut parts = release.trim().split(['.', '-', '+']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Top-level application collections as extended usage: `(page << 16) | usage`.
pub fn application_collections(descriptor: &[u8]) -> Vec<u32> {
    let mut found = Vec::new();
    let mut page: u32 = 0;
    let mut usage: Option<u32> = None;
    let mut depth = 0usize;
    let mut at = 0usize;

    while at < descriptor.len() {
        let prefix = descriptor[at];
        at += 1;

        // Long item (0xFE) carries its own length in the next byte.
        if prefix == 0xFE {
            let Some(&size) = descriptor.get(at) else {
                break;
            };
            at = at.saturating_add(2 + size as usize);
            continue;
        }

        let size = match prefix & 0x03 {
            3 => 4,
            other => other as usize,
        };
        if at + size > descriptor.len() {
            break;
        }
        let mut data: u32 = 0;
        for (shift, byte) in descriptor[at..at + size].iter().enumerate() {
            data |= (*byte as u32) << (8 * shift);
        }
        at += size;

        match prefix & 0xFC {
            0x04 => page = data & 0xFFFF, // Usage Page
            0x08 => {
                // Usage: 4-byte form carries its own page.
                usage = Some(if size == 4 {
                    data
                } else {
                    (page << 16) | (data & 0xFFFF)
                })
            }
            0xA0 => {
                // Collection; 0x01 is Application.
                if depth == 0 && data == 0x01 {
                    if let Some(found_usage) = usage {
                        found.push(found_usage);
                    }
                }
                depth += 1;
                usage = None;
            }
            0xC0 => {
                depth = depth.saturating_sub(1);
                usage = None;
            }
            0x80 | 0x90 | 0xB0 => usage = None, // Other main items clear Usage
            _ => {}
        }
    }
    found
}

/// Mainline ignores this collection by name: the puck's pogo-pin dock.
const POGO_USAGE: u32 = 0xFF00_0002;

/// Is this extended usage a vendor-defined page?
fn is_vendor_usage(usage: u32) -> bool {
    (usage >> 16) >= 0xFF00
}

/// Check if interface carries vendor protocol (true for slot, not dock).
pub fn is_vendor_interface(descriptor: &[u8]) -> bool {
    let collections = application_collections(descriptor);
    if collections.first() == Some(&POGO_USAGE) {
        return false;
    }
    collections.iter().copied().any(is_vendor_usage)
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
        if driver != "hid-generic" {
            continue;
        }
        let Some((vid, pid)) = hid_ids(&device) else {
            continue;
        };
        if has_joypad(&syspath) {
            continue;
        }
        if !looks_like_lizard_mode(&syspath) {
            continue;
        }
        if !LATE_MODELS
            .iter()
            .any(|entry| (entry.vid, entry.pid) == (vid, pid))
        {
            continue;
        }

        let channel = first_hidraw(&syspath);
        if let Some(seen) = found
            .iter_mut()
            .find(|seen| (seen.vid, seen.pid) == (vid, pid))
        {
            seen.channels.extend(channel);
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
            channels: channel.into_iter().collect(),
        });
    }
    for device in &mut found {
        device.channels.sort_by_key(|path| hidraw_index(path));
    }
    found
}

/// Extract numeric index from `/dev/hidrawN` for numeric sorting.
fn hidraw_index(path: &Path) -> usize {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("hidraw"))
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(usize::MAX) // Unnumbered nodes sort last
}

fn hid_ids(device: &udev::Device) -> Option<(u16, u16)> {
    ids_from_hid_id(&device.property_value("HID_ID")?.to_string_lossy())
}

/// Parse HID_ID property: `0003:000028DE:00001304` -> (vid, pid).
pub fn ids_from_hid_id(raw: &str) -> Option<(u16, u16)> {
    let mut parts = raw.trim().split(':');
    let _bus = parts.next()?;
    let vid = u16::from_str_radix(parts.next()?.trim_start_matches('0'), 16).ok()?;
    let pid = u16::from_str_radix(parts.next()?.trim_start_matches('0'), 16).ok()?;
    Some((vid, pid))
}

/// Check if device or sibling offers joypad (asked of USB device, not interface).
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

/// Does device present as keyboard or mouse (lizard mode)?
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

    /// Interface 2 of a real Steam Controller Puck (28de:1304), captured from sysfs on kernel 6.18.44. One of the four wireless slots: an emulated mouse, an emulated keyboard, and the Valve protocol.
    const PUCK_SLOT: [u8; 372] = [
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x40, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19,
        0x01, 0x29, 0x02, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x02, 0x81, 0x02, 0x75, 0x06,
        0x95, 0x01, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75,
        0x08, 0x95, 0x02, 0x81, 0x06, 0x95, 0x01, 0x09, 0x38, 0x81, 0x06, 0x05, 0x0C, 0x0A, 0x38,
        0x02, 0x95, 0x01, 0x81, 0x06, 0xC0, 0xC0, 0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x41,
        0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81,
        0x02, 0x81, 0x01, 0x19, 0x00, 0x29, 0x65, 0x15, 0x00, 0x25, 0x65, 0x75, 0x08, 0x95, 0x06,
        0x81, 0x00, 0xC0, 0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x42, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x35, 0x09, 0x42, 0x81, 0x02, 0x85, 0x44, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x05, 0x09, 0x44, 0x81, 0x02, 0x85, 0x79, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x01, 0x09, 0x79, 0x81, 0x02, 0x85, 0x43, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x0E, 0x09, 0x43, 0x81, 0x02, 0x85, 0x7B, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x0C, 0x09, 0x7B, 0x81, 0x02, 0x85, 0x45, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x2D, 0x09, 0x45, 0x81, 0x02, 0x85, 0x80, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x09, 0x09, 0x80, 0x91, 0x02, 0x85, 0x81, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x07, 0x09, 0x81, 0x91, 0x02, 0x85, 0x82, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x03, 0x09, 0x82, 0x91, 0x02, 0x85, 0x83, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x09, 0x09, 0x83, 0x91, 0x02, 0x85, 0x84, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x08, 0x09, 0x84, 0x91, 0x02, 0x85, 0x85, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x03, 0x09, 0x85, 0x91, 0x02, 0x85, 0x86, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x03, 0x09, 0x86, 0x91, 0x02, 0x85, 0x87, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x3F, 0x09, 0x87, 0x91, 0x02, 0x85, 0x89, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x3F, 0x09, 0x89, 0x91, 0x02, 0x85, 0x88, 0x15, 0x00, 0x26,
        0xFF, 0x00, 0x75, 0x08, 0x95, 0x3F, 0x09, 0x88, 0x91, 0x02, 0x85, 0x01, 0x95, 0x3F, 0x09,
        0x01, 0xB1, 0x02, 0x85, 0x02, 0x95, 0x3F, 0x09, 0x01, 0xB1, 0x02, 0xC0,
    ];

    /// Interface 6 of the same device: the pogo-pin dock.
    const PUCK_POGO: [u8; 54] = [
        0x06, 0x00, 0xFF, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x42, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75,
        0x08, 0x95, 0x35, 0x09, 0x42, 0x81, 0x02, 0x85, 0x79, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75,
        0x08, 0x95, 0x01, 0x09, 0x79, 0x81, 0x02, 0x85, 0x01, 0x95, 0x3F, 0x09, 0x01, 0xB1, 0x02,
        0x85, 0x02, 0x95, 0x3F, 0x09, 0x01, 0xB1, 0x02, 0xC0,
    ];

    #[test]
    fn a_slot_interface_yields_all_three_of_its_collections() {
        assert_eq!(
            application_collections(&PUCK_SLOT),
            vec![0x0001_0002, 0x0001_0006, 0xFF00_0001]
        );
    }

    #[test]
    fn the_dock_yields_only_its_own_collection() {
        assert_eq!(application_collections(&PUCK_POGO), vec![POGO_USAGE]);
    }

    #[test]
    fn a_slot_is_a_vendor_interface_and_the_dock_is_not() {
        assert!(is_vendor_interface(&PUCK_SLOT));
        assert!(!is_vendor_interface(&PUCK_POGO));
    }

    #[test]
    fn a_plain_mouse_is_not_a_vendor_interface() {
        let mouse = [
            0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01,
            0x29, 0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0xC0, 0xC0,
        ];
        assert_eq!(application_collections(&mouse), vec![0x0001_0002]);
        assert!(!is_vendor_interface(&mouse));
    }

    #[test]
    fn a_nested_collection_is_not_mistaken_for_a_top_level_one() {
        let nested = [
            0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, // Application, usage 00010002
            0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x00, // Physical, vendor usage
            0xC0, 0xC0,
        ];
        assert_eq!(application_collections(&nested), vec![0x0001_0002]);
        assert!(!is_vendor_interface(&nested));
    }

    #[test]
    fn a_four_byte_usage_carries_its_own_page() {
        let extended = [
            0x05, 0x01, // Usage Page (Generic Desktop)
            0x0B, 0x01, 0x00, 0x00, 0xFF, // Usage (0xFF000001)
            0xA1, 0x01, 0xC0,
        ];
        assert_eq!(application_collections(&extended), vec![0xFF00_0001]);
    }

    #[test]
    fn a_truncated_descriptor_stops_rather_than_reading_past_the_end() {
        for cut in 0..PUCK_SLOT.len() {
            let _ = application_collections(&PUCK_SLOT[..cut]);
        }
        // An item promising four bytes with one left.
        assert!(application_collections(&[0x07, 0x01]).is_empty());
        assert!(application_collections(&[]).is_empty());
    }

    #[test]
    fn a_long_item_is_stepped_over_not_misread() {
        let with_long = [
            0xFE, 0x02, 0x00, 0xA1, 0x01, // long item, 2 bytes of payload
            0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0xC0,
        ];
        assert_eq!(application_collections(&with_long), vec![0x0001_0005]);
    }

    #[test]
    fn unbalanced_end_collections_do_not_underflow() {
        let extra_ends = [0xC0, 0xC0, 0xC0, 0x05, 0x01, 0x09, 0x05, 0xA1, 0x01];
        assert_eq!(application_collections(&extra_ends), vec![0x0001_0005]);
    }

    #[test]
    fn the_puck_is_named_as_a_receiver_not_as_a_controller() {
        let puck = LATE_MODELS
            .iter()
            .find(|entry| entry.pid == 0x1304)
            .expect("the puck is in the table");
        assert!(puck.receiver, "{}", puck.model);
    }

    #[test]
    fn a_kernel_older_than_the_driver_is_told_to_upgrade() {
        let puck = LATE_MODELS
            .iter()
            .find(|e| e.pid == 0x1304)
            .expect("the puck is in the table");
        assert!(!puck.supported_by((6, 18)));
        let remedy = puck.remedy((6, 18));
        assert!(remedy.contains("Linux 7.3"), "{remedy}");
        assert!(remedy.contains("this kernel is 6.18"), "{remedy}");
        assert!(remedy.contains("does not work"), "{remedy}");
        assert!(!remedy.contains("| sudo tee"), "{remedy}");
    }

    #[test]
    fn a_kernel_new_enough_is_told_to_load_the_module_instead() {
        let puck = LATE_MODELS
            .iter()
            .find(|e| e.pid == 0x1304)
            .expect("the puck is in the table");
        assert!(puck.supported_by((7, 3)));
        let remedy = puck.remedy((7, 3));
        assert!(remedy.contains("modprobe hid-steam"), "{remedy}");
        assert!(!remedy.contains("Upgrade the kernel"), "{remedy}");
    }

    #[test]
    fn support_is_decided_by_the_series_not_the_patch_release() {
        let puck = LATE_MODELS
            .iter()
            .find(|e| e.pid == 0x1304)
            .expect("the puck is in the table");
        assert!(!puck.supported_by((7, 2)));
        assert!(puck.supported_by((7, 3)));
        assert!(puck.supported_by((8, 0)));
    }

    #[test]
    fn kernel_releases_parse_in_the_shapes_they_actually_come_in() {
        assert_eq!(parse_kernel("6.18.44"), Some((6, 18)));
        assert_eq!(parse_kernel("6.18.44\n"), Some((6, 18)));
        assert_eq!(parse_kernel("7.3.0-rc2"), Some((7, 3)));
        assert_eq!(parse_kernel("6.18.44-zen1"), Some((6, 18)));
        assert_eq!(parse_kernel("7.3-rc2"), Some((7, 3)));
        assert_eq!(parse_kernel("6.18.44+"), Some((6, 18)));
        assert_eq!(parse_kernel(""), None);
        assert_eq!(parse_kernel("banana"), None);
        assert_eq!(parse_kernel("6"), None);
    }

    #[test]
    fn this_machine_reports_a_kernel_version() {
        assert!(running_kernel().is_some(), "/proc/sys/kernel/osrelease");
    }

    #[test]
    fn only_devices_with_a_remedy_are_reported() {
        for device in dormant() {
            assert!(
                device.late_model().is_some(),
                "{} ({:04x}:{:04x}) was reported with no remedy to offer",
                device.name,
                device.vid,
                device.pid
            );
        }
    }

    #[test]
    fn a_device_outside_the_table_has_no_remedy_invented_for_it() {
        let other = Dormant {
            name: "Some Other Pad".to_owned(),
            vid: 0x1234,
            pid: 0x5678,
            driver: "hid-generic".to_owned(),
            channels: Vec::new(),
        };
        assert_eq!(other.late_model(), None);
    }

    #[test]
    fn no_dock_interface_is_ever_offered_as_a_channel() {
        for device in dormant() {
            for channel in &device.channels {
                assert!(channel.starts_with("/dev/"), "{}", channel.display());
            }
        }
    }

    #[test]
    fn no_remedy_line_overruns_a_terminal() {
        for model in LATE_MODELS {
            for running in [(6, 18), (7, 3)] {
                for line in model.remedy(running).lines() {
                    assert!(
                        line.chars().count() <= 78,
                        "{} chars: {line:?}",
                        line.chars().count()
                    );
                }
            }
        }
    }

    #[test]
    fn slots_are_ordered_by_number_not_by_spelling() {
        let mut nodes: Vec<PathBuf> = ["hidraw10", "hidraw7", "hidraw9", "hidraw8"]
            .iter()
            .map(|name| PathBuf::from("/dev").join(name))
            .collect();
        nodes.sort_by_key(|path| hidraw_index(path));
        let shown: Vec<String> = nodes
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        assert_eq!(
            shown,
            [
                "/dev/hidraw7",
                "/dev/hidraw8",
                "/dev/hidraw9",
                "/dev/hidraw10"
            ]
        );
    }

    #[test]
    fn a_node_that_is_not_numbered_sorts_last_rather_than_panicking() {
        let mut nodes: Vec<PathBuf> = ["hidrawX", "hidraw7", "/dev"]
            .iter()
            .map(PathBuf::from)
            .collect();
        nodes.sort_by_key(|path| hidraw_index(path));
        assert!(nodes[0].ends_with("hidraw7"), "{nodes:?}");
    }

    #[test]
    fn scanning_a_real_machine_does_not_fail() {
        let _ = dormant();
    }
}
