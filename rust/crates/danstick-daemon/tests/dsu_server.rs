//! danstick's motion server, spoken to the way Cemu speaks to it.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use danstick_core::dsu::{self, Kind, Pad, Port, MAX_PACKET_BYTES};
use danstick_core::motion::Motion;
use danstick_daemon::dsu::Motion as Server;

/// A client socket, and a server bound to a port the kernel chose.
fn pair() -> (Server, UdpSocket, SocketAddr) {
    let probe = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).expect("a free port");
    let port = probe.local_addr().expect("an address").port();
    drop(probe);
    let server = Server::bind(port).expect("bind the motion server");
    let client = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).expect("a client");
    client
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("a read timeout");
    let to = SocketAddr::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    (server, client, to)
}

fn ask(client: &UdpSocket, to: SocketAddr, kind: Kind, body: &[u8]) {
    let mut out = Vec::new();
    out.extend_from_slice(b"DSUC");
    out.extend_from_slice(&dsu::PROTOCOL_VERSION.to_le_bytes());
    out.extend_from_slice(&((body.len() as u16 + 4).to_le_bytes()));
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&99u32.to_le_bytes());
    out.extend_from_slice(&(kind as u32).to_le_bytes());
    out.extend_from_slice(body);
    let crc = dsu::crc32(&out);
    out[8..12].copy_from_slice(&crc.to_le_bytes());
    client.send_to(&out, to).expect("send a request");
}

fn hear(client: &UdpSocket) -> Option<Vec<u8>> {
    let mut buffer = [0u8; MAX_PACKET_BYTES];
    let (size, _) = client.recv_from(&mut buffer).ok()?;
    Some(buffer[..size].to_vec())
}

fn kind_of(packet: &[u8]) -> u32 {
    u32::from_le_bytes([packet[16], packet[17], packet[18], packet[19]])
}

fn seated(slot: u8) -> Port {
    danstick_daemon::dsu::port_for(u32::from(slot) + 1, true)
}

fn moving(at: u64) -> Pad {
    Pad {
        motion: Motion {
            accel: [0.0, -1.0, 0.0],
            gyro: [1.0, 2.0, 3.0],
            timestamp_us: at,
        },
        ..Pad::default()
    }
}

#[test]
fn a_version_request_is_answered_to_whoever_asked() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::Version, &[]);
    server.serve(&[]);
    let reply = hear(&client).expect("a version reply");
    assert_eq!(&reply[0..4], b"DSUS");
    assert_eq!(kind_of(&reply), Kind::Version as u32);
    assert_eq!(
        u16::from_le_bytes([reply[20], reply[21]]),
        dsu::PROTOCOL_VERSION
    );
}

#[test]
fn asking_about_four_slots_is_answered_four_times() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::PortInfo, &[4, 0, 0, 0, 0, 1, 2, 3]);
    server.serve(&[seated(0)]);
    let mut slots = Vec::new();
    for _ in 0..4 {
        let reply = hear(&client).expect("a port reply");
        assert_eq!(kind_of(&reply), Kind::PortInfo as u32);
        slots.push((reply[20], reply[21]));
    }
    assert_eq!(slots[0], (0, 2), "slot 0 is seated and connected");
    for (slot, state) in &slots[1..] {
        assert_eq!(*state, 0, "slot {slot} has nobody in it");
    }
}

#[test]
fn a_slot_danstick_does_not_have_is_answered_as_empty() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::PortInfo, &[1, 0, 0, 0, 3]);
    server.serve(&[]);
    let reply = hear(&client).expect("a port reply");
    assert_eq!(reply[20], 3, "the slot it asked about");
    assert_eq!(reply[21], 0, "empty");
}

#[test]
fn nothing_is_sent_until_somebody_subscribes() {
    let (mut server, client, _to) = pair();
    assert!(!server.has_clients());
    server.publish(&[seated(0)], &[moving(1)]);
    assert!(hear(&client).is_none());
}

#[test]
fn a_subscriber_gets_a_sample_with_the_motion_in_it() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::PadData, &[0, 0, 0, 0, 0, 0, 0, 0]);
    server.serve(&[seated(0)]);
    assert!(server.has_clients());

    server.publish(&[seated(0)], &[moving(1_000)]);
    let reply = hear(&client).expect("a pad sample");
    assert_eq!(reply.len(), MAX_PACKET_BYTES);
    assert_eq!(kind_of(&reply), Kind::PadData as u32);
    let body = &reply[20..];
    assert_eq!(
        u64::from_le_bytes(body[48..56].try_into().expect("eight bytes")),
        1_000
    );
    let accel_y = f32::from_le_bytes(body[60..64].try_into().expect("four bytes"));
    assert_eq!(accel_y, -1.0, "gravity, as handed in");
}

#[test]
fn a_sample_that_has_not_advanced_is_not_sent_twice() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::PadData, &[0, 0, 0, 0, 0, 0, 0, 0]);
    server.serve(&[seated(0)]);

    server.publish(&[seated(0)], &[moving(7)]);
    assert!(hear(&client).is_some(), "the first sample");
    server.publish(&[seated(0)], &[moving(7)]);
    assert!(hear(&client).is_none(), "the same sample again");
    server.publish(&[seated(0)], &[moving(8)]);
    assert!(hear(&client).is_some(), "a sample that moved on");
}

#[test]
fn the_packet_counter_advances_so_a_lost_datagram_can_be_seen() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::PadData, &[0, 0, 0, 0, 0, 0, 0, 0]);
    server.serve(&[seated(0)]);
    let mut counters = Vec::new();
    for at in 1..=3u64 {
        server.publish(&[seated(0)], &[moving(at)]);
        let reply = hear(&client).expect("a sample");
        counters.push(u32::from_le_bytes(
            reply[32..36].try_into().expect("four bytes"),
        ));
    }
    assert_eq!(counters, vec![1, 2, 3]);
}

#[test]
fn a_subscription_to_one_slot_does_not_receive_another() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::PadData, &[1, 1, 0, 0, 0, 0, 0, 0]);
    server.serve(&[seated(0), seated(1)]);
    server.publish(&[seated(0), seated(1)], &[moving(1), moving(2)]);
    let reply = hear(&client).expect("the slot it asked for");
    assert_eq!(reply[20], 1);
    assert!(hear(&client).is_none(), "and only that one");
}

#[test]
fn a_subscription_by_address_follows_the_player_and_not_the_slot() {
    let (mut server, client, to) = pair();
    let mac = dsu::mac_for_player(2);
    let mut body = vec![2, 0];
    body.extend_from_slice(&mac);
    ask(&client, to, Kind::PadData, &body);
    server.serve(&[seated(1)]);
    server.publish(&[seated(1)], &[moving(1)]);
    let reply = hear(&client).expect("a sample for player two");
    assert_eq!(&reply[24..30], &mac);
}

#[test]
fn a_datagram_that_is_not_dsu_is_ignored_rather_than_answered() {
    let (mut server, client, to) = pair();
    for rubbish in [vec![0u8; 1], vec![0xFFu8; 64], b"hello there".to_vec()] {
        client.send_to(&rubbish, to).expect("send rubbish");
    }
    server.serve(&[seated(0)]);
    assert!(hear(&client).is_none());
    assert!(!server.has_clients());

    ask(&client, to, Kind::Version, &[]);
    server.serve(&[]);
    assert!(hear(&client).is_some());
}

#[test]
fn two_servers_cannot_hold_one_port() {
    let (server, _client, to) = pair();
    let second = Server::bind(to.port());
    assert!(second.is_err(), "the port is taken");
    drop(server);
}

#[test]
fn a_client_that_subscribes_late_gets_the_current_sample() {
    let (mut server, first, to) = pair();
    ask(&first, to, Kind::PadData, &[0, 0, 0, 0, 0, 0, 0, 0]);
    server.serve(&[seated(0)]);
    server.publish(&[seated(0)], &[moving(50)]);
    assert!(hear(&first).is_some());

    let second = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).expect("a client");
    second
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("timeout");
    ask(&second, to, Kind::PadData, &[0, 0, 0, 0, 0, 0, 0, 0]);
    server.serve(&[seated(0)]);
    server.publish(&[seated(0)], &[moving(50)]);
    assert!(hear(&second).is_some(), "the late client got nothing");
    assert!(hear(&first).is_none(), "the first was not sent it twice");
}

#[test]
fn a_client_that_stops_asking_stops_being_sent_to() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::PadData, &[0, 0, 0, 0, 0, 0, 0, 0]);
    server.serve(&[seated(0)]);
    assert!(server.has_clients());
    let later = std::time::Instant::now() + Duration::from_secs(dsu::SUBSCRIPTION_SECONDS + 1);
    server.publish_at(later, &[seated(0)], &[moving(1)]);
    assert!(!server.has_clients(), "an expired client is still listed");
    assert!(hear(&client).is_none(), "and was written to anyway");
}

#[test]
fn a_renewed_subscription_outlives_the_timeout() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::PadData, &[0, 0, 0, 0, 0, 0, 0, 0]);
    server.serve(&[seated(0)]);
    ask(&client, to, Kind::PadData, &[0, 0, 0, 0, 0, 0, 0, 0]);
    server.serve_at(
        std::time::Instant::now() + Duration::from_secs(4),
        &[seated(0)],
    );
    let later = std::time::Instant::now() + Duration::from_secs(dsu::SUBSCRIPTION_SECONDS + 1);
    server.publish_at(later, &[seated(0)], &[moving(1)]);
    assert!(
        server.has_clients(),
        "a renewal four seconds in was not counted"
    );
}

#[test]
fn a_pad_without_motion_is_not_streamed_on_every_wakeup() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::PadData, &[0, 0, 0, 0, 0, 0, 0, 0]);
    let port = danstick_daemon::dsu::port_for(1, false);
    server.serve(&[port]);
    let still = Pad::default();
    server.publish(&[port], &[still]);
    assert!(hear(&client).is_some(), "the first picture");
    for _ in 0..5 {
        server.publish(&[port], &[still]);
    }
    assert!(hear(&client).is_none(), "the same picture again");
    let pressed = Pad {
        buttons: danstick_core::dsu::button::CROSS,
        ..Pad::default()
    };
    server.publish(&[port], &[pressed]);
    assert!(hear(&client).is_some(), "a press is a change");
}

#[test]
fn a_slot_whose_player_left_is_reported_empty_when_asked() {
    let (mut server, client, to) = pair();
    ask(&client, to, Kind::PortInfo, &[1, 0, 0, 0, 1]);
    server.serve(&[seated(0), seated(1)]);
    assert_eq!(hear(&client).expect("a reply")[21], 2, "connected");
    ask(&client, to, Kind::PortInfo, &[1, 0, 0, 0, 1]);
    server.serve(&[seated(0)]);
    assert_eq!(hear(&client).expect("a reply")[21], 0, "gone");
}
