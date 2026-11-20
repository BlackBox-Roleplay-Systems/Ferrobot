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
//! neighbour sends about its objects and avatars is acknowledged and left aside. Its seed and
//! event queue are opened when the region gives them (EstablishAgentCommunication on the main
//! region's queue, LLViewerRegion::setSeedCapability), and the region left keeps the ones it had,
//! as the viewer polls every region's queue: what comes on them is drained and left aside.

use super::{CACHE_EMPTY, PING_INTERVAL, SEND_ALL_CACHEABLE, SUPPORTS_SELF_APPEARANCE};
use crate::caps::{self, EqMessage};
use crate::circuit::{Circuit, Resend};
use crate::login::LoginResponse;
use ferro_msg::msgs::*;
use ferro_msg::{IncomingPacket, Msg, MsgId, field_str, parse_packet};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// What a region's RegionHandshake told about it, kept for when the bot moves in: the region
/// sends it once, when the circuit opens.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Handshake {
    pub name: String,
    pub id: Uuid,
    pub estate_manager: bool,
    pub product: String,
    /// RegionProtocols bit 0: the region bakes this avatar server-side.
    pub server_bakes: bool,
}

impl Handshake {
    pub(super) fn read(m: &RegionHandshake) -> Handshake {
        Handshake {
            name: field_str(&m.region_info.sim_name),
            id: m.region_info2.region_id,
            estate_manager: m.region_info.is_estate_manager,
            product: field_str(&m.region_info3.product_name),
            server_bakes: m
                .region_info4
                .first()
                .is_some_and(|r| r.region_protocols & 1 != 0),
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
    /// Its event queue, polled while it is a neighbour.
    queue: Option<Queue>,
}

/// A neighbour's seed and event queue being polled; dropped, the polling stops (the bot moves in
/// and polls it as the main region's, or the neighbour is gone).
pub(super) struct Queue(JoinHandle<()>);

impl Drop for Queue {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Queue {
    /// Ask the seed for the event queue, then poll it, logging what comes.
    pub(super) fn start(
        http: reqwest::Client,
        seed: String,
        name: String,
        max_errors: u32,
    ) -> Queue {
        Queue(tokio::spawn(async move {
            let url = match caps::resolve_seed(&http, &seed).await {
                Ok(caps) => match caps.get("EventQueueGet") {
                    Some(url) => url.clone(),
                    None => {
                        log::info!("neighbour {name}: no event queue");
                        return;
                    }
                },
                Err(e) => {
                    log::info!("neighbour {name}: seed capability failed: {e}");
                    return;
                }
            };
            log::info!("neighbour {name}: event queue open");
            let (tx, mut rx) = mpsc::unbounded_channel();
            let drain = async {
                while let Some(m) = rx.recv().await {
                    match m {
                        EqMessage::Event(e) => {
                            log::debug!("neighbour {name}: event {}", e.message)
                        }
                        EqMessage::Lost => {
                            log::info!("neighbour {name}: event queue lost");
                            return;
                        }
                        EqMessage::Caps(_) => {}
                    }
                }
            };
            tokio::select! {
                _ = caps::event_queue_loop(http.clone(), url, tx, max_errors) => {}
                _ = drain => {}
            }
        }))
    }
}

/// What a region's packet told, beyond the circuit's own business.
#[derive(Debug, PartialEq)]
pub(super) enum Heard {
    Nothing,
    /// Its RegionHandshake: it has a name now.
    Handshake,
    /// DisableSimulator: it closed the circuit.
    Closed,
    /// TeleportFailed: the region the bot left took the agent back (OpenSim's
    /// EntityTransferModule.Fail, after TeleportFinish).
    TeleportFailed(String),
    /// An instant message: a region may hand one to the circuit of a neighbour the bot is a child
    /// agent of (OpenSim's MessageTransferModule does), and a viewer reads it from any circuit.
    InstantMessage(Box<ImprovedInstantMessage>),
}

impl Neighbour {
    /// The circuit of a region the bot was in: it is up already, its handshake known.
    pub(super) fn kept(circuit: Circuit, ucc_seq: u32, handle: u64, handshake: Handshake) -> Self {
        Neighbour {
            circuit,
            handle,
            ucc_seq,
            handshake: Some(handshake),
            last_ping: Instant::now(),
            queue: None,
        }
    }

    /// Poll its event queue from now on (the one it had stops).
    pub(super) fn poll(&mut self, queue: Queue) {
        self.queue = Some(queue);
    }

    /// Whether its event queue is polled.
    pub(super) fn polled(&self) -> bool {
        self.queue.as_ref().is_some_and(|q| !q.0.is_finished())
    }

    /// The region's name, or its handle while its handshake has not come.
    pub(super) fn name(&self) -> String {
        match &self.handshake {
            Some(h) if !h.name.is_empty() => h.name.clone(),
            _ => format!("{:x}", self.handle),
        }
    }

    /// A packet from this region: acked, and answered when it asks.
    pub(super) fn on_datagram(
        &mut self,
        sock: &UdpSocket,
        login: &LoginResponse,
        data: &[u8],
    ) -> Heard {
        let from = self.circuit.addr;
        let pkt = match parse_packet(data) {
            Ok(pkt) => pkt,
            Err(e) => {
                log::debug!("bad packet from {from}: {e}");
                return Heard::Nothing;
            }
        };
        if !self.circuit.on_receive(pkt.flags, pkt.sequence, &pkt.acks) {
            return Heard::Nothing;
        }
        match dispatch(self, sock, login, &pkt) {
            Ok(heard) => {
                self.circuit.accept(pkt.flags, pkt.sequence);
                if heard == Heard::Closed {
                    self.circuit.flush_acks(sock);
                }
                heard
            }
            Err(e) => {
                let name = ferro_msg::message_info(pkt.id).map_or("?", |i| i.name);
                log::warn!("failed to decode {name} from {from}: {e}");
                Heard::Nothing
            }
        }
    }

    /// Resends and pings. Why the circuit is to be given up, when it is.
    pub(super) fn tick(
        &mut self,
        sock: &UdpSocket,
        now: Instant,
        silence: Duration,
    ) -> Option<&'static str> {
        self.circuit.tick(sock);
        if self.ucc_seq != 0 && self.circuit.take_dropped().contains(&self.ucc_seq) {
            return Some("it never acknowledged UseCircuitCode");
        }
        if now.duration_since(self.circuit.last_recv) > silence {
            return Some("nothing from it for too long");
        }
        if now.duration_since(self.last_ping) >= PING_INTERVAL {
            self.last_ping = now;
            let (id, oldest) = self.circuit.next_ping();
            let mut m = StartPingCheck::default();
            m.ping_id.ping_id = id;
            m.ping_id.oldest_unacked = oldest;
            self.circuit.send(sock, &m, false);
        }
        None
    }
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
                queue: None,
            },
        );
        true
    }

    /// The circuit the bot moves into, out of the neighbours.
    pub(super) fn get_mut(&mut self, addr: &SocketAddr) -> Option<&mut Neighbour> {
        self.map.get_mut(addr)
    }

    pub(super) fn take(&mut self, addr: &SocketAddr) -> Option<Neighbour> {
        self.map.remove(addr)
    }

    /// The circuit of the region the bot left, kept as a neighbour. False when neighbours are
    /// off: the caller lets it go.
    pub(super) fn keep(&mut self, n: Neighbour) -> bool {
        if !self.on {
            return false;
        }
        log::info!("region {} kept as a neighbour", n.name());
        self.map.insert(n.circuit.addr, n);
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

    /// A packet from a neighbour: whether the neighbours changed (opened, named or closed), and an
    /// instant message it carried.
    pub(super) fn on_datagram(
        &mut self,
        sock: &UdpSocket,
        login: &LoginResponse,
        data: &[u8],
        from: SocketAddr,
    ) -> (bool, Option<Box<ImprovedInstantMessage>>) {
        let Some(n) = self.map.get_mut(&from) else {
            return (false, None);
        };
        match n.on_datagram(sock, login, data) {
            Heard::Closed => (self.close(&from, "DisableSimulator"), None),
            Heard::Handshake => (true, None),
            Heard::InstantMessage(m) => (false, Some(m)),
            // a teleport long over, or one this bot did not make
            Heard::Nothing | Heard::TeleportFailed(_) => (false, None),
        }
    }

    /// Resends, acks and pings; neighbours that never answered or went silent are dropped. True
    /// when one was.
    pub(super) fn tick(&mut self, sock: &UdpSocket, now: Instant, silence: Duration) -> bool {
        let gone: Vec<(SocketAddr, &str)> = self
            .map
            .iter_mut()
            .filter_map(|(addr, n)| n.tick(sock, now, silence).map(|why| (*addr, why)))
            .collect();
        for (addr, why) in &gone {
            self.close(addr, why);
        }
        !gone.is_empty()
    }
}

/// A neighbour's message.
fn dispatch(
    n: &mut Neighbour,
    sock: &UdpSocket,
    login: &LoginResponse,
    pkt: &IncomingPacket,
) -> Result<Heard, ferro_msg::DecodeError> {
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
        return Ok(Heard::Handshake);
    } else if id == DisableSimulator::ID {
        return Ok(Heard::Closed);
    } else if id == TeleportFailed::ID {
        let m: TeleportFailed = pkt.decode()?;
        return Ok(Heard::TeleportFailed(field_str(&m.info.reason)));
    } else if id == ImprovedInstantMessage::ID {
        let m: ImprovedInstantMessage = pkt.decode()?;
        return Ok(Heard::InstantMessage(Box::new(m)));
    }
    Ok(Heard::Nothing)
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
        assert!(!n.keep(Neighbour::kept(
            Circuit::new(other),
            0,
            5,
            Handshake::default()
        )));
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

    #[tokio::test]
    async fn a_neighbours_instant_message_is_passed_on() {
        // OpenSim's transfer module hands a message to the circuit of a region the bot is a
        // child agent of: a viewer reads it from any circuit, and so does the bot
        let sock = UdpSocket::bind("127.0.0.1:0").await.expect("socket");
        let other: SocketAddr = "127.0.0.1:11".parse().expect("addr");
        let mut n = Neighbours::new(true);
        assert!(n.keep(Neighbour::kept(
            Circuit::new(other),
            0,
            6,
            Handshake::default()
        )));

        let mut m = ImprovedInstantMessage::default();
        m.message_block.dialog = 3;
        m.message_block.id = uuid::Uuid::from_u128(7);
        let mut data = Vec::new();
        ferro_msg::build_packet(&m, 1, true, false, &[], &mut data);

        let (changed, im) = n.on_datagram(&sock, &login(), &data, other);
        assert!(!changed);
        let im = im.expect("the instant message");
        assert_eq!(im.message_block.dialog, 3);
        assert_eq!(im.message_block.id, uuid::Uuid::from_u128(7));
    }
}
