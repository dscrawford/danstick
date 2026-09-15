//! Controllers the kernel has, but not as controllers.
//!
//! Some pads power up pretending to be a mouse and a keyboard -- Valve calls it
//! lizard mode -- and put their real gamepad state on a *vendor-defined* HID
//! collection that only a driver written for that vendor can read. Until
//! something sends the command that leaves lizard mode, the machine has a
//! keyboard where a controller should be.
//!
//! The kernel ships `hid-steam` to do exactly that, and it matches on product
//! id. A model newer than the running kernel's table falls through to
//! `hid-generic`, which faithfully exposes the mouse and the keyboard and
//! cannot know that another collection on the same interface is a gamepad.
//! Nothing reports an error: the device enumerates perfectly, and simply is not
//! a controller.
//!
//! Measured on this machine, kernel 6.18.44, with a Steam Controller Puck:
//!
//! ```text
//! 28de:1304, 7 USB interfaces
//!   0-1  CDC-ACM, internal comms, not HID
//!   2-5  four wireless slots  -> hidraw7..10
//!        05 01 09 02  mouse, report 0x40      |
//!        05 01 09 06  keyboard, report 0x41   | lizard mode
//!        06 00 ff 09 01  usage FF000001, reports 0x42/0x43/0x44/0x45/0x79/0x7b
//!   6    pogo-pin dock     -> hidraw11
//!        06 00 ff 09 02  usage FF000002, 54 bytes, stripped down
//! ```
//!
//! Two things about that layout cost this module a rewrite.
//!
//! The vendor collection is the *third* top-level collection on a slot
//! interface, not the first. Testing the first three bytes of the descriptor --
//! which is what this did -- matches only the pogo interface, whose descriptor
//! does start with the vendor page. So padmap reported the dock as "the gamepad
//! channel" and probed it for input that was never going to arrive there. The
//! descriptor is now walked as HID items, and every top-level application
//! collection is read.
//!
//! And the puck is a *receiver*, not a controller: four slots for four
//! controllers, plus the dock. Mainline distinguishes the dock exactly as this
//! does now, by its collection usage:
//!
//! ```text
//! /* The puck's pogo pin interface should be ignored as it's stripped
//!  * down. It has one collection with usage page FF00 with usage ID 2. */
//! return hdev->collection[0].usage != 0xFF000002;
//! ```
//!
//! The reporting is deliberately narrow. The shape "vendor collection, sibling
//! keyboard and mouse, no joypad" is also, exactly, a wireless
//! keyboard-and-mouse receiver: a Logitech Unifying receiver on this machine
//! matches every part of it, and so does an RGB lighting controller if the
//! keyboard test is left out. There is no way to separate them from outside, so
//! this does not try -- it reports only the exact models in [`LATE_MODELS`],
//! where the remedy is known. A warning about a device nobody can help with is
//! not a diagnosis; it is noise that teaches the user to ignore the one that
//! matters.

use std::path::{Path, PathBuf};

/// A controller the kernel is not treating as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dormant {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    /// The driver actually bound, usually `hid-generic`.
    pub driver: String,
    /// hidraw nodes carrying the vendor protocol, dock interfaces excluded.
    pub channels: Vec<PathBuf>,
}

/// A model whose kernel driver exists, but arrived after some kernels shipped.
///
/// Keyed on the exact product id rather than the vendor. Naming the model is
/// most of the diagnosis -- "this is a four-slot receiver" is the difference
/// between looking for one controller and looking for four -- and a vendor-wide
/// rule cannot say it. The cost is that a model newer than this table is
/// silent, which is the same trade the module note describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LateModel {
    pub vid: u16,
    pub pid: u16,
    /// What it is, in the words the diagnosis needs.
    pub model: &'static str,
    /// Is this a receiver rather than a controller?
    ///
    /// Worth a field rather than a word in `model`, because it changes what
    /// the reader should expect to happen: a receiver never becomes one pad.
    /// Once it is driven, up to four controllers appear *through* it, and
    /// none may be connected yet. Waiting for a single pad is the wrong thing
    /// to wait for, and nothing else on the machine says so.
    pub receiver: bool,
    pub module: &'static str,
    /// First kernel release whose `module` claims this id.
    pub since: (u32, u32),
}

/// Verified against `drivers/hid/hid-ids.h` and `hid-steam.c` in mainline, and
/// against the v6.18 sources, which contain no reference to either codename.
///
/// The series is "HID: steam: Add 2026 Steam Controller support"; `hid-ids.h`
/// gained IBEX/IBEX_BLE/PROTEUS/NEREID in v7.3-rc1 and has them in no earlier
/// tag -- checked at v7.0, v7.1 and v7.2, all absent.
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

    /// What to do about it, given the kernel actually running.
    ///
    /// Two different answers, because there are two different causes. Once the
    /// kernel is new enough the driver simply needs loading; before that, no
    /// amount of loading helps and the kernel itself is the thing to change.
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
        out.push_str("padmap drives it itself over hidraw -- see triton.py and\n");
        out.push_str("docs/STEAM-CONTROLLER.md -- so this is a note, not a fault.\n");
        out.push_str("A newer kernel would hand it to hid-steam instead.\n\n");
        // Spelled out because padmap gave the opposite advice once, and an
        // instruction that was wrong is not retracted by quietly dropping it.
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

/// `"6.18.44"`, `"7.3.0-rc2"`, `"6.18.44-zen1"` -> `(major, minor)`.
///
/// Only the first two fields, because that is what a driver's id table tracks:
/// support arrives in a merge window, and every patch release of that series
/// has it.
fn parse_kernel(release: &str) -> Option<(u32, u32)> {
    let mut parts = release.trim().split(['.', '-', '+']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Every top-level application collection, as an extended usage.
///
/// `(page << 16) | usage`, which is how the kernel stores `collection[].usage`
/// and therefore how its own checks are written -- `0xFF000002` for the puck's
/// dock. Walking the items is the only way to see past the first collection,
/// and the first collection is the one that is misleading here: a slot
/// interface opens with an emulated mouse and keeps the real protocol third.
pub fn application_collections(descriptor: &[u8]) -> Vec<u32> {
    let mut found = Vec::new();
    let mut page: u32 = 0;
    let mut usage: Option<u32> = None;
    let mut depth = 0usize;
    let mut at = 0usize;

    while at < descriptor.len() {
        let prefix = descriptor[at];
        at += 1;

        // A long item carries its own length; nothing defines one, but a
        // descriptor holding one must still be walked past rather than
        // misread as the short items that follow.
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
            // Global, tag 0: Usage Page.
            0x04 => page = data & 0xFFFF,
            // Local, tag 0: Usage. Four bytes is an extended usage, carrying
            // its own page, and must not be combined with the global one.
            0x08 => {
                usage = Some(if size == 4 {
                    data
                } else {
                    (page << 16) | (data & 0xFFFF)
                })
            }
            // Main, tag 10: Collection. Data 0x01 is Application.
            0xA0 => {
                if depth == 0 && data == 0x01 {
                    if let Some(found_usage) = usage {
                        found.push(found_usage);
                    }
                }
                depth += 1;
                usage = None;
            }
            // Main, tag 12: End Collection.
            0xC0 => {
                depth = depth.saturating_sub(1);
                usage = None;
            }
            // Any other main item closes the local scope, Usage included.
            0x80 | 0x90 | 0xB0 => usage = None,
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

/// Does this interface carry a vendor protocol worth reading?
///
/// True for a slot, false for the dock. The dock is a real vendor interface and
/// is deliberately excluded anyway, because it is not where controller input
/// arrives -- pointing a capture tool at it finds nothing, correctly, and looks
/// exactly like a controller that is asleep.
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
        // Only where there is something to do about it. See the module note.
        if !LATE_MODELS
            .iter()
            .any(|entry| (entry.vid, entry.pid) == (vid, pid))
        {
            continue;
        }

        let channel = first_hidraw(&syspath);
        // One device, several interfaces. The puck has four slots, and
        // reporting it four times would read as four broken controllers
        // instead of one receiver -- so they are folded together and the
        // channels accumulate.
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

/// `/dev/hidraw10` -> 10, for ordering.
///
/// Sorting these as strings puts hidraw10 before hidraw7, which reads as a
/// jumbled list of a device's slots and invites the reader to wonder what the
/// order means. It means nothing; it should at least look like it.
fn hidraw_index(path: &Path) -> usize {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("hidraw"))
        .and_then(|digits| digits.parse().ok())
        // Anything unnumbered sorts last rather than being dropped or
        // panicking. The sort is stable, so such nodes keep their order.
        .unwrap_or(usize::MAX)
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

    /// Interface 2 of a real Steam Controller Puck (28de:1304), captured
    /// from sysfs on kernel 6.18.44. One of the four wireless slots: an
    /// emulated mouse, an emulated keyboard, and the Valve protocol.
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

    /// Interface 6 of the same device: the pogo-pin dock. Same vendor page,
    /// usage 2 rather than 1, and stripped down to two input reports.
    const PUCK_POGO: [u8; 54] = [
        0x06, 0x00, 0xFF, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x42, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75,
        0x08, 0x95, 0x35, 0x09, 0x42, 0x81, 0x02, 0x85, 0x79, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75,
        0x08, 0x95, 0x01, 0x09, 0x79, 0x81, 0x02, 0x85, 0x01, 0x95, 0x3F, 0x09, 0x01, 0xB1, 0x02,
        0x85, 0x02, 0x95, 0x3F, 0x09, 0x01, 0xB1, 0x02, 0xC0,
    ];

    #[test]
    fn a_slot_interface_yields_all_three_of_its_collections() {
        // Mouse, keyboard, then the Valve protocol. The third one is the whole
        // point: reading only the first says "this is a mouse".
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
        // The bug this replaces got both of these backwards: it tested the
        // first three bytes, so the slot (which opens 05 01, a mouse) failed
        // and the dock (which opens 06 00 ff) passed. padmap then told the
        // user the dock was the gamepad channel.
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
        // The mouse above has a Physical collection inside the Application
        // one, carrying its own Usage. Only the outer collection counts.
        let nested = [
            0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, // Application, usage 00010002
            0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x00, // Physical, vendor usage
            0xC0, 0xC0,
        ];
        assert_eq!(application_collections(&nested), vec![0x0001_0002]);
        // ...so a vendor usage that is only ever nested does not qualify.
        assert!(!is_vendor_interface(&nested));
    }

    #[test]
    fn a_four_byte_usage_carries_its_own_page() {
        // Extended usage: 0x0B is Usage with size 4, and the page in the high
        // half must win over the global Usage Page rather than being or-ed
        // into it.
        let extended = [
            0x05, 0x01, // Usage Page (Generic Desktop)
            0x0B, 0x01, 0x00, 0x00, 0xFF, // Usage (0xFF000001)
            0xA1, 0x01, 0xC0,
        ];
        assert_eq!(application_collections(&extended), vec![0xFF00_0001]);
    }

    #[test]
    fn a_truncated_descriptor_stops_rather_than_reading_past_the_end() {
        // Sysfs hands this over as a file; a short read is a shape that
        // arrives, and indexing past it would panic in a `list`.
        for cut in 0..PUCK_SLOT.len() {
            let _ = application_collections(&PUCK_SLOT[..cut]);
        }
        // An item promising four bytes with one left.
        assert!(application_collections(&[0x07, 0x01]).is_empty());
        assert!(application_collections(&[]).is_empty());
    }

    #[test]
    fn a_long_item_is_stepped_over_not_misread() {
        // 0xFE is the long-item prefix: the next byte is the payload length.
        // Misreading it as a short item would desynchronise the walk and can
        // invent a collection out of payload bytes.
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
        // Waiting for one pad to appear is the wrong thing to wait for, and
        // the diagnosis has to say so.
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
        // The advice this replaces. It must not come back, and the text has
        // to say why, because it was given once already.
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
        // The dock is filtered before a Dormant is built, so a reported
        // device's channels are all slots. Pointing hidprobe at the dock
        // finds nothing and reads like a controller that is asleep.
        for device in dormant() {
            for channel in &device.channels {
                assert!(channel.starts_with("/dev/"), "{}", channel.display());
            }
        }
    }

    #[test]
    fn no_remedy_line_overruns_a_terminal() {
        // These are read on a machine plugged into a television, where a
        // wrapped line is a wrapped line and there is no scrollback worth
        // the name.
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
