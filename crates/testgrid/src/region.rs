//! The region: one UDP socket, a circuit per login. It acks what the bot sends reliably, answers
//! the connect sequence (UseCircuitCode then RegionHandshake, CompleteAgentMovement then
//! AgentMovementComplete), pings both ways and answers LogoutRequest, as a simulator does.

use crate::{RegionCommand, Seen, Shared};
use ferro_msg::msgs::*;
use ferro_msg::{IncomingPacket, Msg, build_packet, parse_packet, str_field};
use glam::Vec3;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Where the region sits on the grid, in metres (the login reply's region_x / region_y).
pub const REGION_X: u32 = 1000 * 256;
pub const REGION_Y: u32 = 1000 * 256;
const ARRIVAL: Vec3 = Vec3::new(128.0, 128.0, 25.0);

/// The circuit of one login.
struct Client {
    n: u32,
    session_id: Uuid,
    next_seq: u32,
}

struct Region {
    sock: UdpSocket,
    shared: Arc<Shared>,
    clients: HashMap<SocketAddr, Client>,
    buf: Vec<u8>,
}

impl Region {
    fn send<M: Msg>(&mut self, to: SocketAddr, msg: &M, reliable: bool) {
        let Some(c) = self.clients.get_mut(&to) else {
            return;
        };
        let seq = c.next_seq;
        c.next_seq += 1;
        build_packet(msg, seq, reliable, false, &[], &mut self.buf);
        if let Err(e) = self.sock.try_send_to(&self.buf, to) {
            log::debug!("testgrid region send: {e}");
        }
    }

    /// The circuit of the latest login still on the region.
    fn latest(&self) -> Option<SocketAddr> {
        self.clients
            .iter()
            .max_by_key(|(_, c)| c.n)
            .map(|(addr, _)| *addr)
    }

    fn drop_client(&mut self, addr: SocketAddr) {
        if let Some(c) = self.clients.remove(&addr) {
            self.shared.end_session(c.n);
        }
    }

    fn on_datagram(&mut self, data: &[u8], from: SocketAddr) {
        let Ok(pkt) = parse_packet(data) else {
            log::debug!("testgrid region: bad packet from {from}");
            return;
        };
        if !self.clients.contains_key(&from) {
            if pkt.id == UseCircuitCode::ID {
                self.use_circuit_code(&pkt, from);
            }
            return;
        }
        if pkt.reliable() {
            let ack = PacketAck {
                packets: vec![packet_ack::Packets { id: pkt.sequence }],
            };
            self.send(from, &ack, false);
        }
        let n = self.clients[&from].n;
        let id = pkt.id;
        if id == StartPingCheck::ID {
            if let Ok(m) = pkt.decode::<StartPingCheck>() {
                let mut r = CompletePingCheck::default();
                r.ping_id.ping_id = m.ping_id.ping_id;
                self.send(from, &r, false);
            }
        } else if id == CompletePingCheck::ID {
            self.shared.saw(Seen::CompletePingCheck { session: n });
        } else if id == CompleteAgentMovement::ID {
            self.shared.saw(Seen::CompleteAgentMovement { session: n });
            self.agent_movement_complete(from);
        } else if id == AgentThrottle::ID {
            self.shared.saw(Seen::AgentThrottle { session: n });
        } else if id == RegionHandshakeReply::ID {
            if let Ok(m) = pkt.decode::<RegionHandshakeReply>() {
                self.shared.saw(Seen::RegionHandshakeReply {
                    session: n,
                    flags: m.region_info.flags,
                });
            }
        } else if id == AgentUpdate::ID {
            self.shared.saw(Seen::AgentUpdate { session: n });
        } else if id == LogoutRequest::ID {
            self.shared.saw(Seen::LogoutRequest { session: n });
            let mut r = LogoutReply::default();
            r.agent_data.agent_id = self.shared.agent_id;
            r.agent_data.session_id = self.clients[&from].session_id;
            self.send(from, &r, true);
            self.drop_client(from);
        }
    }

    /// A circuit opens only for the circuit code and session of a login that is still on.
    fn use_circuit_code(&mut self, pkt: &IncomingPacket, from: SocketAddr) {
        let Ok(m) = pkt.decode::<UseCircuitCode>() else {
            return;
        };
        let c = &m.circuit_code;
        let found = self
            .shared
            .state()
            .sessions
            .values()
            .find(|s| {
                !s.ended
                    && s.circuit_code == c.code
                    && s.session_id == c.session_id
                    && c.id == self.shared.agent_id
            })
            .map(|s| (s.n, s.session_id));
        let Some((n, session_id)) = found else {
            log::debug!("testgrid region: UseCircuitCode for no session");
            return;
        };
        self.clients.insert(
            from,
            Client {
                n,
                session_id,
                next_seq: 1,
            },
        );
        self.shared.saw(Seen::UseCircuitCode { session: n });
        let ack = PacketAck {
            packets: vec![packet_ack::Packets { id: pkt.sequence }],
        };
        self.send(from, &ack, false);

        let mut h = RegionHandshake::default();
        h.region_info.sim_name = str_field(&self.shared.cfg.region_name);
        h.region_info.water_height = 20.0;
        h.region_info2.region_id = Uuid::new_v4();
        self.send(from, &h, true);
    }

    fn agent_movement_complete(&mut self, to: SocketAddr) {
        let Some(c) = self.clients.get(&to) else {
            return;
        };
        let mut m = AgentMovementComplete::default();
        m.agent_data.agent_id = self.shared.agent_id;
        m.agent_data.session_id = c.session_id;
        m.data.position = ARRIVAL;
        m.data.look_at = Vec3::X;
        m.data.region_handle = (u64::from(REGION_X) << 32) | u64::from(REGION_Y);
        m.sim_data.channel_version = str_field("Ferrobot Test Grid 1.0");
        self.send(to, &m, true);
    }

    fn ping_all(&mut self, ping_id: u8) {
        let addrs: Vec<SocketAddr> = self.clients.keys().copied().collect();
        for addr in addrs {
            let mut m = StartPingCheck::default();
            m.ping_id.ping_id = ping_id;
            self.send(addr, &m, false);
        }
    }

    fn on_command(&mut self, cmd: RegionCommand) {
        let Some(to) = self.latest() else {
            log::warn!("testgrid region: {cmd:?} with nobody on the region");
            return;
        };
        let session_id = self.clients[&to].session_id;
        match cmd {
            RegionCommand::Say { from, text } => {
                let mut m = ChatFromSimulator::default();
                let c = &mut m.chat_data;
                c.from_name = str_field(&from);
                c.source_id = Uuid::new_v4();
                c.owner_id = c.source_id;
                c.source_type = 1; // an avatar
                c.chat_type = 1; // say
                c.audible = 1;
                c.position = ARRIVAL + Vec3::X;
                c.message = str_field(&text);
                self.send(to, &m, false);
            }
            RegionCommand::Im { from, text } => {
                let mut m = ImprovedInstantMessage::default();
                m.agent_data.agent_id = Uuid::new_v4();
                m.agent_data.session_id = Uuid::nil();
                let b = &mut m.message_block;
                b.to_agent_id = self.shared.agent_id;
                b.dialog = 0;
                b.id = Uuid::new_v4();
                b.from_agent_name = str_field(&from);
                b.message = str_field(&text);
                self.send(to, &m, true);
            }
            RegionCommand::Kick { reason } => {
                let mut m = KickUser::default();
                m.user_info.agent_id = self.shared.agent_id;
                m.user_info.session_id = session_id;
                m.user_info.reason = str_field(&reason);
                self.send(to, &m, false);
                self.drop_client(to);
            }
            RegionCommand::DisableSimulator => {
                self.send(to, &DisableSimulator::default(), false);
                self.drop_client(to);
            }
        }
    }
}

pub async fn run(
    sock: UdpSocket,
    shared: Arc<Shared>,
    mut commands: mpsc::UnboundedReceiver<RegionCommand>,
) {
    let mut ping = tokio::time::interval(shared.cfg.ping_interval);
    let mut r = Region {
        sock,
        shared,
        clients: HashMap::new(),
        buf: Vec::with_capacity(1500),
    };
    let mut data = vec![0u8; 8192];
    let mut ping_id = 0u8;
    loop {
        tokio::select! {
            got = r.sock.recv_from(&mut data) => match got {
                Ok((n, from)) => r.on_datagram(&data[..n], from),
                // a bot that went away makes the next receive fail on some systems: go on
                Err(e) => log::debug!("testgrid region receive: {e}"),
            },
            _ = ping.tick() => {
                ping_id = ping_id.wrapping_add(1);
                r.ping_all(ping_id);
            }
            cmd = commands.recv() => match cmd {
                Some(cmd) => r.on_command(cmd),
                None => return,
            },
        }
    }
}
