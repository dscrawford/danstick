//! padmap as a DSU server: motion on a stable port, by player number.

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

const SERVER_ID: u32 = 0x7061_646D; // "padm"

#[derive(Debug, Clone, Copy)]
struct Client {
    subscribe: Subscribe,
    last_asked: Instant,
    sent: [Option<Pad>; MAX_SLOTS],
}

#[derive(Debug)]
pub struct Motion {
    socket: UdpSocket,
    clients: HashMap<SocketAddr, Client>,
    counters: [u32; MAX_SLOTS],
}

impl Motion {
    pub fn bind(port: u16) -> io::Result<Motion> {
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let socket = UdpSocket::bind(address)?;
        socket.set_nonblocking(true)?;
        info!("motion server listening on {address} (DSU)");
        Ok(Motion {
            socket,
            clients: HashMap::new(),
            counters: [0; MAX_SLOTS],
        })
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }

    pub fn has_clients(&self) -> bool {
        !self.clients.is_empty()
    }

    pub fn serve(&mut self, ports: &[Port]) {
        self.serve_at(Instant::now(), ports);
    }

    pub fn serve_at(&mut self, now: Instant, ports: &[Port]) {
        // Bounded: fast peers must not starve the forwarding thread.
        const MAX_PER_WAKE: usize = 32;
        let mut buffer = [0u8; dsu::MAX_PACKET_BYTES];
        for _ in 0..MAX_PER_WAKE {
            let (size, from) = match self.socket.recv_from(&mut buffer) {
                Ok(pair) => pair,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    debug!("motion server: {error}");
                    break;
                }
            };
            let Some(incoming) = dsu::parse_request(&buffer[..size]) else {
                debug!("motion server: ignoring {size} bytes from {from}");
                continue;
            };
            self.answer(now, from, incoming, ports);
        }
    }

    fn answer(&mut self, now: Instant, from: SocketAddr, incoming: Incoming, ports: &[Port]) {
        match incoming.request {
            Request::Version => self.send(from, &dsu::version_reply(SERVER_ID)),
            Request::PortInfo { slots, count } => {
                for slot in &slots[..count] {
                    let port = ports
                        .iter()
                        .find(|port| port.slot == *slot)
                        .copied()
                        .unwrap_or_else(|| Port::empty(*slot));
                    self.send(from, &dsu::port_reply(SERVER_ID, &port));
                }
            }
            Request::PadData(subscribe) => {
                let sent = match self.clients.get(&from) {
                    Some(client) if client.subscribe == subscribe => client.sent,
                    Some(_) => [None; MAX_SLOTS],
                    None => {
                        info!("motion: {from} subscribed ({subscribe:?})");
                        [None; MAX_SLOTS]
                    }
                };
                self.clients.insert(
                    from,
                    Client {
                        subscribe,
                        last_asked: now,
                        sent,
                    },
                );
            }
        }
    }

    pub fn publish(&mut self, ports: &[Port], pads: &[Pad]) {
        self.publish_at(Instant::now(), ports, pads);
    }

    pub fn publish_at(&mut self, now: Instant, ports: &[Port], pads: &[Pad]) {
        self.expire(now);
        if self.clients.is_empty() {
            return;
        }
        for (port, pad) in ports.iter().zip(pads) {
            let slot = port.slot as usize;
            if slot >= MAX_SLOTS {
                continue;
            }
            let interested: Vec<SocketAddr> = self
                .clients
                .iter()
                .filter(|(_, client)| {
                    client.subscribe.covers(port) && client.sent[slot].as_ref() != Some(pad)
                })
                .map(|(address, _)| *address)
                .collect();
            if interested.is_empty() {
                continue;
            }
            self.counters[slot] = self.counters[slot].wrapping_add(1);
            let packet = dsu::pad_reply(SERVER_ID, port, self.counters[slot], pad);
            for address in interested {
                self.send(address, &packet);
                if let Some(client) = self.clients.get_mut(&address) {
                    client.sent[slot] = Some(*pad);
                }
            }
        }
    }

    /// Forget anyone who has not asked lately.
    fn expire(&mut self, now: Instant) {
        let cutoff = Duration::from_secs(SUBSCRIPTION_SECONDS);
        self.clients.retain(|address, client| {
            let alive = now.saturating_duration_since(client.last_asked) < cutoff;
            if !alive {
                info!("motion: {address} stopped asking");
            }
            alive
        });
    }

    fn send(&self, to: SocketAddr, packet: &[u8]) {
        match self.socket.send_to(packet, to) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                debug!("motion: {to} is not draining, dropping a sample");
            }
            Err(error) => warn!("motion: could not answer {to}: {error}"),
        }
    }
}

pub fn port_for(player: u32, has_motion: bool) -> Port {
    Port {
        slot: player.saturating_sub(1) as u8,
        connected: Connected::Yes,
        model: if has_motion {
            Model::FullGyro
        } else {
            Model::None
        },
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
        assert_eq!(port_for(1, true).model, Model::FullGyro);
        assert_eq!(port_for(1, false).model, Model::None);
    }

    #[test]
    fn a_player_keeps_their_address_across_a_reconnect() {
        assert_eq!(port_for(2, true).mac, port_for(2, false).mac);
        assert_ne!(port_for(2, true).mac, port_for(3, true).mac);
    }
}
