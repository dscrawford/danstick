//! The cemuhook DSU protocol, on the wire.
//!
//! Motion cannot ride padmap's virtual pad. A uinput node cannot set
//! `EVIOCGUNIQ`, and SDL pairs a joystick with its sensor by comparing exactly
//! that string (`GetSensor`, `SDL_sysjoystick.c`) -- two padmap clones both
//! report `uniq = ""`, so with two players SDL hands player one's gyro to
//! whichever pad it enumerated first. There is no flag for it; the ioctl does
//! not exist.
//!
//! DSU is the road every emulator already supports: UDP on port 26760,
//! addressing controllers by *slot*, which is padmap's player number.
//!
//! The layout is byte for byte from Citra's
//! `src/input_common/helpers/udp_protocol.h` (GPL-2.0-or-later, 2018 Citra
//! Emulator Project), whose `static_assert`s pin every size repeated here as a
//! constant. Three things in it are easy to get wrong, and each has a test:
//! `payload_length` counts the message type as well as the payload; the CRC
//! covers the whole datagram with its own field zeroed; and the twelve bytes
//! that open a `PadData` end in 1 where a `PortInfo` reply ends in 0.
//!
//! [`crate::motion`] owns the frame and the units.

use crate::motion::Motion;

/// UDP port every DSU consumer defaults to.
pub const PORT: u16 = 26760;

/// Where a consumer on this machine finds padmap.
///
/// Loopback, and written into every config padmap generates: this is an
/// unauthenticated protocol reporting what buttons somebody in the room is
/// pressing, and there is no reason for it to leave the machine.
pub const HOST: &str = "127.0.0.1";

/// `PROTOCOL_VERSION`, `udp_protocol.h`.
pub const PROTOCOL_VERSION: u16 = 1001;

/// `CLIENT_MAGIC` -- an emulator asking.
pub const CLIENT_MAGIC: [u8; 4] = *b"DSUC";
/// `SERVER_MAGIC` -- padmap answering.
pub const SERVER_MAGIC: [u8; 4] = *b"DSUS";

/// `MAX_PORTS`. Also padmap's player count, which is not a coincidence: both
/// are four because that is what a living room has.
pub const MAX_SLOTS: usize = 4;

/// `sizeof(Header)`, asserted 20 upstream.
pub const HEADER_BYTES: usize = 20;
/// `sizeof(Response::PortInfo)`, asserted 12 upstream.
pub const PORT_INFO_BYTES: usize = 12;
/// `sizeof(Response::PadData)`, asserted 80 upstream.
pub const PAD_DATA_BYTES: usize = 80;
/// `MAX_PACKET_SIZE`, asserted equal to `sizeof(Message<PadData>)` upstream.
pub const MAX_PACKET_BYTES: usize = HEADER_BYTES + PAD_DATA_BYTES;

/// How long a subscription lasts without being renewed.
///
/// "The default timeout seems to be 5 seconds" -- `udp_protocol.h`, describing
/// the servers its client was written against. A client that goes quiet for
/// longer has exited, and padmap stops sending to it rather than filling a
/// socket buffer nobody is reading.
pub const SUBSCRIPTION_SECONDS: u64 = 5;

/// `enum class Type`.
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

/// `enum class State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connected {
    /// No controller in this slot.
    No = 0,
    /// Reserved. padmap never sends this: a seat held by an absent pad is
    /// reported disconnected, because a consumer that sees `Reserved` shows a
    /// controller that cannot be moved.
    Reserved = 1,
    Yes = 2,
}

/// `enum class Model`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    None = 0,
    /// Accelerometer but no gyroscope.
    PartialGyro = 1,
    /// Both. What padmap reports for a pad it has motion for.
    FullGyro = 2,
    Generic = 3,
}

/// `enum class ConnectionType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    None = 0,
    Usb = 1,
    Bluetooth = 2,
}

/// `enum class Battery`.
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

/// One slot's identity: the twelve bytes that open both replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Port {
    /// Zero-based. Player 1 is slot 0, which is the off-by-one that binds
    /// player one's gyro to player two.
    pub slot: u8,
    pub connected: Connected,
    pub model: Model,
    pub link: Link,
    /// Six bytes a consumer uses to tell one controller from another across a
    /// reconnect. See [`mac_for_player`].
    pub mac: [u8; 6],
    pub battery: Battery,
}

impl Port {
    /// An empty slot, which is what a consumer is told about a seat nobody
    /// holds. Reporting nothing at all would leave a stale controller on
    /// screen, because DSU has no "forget this slot" message.
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

    /// The twelve bytes, with `is_pad_active` set as the caller says.
    ///
    /// `active` is the difference between the two uses: zero in a `PortInfo`
    /// reply, where the byte is only padding, and one in a `PadData`, where it
    /// says the sample that follows is real.
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

/// A stable six-byte address for a player.
///
/// Locally administered (bit 1 of the first octet) and unicast (bit 0 clear),
/// so it cannot collide with a real NIC, then `padmap` in the middle and the
/// player last. It has to be **stable across a reconnect**: a consumer
/// subscribed by MAC address stops receiving the moment it changes, and a
/// controller that went to sleep would come back as a different device.
pub fn mac_for_player(player: u32) -> [u8; 6] {
    // 0x02 = locally administered, individual. 'p','m' for padmap.
    [0x02, b'p', b'm', 0x00, 0x00, player as u8]
}

/// What a client asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// "What protocol do you speak?"
    Version,
    /// "What is in these slots?" -- `slots[..count]`.
    PortInfo {
        slots: [u8; MAX_SLOTS],
        count: usize,
    },
    /// "Send me samples", renewed every second or so for as long as it wants
    /// them.
    PadData(Subscribe),
}

/// Which controllers a `PadData` subscription covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subscribe {
    /// `RegisterFlags::AllPads`. Cemu and Ryujinx both send this.
    All,
    /// `RegisterFlags::PadID`.
    Slot(u8),
    /// `RegisterFlags::PadMACAddress`.
    Mac([u8; 6]),
}

impl Subscribe {
    /// Whether this subscription wants samples for `port`.
    pub fn covers(&self, port: &Port) -> bool {
        match self {
            Subscribe::All => true,
            Subscribe::Slot(slot) => *slot == port.slot,
            Subscribe::Mac(mac) => *mac == port.mac,
        }
    }
}

/// A parsed client datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Incoming {
    /// The client's own id, echoed back in nothing -- padmap sends its own.
    /// Kept because a client that changes it has restarted.
    pub client_id: u32,
    pub request: Request,
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Parse one datagram from a consumer, or `None` if it is not one.
///
/// Every field is checked before it is used, and a datagram that fails any
/// check is dropped rather than answered. This is a UDP socket on localhost
/// with no authentication: anything on the machine can send to it, and the
/// only defence against a malformed packet is that nothing here indexes past
/// what it has measured.
///
/// The CRC is **not** required to match. Some clients send zero, and refusing
/// them would mean padmap silently never answers an emulator that works with
/// every other DSU server. It is checked when non-zero.
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
    // The length field counts the type and the payload, and the datagram may
    // be longer than it claims -- a client is free to pad. Shorter is a lie.
    let claimed = u16_at(datagram, 6) as usize;
    if claimed < 4 || datagram.len() < HEADER_BYTES - 4 + claimed {
        return None;
    }
    let crc = u32_at(datagram, 8);
    if crc != 0 {
        let mut copy = [0u8; MAX_PACKET_BYTES];
        let len = datagram.len().min(MAX_PACKET_BYTES);
        copy[..len].copy_from_slice(&datagram[..len]);
        copy[8..12].fill(0);
        if crc32(&copy[..len]) != crc {
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
            // A count larger than the slots that follow is the shape of a
            // hostile packet; clamp to what is actually there rather than
            // trusting the field.
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

/// Wrap a payload in a server header, CRC and all.
fn message(kind: Kind, server_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + payload.len());
    out.extend_from_slice(&SERVER_MAGIC);
    out.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    // Four more than the payload: the type counts towards the length.
    out.extend_from_slice(&((payload.len() as u16 + 4).to_le_bytes()));
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&server_id.to_le_bytes());
    out.extend_from_slice(&(kind as u32).to_le_bytes());
    out.extend_from_slice(payload);
    let crc = crc32(&out);
    out[8..12].copy_from_slice(&crc.to_le_bytes());
    out
}

/// "I speak 1001."
pub fn version_reply(server_id: u32) -> Vec<u8> {
    message(Kind::Version, server_id, &PROTOCOL_VERSION.to_le_bytes())
}

/// "This is what is in that slot."
pub fn port_reply(server_id: u32, port: &Port) -> Vec<u8> {
    message(Kind::PortInfo, server_id, &port.bytes(false))
}

/// One motion sample, with the pad state that came with it.
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

/// A finger on a touchpad. padmap sends neither down; the field exists because
/// the packet has a fixed size and a consumer parses past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Touch {
    pub down: bool,
    pub id: u8,
    pub x: u16,
    pub y: u16,
}

/// Everything about a controller a `PadData` carries.
///
/// The buttons and sticks are here because the packet has room for them and a
/// consumer that reads padmap as a DSU *controller* rather than only a motion
/// source then works. Cemu and Ryujinx take only the motion; Dolphin's DSU
/// backend exposes all of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pad {
    /// `digital_button`, the bitfield named in `udp_protocol.h`.
    pub buttons: u16,
    /// The guide button, which sits outside the bitfield.
    pub home: u8,
    /// A touchpad pressed rather than touched.
    pub touch_click: u8,
    pub left_x: u8,
    pub left_y: u8,
    pub right_x: u8,
    pub right_y: u8,
    /// `AnalogButton`, in its declared order -- see [`ANALOG_ORDER`].
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
            // A stick at rest is the middle of a u8 range, not zero. Sending
            // zero is a stick held hard left and up, which walks a character
            // into a wall for as long as the emulator is listening.
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

/// Bit positions in `digital_button`, from the union `udp_protocol.h` spells
/// out in a comment. The names are the DualShock's, because the format is.
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

/// The order of `AnalogButton`'s twelve bytes, as indices into [`Pad::analog`].
///
/// Declared here because the order is not the bit order above and writing a
/// trigger into the d-pad's byte is silent.
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

/// The declared order, for tests and documentation.
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

/// CRC-32/ISO-HDLC, the one `boost::crc_32_type` computes.
///
/// Bitwise rather than table-driven: a DSU packet is a hundred bytes at a
/// hundred hertz, so this is a few microseconds a second, and a 1KiB table
/// would be a thing to get wrong for no measurable return.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            // The reflected polynomial, 0x04C11DB7 bit-reversed.
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
        // Two bytes of payload plus the four-byte type.
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
        // MAX_PACKET_SIZE upstream, and the static_assert that it equals
        // sizeof(Message<PadData>). A client reads a fixed-size struct off the
        // wire: one byte out and every field after it is garbage.
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
        // Offsets inside PadData: info 0..12, counter 12, buttons 16, home 18,
        // touch_click 19, sticks 20..24, analog 24..36, touch 36..48,
        // timestamp 48, accel 56, gyro 68.
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
        // Not rejected: a client that asks for four and sends two is asking
        // about two. Trusting the count would read past the datagram.
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
        // Every one of these arrived on a socket anything on the machine can
        // write to. None may index past what it measured.
        assert!(parse_request(&[]).is_none());
        assert!(parse_request(&[0; 19]).is_none());
        let mut wrong_magic = client_packet(Kind::Version, &[]);
        wrong_magic[0] = b'X';
        assert!(parse_request(&wrong_magic).is_none());
        let mut wrong_version = client_packet(Kind::Version, &[]);
        wrong_version[4..6].copy_from_slice(&1000u16.to_le_bytes());
        assert!(parse_request(&wrong_version).is_none());
        // A type padmap does not implement.
        let mut wrong_kind = client_packet(Kind::Version, &[]);
        wrong_kind[16..20].copy_from_slice(&0x0010_0009u32.to_le_bytes());
        wrong_kind[8..12].fill(0);
        assert!(parse_request(&wrong_kind).is_none());
        // A PadData request with no body.
        let short = client_packet(Kind::PadData, &[0, 0]);
        assert!(parse_request(&short).is_none());
        // A length field longer than the datagram.
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
        // Stable, because a client that subscribed by MAC stops receiving the
        // moment it changes -- a pad that slept would come back as a device
        // the emulator has never heard of.
        for player in 1..=4u32 {
            let mac = mac_for_player(player);
            assert_eq!(mac, mac_for_player(player));
            assert_eq!(mac[0] & 0b11, 0b10, "locally administered, unicast");
        }
        assert_ne!(mac_for_player(1), mac_for_player(2));
    }
}
