//! The region: one UDP socket, a circuit per login. It acks what the bot sends reliably, answers
//! the connect sequence (UseCircuitCode then RegionHandshake, CompleteAgentMovement then
//! AgentMovementComplete), pings both ways and answers LogoutRequest, as a simulator does.

use crate::{RegionCommand, Seen, Shared};
use ferro_llsd::{Llsd, Map};
use ferro_msg::msgs::*;
use ferro_msg::{IncomingPacket, Msg, build_packet, field_str, parse_packet, str_field};
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
        } else if id == AgentDataUpdateRequest::ID {
            self.shared.saw(Seen::AgentDataUpdateRequest { session: n });
            self.agent_data(from, n);
        } else if id == UUIDNameRequest::ID {
            if let Ok(m) = pkt.decode::<UUIDNameRequest>() {
                let ids: Vec<Uuid> = m.uuid_name_block.iter().map(|b| b.id).collect();
                self.shared.saw(Seen::UuidNameRequest {
                    session: n,
                    ids: ids.clone(),
                });
                self.names(from, &ids);
            }
        } else if id == AvatarPickerRequest::ID {
            if let Ok(m) = pkt.decode::<AvatarPickerRequest>() {
                let name = field_str(&m.data.name);
                self.shared.saw(Seen::AvatarPickerRequest {
                    session: n,
                    name: name.clone(),
                });
                self.avatar_picker(from, m.agent_data.query_id, &name);
            }
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

    /// LLAgent's AgentDataUpdateRequest: AgentDataUpdate (the active group) on UDP, and the
    /// group list as AgentGroupDataUpdate on the event queue, as the grid sends them.
    fn agent_data(&mut self, to: SocketAddr, n: u32) {
        let cfg = &self.shared.cfg;
        let active = cfg
            .active_group
            .and_then(|id| cfg.groups.iter().find(|g| g.id == id));
        let mut m = AgentDataUpdate::default();
        m.agent_data.agent_id = self.shared.agent_id;
        m.agent_data.first_name = str_field(&cfg.first);
        m.agent_data.last_name = str_field(&cfg.last);
        if let Some(g) = active {
            m.agent_data.active_group_id = g.id;
            m.agent_data.group_name = str_field(&g.name);
            m.agent_data.group_title = str_field(&g.title);
            m.agent_data.group_powers = g.powers;
        }
        let groups: Vec<Llsd> = cfg
            .groups
            .iter()
            .map(|g| {
                let mut e = Map::new();
                e.insert("GroupID".into(), g.id.into());
                e.insert("GroupName".into(), g.name.clone().into());
                e.insert("GroupInsigniaID".into(), Uuid::nil().into());
                // U64 travels as 8 big-endian bytes in LLSD
                e.insert("GroupPowers".into(), g.powers.to_be_bytes().to_vec().into());
                e.insert("AcceptNotices".into(), true.into());
                e.insert("Contribution".into(), 0.into());
                Llsd::Map(e)
            })
            .collect();
        let mut body = Map::new();
        let mut agent = Map::new();
        agent.insert("AgentID".into(), self.shared.agent_id.into());
        body.insert("AgentData".into(), Llsd::Array(vec![Llsd::Map(agent)]));
        body.insert("GroupData".into(), Llsd::Array(groups));
        self.send(to, &m, true);
        self.shared
            .queue_event(n, "AgentGroupDataUpdate", Llsd::Map(body));
    }

    /// UUIDNameReply with the residents the grid knows; unknown ids get no answer.
    fn names(&mut self, to: SocketAddr, ids: &[Uuid]) {
        let mut agent = None;
        if ids.contains(&self.shared.agent_id) {
            agent = Some((self.shared.cfg.first.clone(), self.shared.cfg.last.clone()));
        }
        let blocks: Vec<uuid_name_reply::UUIDNameBlock> = self
            .shared
            .cfg
            .residents
            .iter()
            .filter(|r| ids.contains(&r.id))
            .map(|r| (r.id, r.first.clone(), r.last.clone()))
            .chain(agent.map(|(f, l)| (self.shared.agent_id, f, l)))
            .map(|(id, first, last)| uuid_name_reply::UUIDNameBlock {
                id,
                first_name: str_field(&first),
                last_name: str_field(&last),
            })
            .collect();
        if !blocks.is_empty() {
            self.send(
                to,
                &UUIDNameReply {
                    uuid_name_block: blocks,
                },
                true,
            );
        }
    }

    /// AvatarPickerReply: residents whose name holds every word searched for; one empty block
    /// when nobody matches, as the grid answers.
    fn avatar_picker(&mut self, to: SocketAddr, query: Uuid, name: &str) {
        let words: Vec<String> = name.split_whitespace().map(str::to_lowercase).collect();
        let mut data: Vec<avatar_picker_reply::Data> = self
            .shared
            .cfg
            .residents
            .iter()
            .filter(|r| {
                let full = format!("{} {}", r.first, r.last).to_lowercase();
                words.iter().all(|w| full.contains(w.as_str()))
            })
            .map(|r| avatar_picker_reply::Data {
                avatar_id: r.id,
                first_name: str_field(&r.first),
                last_name: str_field(&r.last),
            })
            .collect();
        if data.is_empty() {
            data.push(avatar_picker_reply::Data::default());
        }
        let mut m = AvatarPickerReply::default();
        m.agent_data.agent_id = self.shared.agent_id;
        m.agent_data.query_id = query;
        m.data = data;
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
