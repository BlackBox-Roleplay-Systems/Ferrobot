//! Circuits to the regions around the bot's, where the region keeps a child agent for it, as the
//! viewer keeps them: EnableSimulator opens one (process_enable_simulator, llworld.cpp; Aurora's
//! session add_sim), DisableSimulator or silence closes it (Aurora's remove_sim). A crossing or a
//! teleport into one of them takes its circuit over instead of opening a new one, and the region
//! left behind is kept as a neighbour, as Aurora's switch_region does.
//!
//! Off by default: like LibreMetaverse's bots (AgentSettings.MultipleSims = false), a bot needs no
//! neighbours to work. With them on, the regions see the bot as they see a viewer, which is what a
//! test of crossings and child agents needs.
//!
//! A neighbour's circuit only stays up: acks, pings both ways, the handshake answered. What the
//! neighbour sends about its objects and avatars is acknowledged and left aside, and its seed and
//! event queue are not opened (Aurora opens them, for the objects it draws across the border).

use super::{CACHE_EMPTY, PING_INTERVAL, SEND_ALL_CACHEABLE, SUPPORTS_SELF_APPEARANCE};
use crate::circuit::{Circuit, Resend};
use crate::login::LoginResponse;
use ferro_msg::msgs::*;
use ferro_msg::{IncomingPacket, Msg, MsgId, field_str, parse_packet};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use uuid::Uuid;

/// What a region's RegionHandshake told about it, kept for when the bot moves in: the region
/// sends it once, when the circuit opens.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Handshake {
    pub name: String,
    pub id: Uuid,
    pub estate_manager: bool,
    pub product: String,
}

impl Handshake {
    pub(super) fn read(m: &RegionHandshake) -> Handshake {
        Handshake {
            name: field_str(&m.region_info.sim_name),
            id: m.region_info2.region_id,
            estate_manager: m.region_info.is_estate_manager,
            product: field_str(&m.region_info3.product_name),
        }
    }
}

/// The circuit to one neighbour region.
pub(super) struct Neighbour {
    pub circuit: Circuit,
    pub handle: u64,
    /// The UseCircuitCode that opened the circuit: given up on, the neighbour is dropped.
    pub ucc_seq: u32,
    pub handshake: Option<Handshake>,
    last_ping: Instant,
}

/// RegionHandshakeReply, as the main circuit sends it (LLViewerRegion::unpackRegionHandshake).
pub(super) fn handshake_reply(login: &LoginResponse) -> RegionHandshakeReply {
    let mut reply = RegionHandshakeReply::default();
    reply.agent_data.agent_id = login.agent_id;
    reply.agent_data.session_id = login.session_id;
    // send everything cacheable, the bot keeps no object cache, and it bakes its own appearance
    // (Aurora's object_cache.rs, LibreMetaverse's NetworkManager both send these)
    reply.region_info.flags = SEND_ALL_CACHEABLE | CACHE_EMPTY | SUPPORTS_SELF_APPEARANCE;
    reply
}

/// UseCircuitCode for the login's circuit code: the same one opens every region's circuit.
pub(super) fn use_circuit_code(login: &LoginResponse) -> UseCircuitCode {
    let mut m = UseCircuitCode::default();
    m.circuit_code.code = login.circuit_code;
    m.circuit_code.session_id = login.session_id;
    m.circuit_code.id = login.agent_id;
    m
}

/// The neighbours' circuits.
#[derive(Default)]
pub(super) struct Neighbours {
    /// EnableSimulator opens circuits; off, it is ignored.
    pub on: bool,
    map: HashMap<SocketAddr, Neighbour>,
}

impl Neighbours {
    pub(super) fn new(on: bool) -> Neighbours {
        Neighbours {
            on,
            map: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn contains(&self, addr: &SocketAddr) -> bool {
        self.map.contains_key(addr)
    }

    /// The regions and their names, as far as their handshakes came.
    pub(super) fn list(&self) -> Vec<(u64, String)> {
        let mut v: Vec<(u64, String)> = self
            .map
            .values()
            .map(|n| {
                (
                    n.handle,
                    n.handshake
                        .as_ref()
                        .map(|h| h.name.clone())
                        .unwrap_or_default(),
                )
            })
            .collect();
        v.sort();
        v
    }

    /// EnableSimulator: a circuit to the region, unless it is the bot's own (`main`) or one is
    /// open. True when one was opened.
    pub(super) fn enable(
        &mut self,
        sock: &UdpSocket,
        login: &LoginResponse,
        main: SocketAddr,
        addr: SocketAddr,
        handle: u64,
    ) -> bool {
        if !self.on {
            log::debug!("EnableSimulator for {addr} ignored: neighbours off");
            return false;
        }
        if addr == main {
            return false;
        }
        if let Some(n) = self.map.get_mut(&addr) {
            n.handle = handle;
            return false;
        }
        log::info!("neighbour region {handle:x} at {addr}: opening its circuit");
        let mut circuit = Circuit::new(addr);
        let ucc_seq = circuit.send_reliable_with(sock, &use_circuit_code(login), Resend::DEFAULT);
        self.map.insert(
            addr,
            Neighbour {
                circuit,
                handle,
                ucc_seq,
                handshake: None,
                last_ping: Instant::now(),
            },
        );
        true
    }

    /// The circuit the bot moves into, out of the neighbours.
    pub(super) fn take(&mut self, addr: &SocketAddr) -> Option<Neighbour> {
        self.map.remove(addr)
    }

    /// The circuit of the region the bot left, kept as a neighbour. False when neighbours are
    /// off: the caller lets it go.
    pub(super) fn keep(&mut self, circuit: Circuit, handle: u64, handshake: Handshake) -> bool {
        if !self.on {
            return false;
        }
        log::info!(
            "region {} kept as a neighbour",
            if handshake.name.is_empty() {
                format!("{handle:x}")
            } else {
                handshake.name.clone()
            }
        );
        let addr = circuit.addr;
        self.map.insert(
            addr,
            Neighbour {
                circuit,
                handle,
                ucc_seq: 0,
                handshake: Some(handshake),
                last_ping: Instant::now(),
            },
        );
        true
    }

    pub(super) fn close(&mut self, addr: &SocketAddr, why: &str) -> bool {
        match self.map.remove(addr) {
            Some(n) => {
                log::info!("neighbour region {:x} closed: {why}", n.handle);
                true
            }
            None => false,
        }
    }

    pub(super) fn close_all(&mut self, sock: &UdpSocket) {
        for n in self.map.values_mut() {
            n.circuit.flush_acks(sock);
        }
        self.map.clear();
    }

    /// A packet from a neighbour. True when the neighbours changed (opened, named or closed).
    pub(super) fn on_datagram(
        &mut self,
        sock: &UdpSocket,
        login: &LoginResponse,
        data: &[u8],
        from: SocketAddr,
    ) -> bool {
        let Some(n) = self.map.get_mut(&from) else {
            return false;
        };
        let pkt = match parse_packet(data) {
            Ok(pkt) => pkt,
            Err(e) => {
                log::debug!("bad packet from neighbour {from}: {e}");
                return false;
            }
        };
        if !n.circuit.on_receive(pkt.flags, pkt.sequence, &pkt.acks) {
            return false;
        }
        match dispatch(n, sock, login, &pkt) {
            Ok(closed) => {
                n.circuit.accept(pkt.flags, pkt.sequence);
                if closed {
                    n.circuit.flush_acks(sock);
                    self.close(&from, "DisableSimulator");
                    return true;
                }
                pkt.id == RegionHandshake::ID
            }
            Err(e) => {
                let name = ferro_msg::message_info(pkt.id).map_or("?", |i| i.name);
                log::warn!("failed to decode {name} from neighbour {from}: {e}");
                false
            }
        }
    }

    /// Resends, acks and pings; neighbours that never answered or went silent are dropped. True
    /// when one was.
    pub(super) fn tick(&mut self, sock: &UdpSocket, now: Instant, silence: Duration) -> bool {
        let mut gone = Vec::new();
        for (addr, n) in &mut self.map {
            n.circuit.tick(sock);
            if n.ucc_seq != 0 && n.circuit.take_dropped().contains(&n.ucc_seq) {
                gone.push((*addr, "it never acknowledged UseCircuitCode"));
                continue;
            }
            if now.duration_since(n.circuit.last_recv) > silence {
                gone.push((*addr, "nothing from it for too long"));
                continue;
            }
            if now.duration_since(n.last_ping) >= PING_INTERVAL {
                n.last_ping = now;
                let (id, oldest) = n.circuit.next_ping();
                let mut m = StartPingCheck::default();
                m.ping_id.ping_id = id;
                m.ping_id.oldest_unacked = oldest;
                n.circuit.send(sock, &m, false);
            }
        }
        for (addr, why) in &gone {
            self.close(addr, why);
        }
        !gone.is_empty()
    }
}

/// A neighbour's message. True when it closed the circuit (DisableSimulator).
fn dispatch(
    n: &mut Neighbour,
    sock: &UdpSocket,
    login: &LoginResponse,
    pkt: &IncomingPacket,
) -> Result<bool, ferro_msg::DecodeError> {
    let id: MsgId = pkt.id;
    if id == PacketAck::ID {
        let m: PacketAck = pkt.decode()?;
        n.circuit.ack_received(m.packets.iter().map(|p| p.id));
    } else if id == StartPingCheck::ID {
        let m: StartPingCheck = pkt.decode()?;
        let mut r = CompletePingCheck::default();
        r.ping_id.ping_id = m.ping_id.ping_id;
        n.circuit.send(sock, &r, false);
    } else if id == CompletePingCheck::ID {
        let m: CompletePingCheck = pkt.decode()?;
        n.circuit.on_ping_reply(m.ping_id.ping_id);
    } else if id == RegionHandshake::ID {
        let m: RegionHandshake = pkt.decode()?;
        let h = Handshake::read(&m);
        log::info!("neighbour region handshake: {}", h.name);
        n.handshake = Some(h);
        n.circuit.send(sock, &handshake_reply(login), true);
    } else if id == DisableSimulator::ID {
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login() -> LoginResponse {
        super::super::tests::login(9, String::new())
    }

    #[tokio::test]
    async fn off_ignores_enable_simulator() {
        let sock = UdpSocket::bind("127.0.0.1:0").await.expect("socket");
        let main: SocketAddr = "127.0.0.1:9".parse().expect("addr");
        let other: SocketAddr = "127.0.0.1:10".parse().expect("addr");
        let mut n = Neighbours::new(false);
        assert!(!n.enable(&sock, &login(), main, other, 5));
        assert!(!n.contains(&other));
        assert!(!n.keep(Circuit::new(other), 5, Handshake::default()));
    }

    #[tokio::test]
    async fn on_opens_one_circuit_per_region_never_the_main_one() {
        let sock = UdpSocket::bind("127.0.0.1:0").await.expect("socket");
        // a send before the reactor saw the socket is dropped (as in network::run)
        sock.writable().await.expect("writable");
        let region = UdpSocket::bind("127.0.0.1:0").await.expect("socket");
        let main: SocketAddr = "127.0.0.1:9".parse().expect("addr");
        let other = region.local_addr().expect("addr");
        let mut n = Neighbours::new(true);
        assert!(!n.enable(&sock, &login(), main, main, 4));
        assert!(n.enable(&sock, &login(), main, other, 5));
        assert!(!n.enable(&sock, &login(), main, other, 5));
        assert_eq!(n.list(), vec![(5, String::new())]);

        // the region got the UseCircuitCode, with the login's circuit code
        let mut buf = [0u8; 1500];
        let (len, _) = tokio::time::timeout(Duration::from_secs(5), region.recv_from(&mut buf))
            .await
            .expect("the UseCircuitCode in time")
            .expect("a packet");
        let pkt = parse_packet(&buf[..len]).expect("a packet");
        assert_eq!(pkt.id, UseCircuitCode::ID);
        let m: UseCircuitCode = pkt.decode().expect("UseCircuitCode");
        assert_eq!(m.circuit_code.code, 3);
        assert!(pkt.reliable());

        assert!(n.close(&other, "test"));
        assert!(!n.contains(&other));
    }
}
