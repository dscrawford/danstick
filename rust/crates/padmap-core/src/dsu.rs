//! Cemuhook DSU protocol (UDP port 26760). Keys: payload_length includes type,
//! CRC covers whole packet, PadData byte 11 is 1 (PortInfo: 0).

use crate::motion::Motion;

/// UDP port every DSU consumer defaults to.
pub const PORT: u16 = 26760;

/// Loopback (unauthenticated protocol, no reason to leave machine).
pub const HOST: &str = "127.0.0.1";

/// `PROTOCOL_VERSION`, `udp_protocol.h`.
pub const PROTOCOL_VERSION: u16 = 1001;

/// `CLIENT_MAGIC` -- an emulator asking.
pub const CLIENT_MAGIC: [u8; 4] = *b"DSUC";
/// `SERVER_MAGIC` -- padmap answering.
pub const SERVER_MAGIC: [u8; 4] = *b"DSUS";

/// MAX_PORTS; also padmap's player count (4 for a living room).
pub const MAX_SLOTS: usize = 4;

pub const HEADER_BYTES: usize = 20;
pub const PORT_INFO_BYTES: usize = 12;
pub const PAD_DATA_BYTES: usize = 80;
pub const MAX_PACKET_BYTES: usize = HEADER_BYTES + PAD_DATA_BYTES;

/// Subscription timeout; stop sending if client goes quiet.
pub const SUBSCRIPTION_SECONDS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Version = 0x0010_0000,
    PortInfo = 0x0010_0001,
    PadData = 0x0010_0002,
}

impl Kind {
    fn from_u32(value: u32) -> Option<Kind> {
        match value {
            0x0010_0000 => Some(Kind::Version),
            0x0010_0001 => Some(Kind::PortInfo),
            0x0010_0002 => Some(Kind::PadData),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connected {
    No = 0,
    /// Reserved (padmap never sends this; absent pad is disconnected).
    Reserved = 1,
    Yes = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    None = 0,
    PartialGyro = 1,
    FullGyro = 2,
    Generic = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    None = 0,
    Usb = 1,
    Bluetooth = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Battery {
    None = 0x00,
    Dying = 0x01,
    Low = 0x02,
    Medium = 0x03,
    High = 0x04,
    Full = 0x05,
    Charging = 0xEE,
    Charged = 0xEF,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Port {
    pub slot: u8,
    pub connected: Connected,
    pub model: Model,
    pub link: Link,
    pub mac: [u8; 6],
    pub battery: Battery,
}

impl Port {
    pub fn empty(slot: u8) -> Port {
        Port {
            slot,
            connected: Connected::No,
            model: Model::None,
            link: Link::None,
            mac: [0; 6],
            battery: Battery::None,
        }
    }

    // Returns twelve bytes: byte 11 is 0 in PortInfo, 1 in PadData.
    fn bytes(&self, active: bool) -> [u8; PORT_INFO_BYTES] {
        let mut out = [0u8; PORT_INFO_BYTES];
        out[0] = self.slot;
        out[1] = self.connected as u8;
        out[2] = self.model as u8;
        out[3] = self.link as u8;
        out[4..10].copy_from_slice(&self.mac);
        out[10] = self.battery as u8;
        out[11] = u8::from(active);
        out
    }
}

/// Stable MAC per player; locally administered unicast, won't collide with real NIC.
pub fn mac_for_player(player: u32) -> [u8; 6] {
    [0x02, b'p', b'm', 0x00, 0x00, player as u8] // 0x02: locally admin, unicast
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Version,
    PortInfo {
        slots: [u8; MAX_SLOTS],
        count: usize,
    },
    PadData(Subscribe),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subscribe {
    All,
    Slot(u8),
    Mac([u8; 6]),
}

impl Subscribe {
    pub fn covers(&self, port: &Port) -> bool {
        match self {
            Subscribe::All => true,
            Subscribe::Slot(slot) => *slot == port.slot,
            Subscribe::Mac(mac) => *mac == port.mac,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Incoming {
    pub client_id: u32,
    pub request: Request,
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Parse client datagram. CRC checked only when non-zero (some clients send 0).
pub fn parse_request(datagram: &[u8]) -> Option<Incoming> {
    if datagram.len() < HEADER_BYTES {
        return None;
    }
    if datagram[0..4] != CLIENT_MAGIC {
        return None;
    }
    if u16_at(datagram, 4) != PROTOCOL_VERSION {
        return None;
    }
    // Length counts type and payload; datagram can be longer (padding ok), shorter is lie.
    let claimed = u16_at(datagram, 6) as usize;
    if claimed < 4 || datagram.len() < HEADER_BYTES - 4 + claimed {
        return None;
    }
    let crc = u32_at(datagram, 8);
    if crc != 0 {
        // CRC over actual datagram length (client's padding is included in their CRC).
        let mut copy = datagram.to_vec();
        copy[8..12].fill(0);
        if crc32(&copy) != crc {
            return None;
        }
    }
    let client_id = u32_at(datagram, 12);
    let kind = Kind::from_u32(u32_at(datagram, 16))?;
    let body = &datagram[HEADER_BYTES..];
    let request = match kind {
        Kind::Version => Request::Version,
        Kind::PortInfo => {
            if body.len() < 4 {
                return None;
            }
            // Clamp count to prevent hostile packets from over-reading.
            let asked = u32_at(body, 0) as usize;
            let count = asked.min(MAX_SLOTS).min(body.len() - 4);
            let mut slots = [0u8; MAX_SLOTS];
            slots[..count].copy_from_slice(&body[4..4 + count]);
            Request::PortInfo { slots, count }
        }
        Kind::PadData => {
            if body.len() < 8 {
                return None;
            }
            let subscribe = match body[0] {
                0 => Subscribe::All,
                1 => Subscribe::Slot(body[1]),
                2 => {
                    let mut mac = [0u8; 6];
                    mac.copy_from_slice(&body[2..8]);
                    Subscribe::Mac(mac)
                }
                _ => return None,
            };
            Request::PadData(subscribe)
        }
    };
    Some(Incoming { client_id, request })
}

fn message(kind: Kind, server_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + payload.len());
    out.extend_from_slice(&SERVER_MAGIC);
    out.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    // Length includes type field (4 bytes).
    out.extend_from_slice(&((payload.len() as u16 + 4).to_le_bytes()));
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&server_id.to_le_bytes());
    out.extend_from_slice(&(kind as u32).to_le_bytes());
    out.extend_from_slice(payload);
    let crc = crc32(&out);
    out[8..12].copy_from_slice(&crc.to_le_bytes());
    out
}

pub fn version_reply(server_id: u32) -> Vec<u8> {
    message(Kind::Version, server_id, &PROTOCOL_VERSION.to_le_bytes())
}

pub fn port_reply(server_id: u32, port: &Port) -> Vec<u8> {
    message(Kind::PortInfo, server_id, &port.bytes(false))
}

pub fn pad_reply(server_id: u32, port: &Port, counter: u32, pad: &Pad) -> Vec<u8> {
    let mut body = Vec::with_capacity(PAD_DATA_BYTES);
    body.extend_from_slice(&port.bytes(true));
    body.extend_from_slice(&counter.to_le_bytes());
    body.extend_from_slice(&pad.buttons.to_le_bytes());
    body.push(pad.home);
    body.push(pad.touch_click);
    body.push(pad.left_x);
    body.push(pad.left_y);
    body.push(pad.right_x);
    body.push(pad.right_y);
    body.extend_from_slice(&pad.analog);
    for touch in &pad.touch {
        body.push(u8::from(touch.down));
        body.push(touch.id);
        body.extend_from_slice(&touch.x.to_le_bytes());
        body.extend_from_slice(&touch.y.to_le_bytes());
    }
    body.extend_from_slice(&pad.motion.timestamp_us.to_le_bytes());
    for value in pad.motion.accel {
        body.extend_from_slice(&value.to_le_bytes());
    }
    for value in pad.motion.gyro {
        body.extend_from_slice(&value.to_le_bytes());
    }
    debug_assert_eq!(body.len(), PAD_DATA_BYTES);
    message(Kind::PadData, server_id, &body)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Touch {
    pub down: bool,
    pub id: u8,
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pad {
    pub buttons: u16,
    pub home: u8,
    pub touch_click: u8,
    pub left_x: u8,
    pub left_y: u8,
    pub right_x: u8,
    pub right_y: u8,
    pub analog: [u8; 12],
    pub touch: [Touch; 2],
    pub motion: Motion,
}

impl Default for Pad {
    fn default() -> Pad {
        Pad {
            buttons: 0,
            home: 0,
            touch_click: 0,
            // Stick at rest is CENTRE, not 0 (which is hard left+up).
            left_x: CENTRE,
            left_y: CENTRE,
            right_x: CENTRE,
            right_y: CENTRE,
            analog: [0; 12],
            touch: [Touch::default(); 2],
            motion: Motion::default(),
        }
    }
}

/// Where a stick rests in DSU's `u8` range.
pub const CENTRE: u8 = 128;

// Bit positions from udp_protocol.h; DualShock naming.
pub mod button {
    pub const SHARE: u16 = 1 << 0;
    pub const L3: u16 = 1 << 1;
    pub const R3: u16 = 1 << 2;
    pub const OPTIONS: u16 = 1 << 3;
    pub const UP: u16 = 1 << 4;
    pub const RIGHT: u16 = 1 << 5;
    pub const DOWN: u16 = 1 << 6;
    pub const LEFT: u16 = 1 << 7;
    pub const L2: u16 = 1 << 8;
    pub const R2: u16 = 1 << 9;
    pub const L1: u16 = 1 << 10;
    pub const R1: u16 = 1 << 11;
    pub const TRIANGLE: u16 = 1 << 12;
    pub const CIRCLE: u16 = 1 << 13;
    pub const CROSS: u16 = 1 << 14;
    pub const SQUARE: u16 = 1 << 15;
}

// AnalogButton indices (not same as bit order; wrong index is silent failure).
pub mod analog {
    pub const DPAD_LEFT: usize = 0;
    pub const DPAD_DOWN: usize = 1;
    pub const DPAD_RIGHT: usize = 2;
    pub const DPAD_UP: usize = 3;
    pub const SQUARE: usize = 4;
    pub const CROSS: usize = 5;
    pub const CIRCLE: usize = 6;
    pub const TRIANGLE: usize = 7;
    pub const R1: usize = 8;
    pub const L1: usize = 9;
    pub const R2: usize = 10;
    pub const L2: usize = 11;
}

pub const ANALOG_ORDER: [&str; 12] = [
    "dpad_left",
    "dpad_down",
    "dpad_right",
    "dpad_up",
    "square",
    "cross",
    "circle",
    "triangle",
    "r1",
    "l1",
    "r2",
    "l2",
];

/// CRC-32/ISO-HDLC (bitwise, not table-driven: negligible overhead).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            // Reflected polynomial (0x04C11DB7 bit-reversed).
            crc = (crc >> 1) ^ (0xEDB8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        // The CRC-32/ISO-HDLC "check" vector: every catalogue lists 0xCBF43926
        // for the nine bytes "123456789". If this is wrong, every packet
        // padmap sends is rejected by a client that validates, and the symptom
        // is an emulator that sees the server and never a sample.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_header_is_twenty_bytes_and_says_so() {
        let packet = version_reply(7);
        assert_eq!(packet.len(), HEADER_BYTES + 2);
        assert_eq!(&packet[0..4], b"DSUS");
        assert_eq!(u16_at(&packet, 4), PROTOCOL_VERSION);
        // Length = payload (2) + type (4).
        assert_eq!(u16_at(&packet, 6), 6);
        assert_eq!(u32_at(&packet, 12), 7);
        assert_eq!(u32_at(&packet, 16), Kind::Version as u32);
        assert_eq!(u16_at(&packet, 20), PROTOCOL_VERSION);
    }

    #[test]
    fn the_crc_covers_the_whole_packet_with_its_own_field_zeroed() {
        let packet = version_reply(7);
        let stated = u32_at(&packet, 8);
        let mut zeroed = packet.clone();
        zeroed[8..12].fill(0);
        assert_eq!(crc32(&zeroed), stated);
        assert_ne!(stated, 0);
    }

    #[test]
    fn a_pad_reply_is_exactly_a_hundred_bytes() {
        // Client reads fixed-size struct; off by one = garbage.
        let port = Port {
            slot: 1,
            connected: Connected::Yes,
            model: Model::FullGyro,
            link: Link::Usb,
            mac: mac_for_player(2),
            battery: Battery::Full,
        };
        let packet = pad_reply(0, &port, 5, &Pad::default());
        assert_eq!(packet.len(), MAX_PACKET_BYTES);
        assert_eq!(u16_at(&packet, 6), PAD_DATA_BYTES as u16 + 4);
    }

    #[test]
    fn the_motion_block_sits_where_the_struct_says() {
        // PadData layout: motion at offset 48+ (timestamp, accel, gyro).
        let pad = Pad {
            motion: Motion {
                accel: [1.0, 2.0, 3.0],
                gyro: [4.0, 5.0, 6.0],
                timestamp_us: 0x0102_0304_0506_0708,
            },
            ..Pad::default()
        };
        let packet = pad_reply(0, &Port::empty(0), 0, &pad);
        let body = &packet[HEADER_BYTES..];
        assert_eq!(
            u64::from_le_bytes(body[48..56].try_into().expect("eight bytes")),
            0x0102_0304_0506_0708
        );
        for (index, expected) in [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0].iter().enumerate() {
            let at = 56 + index * 4;
            let got = f32::from_le_bytes(body[at..at + 4].try_into().expect("four bytes"));
            assert_eq!(got, *expected, "float {index}");
        }
    }

    #[test]
    fn a_resting_pad_reports_centred_sticks() {
        // Zero here is a stick held hard over, which is the difference between
        // "nobody is playing" and "somebody is walking into a wall".
        let packet = pad_reply(0, &Port::empty(0), 0, &Pad::default());
        let body = &packet[HEADER_BYTES..];
        assert_eq!(&body[20..24], &[128, 128, 128, 128]);
    }

    #[test]
    fn port_info_pads_where_pad_data_says_active() {
        let port = Port::empty(2);
        let info = port_reply(0, &port);
        assert_eq!(info[HEADER_BYTES + 11], 0);
        let data = pad_reply(0, &port, 0, &Pad::default());
        assert_eq!(data[HEADER_BYTES + 11], 1);
    }

    fn client_packet(kind: Kind, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&CLIENT_MAGIC);
        out.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        out.extend_from_slice(&((body.len() as u16 + 4).to_le_bytes()));
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&1234u32.to_le_bytes());
        out.extend_from_slice(&(kind as u32).to_le_bytes());
        out.extend_from_slice(body);
        let crc = crc32(&out);
        out[8..12].copy_from_slice(&crc.to_le_bytes());
        out
    }

    #[test]
    fn a_version_request_parses() {
        let packet = client_packet(Kind::Version, &[]);
        let got = parse_request(&packet).expect("a version request");
        assert_eq!(got.client_id, 1234);
        assert_eq!(got.request, Request::Version);
    }

    #[test]
    fn a_port_request_parses_its_slots() {
        let packet = client_packet(Kind::PortInfo, &[4, 0, 0, 0, 0, 1, 2, 3]);
        let got = parse_request(&packet).expect("a port request");
        assert_eq!(
            got.request,
            Request::PortInfo {
                slots: [0, 1, 2, 3],
                count: 4
            }
        );
    }

    #[test]
    fn a_port_request_claiming_more_slots_than_it_carries_is_clamped() {
        // Clamp to actual slots (don't trust count, don't read past datagram).
        let packet = client_packet(Kind::PortInfo, &[255, 255, 255, 255, 0, 1]);
        let got = parse_request(&packet).expect("a clamped port request");
        assert_eq!(
            got.request,
            Request::PortInfo {
                slots: [0, 1, 0, 0],
                count: 2
            }
        );
    }

    #[test]
    fn the_three_subscription_shapes_parse() {
        let all = client_packet(Kind::PadData, &[0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            parse_request(&all).expect("all pads").request,
            Request::PadData(Subscribe::All)
        );
        let slot = client_packet(Kind::PadData, &[1, 3, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            parse_request(&slot).expect("one slot").request,
            Request::PadData(Subscribe::Slot(3))
        );
        let mac = client_packet(Kind::PadData, &[2, 0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(
            parse_request(&mac).expect("one address").request,
            Request::PadData(Subscribe::Mac([1, 2, 3, 4, 5, 6]))
        );
    }

    #[test]
    fn a_zero_crc_is_accepted_and_a_wrong_one_is_not() {
        let mut packet = client_packet(Kind::Version, &[]);
        packet[8..12].fill(0);
        assert!(parse_request(&packet).is_some());
        packet[8] = 0x01;
        assert!(parse_request(&packet).is_none());
    }

    #[test]
    fn nothing_hostile_gets_past_the_header() {
        assert!(parse_request(&[]).is_none());
        assert!(parse_request(&[0; 19]).is_none());
        let mut wrong_magic = client_packet(Kind::Version, &[]);
        wrong_magic[0] = b'X';
        assert!(parse_request(&wrong_magic).is_none());
        let mut wrong_version = client_packet(Kind::Version, &[]);
        wrong_version[4..6].copy_from_slice(&1000u16.to_le_bytes());
        assert!(parse_request(&wrong_version).is_none());
        let mut wrong_kind = client_packet(Kind::Version, &[]);
        wrong_kind[16..20].copy_from_slice(&0x0010_0009u32.to_le_bytes());
        wrong_kind[8..12].fill(0);
        assert!(parse_request(&wrong_kind).is_none());
        let short = client_packet(Kind::PadData, &[0, 0]);
        assert!(parse_request(&short).is_none());
        let mut lying = client_packet(Kind::Version, &[]);
        lying[6..8].copy_from_slice(&600u16.to_le_bytes());
        lying[8..12].fill(0);
        assert!(parse_request(&lying).is_none());
    }

    #[test]
    fn a_subscription_covers_what_it_named() {
        let port = Port {
            slot: 2,
            mac: mac_for_player(3),
            ..Port::empty(2)
        };
        assert!(Subscribe::All.covers(&port));
        assert!(Subscribe::Slot(2).covers(&port));
        assert!(!Subscribe::Slot(1).covers(&port));
        assert!(Subscribe::Mac(mac_for_player(3)).covers(&port));
        assert!(!Subscribe::Mac(mac_for_player(4)).covers(&port));
    }

    #[test]
    fn a_players_address_is_locally_administered_and_stable() {
        // MAC must be stable across reconnect (sleeping pad = new device).
        for player in 1..=4u32 {
            let mac = mac_for_player(player);
            assert_eq!(mac, mac_for_player(player));
            assert_eq!(mac[0] & 0b11, 0b10, "locally administered, unicast");
        }
        assert_ne!(mac_for_player(1), mac_for_player(2));
    }
}
