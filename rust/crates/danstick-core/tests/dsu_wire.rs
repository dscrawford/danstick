//! The DSU wire format, attacked: the socket is unauthenticated, so the parser must never read past what it measured.

use danstick_core::dsu::{self, analog, button, Kind, Pad, Port, Request, Subscribe, Touch};
use danstick_core::dsupad::Range;
use danstick_core::motion::Motion;
use proptest::prelude::*;

fn client_packet(kind: Kind, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&dsu::CLIENT_MAGIC);
    out.extend_from_slice(&dsu::PROTOCOL_VERSION.to_le_bytes());
    out.extend_from_slice(&((body.len() as u16 + 4).to_le_bytes()));
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&7u32.to_le_bytes());
    out.extend_from_slice(&(kind as u32).to_le_bytes());
    out.extend_from_slice(body);
    stamp_crc(&mut out);
    out
}

fn stamp_crc(packet: &mut [u8]) {
    packet[8..12].fill(0);
    let crc = dsu::crc32(packet);
    packet[8..12].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn a_client_that_pads_its_datagram_past_a_hundred_bytes_is_still_believed() {
    // The client's CRC covers everything it sent; checking only the first hundred rejects it for ever.
    let mut packet = client_packet(Kind::Version, &[]);
    packet.extend_from_slice(&[0xAB; 200]);
    stamp_crc(&mut packet);
    let got = dsu::parse_request(&packet).expect("a padded but honest packet");
    assert_eq!(got.request, Request::Version);
}

#[test]
fn every_truncation_of_a_real_packet_is_declined_or_answered_without_panic() {
    for kind in [Kind::Version, Kind::PortInfo, Kind::PadData] {
        let full = client_packet(kind, &[4, 0, 0, 0, 0, 1, 2, 3]);
        for cut in 0..full.len() {
            let _ = dsu::parse_request(&full[..cut]);
        }
    }
}

proptest! {
    #[test]
    fn random_bytes_never_panic_the_parser(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = dsu::parse_request(&bytes);
    }

    #[test]
    fn a_real_header_with_a_random_body_never_panics(
        kind in prop_oneof![Just(Kind::Version), Just(Kind::PortInfo), Just(Kind::PadData)],
        body in proptest::collection::vec(any::<u8>(), 0..120),
    ) {
        let packet = client_packet(kind, &body);
        let _ = dsu::parse_request(&packet);
        // Some clients send a blank CRC.
        let mut blank = packet.clone();
        blank[8..12].fill(0);
        let _ = dsu::parse_request(&blank);
    }

    #[test]
    fn a_lying_length_field_never_panics(
        claimed in any::<u16>(),
        body in proptest::collection::vec(any::<u8>(), 0..40),
    ) {
        let mut packet = client_packet(Kind::PortInfo, &body);
        packet[6..8].copy_from_slice(&claimed.to_le_bytes());
        packet[8..12].fill(0);
        let _ = dsu::parse_request(&packet);
    }

    #[test]
    fn a_stick_value_always_lands_inside_the_byte(
        min in -40000i32..40000,
        max in -40000i32..40000,
        value in any::<i32>(),
    ) {
        let range = Range::declared(min, max, min);
        let _ = range.to_u8(value);
        let _ = range.to_u8_inverted(value);
        if max > min {
            prop_assert_eq!(range.to_u8(min), 0);
            prop_assert_eq!(range.to_u8(max), 255);
            prop_assert_eq!(range.to_u8_inverted(min), 255);
        }
    }

    #[test]
    fn scaling_is_monotonic_across_the_declared_range(
        min in -40000i32..40000,
        span in 1i32..80000,
        a in any::<i32>(),
        b in any::<i32>(),
    ) {
        let range = Range::declared(min, min + span, min);
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        prop_assert!(range.to_u8(lo) <= range.to_u8(hi));
        prop_assert!(range.to_u8_inverted(lo) >= range.to_u8_inverted(hi));
    }
}

#[test]
fn a_port_request_naming_more_slots_than_exist_asks_only_about_the_first_four() {
    let packet = client_packet(Kind::PortInfo, &[8, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7]);
    let got = dsu::parse_request(&packet).expect("a clamped request");
    assert_eq!(
        got.request,
        Request::PortInfo {
            slots: [0, 1, 2, 3],
            count: 4
        }
    );
}

#[test]
fn an_unknown_subscription_flag_is_declined_rather_than_treated_as_all() {
    // Flag 3 is not in RegisterFlags; "all pads" would hand a malformed client every player.
    let packet = client_packet(Kind::PadData, &[3, 0, 0, 0, 0, 0, 0, 0]);
    assert!(dsu::parse_request(&packet).is_none());
}

#[test]
fn a_subscription_by_slot_ignores_the_mac_bytes_and_vice_versa() {
    let by_slot = client_packet(Kind::PadData, &[1, 2, 9, 9, 9, 9, 9, 9]);
    assert_eq!(
        dsu::parse_request(&by_slot).expect("parses").request,
        Request::PadData(Subscribe::Slot(2))
    );
    let by_mac = client_packet(Kind::PadData, &[2, 9, 1, 2, 3, 4, 5, 6]);
    assert_eq!(
        dsu::parse_request(&by_mac).expect("parses").request,
        Request::PadData(Subscribe::Mac([1, 2, 3, 4, 5, 6]))
    );
}

/// Every offset inside `Response::PadData`, worked from the packed struct.
mod layout {
    pub const INFO: usize = 0;
    pub const COUNTER: usize = 12;
    pub const BUTTONS: usize = 16;
    pub const HOME: usize = 18;
    pub const TOUCH_CLICK: usize = 19;
    pub const LEFT_X: usize = 20;
    pub const LEFT_Y: usize = 21;
    pub const RIGHT_X: usize = 22;
    pub const RIGHT_Y: usize = 23;
    pub const ANALOG: usize = 24;
    pub const TOUCH: usize = 36;
    pub const TIMESTAMP: usize = 48;
    pub const ACCEL: usize = 56;
    pub const GYRO: usize = 68;
    pub const END: usize = 80;
}

#[test]
fn every_field_of_a_pad_sample_sits_at_its_struct_offset() {
    let port = Port {
        slot: 2,
        connected: dsu::Connected::Yes,
        model: dsu::Model::FullGyro,
        link: dsu::Link::Bluetooth,
        mac: [1, 2, 3, 4, 5, 6],
        battery: dsu::Battery::High,
    };
    let mut analog = [0u8; 12];
    analog[analog::L2] = 200;
    analog[analog::DPAD_UP] = 255;
    let pad = Pad {
        buttons: button::CROSS | button::UP | button::L2,
        home: 1,
        touch_click: 0,
        left_x: 10,
        left_y: 20,
        right_x: 30,
        right_y: 40,
        analog,
        touch: [
            Touch {
                down: true,
                id: 5,
                x: 0x1234,
                y: 0x5678,
            },
            Touch::default(),
        ],
        motion: Motion {
            accel: [0.5, -1.0, 0.25],
            gyro: [10.0, -20.0, 30.0],
            timestamp_us: 0xDEAD_BEEF_CAFE,
        },
    };
    let packet = dsu::pad_reply(0x11, &port, 0x0102_0304, &pad);
    let body = &packet[dsu::HEADER_BYTES..];
    assert_eq!(body.len(), layout::END);

    assert_eq!(
        &body[layout::INFO..layout::INFO + 12],
        &[2, 2, 2, 2, 1, 2, 3, 4, 5, 6, 4, 1]
    );
    assert_eq!(&body[layout::COUNTER..layout::COUNTER + 4], &[4, 3, 2, 1]);
    let buttons = u16::from_le_bytes([body[layout::BUTTONS], body[layout::BUTTONS + 1]]);
    assert_eq!(buttons, button::CROSS | button::UP | button::L2);
    for (at, want) in [
        (layout::HOME, 1),
        (layout::TOUCH_CLICK, 0),
        (layout::LEFT_X, 10),
        (layout::LEFT_Y, 20),
        (layout::RIGHT_X, 30),
        (layout::RIGHT_Y, 40),
        (layout::ANALOG + analog::L2, 200),
        (layout::ANALOG + analog::DPAD_UP, 255),
    ] {
        assert_eq!(body[at], want, "byte {at}");
    }
    assert_eq!(
        &body[layout::TOUCH..layout::TOUCH + 6],
        &[1, 5, 0x34, 0x12, 0x78, 0x56]
    );
    assert_eq!(&body[layout::TOUCH + 6..layout::TOUCH + 12], &[0; 6]);
    let timestamp = u64::from_le_bytes(
        body[layout::TIMESTAMP..layout::TIMESTAMP + 8]
            .try_into()
            .expect("8"),
    );
    assert_eq!(timestamp, 0xDEAD_BEEF_CAFE);
    let f = |at: usize| f32::from_le_bytes(body[at..at + 4].try_into().expect("4"));
    assert_eq!(
        [f(layout::ACCEL), f(layout::ACCEL + 4), f(layout::ACCEL + 8)],
        [0.5, -1.0, 0.25]
    );
    assert_eq!(
        [f(layout::GYRO), f(layout::GYRO + 4), f(layout::GYRO + 8)],
        [10.0, -20.0, 30.0]
    );
}

#[test]
fn the_analog_block_is_in_the_structs_order_not_the_bitfields() {
    assert_eq!(dsu::ANALOG_ORDER[analog::DPAD_LEFT], "dpad_left");
    assert_eq!(dsu::ANALOG_ORDER[analog::CROSS], "cross");
    assert_eq!(dsu::ANALOG_ORDER[analog::R1], "r1");
    assert_eq!(dsu::ANALOG_ORDER[analog::L1], "l1");
    assert_eq!(dsu::ANALOG_ORDER[analog::R2], "r2");
    assert_eq!(dsu::ANALOG_ORDER[analog::L2], "l2");
    assert_eq!(
        [analog::R1, analog::L1, analog::R2, analog::L2],
        [8, 9, 10, 11]
    );
}

#[test]
fn the_button_bits_are_the_dualshocks() {
    for (name, bit, want) in [
        ("SHARE", button::SHARE, 1),
        ("L3", button::L3, 2),
        ("R3", button::R3, 4),
        ("OPTIONS", button::OPTIONS, 8),
        ("UP", button::UP, 16),
        ("RIGHT", button::RIGHT, 32),
        ("DOWN", button::DOWN, 64),
        ("LEFT", button::LEFT, 128),
        ("L2", button::L2, 256),
        ("R2", button::R2, 512),
        ("L1", button::L1, 1024),
        ("R1", button::R1, 2048),
        ("TRIANGLE", button::TRIANGLE, 4096),
        ("CIRCLE", button::CIRCLE, 8192),
        ("CROSS", button::CROSS, 16384),
        ("SQUARE", button::SQUARE, 32768),
    ] {
        assert_eq!(bit, want, "{name}");
    }
}

#[test]
fn a_server_reply_validates_under_its_own_parser_rules() {
    for packet in [
        dsu::version_reply(1),
        dsu::port_reply(1, &Port::empty(0)),
        dsu::pad_reply(1, &Port::empty(0), 0, &Pad::default()),
    ] {
        let stated = u32::from_le_bytes(packet[8..12].try_into().expect("4"));
        let mut zeroed = packet.clone();
        zeroed[8..12].fill(0);
        assert_eq!(dsu::crc32(&zeroed), stated);
        let claimed = u16::from_le_bytes([packet[6], packet[7]]) as usize;
        assert_eq!(claimed, packet.len() - 16, "length counts the type too");
    }
}
