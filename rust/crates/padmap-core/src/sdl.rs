//! SDL's side of a mapping: the GUID it keys its database on, and the line it
//! stores under that key.
//!
//! SDL never reports a mapping it failed to match. A GUID computed one way and
//! a device created another simply does nothing, and says nothing, so both the
//! checksum and the field order are pinned in tests against a line SDL itself
//! wrote.

use std::collections::BTreeMap;

use crate::binding::{axis_index, Binding};
use crate::control::{Control, CANONICAL_ORDER};
use crate::fields::Fields;

/// SDL's CRC-16, needed to reproduce a joystick GUID.
///
/// SDL 2.26 onwards stores a checksum of the device name in bytes 2-3 of the
/// GUID, so a mapping written for the wrong checksum is never matched. This is
/// CRC-16/ARC -- reflected, polynomial 0xA001, zero initial value.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for byte in data {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// The GUID SDL will compute for a device, as it appears in its database.
///
/// Sixteen little-endian 16-bit fields with padding between them; the name
/// checksum sits second. padmap creates the virtual pads, so it knows every
/// input here and can write a mapping before SDL has ever seen the device.
pub fn guid(bus: u16, vendor: u16, product: u16, version: u16, name: &str) -> String {
    let words = [
        bus,
        crc16(name.as_bytes()),
        vendor,
        0,
        product,
        0,
        version,
        0,
    ];
    let mut out = String::with_capacity(32);
    for word in words {
        out.push_str(&format!("{:02x}{:02x}", word & 0xFF, (word >> 8) & 0xFF));
    }
    out
}

/// evdev ABS code -> the SDL stick field it feeds.
///
/// Sticks are not captured: the wizard asks about buttons, and asking someone
/// to "press left stick X" is both awkward and unnecessary, since a stick is
/// already unambiguous from the device's own axes. They still have to be in the
/// mapping -- without leftx/lefty SDL reports no stick at all and a front-end
/// loses every form of navigation except the d-pad.
pub const STICK_AXES: [(u16, &str); 4] = [
    (0x00, "leftx"),  // ABS_X
    (0x01, "lefty"),  // ABS_Y
    (0x03, "rightx"), // ABS_RX
    (0x04, "righty"), // ABS_RY
];

/// One axis's declared travel plus where it sits untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisSpan {
    pub minimum: i32,
    pub maximum: i32,
    pub rest: i32,
}

impl AxisSpan {
    pub const fn new(minimum: i32, maximum: i32, rest: i32) -> Self {
        AxisSpan {
            minimum,
            maximum,
            rest,
        }
    }

    /// Whether the axis sits near the middle of its travel when untouched.
    ///
    /// A real stick centres and a trigger does not, which is the difference the
    /// evdev code number cannot express. The Mayflash GameCube adapter reports
    /// its analogue triggers as ABS_RX and ABS_RY, so trusting the code told
    /// SDL the right stick was jammed 80% to the upper-left and held there --
    /// reported as the pad being "stuck to the left".
    pub fn rests_centred(&self) -> bool {
        if self.maximum <= self.minimum {
            return false;
        }
        let half = f64::from(self.maximum - self.minimum) / 2.0;
        let middle = f64::from(self.minimum + self.maximum) / 2.0;
        let offset = (f64::from(self.rest) - middle) / half;
        offset.abs() <= STICK_REST_TOLERANCE
    }
}

/// How far from the middle of its range an axis may rest and still be called a
/// stick, as a fraction of half that range. A real stick centres within a few
/// percent; the pads measured here sit inside 4%. Anything near an end is a
/// trigger, and this is a wide berth around that distinction.
pub const STICK_REST_TOLERANCE: f64 = 0.5;

/// SDL stick entries for the axes a pad actually reports.
///
/// Two axes are refused: one that does not rest near the middle of its range
/// (a trigger, see [`AxisSpan::rests_centred`]), and one a capture already
/// claims -- nothing good comes of an axis being both a stick and a button, as
/// the front-end believes whichever it reads first.
///
/// Without `axes` the old guess by evdev code stands, since a caller with no
/// absinfo is no worse off than before.
pub fn stick_fields(
    axis_codes: &[u16],
    bindings: &BTreeMap<Control, Binding>,
    axes: Option<&BTreeMap<u16, AxisSpan>>,
) -> Fields {
    let taken: Vec<i32> = bindings
        .values()
        .filter(|binding| binding.kind == crate::binding::BindingKind::Axis)
        .map(|binding| binding.index)
        .collect();

    let mut fields = Fields::new();
    for (code, field) in STICK_AXES {
        if !axis_codes.contains(&code) {
            continue;
        }
        let Some(index) = axis_index(axis_codes, code) else {
            continue;
        };
        if taken.contains(&index) {
            continue;
        }
        if let Some(span) = axes.and_then(|axes| axes.get(&code)) {
            if !span.rests_centred() {
                continue;
            }
        }
        fields.insert(field, format!("a{index}"));
    }
    fields
}

/// Characters a device name may not carry into a database line.
///
/// The comma is the field separator. The rest all end a line for one reader or
/// the other: SDL splits the file on newline, and padmap's own rewriter uses a
/// line split that additionally breaks on \v, \f, \x1c-\x1e, NEL and the
/// Unicode line/paragraph separators. A name is a USB string descriptor written
/// by somebody else, so none of these is hypothetical.
const NAME_FORBIDDEN: [char; 11] = [
    ',', '\n', '\r', '\x0b', '\x0c', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}',
];

/// A device name with everything that could break the framing removed.
pub fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !NAME_FORBIDDEN.contains(c))
        .collect()
}

/// One line for SDL's controller database, from plain field:target pairs.
///
/// Separate from [`mapping_line`] because not every line padmap writes comes
/// from a capture: one carried over from another database has fields padmap
/// never asks about (`guide`, `leftstick`), and dropping them on the way
/// through would quietly cost the user bindings they already had.
pub fn line(guid: &str, name: &str, fields: &Fields, platform: &str) -> String {
    let mut parts: Vec<String> = vec![guid.to_owned(), clean_name(name)];
    for (field, target) in fields.iter() {
        parts.push(format!("{field}:{target}"));
    }
    parts.push(format!("platform:{platform}"));
    parts.join(",") + ","
}

/// A database line split into guid, name and fields, or `None` if it is not one.
///
/// Tolerant on purpose: this reads files the user and other programs write,
/// where a comment, a blank line or a trailing comma are all normal.
pub fn parse_line(raw: &str) -> Option<(String, String, Fields)> {
    let stripped = raw.trim();
    if stripped.is_empty() || stripped.starts_with('#') {
        return None;
    }
    let parts: Vec<&str> = stripped.split(',').collect();
    if parts.len() < 2 || parts[0].len() != 32 {
        return None;
    }
    let mut fields = Fields::new();
    for part in &parts[2..] {
        if let Some((field, target)) = part.split_once(':') {
            let (field, target) = (field.trim(), target.trim());
            if !field.is_empty() && !target.is_empty() {
                fields.insert(field, target);
            }
        }
    }
    Some((parts[0].to_lowercase(), parts[1].to_owned(), fields))
}

/// One line for SDL's controller database, from a capture.
///
/// A binding SDL cannot express is left out rather than refused: the shape that
/// arrives here is a hat value that is not a single direction, off a profile
/// somebody edited or a write that was cut short, and failing halfway through
/// means the pad gets no line at all -- every control lost to save one. Left
/// out, the direction reads as unmapped and the wizard can be run again.
pub fn mapping_line(
    guid: &str,
    name: &str,
    bindings: &BTreeMap<Control, Binding>,
    platform: &str,
    sticks: Option<&Fields>,
) -> String {
    let mut fields = Fields::new();
    for control in CANONICAL_ORDER {
        if let Some(binding) = bindings.get(&control) {
            if let Ok(target) = binding.sdl() {
                fields.insert(control.sdl_field(), target);
            }
        }
    }
    if let Some(sticks) = sticks {
        fields.extend(sticks);
    }
    line(guid, name, &fields, platform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::Binding;

    // Written by SDL itself, via Pegasus's gamepad editor, for one of padmap's
    // virtual pads. bus 6 (BUS_VIRTUAL, since the pad is uinput), vendor
    // 0x0079, product 0x1879, version 1.
    const REAL_GUID: &str = "0600c9a7790000007918000001000000";
    const REAL_NAME: &str = "padmap Player 1";
    const BUS_VIRTUAL: u16 = 0x06;

    #[test]
    fn the_guid_matches_one_sdl_wrote_itself() {
        let computed = guid(BUS_VIRTUAL, 0x0079, 0x1879, 0x0001, REAL_NAME);
        assert_eq!(
            computed, REAL_GUID,
            "a mapping written under a wrong GUID is never matched, and SDL \
             does not complain"
        );
    }

    #[test]
    fn the_name_checksum_is_actually_doing_something() {
        let one = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, "padmap Player 1");
        let two = guid(BUS_VIRTUAL, 0x0079, 0x1879, 1, "padmap Player 2");
        assert_ne!(one, two);
        // ...and only in the checksum field, bytes 2-3.
        assert_eq!(&one[..4], &two[..4], "the bus moved");
        assert_eq!(
            &one[8..],
            &two[8..],
            "something other than the checksum moved"
        );
    }

    #[test]
    fn the_bus_is_the_first_field_and_changing_it_changes_the_guid() {
        // Measured against SDL: bus 3 with ids 0079:1830 matches its database
        // entry, bus 6 with the same ids matches nothing.
        let usb = guid(0x03, 0x0079, 0x1830, 0x0110, "Arcade Fightstick F300");
        let virt = guid(0x06, 0x0079, 0x1830, 0x0110, "Arcade Fightstick F300");
        assert_eq!(&usb[..4], "0300");
        assert_eq!(&virt[..4], "0600");
        assert_ne!(usb, virt);
    }

    #[test]
    fn a_guid_is_always_thirty_two_hex_characters() {
        for name in ["", "x", &"long".repeat(64)] {
            let computed = guid(0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, name);
            assert_eq!(computed.len(), 32);
            assert!(computed.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn crc16_is_the_arc_variant() {
        // The standard check vector for CRC-16/ARC.
        assert_eq!(crc16(b"123456789"), 0xBB3D);
        assert_eq!(crc16(b""), 0x0000);
    }

    #[test]
    fn a_comma_in_a_name_cannot_split_the_fields() {
        let built = line(REAL_GUID, "Evil, Pad", &Fields::new(), "Linux");
        assert_eq!(built.split(',').count() - 1, 3, "guid, name, platform");
        assert!(built.contains("Evil Pad"));
    }

    #[test]
    fn a_newline_in_a_name_cannot_produce_two_physical_lines() {
        // Worse than a comma: SDL reads the first line as a device with no
        // bindings and drops the second as junk, so the pad gets no mapping at
        // all rather than a damaged one.
        for bad in ['\n', '\r', '\x0b', '\x0c', '\u{85}', '\u{2028}', '\u{2029}'] {
            let name = format!("Pad{bad}Two");
            let built = line(REAL_GUID, &name, &Fields::new(), "Linux");
            assert_eq!(built.lines().count(), 1, "{bad:?} split the line");
            assert!(!built.contains(bad), "{bad:?} survived into the line");
        }
    }

    #[test]
    fn a_line_round_trips_through_the_parser() {
        let fields: Fields = [("a", "b1"), ("b", "b2")].into_iter().collect();
        let built = line(REAL_GUID, REAL_NAME, &fields, "Linux");
        let (parsed_guid, parsed_name, parsed_fields) =
            parse_line(&built).expect("the line we just built must parse");
        assert_eq!(parsed_guid, REAL_GUID);
        assert_eq!(parsed_name, REAL_NAME);
        assert_eq!(parsed_fields.get("a"), Some("b1"));
        assert_eq!(parsed_fields.get("platform"), Some("Linux"));
    }

    #[test]
    fn the_parser_tolerates_what_a_real_file_contains() {
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("   "), None);
        assert_eq!(parse_line("# a comment"), None);
        assert_eq!(parse_line("  # indented comment"), None);
        assert_eq!(parse_line("not,a,guid"), None, "the guid must be 32 chars");
        assert_eq!(
            parse_line(REAL_GUID),
            None,
            "a guid with no name is not a line"
        );
    }

    #[test]
    fn the_parser_lowercases_the_guid_so_two_spellings_are_one_key() {
        let upper = REAL_GUID.to_uppercase();
        let (parsed, _, _) = parse_line(&format!("{upper},Pad,a:b0,")).expect("parse");
        assert_eq!(parsed, REAL_GUID);
    }

    #[test]
    fn a_field_with_no_target_is_dropped_rather_than_stored_empty() {
        let (_, _, fields) =
            parse_line(&format!("{REAL_GUID},Pad,a:b0,broken,b:,:c,")).expect("parse");
        assert_eq!(fields.get("a"), Some("b0"));
        assert_eq!(fields.get("b"), None);
        assert_eq!(fields.len(), 1);
    }

    #[test]
    fn a_mapping_line_writes_controls_in_canonical_order() {
        let bindings: BTreeMap<Control, Binding> = [
            (Control::Start, Binding::button(9)),
            (Control::A, Binding::button(1)),
            (Control::DpadUp, Binding::hat(0, 1)),
        ]
        .into_iter()
        .collect();
        let built = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", None);
        let a = built.find("a:b1").expect("a");
        let start = built.find("start:b9").expect("start");
        let dpup = built.find("dpup:h0.1").expect("dpup");
        assert!(
            a < start && start < dpup,
            "canonical order was not kept: {built}"
        );
    }

    #[test]
    fn a_binding_sdl_cannot_express_is_left_out_not_fatal() {
        // A half-written profile holds a hat value naming two directions. The
        // pad must still get a line for everything else.
        let bindings: BTreeMap<Control, Binding> = [
            (Control::A, Binding::button(1)),
            (Control::DpadUp, Binding::hat(0, 3)),
        ]
        .into_iter()
        .collect();
        let built = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", None);
        assert!(built.contains("a:b1"));
        assert!(!built.contains("dpup"));
    }

    #[test]
    fn stick_fields_come_after_the_captured_controls() {
        let bindings: BTreeMap<Control, Binding> =
            [(Control::A, Binding::button(1))].into_iter().collect();
        let sticks: Fields = [("leftx", "a0"), ("lefty", "a1")].into_iter().collect();
        let built = mapping_line(REAL_GUID, REAL_NAME, &bindings, "Linux", Some(&sticks));
        assert!(built.find("a:b1") < built.find("leftx:a0"));
    }

    #[test]
    fn a_pad_with_four_centred_axes_gets_both_sticks() {
        let codes = [0x00_u16, 0x01, 0x03, 0x04];
        let fields = stick_fields(&codes, &BTreeMap::new(), None);
        let entries: Vec<(&str, &str)> = fields
            .iter()
            .map(|(name, target)| (name.as_str(), target.as_str()))
            .collect();
        assert_eq!(
            entries,
            [
                ("leftx", "a0"),
                ("lefty", "a1"),
                ("rightx", "a2"),
                ("righty", "a3")
            ]
        );
    }

    #[test]
    fn an_axis_that_rests_at_one_end_is_a_trigger_not_a_stick() {
        // The Mayflash GameCube adapter: its analogue triggers are ABS_RX and
        // ABS_RY, and calling them the right stick jams that stick to a corner.
        let codes = [0x00_u16, 0x01, 0x03, 0x04];
        let axes: BTreeMap<u16, AxisSpan> = [
            (0x00, AxisSpan::new(0, 255, 128)),
            (0x01, AxisSpan::new(0, 255, 127)),
            (0x03, AxisSpan::new(0, 255, 20)),
            (0x04, AxisSpan::new(0, 255, 235)),
        ]
        .into_iter()
        .collect();
        let fields = stick_fields(&codes, &BTreeMap::new(), Some(&axes));
        assert_eq!(fields.get("leftx"), Some("a0"));
        assert_eq!(fields.get("lefty"), Some("a1"));
        assert_eq!(fields.get("rightx"), None);
        assert_eq!(fields.get("righty"), None);
    }

    #[test]
    fn an_axis_a_capture_already_claims_is_not_also_a_stick() {
        let codes = [0x00_u16, 0x01, 0x03, 0x04];
        // The user bound a trigger to axis 2 (ABS_RX).
        let bindings: BTreeMap<Control, Binding> = [(Control::LeftTrigger, Binding::axis(2, 1))]
            .into_iter()
            .collect();
        let fields = stick_fields(&codes, &bindings, None);
        assert_eq!(fields.get("rightx"), None, "an axis cannot be both");
        assert_eq!(fields.get("righty"), Some("a3"));
    }

    #[test]
    fn rests_centred_refuses_a_degenerate_range_rather_than_dividing_by_zero() {
        assert!(!AxisSpan::new(0, 0, 0).rests_centred());
        assert!(!AxisSpan::new(10, 5, 7).rests_centred());
    }

    #[test]
    fn the_stick_tolerance_is_a_wide_berth_around_the_distinction() {
        // Inside: a worn N64 stick resting at 174 on 0..255 is 36% deflected.
        assert!(AxisSpan::new(0, 255, 174).rests_centred());
        // Outside: exactly at the boundary is still a stick, past it is not.
        assert!(
            AxisSpan::new(0, 200, 150).rests_centred(),
            "0.5 is inclusive"
        );
        assert!(!AxisSpan::new(0, 200, 151).rests_centred());
    }
}
