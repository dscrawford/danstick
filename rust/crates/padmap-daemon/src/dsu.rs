//! padmap as a DSU server: motion on a stable port, by player number.
//!
//! One UDP socket, bound to loopback. A consumer asks what is in each slot and
//! subscribes; padmap answers, and then sends a sample per slot each time the
//! controller in it says something new. Subscriptions expire, so an emulator
//! that exits stops being written to without telling anybody.
//!
//! Loopback only, deliberately. This is an unauthenticated protocol that
//! reports what buttons somebody in the room is pressing, and binding it to
//! every interface would put that on the network.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::os::fd::{AsFd, BorrowedFd};
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use padmap_core::dsu::{
    self, Battery, Connected, Incoming, Link, Model, Pad, Port, Request, Subscribe, MAX_SLOTS,
    SUBSCRIPTION_SECONDS,
};

/// What padmap calls itself to a client. Arbitrary, and constant so a client
/// that reconnects sees the same server rather than a new one.
const SERVER_ID: u32 = 0x7061_646D; // "padm"

/// A client that has asked for samples.
#[derive(Debug, Clone, Copy)]
struct Client {
    subscribe: Subscribe,
    last_asked: Instant,
}

/// The UDP server, and what it has told whom.
#[derive(Debug)]
pub struct Motion {
    socket: UdpSocket,
    /// Who wants samples, by where they asked from. An emulator renews about
    /// once a second; the map stays at one or two entries.
    clients: HashMap<SocketAddr, Client>,
    /// Per slot, so a consumer can tell a dropped datagram from a still pad.
    /// UDP loses packets and nothing retransmits them.
    counters: [u32; MAX_SLOTS],
    /// The last sample sent per slot, so an unchanged pad is not resent at the
    /// rate the loop happens to run at.
    sent: [Option<u64>; MAX_SLOTS],
}

impl Motion {
    /// Bind the server, or say why not.
    ///
    /// A port already in use is the ordinary failure: another DSU server is
    /// running, and padmap says so rather than fighting it. The daemon carries
    /// on without motion, because a controller that works without a gyro beats
    /// a daemon that refused to start.
    pub fn bind(port: u16) -> io::Result<Motion> {
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let socket = UdpSocket::bind(address)?;
        socket.set_nonblocking(true)?;
        info!("motion server listening on {address} (DSU)");
        Ok(Motion {
            socket,
            clients: HashMap::new(),
            counters: [0; MAX_SLOTS],
            sent: [None; MAX_SLOTS],
        })
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }

    /// Whether anything is listening. Nothing is sent when nothing is.
    pub fn has_clients(&self) -> bool {
        !self.clients.is_empty()
    }

    /// Answer everything waiting on the socket.
    ///
    /// `ports` is what padmap would say about each slot right now, so a
    /// consumer that asks while a controller is being assigned gets the truth
    /// rather than a cached roster.
    pub fn serve(&mut self, ports: &[Port]) {
        // Bounded, like every other drain in the daemon: a peer that sends as
        // fast as we read must not hold the thread that forwards input.
        const MAX_PER_WAKE: usize = 32;
        let mut buffer = [0u8; dsu::MAX_PACKET_BYTES];
        for _ in 0..MAX_PER_WAKE {
            let (size, from) = match self.socket.recv_from(&mut buffer) {
                Ok(pair) => pair,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                // A datagram to a closed peer is reported on the *next* read
                // here, not on the write. Dropping the server for it would let
                // any process on the machine kill motion by sending one packet
                // and exiting.
                Err(error) => {
                    debug!("motion server: {error}");
                    break;
                }
            };
            let Some(incoming) = dsu::parse_request(&buffer[..size]) else {
                debug!("motion server: ignoring {size} bytes from {from}");
                continue;
            };
            self.answer(from, incoming, ports);
        }
    }

    fn answer(&mut self, from: SocketAddr, incoming: Incoming, ports: &[Port]) {
        match incoming.request {
            Request::Version => self.send(from, &dsu::version_reply(SERVER_ID)),
            Request::PortInfo { slots, count } => {
                for slot in &slots[..count] {
                    // A slot padmap does not have is answered as empty rather
                    // than ignored: a consumer that asked about four and heard
                    // about two waits for the rest.
                    let port = ports
                        .iter()
                        .find(|port| port.slot == *slot)
                        .copied()
                        .unwrap_or_else(|| Port::empty(*slot));
                    self.send(from, &dsu::port_reply(SERVER_ID, &port));
                }
            }
            Request::PadData(subscribe) => {
                let fresh = !self.clients.contains_key(&from);
                self.clients.insert(
                    from,
                    Client {
                        subscribe,
                        last_asked: Instant::now(),
                    },
                );
                if fresh {
                    info!("motion: {from} subscribed ({subscribe:?})");
                }
            }
        }
    }

    /// Send one sample per slot to everyone who asked for it.
    ///
    /// `pads` is indexed alongside `ports`. A slot whose sample has not
    /// advanced is skipped: a consumer integrates the gap between timestamps,
    /// and resending one it already has is either discarded (Cemu) or
    /// integrated twice.
    pub fn publish(&mut self, ports: &[Port], pads: &[Pad]) {
        self.expire();
        if self.clients.is_empty() {
            return;
        }
        for (port, pad) in ports.iter().zip(pads) {
            let slot = port.slot as usize;
            if slot >= MAX_SLOTS {
                continue;
            }
            if self.sent[slot] == Some(pad.motion.timestamp_us) && pad.motion.timestamp_us != 0 {
                continue;
            }
            let interested: Vec<SocketAddr> = self
                .clients
                .iter()
                .filter(|(_, client)| client.subscribe.covers(port))
                .map(|(address, _)| *address)
                .collect();
            if interested.is_empty() {
                continue;
            }
            self.counters[slot] = self.counters[slot].wrapping_add(1);
            let packet = dsu::pad_reply(SERVER_ID, port, self.counters[slot], pad);
            for address in interested {
                self.send(address, &packet);
            }
            self.sent[slot] = Some(pad.motion.timestamp_us);
        }
    }

    /// Forget anyone who has not asked lately.
    fn expire(&mut self) {
        let cutoff = Duration::from_secs(SUBSCRIPTION_SECONDS);
        let now = Instant::now();
        self.clients.retain(|address, client| {
            let alive = now.duration_since(client.last_asked) < cutoff;
            if !alive {
                info!("motion: {address} stopped asking");
            }
            alive
        });
    }

    fn send(&self, to: SocketAddr, packet: &[u8]) {
        match self.socket.send_to(packet, to) {
            Ok(_) => {}
            // Never fatal. A client that went away is the normal case, and a
            // full socket buffer means the client is not draining -- neither
            // is a reason to stop serving everybody else.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                debug!("motion: {to} is not draining, dropping a sample");
            }
            Err(error) => warn!("motion: could not answer {to}: {error}"),
        }
    }
}

/// What padmap says about a seated player's slot.
///
/// `has_motion` is the difference between a pad a consumer should offer a gyro
/// option for and one it should not.
pub fn port_for(player: u32, has_motion: bool) -> Port {
    Port {
        // Player one is slot zero. The off-by-one here binds player one's
        // motion to player two's character.
        slot: player.saturating_sub(1) as u8,
        connected: Connected::Yes,
        model: if has_motion {
            Model::FullGyro
        } else {
            Model::None
        },
        // padmap does not know, and the field is cosmetic -- a consumer draws
        // an icon with it. USB is the honest guess for a pad the daemon has
        // open right now.
        link: Link::Usb,
        mac: dsu::mac_for_player(player),
        battery: Battery::Full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_one_is_slot_zero() {
        assert_eq!(port_for(1, true).slot, 0);
        assert_eq!(port_for(4, true).slot, 3);
    }

    #[test]
    fn a_pad_without_a_gyro_says_so() {
        // A consumer offers a motion option for a FullGyro slot and not for a
        // None one, so claiming motion padmap cannot deliver is an option that
        // silently does nothing.
        assert_eq!(port_for(1, true).model, Model::FullGyro);
        assert_eq!(port_for(1, false).model, Model::None);
    }

    #[test]
    fn a_player_keeps_their_address_across_a_reconnect() {
        assert_eq!(port_for(2, true).mac, port_for(2, false).mac);
        assert_ne!(port_for(2, true).mac, port_for(3, true).mac);
    }
}
