//! A region: one UDP socket, a circuit per login. It acks what the bot sends reliably, answers
//! the connect sequence (UseCircuitCode then RegionHandshake, CompleteAgentMovement then
//! AgentMovementComplete), pings both ways, answers LogoutRequest, and sends the agent to the
//! other region on a teleport (TeleportStart, then TeleportFinish on the event queue), as a
//! simulator does.

use crate::{FIRST_REGION, RegionCommand, SECOND_REGION, Seen, Shared};
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

/// TELEPORT_FLAGS_VIA_LURE, VIA_LANDMARK, VIA_HOME, VIA_LOCATION (lltelepostflags.h)
const VIA_LURE: u32 = 1 << 2;
const VIA_HOME: u32 = 1 << 4;
const VIA_LOCATION: u32 = 1 << 5;
pub const ARRIVAL: Vec3 = Vec3::new(128.0, 128.0, 25.0);

/// The circuit of one login.
struct Client {
    n: u32,
    session_id: Uuid,
    next_seq: u32,
}

struct Region {
    index: usize,
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
        // a UseCircuitCode always opens a fresh circuit: an agent that comes back after a
        // teleport finds its old one gone
        if pkt.id == UseCircuitCode::ID {
            self.clients.remove(&from);
            self.use_circuit_code(&pkt, from);
            return;
        }
        if !self.clients.contains_key(&from) {
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
            self.shared.saw(Seen::CompleteAgentMovement {
                session: n,
                region: self.index,
            });
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
        } else if id == TeleportLureRequest::ID {
            if let Ok(m) = pkt.decode::<TeleportLureRequest>() {
                let lure = m.info.lure_id;
                self.shared
                    .saw(Seen::TeleportLureRequest { session: n, lure });
                let known = self.shared.state().lures.remove(&lure);
                if known {
                    let to = self.other();
                    self.teleport(from, n, to, ARRIVAL, VIA_LURE);
                } else {
                    self.teleport_failed(n, "That teleport offer is no longer valid.");
                }
            }
        } else if id == TeleportLandmarkRequest::ID {
            if let Ok(m) = pkt.decode::<TeleportLandmarkRequest>() {
                self.shared.saw(Seen::TeleportLandmarkRequest {
                    session: n,
                    landmark: m.info.landmark_id,
                });
                if m.info.landmark_id.is_nil() {
                    // home: in the second region (GridConfig::home)
                    self.teleport(
                        from,
                        n,
                        SECOND_REGION,
                        Vec3::new(100.0, 110.0, 22.0),
                        VIA_HOME,
                    );
                } else {
                    self.teleport_failed(n, "The landmark could not be found.");
                }
            }
        } else if id == TeleportLocationRequest::ID {
            if let Ok(m) = pkt.decode::<TeleportLocationRequest>() {
                let handle = m.info.region_handle;
                self.shared.saw(Seen::TeleportLocationRequest {
                    session: n,
                    region_handle: handle,
                });
                match self
                    .shared
                    .regions
                    .iter()
                    .position(|r| r.handle() == handle)
                {
                    Some(to) => self.teleport(from, n, to, m.info.position, VIA_LOCATION),
                    None => self.teleport_failed(n, "Could not find that region."),
                }
            }
        } else if self.on_group_packet(&pkt, from, n) {
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
        // the login region of a new session, or the region a teleport sends the agent to
        let found = {
            let mut st = self.shared.state();
            let index = self.index;
            let s = st.sessions.values_mut().find(|s| {
                !s.ended
                    && s.circuit_code == c.code
                    && s.session_id == c.session_id
                    && c.id == self.shared.agent_id
                    && (s.expected == Some(index) || (s.expected.is_none() && s.region == index))
            });
            s.map(|s| {
                if s.expected == Some(index) {
                    s.region = index;
                    s.expected = None;
                    // the queue of the new region starts empty
                    s.events.clear();
                }
                (s.n, s.session_id)
            })
        };
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
        self.shared.saw(Seen::UseCircuitCode {
            session: n,
            region: self.index,
        });
        let ack = PacketAck {
            packets: vec![packet_ack::Packets { id: pkt.sequence }],
        };
        self.send(from, &ack, false);

        let mut h = RegionHandshake::default();
        h.region_info.sim_name = str_field(&self.shared.regions[self.index].name);
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
        let n = c.n;
        m.data.position = self
            .shared
            .state()
            .sessions
            .get(&n)
            .map_or(ARRIVAL, |s| s.arrival);
        m.data.look_at = Vec3::X;
        m.data.region_handle = self.shared.regions[self.index].handle();
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
        let body = {
            let st = self.shared.state();
            crate::groups::agent_group_data(&st.groups, self.shared.agent_id)
        };
        self.send(to, &m, true);
        self.shared.queue_event(n, "AgentGroupDataUpdate", body);
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

    /// The group messages; false when `pkt` is not one of them.
    fn on_group_packet(&mut self, pkt: &IncomingPacket, from: SocketAddr, n: u32) -> bool {
        use crate::groups as g;
        let bot = self.shared.agent_id;
        let group_of = |shared: &Shared, id: Uuid| {
            shared
                .state()
                .groups
                .iter()
                .find(|x| x.id == id && x.bot_member)
                .cloned()
        };
        let id = pkt.id;
        if id == GroupRoleDataRequest::ID {
            if let Ok(m) = pkt.decode::<GroupRoleDataRequest>() {
                let group = m.group_data.group_id;
                self.shared.saw(Seen::GroupRolesRequest { group });
                if let Some(grp) = group_of(&self.shared, group) {
                    for r in g::role_data_replies(&grp, bot, m.group_data.request_id) {
                        self.send(from, &r, true);
                    }
                }
            }
        } else if id == GroupMembersRequest::ID {
            if let Ok(m) = pkt.decode::<GroupMembersRequest>() {
                let group = m.group_data.group_id;
                self.shared
                    .saw(Seen::GroupMembersRequest { group, cap: false });
                if let Some(grp) = group_of(&self.shared, group) {
                    for r in g::members_replies(&grp, bot, m.group_data.request_id) {
                        self.send(from, &r, true);
                    }
                }
            }
        } else if id == GroupRoleMembersRequest::ID {
            if let Ok(m) = pkt.decode::<GroupRoleMembersRequest>() {
                let group = m.group_data.group_id;
                self.shared.saw(Seen::GroupRoleMembersRequest { group });
                if let Some(grp) = group_of(&self.shared, group) {
                    for r in g::role_members_replies(&grp, bot, m.group_data.request_id) {
                        self.send(from, &r, true);
                    }
                }
            }
        } else if id == GroupTitlesRequest::ID {
            if let Ok(m) = pkt.decode::<GroupTitlesRequest>() {
                let group = m.agent_data.group_id;
                self.shared.saw(Seen::GroupTitlesRequest { group });
                if let Some(grp) = group_of(&self.shared, group) {
                    let r = g::titles_reply(&grp, bot, m.agent_data.request_id);
                    self.send(from, &r, true);
                }
            }
        } else if id == InviteGroupRequest::ID {
            if let Ok(m) = pkt.decode::<InviteGroupRequest>() {
                for i in &m.invite_data {
                    self.shared.saw(Seen::GroupInvite {
                        group: m.group_data.group_id,
                        invitee: i.invitee_id,
                        role: i.role_id,
                    });
                }
            }
        } else if id == EjectGroupMemberRequest::ID {
            if let Ok(m) = pkt.decode::<EjectGroupMemberRequest>() {
                let group = m.group_data.group_id;
                let mut all = true;
                for e in &m.eject_data {
                    let done = self.eject(group, e.ejectee_id);
                    self.shared.saw(Seen::GroupEject {
                        group,
                        member: e.ejectee_id,
                        done,
                    });
                    all &= done;
                }
                let mut r = EjectGroupMemberReply::default();
                r.agent_data.agent_id = bot;
                r.group_data.group_id = group;
                r.eject_data.success = all;
                self.send(from, &r, true);
            }
        } else if id == GroupRoleChanges::ID {
            if let Ok(m) = pkt.decode::<GroupRoleChanges>() {
                let group = m.agent_data.group_id;
                for c in &m.role_change {
                    let add = c.change == 0;
                    self.change_role(group, c.role_id, c.member_id, add);
                    self.shared.saw(Seen::GroupRoleChange {
                        group,
                        role: c.role_id,
                        member: c.member_id,
                        add,
                    });
                }
            }
        } else if id == ImprovedInstantMessage::ID {
            if let Ok(m) = pkt.decode::<ImprovedInstantMessage>() {
                let b = &m.message_block;
                // send_join_group_response: an IM to the group
                if matches!(b.dialog, 35 | 36) {
                    let accept = b.dialog == 35;
                    let group = self.shared.state().group_invites.remove(&b.id);
                    if let Some(group) = group.filter(|g| *g == b.to_agent_id) {
                        self.shared.saw(Seen::GroupInviteAnswer { group, accept });
                        if accept {
                            self.join(group, n);
                        }
                    }
                }
            }
        } else {
            return false;
        }
        true
    }

    /// An ejection by the bot: it needs the power, and an owner cannot be ejected.
    fn eject(&mut self, group: Uuid, member: Uuid) -> bool {
        let mut st = self.shared.state();
        let Some(g) = st.groups.iter_mut().find(|g| g.id == group && g.bot_member) else {
            return false;
        };
        if g.powers & crate::groups::MEMBER_EJECT == 0 {
            return false;
        }
        match g.members.iter().position(|m| m.id == member && !m.owner) {
            Some(i) => {
                g.members.remove(i);
                true
            }
            None => false,
        }
    }

    /// A role given or taken: the bot needs the power, the member and the role must exist; the
    /// grid ignores the others silently.
    fn change_role(&mut self, group: Uuid, role: Uuid, member: Uuid, add: bool) {
        let mut st = self.shared.state();
        let Some(g) = st.groups.iter_mut().find(|g| g.id == group && g.bot_member) else {
            return;
        };
        let power = if add {
            crate::groups::ROLE_ASSIGN_MEMBER
        } else {
            crate::groups::ROLE_REMOVE_MEMBER
        };
        if g.powers & power == 0 || role.is_nil() || !g.roles.iter().any(|r| r.id == role) {
            return;
        }
        if let Some(m) = g.members.iter_mut().find(|m| m.id == member) {
            if add && !m.roles.contains(&role) {
                m.roles.push(role);
            } else if !add {
                m.roles.retain(|r| *r != role);
            }
        }
    }

    /// The bot joins a group it accepted: the group list goes out again.
    fn join(&mut self, group: Uuid, n: u32) {
        let body = {
            let mut st = self.shared.state();
            if let Some(g) = st.groups.iter_mut().find(|g| g.id == group) {
                g.bot_member = true;
            }
            crate::groups::agent_group_data(&st.groups, self.shared.agent_id)
        };
        self.shared.queue_event(n, "AgentGroupDataUpdate", body);
    }

    fn other(&self) -> usize {
        if self.index == FIRST_REGION {
            SECOND_REGION
        } else {
            FIRST_REGION
        }
    }

    /// The agent goes to region `to`: in this region, TeleportLocal; elsewhere TeleportStart,
    /// then TeleportFinish on the event queue with the region's address and seed, as the grid
    /// sends them (LLAgent::processTeleportFinish reads them). Fails if a test asked for it.
    fn teleport(&mut self, from: SocketAddr, n: u32, to: usize, position: Vec3, flags: u32) {
        let failure = self.shared.state().fail_teleport.take();
        if let Some(reason) = failure {
            self.teleport_failed(n, &reason);
            return;
        }
        if to == self.index {
            let mut m = TeleportLocal::default();
            m.info.agent_id = self.shared.agent_id;
            m.info.position = position;
            m.info.look_at = Vec3::X;
            m.info.teleport_flags = flags;
            self.send(from, &m, true);
            return;
        }
        let mut start = TeleportStart::default();
        start.info.teleport_flags = flags;
        self.send(from, &start, true);
        if let Some(s) = self.shared.state().sessions.get_mut(&n) {
            s.expected = Some(to);
            s.arrival = position;
        }
        let region = &self.shared.regions[to];
        let ip = match region.addr.ip() {
            std::net::IpAddr::V4(ip) => ip.octets().to_vec(),
            std::net::IpAddr::V6(_) => vec![127, 0, 0, 1],
        };
        let mut info = Map::new();
        info.insert("AgentID".into(), self.shared.agent_id.into());
        info.insert("LocationID".into(), 4.into());
        info.insert("SimIP".into(), ip.into());
        info.insert("SimPort".into(), i32::from(region.addr.port()).into());
        info.insert(
            "RegionHandle".into(),
            region.handle().to_be_bytes().to_vec().into(),
        );
        info.insert("SeedCapability".into(), self.shared.seed_url(n, to).into());
        info.insert("SimAccess".into(), 13.into());
        info.insert("TeleportFlags".into(), (flags as i32).into());
        info.insert("RegionSizeX".into(), 256.into());
        info.insert("RegionSizeY".into(), 256.into());
        let mut body = Map::new();
        body.insert("Info".into(), Llsd::Array(vec![Llsd::Map(info)]));
        self.shared
            .queue_event(n, "TeleportFinish", Llsd::Map(body));
    }

    /// TeleportFailed on the event queue, as the grid sends it.
    fn teleport_failed(&mut self, n: u32, reason: &str) {
        let mut info = Map::new();
        info.insert("AgentID".into(), self.shared.agent_id.into());
        info.insert("Reason".into(), reason.into());
        let mut body = Map::new();
        body.insert("Info".into(), Llsd::Array(vec![Llsd::Map(info)]));
        self.shared
            .queue_event(n, "TeleportFailed", Llsd::Map(body));
    }

    /// Circuits of agents that went to the other region are closed here.
    fn forget_departed(&mut self) {
        let here: Vec<SocketAddr> = {
            let st = self.shared.state();
            self.clients
                .iter()
                .filter(|(_, c)| {
                    st.sessions
                        .get(&c.n)
                        .is_some_and(|s| s.region != self.index)
                })
                .map(|(addr, _)| *addr)
                .collect()
        };
        for addr in here {
            self.clients.remove(&addr);
        }
    }

    fn ping_all(&mut self, ping_id: u8) {
        self.forget_departed();
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
            RegionCommand::OfferTeleport { from, lure } => {
                let mut m = ImprovedInstantMessage::default();
                m.agent_data.agent_id = from.id;
                let b = &mut m.message_block;
                b.to_agent_id = self.shared.agent_id;
                b.dialog = 22; // IM_LURE_USER
                b.id = lure;
                b.from_agent_name = str_field(&format!("{} {}", from.first, from.last));
                b.message = str_field("Join me in Second Region");
                self.send(to, &m, true);
            }
            RegionCommand::GroupInvitation {
                from,
                group,
                transaction,
                fee,
            } => {
                let mut m = ImprovedInstantMessage::default();
                // the invitation comes from the group (process_improved_im, IM_GROUP_INVITATION)
                m.agent_data.agent_id = group;
                let b = &mut m.message_block;
                b.to_agent_id = self.shared.agent_id;
                b.dialog = 3; // IM_GROUP_INVITATION
                b.id = transaction;
                b.from_agent_name = str_field(&format!("{} {}", from.first, from.last));
                b.message = str_field("You have been invited to join a group.");
                // invite_bucket_t: the fee (big-endian S32) and the role
                let mut bucket = fee.to_be_bytes().to_vec();
                bucket.extend_from_slice(Uuid::nil().as_bytes());
                b.binary_bucket = bucket;
                self.send(to, &m, true);
            }
            RegionCommand::Kick { reason } => {
                let mut m = KickUser::default();
                m.user_info.agent_id = self.shared.agent_id;
                m.user_info.session_id = session_id;
                // padded as OpenSim pads it (NGC's KickUser)
                m.user_info.reason = str_field(&format!("\n    {reason}\n    "));
                self.send(to, &m, false);
                self.drop_client(to);
            }
            RegionCommand::RestartNotice { seconds } => {
                let name = &self.shared.regions[self.index].name;
                let (notice, extra) = if seconds >= 60 && seconds % 60 == 0 {
                    (
                        "RegionRestartMinutes",
                        format!("{{'MINUTES':i{},'NAME':'{name}'}}", seconds / 60),
                    )
                } else {
                    (
                        "RegionRestartSeconds",
                        format!("{{'SECONDS':i{seconds},'NAME':'{name}'}}"),
                    )
                };
                let mut m = AlertMessage::default();
                m.alert_data.message = str_field("The region you are in now is about to restart.");
                m.alert_info.push(alert_message::AlertInfo {
                    message: str_field(notice),
                    extra_params: str_field(&extra),
                });
                self.send(to, &m, true);
            }
            RegionCommand::DisableSimulator => {
                self.send(to, &DisableSimulator::default(), false);
                self.drop_client(to);
            }
        }
    }
}

pub async fn run(
    index: usize,
    sock: UdpSocket,
    shared: Arc<Shared>,
    mut commands: mpsc::UnboundedReceiver<RegionCommand>,
) {
    let mut ping = tokio::time::interval(shared.cfg.ping_interval);
    let mut r = Region {
        index,
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
