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
    /// Packets to send, in order.
    outbox: Vec<(Vec<u8>, SocketAddr)>,
    /// The way each bot faces, as its agent updates say.
    facing: HashMap<SocketAddr, i32>,
    /// The control flags each bot last sent.
    controls: HashMap<SocketAddr, u32>,
    /// The animations each bot plays.
    playing: HashMap<SocketAddr, Vec<Uuid>>,
    /// What the bot wears: its objects' local ids by the item they were worn from.
    attached: HashMap<Uuid, u32>,
    /// Where each bot's avatar stands, as the object updates told it.
    avatar_at: HashMap<SocketAddr, Vec3>,
}

impl Region {
    fn send<M: Msg>(&mut self, to: SocketAddr, msg: &M, reliable: bool) {
        let Some(c) = self.clients.get_mut(&to) else {
            return;
        };
        if self.shared.silent(c.n) {
            return;
        }
        let seq = c.next_seq;
        c.next_seq += 1;
        build_packet(msg, seq, reliable, false, &[], &mut self.buf);
        // queued and sent by the loop, awaited: a busy socket must not lose a packet, as the
        // region never resends one
        self.outbox.push((self.buf.clone(), to));
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
        if self.shared.silent(n) {
            return;
        }
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
            if let Ok(m) = pkt.decode::<AgentUpdate>() {
                let a = &m.agent_data;
                if self.controls.insert(from, a.control_flags).unwrap_or(0) != a.control_flags {
                    self.shared.saw(Seen::Controls {
                        flags: a.control_flags,
                    });
                    // AGENT_CONTROL_STAND_UP: off the seat, next to it
                    if a.control_flags & 0x1_0000 != 0 {
                        self.avatar_update(from, SEAT_POSITION + Vec3::X, 0);
                    }
                }
                let q = a.body_rotation;
                let yaw = 2.0 * q.z.atan2(q.w);
                let heading = (90.0 - yaw.to_degrees()).rem_euclid(360.0).round() as i32 % 360;
                if self.facing.insert(from, heading) != Some(heading) {
                    self.shared.saw(Seen::Facing { heading });
                }
            }
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
        h.region_info3.product_name = str_field("Estate / Full Region");
        // the bot manages the first region's estate, not the second's
        h.region_info.is_estate_manager = self.index == 0;
        h.region_info2.region_id = Uuid::from_u128(0x2e61_0000 + self.index as u128);
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
        let at = m.data.position;
        self.send(to, &m, true);
        self.avatar_update(to, at, 0);
        // the parcel the agent arrives on
        let body = self
            .shared
            .state()
            .parcel
            .properties(0, self.shared.agent_id);
        self.shared.queue_event(n, "ParcelProperties", body);
        // the hat, worn on the skull
        self.attachment(to, crate::inventory::HAT, 2);
        // a linkset of two prims on the parcel, owned by a resident whom the updates do not name
        // (the region names the owner only of an object with a sound): ObjectProperties does
        let root = object_block(OBJECT_LOCAL_ID, OBJECT, 0, LL_PCODE_VOLUME, OBJECT_POSITION);
        let mut child = object_block(
            OBJECT_LOCAL_ID + 1,
            OBJECT_CHILD,
            OBJECT_LOCAL_ID,
            LL_PCODE_VOLUME,
            Vec3::Z,
        );
        // the child is sculpted (PARAMS_SCULPT: its texture, LL_SCULPT_TYPE_SPHERE)
        let mut sculpt = vec![1u8];
        sculpt.extend(0x30u16.to_le_bytes());
        sculpt.extend(17u32.to_le_bytes());
        sculpt.extend(Uuid::from_u128(0x5c01_7e00).as_bytes());
        sculpt.push(1);
        child.extra_params = sculpt;
        let m = ObjectUpdate {
            region_data: object_update::RegionData {
                region_handle: self.shared.regions[self.index].handle(),
                time_dilation: u16::MAX,
            },
            object_data: vec![root, child],
        };
        self.send(to, &m, true);
        // the residents, standing in a row to the west, as the coarse updates place them
        let residents: Vec<Uuid> = self.shared.cfg.residents.iter().map(|r| r.id).collect();
        let blocks = residents
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let at = Vec3::new(98.0 - 10.0 * i as f32, 128.0, 25.0);
                object_block(RESIDENT_LOCAL_ID + i as u32, *id, 0, LL_PCODE_AVATAR, at)
            })
            .collect();
        let m = ObjectUpdate {
            region_data: object_update::RegionData {
                region_handle: self.shared.regions[self.index].handle(),
                time_dilation: u16::MAX,
            },
            object_data: blocks,
        };
        self.send(to, &m, true);
    }

    /// An inventory asset asked for (LLTransferSourceParamsInvItem: agent, session, owner, task,
    /// item, asset, type): sent in packets of 1000 bytes when the item is the bot's, else refused
    /// as the grid does (LLTS_INSUFFICIENT_PERMISSIONS).
    fn transfer(&mut self, to: SocketAddr, m: &TransferRequest) {
        let t = &m.transfer_info;
        let key = |at: usize| {
            t.params
                .get(at..at + 16)
                .and_then(|b| Uuid::from_slice(b).ok())
                .unwrap_or_default()
        };
        let bot = self.shared.agent_id;
        let data = if t.source_type == 4 {
            // LLTST_SIM_ESTATE: the estate's covenant, to its manager
            self.shared.saw(Seen::TransferRequest { item: Uuid::nil() });
            (key(0) == bot)
                .then(|| crate::inventory::asset(crate::inventory::NOTECARD_ASSET))
                .flatten()
        } else {
            let (owner, item, asset) = (key(32), key(64), key(80));
            self.shared.saw(Seen::TransferRequest { item });
            let found = self
                .shared
                .state()
                .inventory
                .items
                .iter()
                .any(|i| i.id == item && i.asset_id == asset);
            (found && owner == bot)
                .then(|| crate::inventory::asset(asset))
                .flatten()
        };
        let mut info = TransferInfo::default();
        info.transfer_info.transfer_id = t.transfer_id;
        info.transfer_info.channel_type = t.channel_type;
        info.transfer_info.params = t.params.clone();
        let Some(data) = data else {
            info.transfer_info.status = -3;
            self.send(to, &info, true);
            return;
        };
        info.transfer_info.size = data.len() as i32;
        self.send(to, &info, true);
        let chunks: Vec<&[u8]> = data.chunks(1000).collect();
        for (n, chunk) in chunks.iter().enumerate() {
            let mut p = TransferPacket::default();
            p.transfer_data.transfer_id = t.transfer_id;
            p.transfer_data.channel_type = t.channel_type;
            p.transfer_data.packet = n as i32;
            p.transfer_data.status = i32::from(n + 1 == chunks.len());
            p.transfer_data.data = chunk.to_vec();
            self.send(to, &p, true);
        }
    }

    /// An object worn from `item` at `point`: a child of the avatar, its state the point with its
    /// nibbles swapped, its AttachItemID name value.
    fn attachment(&mut self, to: SocketAddr, item: Uuid, point: u8) {
        let next = 0xa700 + self.attached.len() as u32;
        let local = *self.attached.entry(item).or_insert(next);
        let mut b = object_block(
            local,
            Uuid::from_u128(item.as_u128() + 0x0a00_0000),
            AVATAR_LOCAL_ID,
            LL_PCODE_VOLUME,
            Vec3::ZERO,
        );
        b.state = ((point & 0x0F) << 4) | ((point & 0xF0) >> 4);
        b.name_value = format!("AttachItemID STRING RW SV {item}\0").into_bytes();
        let m = ObjectUpdate {
            region_data: object_update::RegionData {
                region_handle: self.shared.regions[self.index].handle(),
                time_dilation: u16::MAX,
            },
            object_data: vec![b],
        };
        self.send(to, &m, true);
    }

    /// An item offered to the bot was accepted: its copy comes into the folder the answer names,
    /// told by BulkUpdateInventory, as the grid does.
    fn inventory_accepted(&mut self, to: SocketAddr, session: Uuid, bucket: &[u8]) {
        let Some((kind, name)) = self.shared.state().inventory_offers.remove(&session) else {
            return;
        };
        let folder = Uuid::from_slice(bucket).unwrap_or(crate::inventory::ROOT);
        let mut m = BulkUpdateInventory::default();
        m.agent_data.agent_id = self.shared.agent_id;
        m.agent_data.transaction_id = session;
        m.item_data = vec![bulk_update_inventory::ItemData {
            item_id: Uuid::new_v4(),
            folder_id: folder,
            owner_id: self.shared.agent_id,
            type_: kind,
            inv_type: kind,
            name: str_field(&name),
            ..Default::default()
        }];
        self.send(to, &m, true);
    }

    /// The bot's avatar in a full ObjectUpdate, standing or seated on `parent`.
    fn avatar_update(&mut self, to: SocketAddr, position: Vec3, parent: u32) {
        self.avatar_at.insert(to, position);
        let block = object_block(
            AVATAR_LOCAL_ID,
            self.shared.agent_id,
            parent,
            LL_PCODE_AVATAR,
            position,
        );
        let m = ObjectUpdate {
            region_data: object_update::RegionData {
                region_handle: self.shared.regions[self.index].handle(),
                time_dilation: u16::MAX,
            },
            object_data: vec![block],
        };
        self.send(to, &m, true);
    }

    /// The avatar moved: an ImprovedTerseObjectUpdate, its rotation at identity.
    fn avatar_moved(&mut self, to: SocketAddr, position: Vec3) {
        self.avatar_at.insert(to, position);
        let mut data = Vec::new();
        data.extend(AVATAR_LOCAL_ID.to_le_bytes());
        data.extend([0, 1]);
        // the foot plane
        data.extend([0u8; 16]);
        for f in position.to_array() {
            data.extend(f.to_le_bytes());
        }
        // velocity, acceleration, rotation (x, y, z, w), angular velocity, by U16 in a range:
        // 0x7fff is zero, 0xffff the top of the range
        for i in 0..13 {
            let v: u16 = if i == 9 { u16::MAX } else { 0x7fff };
            data.extend(v.to_le_bytes());
        }
        let m = ImprovedTerseObjectUpdate {
            region_data: improved_terse_object_update::RegionData {
                region_handle: self.shared.regions[self.index].handle(),
                time_dilation: u16::MAX,
            },
            object_data: vec![improved_terse_object_update::ObjectData {
                data,
                texture_entry: vec![],
            }],
        };
        self.send(to, &m, false);
    }

    /// LLAgent's AgentDataUpdateRequest: AgentDataUpdate (the active group) on UDP, and the
    /// group list as AgentGroupDataUpdate on the event queue, as the grid sends them.
    fn agent_data(&mut self, to: SocketAddr, n: u32) {
        let cfg = &self.shared.cfg;
        let active = {
            let st = self.shared.state();
            st.active_group.and_then(|id| {
                st.groups
                    .iter()
                    .find(|g| g.id == id && g.bot_member)
                    .cloned()
            })
        };
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
        } else if id == ParcelPropertiesRequest::ID {
            if let Ok(m) = pkt.decode::<ParcelPropertiesRequest>() {
                let sequence = m.parcel_data.sequence_id;
                self.shared.saw(Seen::ParcelPropertiesRequest { sequence });
                // the parcel of the cell asked for (its south-west corner)
                let (x, y) = (
                    (m.parcel_data.west / 4.0).clamp(0.0, 63.0) as usize,
                    (m.parcel_data.south / 4.0).clamp(0.0, 63.0) as usize,
                );
                let body = if crate::parcels::parcel_at(x, y) == crate::parcels::PARCEL_LOCAL_ID {
                    self.shared
                        .state()
                        .parcel
                        .properties(sequence, self.shared.agent_id)
                } else {
                    crate::parcels::outskirts(sequence, self.shared.agent_id)
                };
                self.shared.queue_event(n, "ParcelProperties", body);
            }
        } else if id == EstateOwnerMessage::ID {
            if let Ok(m) = pkt.decode::<EstateOwnerMessage>() {
                let method = field_str(&m.method_data.method);
                let params: Vec<String> = m
                    .param_list
                    .iter()
                    .map(|p| field_str(&p.parameter))
                    .collect();
                self.shared.saw(Seen::EstateMessage {
                    method: method.clone(),
                    params,
                });
                if method == "getinfo" {
                    // the estate's info, with the invoice it was asked with
                    let strings = [
                        "Test Estate".to_owned(),
                        Uuid::from_u128(0xe57a_7e00).to_string(),
                        "101".to_owned(),
                        // SUN_FIXED and DENY_BOTS, then one the commands do not touch (1 << 1)
                        ((1u64 << 4) | (1 << 31) | (1 << 1)).to_string(),
                        (6 * 1024).to_string(),
                    ];
                    let mut r = EstateOwnerMessage::default();
                    r.agent_data.agent_id = self.shared.agent_id;
                    r.method_data.method = str_field("estateupdateinfo");
                    r.method_data.invoice = m.method_data.invoice;
                    r.param_list = strings
                        .iter()
                        .map(|s| estate_owner_message::ParamList {
                            parameter: str_field(s),
                        })
                        .collect();
                    self.send(from, &r, true);
                }
            }
        } else if id == EstateCovenantRequest::ID {
            let mut r = EstateCovenantReply::default();
            r.data.covenant_id = crate::inventory::NOTECARD_ASSET;
            r.data.covenant_timestamp = 1_700_000_000;
            r.data.estate_name = str_field("Test Estate");
            r.data.estate_owner_id = Uuid::from_u128(0xe57a_7e00);
            self.send(from, &r, true);
        } else if id == EjectUser::ID {
            if let Ok(m) = pkt.decode::<EjectUser>() {
                self.shared.saw(Seen::Parcel {
                    what: format!("eject {} {}", m.data.target_id, m.data.flags),
                });
            }
        } else if id == FreezeUser::ID {
            if let Ok(m) = pkt.decode::<FreezeUser>() {
                self.shared.saw(Seen::Parcel {
                    what: format!("freeze {} {}", m.data.target_id, m.data.flags),
                });
            }
        } else if id == ParcelAccessListRequest::ID {
            if let Ok(m) = pkt.decode::<ParcelAccessListRequest>() {
                // the list as kept, a null entry when empty (LLParcel::packAccessEntries)
                let flags = m.data.flags;
                let list = if flags & 2 != 0 {
                    self.shared.state().parcel_bans.clone()
                } else {
                    Vec::new()
                };
                let mut r = ParcelAccessListReply::default();
                r.data.agent_id = self.shared.agent_id;
                r.data.flags = flags;
                r.data.local_id = m.data.local_id;
                r.list = if list.is_empty() {
                    vec![parcel_access_list_reply::List::default()]
                } else {
                    list.iter()
                        .map(|id| parcel_access_list_reply::List {
                            id: *id,
                            time: 0,
                            flags,
                        })
                        .collect()
                };
                self.send(from, &r, true);
            }
        } else if id == ParcelAccessListUpdate::ID {
            if let Ok(m) = pkt.decode::<ParcelAccessListUpdate>() {
                let ids: Vec<Uuid> = m
                    .list
                    .iter()
                    .map(|e| e.id)
                    .filter(|id| !id.is_nil())
                    .collect();
                self.shared.saw(Seen::AccessListUpdate {
                    flags: m.data.flags,
                    ids: ids.clone(),
                });
                if m.data.flags & 2 != 0 {
                    self.shared.state().parcel_bans = ids;
                }
            }
        } else if id == ParcelObjectOwnersRequest::ID {
            // two owners with objects on the parcel
            let data = [
                (Uuid::from_u128(0x5eed_0001), 3),
                (Uuid::from_u128(0x5eed_0002), 1),
            ]
            .iter()
            .map(|(id, n)| parcel_object_owners_reply::Data {
                owner_id: *id,
                is_group_owned: false,
                count: *n,
                online_status: false,
            })
            .collect();
            self.send(from, &ParcelObjectOwnersReply { data }, true);
        } else if id == ParcelReturnObjects::ID {
            if let Ok(m) = pkt.decode::<ParcelReturnObjects>() {
                let owners: Vec<String> = m
                    .owner_i_ds
                    .iter()
                    .map(|o| o.owner_id.to_string())
                    .collect();
                self.shared.saw(Seen::Parcel {
                    what: format!("return {} {}", m.parcel_data.return_type, owners.join(",")),
                });
            }
        } else if id == ParcelDeedToGroup::ID {
            if let Ok(m) = pkt.decode::<ParcelDeedToGroup>() {
                self.shared.saw(Seen::Parcel {
                    what: format!("deed {} {}", m.data.local_id, m.data.group_id),
                });
            }
        } else if id == ParcelRelease::ID {
            if let Ok(m) = pkt.decode::<ParcelRelease>() {
                self.shared.saw(Seen::Parcel {
                    what: format!("release {}", m.data.local_id),
                });
            }
        } else if id == ParcelBuy::ID {
            if let Ok(m) = pkt.decode::<ParcelBuy>() {
                self.shared.saw(Seen::Parcel {
                    what: format!(
                        "buy {} {} {}",
                        m.data.local_id, m.parcel_data.price, m.parcel_data.area
                    ),
                });
            }
        } else if id == ParcelReclaim::ID {
            if let Ok(m) = pkt.decode::<ParcelReclaim>() {
                self.shared.saw(Seen::Parcel {
                    what: format!("reclaim {}", m.data.local_id),
                });
            }
        } else if id == DirLandQuery::ID {
            if let Ok(m) = pkt.decode::<DirLandQuery>() {
                let q = &m.query_data;
                self.shared.saw(Seen::LandQuery {
                    flags: q.query_flags,
                    kind: q.search_type,
                    price: q.price,
                    area: q.area,
                    start: q.query_start,
                });
                let mut r = DirLandReply::default();
                r.agent_data.agent_id = self.shared.agent_id;
                r.query_data.query_id = q.query_id;
                r.query_replies = vec![dir_land_reply::QueryReplies {
                    parcel_id: Uuid::from_u128(0x1a4d_0001),
                    name: str_field("Beach Lot"),
                    auction: false,
                    for_sale: true,
                    sale_price: 1200,
                    actual_area: 512,
                }];
                self.send(from, &r, true);
            }
        } else if id == ParcelDwellRequest::ID {
            if let Ok(m) = pkt.decode::<ParcelDwellRequest>() {
                let mut r = ParcelDwellReply::default();
                r.agent_data.agent_id = self.shared.agent_id;
                r.data.local_id = m.data.local_id;
                r.data.parcel_id = crate::parcels::PARCEL_ID;
                r.data.dwell = crate::parcels::DWELL;
                self.send(from, &r, true);
            }
        } else if id == AgentAnimation::ID {
            if let Ok(m) = pkt.decode::<AgentAnimation>() {
                let playing = self.playing.entry(from).or_default();
                for a in &m.animation_list {
                    self.shared.saw(Seen::Animation {
                        id: a.anim_id,
                        start: a.start_anim,
                    });
                    playing.retain(|p| *p != a.anim_id);
                    if a.start_anim {
                        playing.push(a.anim_id);
                    }
                }
                // the region tells what the avatar plays now
                let r = AvatarAnimation {
                    sender: avatar_animation::Sender {
                        id: self.shared.agent_id,
                    },
                    animation_list: playing
                        .iter()
                        .enumerate()
                        .map(|(i, a)| avatar_animation::AnimationList {
                            anim_id: *a,
                            anim_sequence_id: i as i32 + 1,
                        })
                        .collect(),
                    animation_source_list: vec![],
                    physical_avatar_event_list: vec![],
                };
                self.send(from, &r, true);
            }
        } else if id == LinkInventoryItem::ID {
            if let Ok(m) = pkt.decode::<LinkInventoryItem>() {
                let b = &m.inventory_block;
                self.shared.saw(Seen::Object {
                    what: format!("link {} in {}", b.old_item_id, b.folder_id),
                });
                self.shared
                    .state()
                    .inventory
                    .items
                    .push(crate::inventory::Item {
                        id: Uuid::new_v4(),
                        parent: b.folder_id,
                        name: field_str(&b.name),
                        asset_type: 24,
                        inv_type: i32::from(b.inv_type),
                        asset_id: b.old_item_id,
                        flags: 0,
                        created_at: 0,
                    });
            }
        } else if id == RemoveInventoryItem::ID {
            if let Ok(m) = pkt.decode::<RemoveInventoryItem>() {
                let gone: Vec<Uuid> = m.inventory_data.iter().map(|d| d.item_id).collect();
                let mut st = self.shared.state();
                for id in &gone {
                    // a link: what it pointed to, for the tests
                    let target = st
                        .inventory
                        .items
                        .iter()
                        .find(|i| i.id == *id)
                        .map(|i| i.asset_id);
                    st.inventory.items.retain(|i| i.id != *id);
                    drop(st);
                    self.shared.saw(Seen::Object {
                        what: format!("unlink {}", target.unwrap_or_default()),
                    });
                    st = self.shared.state();
                }
            }
        } else if id == RezSingleAttachmentFromInv::ID {
            if let Ok(m) = pkt.decode::<RezSingleAttachmentFromInv>() {
                let d = &m.object_data;
                self.shared.saw(Seen::Object {
                    what: format!("attach {} pt {}", d.item_id, d.attachment_pt),
                });
                // worn at the chest
                self.attachment(from, d.item_id, 1);
            }
        } else if id == DetachAttachmentIntoInv::ID {
            if let Ok(m) = pkt.decode::<DetachAttachmentIntoInv>() {
                let item = m.object_data.item_id;
                self.shared.saw(Seen::Object {
                    what: format!("detach {item}"),
                });
                if let Some(local) = self.attached.remove(&item) {
                    let k = KillObject {
                        object_data: vec![kill_object::ObjectData { id: local }],
                    };
                    self.send(from, &k, true);
                }
            }
        } else if id == RezObject::ID {
            if let Ok(m) = pkt.decode::<RezObject>() {
                let r = &m.rez_data;
                self.shared.saw(Seen::Object {
                    what: format!(
                        "rez {} at {:.0},{:.0},{:.0} bypass {}",
                        m.inventory_data.item_id,
                        r.ray_end.x,
                        r.ray_end.y,
                        r.ray_end.z,
                        r.bypass_raycast
                    ),
                });
            }
        } else if id == DeRezObject::ID {
            if let Ok(m) = pkt.decode::<DeRezObject>() {
                let ids: Vec<String> = m
                    .object_data
                    .iter()
                    .map(|o| o.object_local_id.to_string())
                    .collect();
                self.shared.saw(Seen::Object {
                    what: format!(
                        "derez {} to {} in {}",
                        ids.join(","),
                        m.agent_block.destination,
                        m.agent_block.destination_id
                    ),
                });
            }
        } else if id == ObjectSelect::ID {
            if let Ok(m) = pkt.decode::<ObjectSelect>() {
                let ids: Vec<u32> = m.object_data.iter().map(|o| o.object_local_id).collect();
                self.shared.saw(Seen::Object {
                    what: format!("select {}", local_ids(&ids)),
                });
                // the objects it knows, in one ObjectProperties (LLSelectMgr's answer)
                let object_data: Vec<_> = ids.iter().filter_map(|id| properties(*id)).collect();
                if !object_data.is_empty() {
                    self.send(from, &ObjectProperties { object_data }, true);
                }
            }
        } else if id == ObjectDeselect::ID {
            if let Ok(m) = pkt.decode::<ObjectDeselect>() {
                let ids: Vec<u32> = m.object_data.iter().map(|o| o.object_local_id).collect();
                self.shared.saw(Seen::Object {
                    what: format!("deselect {}", local_ids(&ids)),
                });
            }
        } else if id == ObjectGrab::ID {
            if let Ok(m) = pkt.decode::<ObjectGrab>() {
                self.shared.saw(Seen::Object {
                    what: format!("grab {}", m.object_data.local_id),
                });
            }
        } else if id == ObjectDeGrab::ID {
            if let Ok(m) = pkt.decode::<ObjectDeGrab>() {
                self.shared.saw(Seen::Object {
                    what: format!("degrab {}", m.object_data.local_id),
                });
            }
        } else if id == UpdateTaskInventory::ID {
            if let Ok(m) = pkt.decode::<UpdateTaskInventory>() {
                self.shared.saw(Seen::Object {
                    what: format!(
                        "contents {} {}",
                        m.update_data.local_id, m.inventory_data.item_id
                    ),
                });
            }
        } else if id == RezScript::ID {
            if let Ok(m) = pkt.decode::<RezScript>() {
                self.shared.saw(Seen::Object {
                    what: format!(
                        "script {} {} running {}",
                        m.update_block.object_local_id,
                        m.inventory_block.item_id,
                        m.update_block.enabled
                    ),
                });
            }
        } else if id == AvatarPropertiesRequest::ID {
            if let Ok(m) = pkt.decode::<AvatarPropertiesRequest>() {
                // as OpenSim answers, without the AgentProfile capability
                let who = m.agent_data.avatar_id;
                let known = self.shared.state().profiles.get(&who).cloned();
                if let Some((about, image)) = known
                    .or_else(|| (who == self.shared.agent_id).then(|| (String::new(), Uuid::nil())))
                {
                    let mut r = AvatarPropertiesReply::default();
                    r.agent_data.agent_id = self.shared.agent_id;
                    r.agent_data.avatar_id = who;
                    r.properties_data.about_text = str_field(&about);
                    r.properties_data.image_id = image;
                    r.properties_data.fl_about_text = str_field("first life");
                    r.properties_data.flags = 1;
                    self.send(from, &r, true);
                }
            }
        } else if id == AvatarPropertiesUpdate::ID {
            if let Ok(m) = pkt.decode::<AvatarPropertiesUpdate>() {
                let d = &m.properties_data;
                let (about, image) = (field_str(&d.about_text), d.image_id);
                self.shared.saw(Seen::ProfileUpdate {
                    about: about.clone(),
                    image,
                    rest_kept: field_str(&d.fl_about_text) == "first life" && d.allow_publish,
                });
                self.shared
                    .state()
                    .profiles
                    .insert(self.shared.agent_id, (about, image));
            }
        } else if id == SoundTrigger::ID {
            if let Ok(m) = pkt.decode::<SoundTrigger>() {
                self.shared.saw(Seen::SoundTrigger {
                    asset: m.sound_data.sound_id,
                });
            }
        } else if id == ViewerEffect::ID {
            if let Ok(m) = pkt.decode::<ViewerEffect>() {
                for e in &m.effect {
                    let target = e
                        .type_data
                        .get(16..32)
                        .and_then(|b| Uuid::from_slice(b).ok())
                        .unwrap_or_default();
                    self.shared.saw(Seen::Effect {
                        kind: e.type_,
                        target,
                    });
                }
            }
        } else if id == ScriptAnswerYes::ID {
            if let Ok(m) = pkt.decode::<ScriptAnswerYes>() {
                self.shared.saw(Seen::ScriptAnswer {
                    task: m.data.task_id,
                    questions: m.data.questions,
                });
            }
        } else if id == FindAgent::ID {
            if let Ok(mut m) = pkt.decode::<FindAgent>() {
                let prey = m.agent_block.prey;
                self.shared.saw(Seen::FindAgent { prey });
                // the friend who lets the bot map them stands in this region, at 100.5, 60.25
                if prey == MAPPABLE_FRIEND {
                    let region = &self.shared.regions[self.index];
                    m.location_block = vec![find_agent::LocationBlock {
                        global_x: f64::from(region.x) + 100.5,
                        global_y: f64::from(region.y) + 60.25,
                    }];
                    self.send(from, &m, true);
                }
            }
        } else if id == MuteListRequest::ID {
            self.shared.saw(Seen::MuteListRequest);
            let mut r = MuteListUpdate::default();
            r.mute_data.agent_id = self.shared.agent_id;
            r.mute_data.filename = str_field("mutes.list");
            self.send(from, &r, true);
        } else if id == RequestXfer::ID {
            if let Ok(m) = pkt.decode::<RequestXfer>() {
                // the mute list's file, in two packets: its size first, the last one marked
                let file = mute_file();
                let (a, b) = file.split_at(file.len() / 2);
                let mut first = (file.len() as i32).to_le_bytes().to_vec();
                first.extend_from_slice(a);
                for (n, data) in [(0u32, first), (1 | 0x8000_0000, b.to_vec())] {
                    let mut p = SendXferPacket::default();
                    p.xfer_id.id = m.xfer_id.id;
                    p.xfer_id.packet = n;
                    p.data_packet.data = data;
                    self.send(from, &p, true);
                }
            }
        } else if id == ConfirmXferPacket::ID {
            if let Ok(m) = pkt.decode::<ConfirmXferPacket>() {
                self.shared.saw(Seen::XferConfirmed {
                    packet: m.xfer_id.packet,
                });
            }
        } else if id == TransferRequest::ID {
            if let Ok(m) = pkt.decode::<TransferRequest>() {
                self.transfer(from, &m);
            }
        } else if id == AgentRequestSit::ID {
            if let Ok(m) = pkt.decode::<AgentRequestSit>() {
                let target = m.target_object.target_id;
                self.shared.saw(Seen::AgentRequestSit { target });
                let mut r = AvatarSitResponse::default();
                r.sit_object.id = target;
                r.sit_transform.auto_pilot = true;
                self.send(from, &r, true);
            }
        } else if id == AgentSit::ID {
            self.shared.saw(Seen::AgentSit);
            let seat = object_block(SEAT_LOCAL_ID, SEAT, 0, LL_PCODE_VOLUME, SEAT_POSITION);
            let m = ObjectUpdate {
                region_data: object_update::RegionData {
                    region_handle: self.shared.regions[self.index].handle(),
                    time_dilation: u16::MAX,
                },
                object_data: vec![seat],
            };
            self.send(from, &m, true);
            self.avatar_update(from, Vec3::Z, SEAT_LOCAL_ID);
        } else if id == SetAlwaysRun::ID {
            if let Ok(m) = pkt.decode::<SetAlwaysRun>() {
                self.shared.saw(Seen::AlwaysRun {
                    on: m.agent_data.always_run,
                });
            }
        } else if id == GenericMessage::ID {
            if let Ok(m) = pkt.decode::<GenericMessage>()
                && field_str(&m.method_data.method) == "autopilot"
            {
                let params: Vec<String> = m
                    .param_list
                    .iter()
                    .map(|p| field_str(&p.parameter))
                    .collect();
                // global x and y, the height in the region (LLClientView.HandleAutopilot): the
                // avatar is there at once
                let region = &self.shared.regions[self.index];
                let (rx, ry) = (f64::from(region.x), f64::from(region.y));
                let num = |i: usize| params.get(i).and_then(|p| p.parse::<f64>().ok());
                if let (Some(x), Some(y), Some(z)) = (num(0), num(1), num(2)) {
                    let to = Vec3::new((x - rx) as f32, (y - ry) as f32, z as f32);
                    self.avatar_moved(from, to);
                }
                self.shared.saw(Seen::AutoPilot { params });
            }
        } else if id == MoneyBalanceRequest::ID {
            self.balance(from);
        } else if id == MoneyTransferRequest::ID {
            if let Ok(m) = pkt.decode::<MoneyTransferRequest>() {
                let d = &m.money_data;
                self.shared.state().balance -= d.amount;
                self.shared.saw(Seen::Pay {
                    to: d.dest_id,
                    amount: d.amount,
                    transaction_type: d.transaction_type,
                });
                self.balance(from);
            }
        } else if id == TerminateFriendship::ID {
            if let Ok(m) = pkt.decode::<TerminateFriendship>() {
                self.shared.saw(Seen::EndFriendship {
                    other: m.ex_block.other_id,
                });
            }
        } else if id == GrantUserRights::ID {
            if let Ok(m) = pkt.decode::<GrantUserRights>() {
                for r in &m.rights {
                    self.shared.saw(Seen::GrantRights {
                        to: r.agent_related,
                        rights: r.related_rights,
                    });
                }
            }
        } else if id == CreateInventoryItem::ID {
            if let Ok(m) = pkt.decode::<CreateInventoryItem>() {
                let b = &m.inventory_block;
                let item = Uuid::new_v4();
                let name = field_str(&b.name);
                self.shared
                    .state()
                    .inventory
                    .items
                    .push(crate::inventory::Item {
                        id: item,
                        parent: b.folder_id,
                        name: name.clone(),
                        asset_type: i32::from(b.type_),
                        inv_type: i32::from(b.inv_type),
                        asset_id: Uuid::nil(),
                        flags: 0,
                        created_at: 0,
                    });
                self.shared.saw(Seen::CreateInventoryItem {
                    item,
                    folder: b.folder_id,
                    name: name.clone(),
                    asset_type: b.type_,
                });
                let mut r = UpdateCreateInventoryItem::default();
                r.agent_data.agent_id = bot;
                r.agent_data.sim_approved = true;
                r.inventory_data = vec![update_create_inventory_item::InventoryData {
                    item_id: item,
                    folder_id: b.folder_id,
                    callback_id: b.callback_id,
                    creator_id: bot,
                    owner_id: bot,
                    base_mask: 0x7fff_ffff,
                    owner_mask: 0x7fff_ffff,
                    next_owner_mask: b.next_owner_mask,
                    type_: b.type_,
                    inv_type: b.inv_type,
                    name: str_field(&name),
                    ..Default::default()
                }];
                self.send(from, &r, true);
            }
        } else if id == CreateInventoryFolder::ID {
            if let Ok(m) = pkt.decode::<CreateInventoryFolder>() {
                let f = &m.folder_data;
                let name = field_str(&f.name);
                self.shared
                    .state()
                    .inventory
                    .folders
                    .push(crate::inventory::Folder {
                        id: f.folder_id,
                        parent: f.parent_id,
                        name: name.clone(),
                        kind: i32::from(f.type_),
                    });
                self.shared.saw(Seen::CreateInventoryFolder {
                    id: f.folder_id,
                    parent: f.parent_id,
                    name,
                });
            }
        } else if id == MoveInventoryItem::ID {
            if let Ok(m) = pkt.decode::<MoveInventoryItem>() {
                for d in &m.inventory_data {
                    let name = field_str(&d.new_name);
                    if let Some(i) = self
                        .shared
                        .state()
                        .inventory
                        .items
                        .iter_mut()
                        .find(|i| i.id == d.item_id)
                    {
                        i.parent = d.folder_id;
                        if !name.is_empty() {
                            i.name = name.clone();
                        }
                    }
                    self.shared.saw(Seen::MoveInventoryItem {
                        item: d.item_id,
                        folder: d.folder_id,
                        name,
                    });
                }
            }
        } else if id == MoveInventoryFolder::ID {
            if let Ok(m) = pkt.decode::<MoveInventoryFolder>() {
                for d in &m.inventory_data {
                    if let Some(f) = self
                        .shared
                        .state()
                        .inventory
                        .folders
                        .iter_mut()
                        .find(|f| f.id == d.folder_id)
                    {
                        f.parent = d.parent_id;
                    }
                    self.shared.saw(Seen::MoveInventoryFolder {
                        folder: d.folder_id,
                        parent: d.parent_id,
                    });
                }
            }
        } else if id == GroupTitleUpdate::ID {
            if let Ok(m) = pkt.decode::<GroupTitleUpdate>() {
                self.shared.saw(Seen::GroupTitleUpdate {
                    group: m.agent_data.group_id,
                    role: m.agent_data.title_role_id,
                });
            }
        } else if id == ActivateGroup::ID {
            if let Ok(m) = pkt.decode::<ActivateGroup>() {
                let group = m.agent_data.group_id;
                self.shared.saw(Seen::ActivateGroup { group });
                self.shared.state().active_group = (!group.is_nil()).then_some(group);
                // the grid answers with the agent's data
                self.agent_data_only(from);
            }
        } else if id == JoinGroupRequest::ID {
            if let Ok(m) = pkt.decode::<JoinGroupRequest>() {
                let group = m.group_data.group_id;
                self.shared.saw(Seen::JoinGroup { group });
                // the third group has open enrollment; the others do not exist for joining
                let ok = group == crate::THIRD_GROUP && self.set_member(group, true, n);
                let mut r = JoinGroupReply::default();
                r.agent_data.agent_id = bot;
                r.group_data.group_id = group;
                r.group_data.success = ok;
                self.send(from, &r, true);
            }
        } else if id == LeaveGroupRequest::ID {
            if let Ok(m) = pkt.decode::<LeaveGroupRequest>() {
                let group = m.group_data.group_id;
                self.shared.saw(Seen::LeaveGroup { group });
                let ok = self.set_member(group, false, n);
                let mut r = LeaveGroupReply::default();
                r.agent_data.agent_id = bot;
                r.group_data.group_id = group;
                r.group_data.success = ok;
                self.send(from, &r, true);
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
        } else if id == AcceptFriendship::ID {
            if let Ok(m) = pkt.decode::<AcceptFriendship>() {
                self.shared.saw(Seen::FriendshipAccepted {
                    transaction: m.transaction_block.transaction_id,
                });
            }
        } else if id == MapNameRequest::ID {
            if let Ok(m) = pkt.decode::<MapNameRequest>() {
                let name = field_str(&m.name_data.name);
                self.shared.saw(Seen::MapNameRequest { name: name.clone() });
                // the regions whose name starts so, then the end of the list (access 255), as
                // OpenSim's MapSearchModule answers
                let want = name.to_lowercase();
                let mut r = MapBlockReply::default();
                r.agent_data.agent_id = self.shared.agent_id;
                r.agent_data.flags = 2;
                for region in &self.shared.regions {
                    if region.name.to_lowercase().starts_with(&want) {
                        r.data.push(map_block_reply::Data {
                            x: (region.x / 256) as u16,
                            y: (region.y / 256) as u16,
                            name: str_field(&region.name),
                            access: 13,
                            map_image_id: map_tile(region.x / 256, region.y / 256),
                            ..Default::default()
                        });
                    }
                }
                r.data.push(map_block_reply::Data {
                    name: str_field(&name),
                    access: 255,
                    ..Default::default()
                });
                self.send(from, &r, true);
            }
        } else if id == ScriptDialogReply::ID {
            if let Ok(m) = pkt.decode::<ScriptDialogReply>() {
                self.shared.saw(Seen::DialogReply {
                    object_id: m.data.object_id,
                    channel: m.data.chat_channel,
                    index: m.data.button_index,
                    label: field_str(&m.data.button_label),
                });
            }
        } else if id == StartLure::ID {
            if let Ok(m) = pkt.decode::<StartLure>() {
                for t in &m.target_data {
                    self.shared.saw(Seen::StartLure {
                        target: t.target_id,
                    });
                }
            }
        } else if id == ChatFromViewer::ID {
            if let Ok(m) = pkt.decode::<ChatFromViewer>() {
                self.shared.saw(Seen::Chat {
                    message: field_str(&m.chat_data.message),
                    channel: m.chat_data.channel,
                    chat_type: m.chat_data.type_,
                });
            }
        } else if id == ImprovedInstantMessage::ID {
            if let Ok(m) = pkt.decode::<ImprovedInstantMessage>() {
                let b = &m.message_block;
                let group = b.to_agent_id;
                match b.dialog {
                    // IM_SESSION_GROUP_START: ChatterBoxSessionStartReply on the event queue
                    15 => {
                        self.shared.saw(Seen::GroupChatStart { group });
                        let member = group_of(&self.shared, group).is_some();
                        if member {
                            self.shared.state().group_chats.insert(group);
                        }
                        let mut body = Map::new();
                        body.insert("temp_session_id".into(), group.into());
                        body.insert("session_id".into(), group.into());
                        body.insert("success".into(), member.into());
                        if !member {
                            body.insert(
                                "error".into(),
                                "You are not a member of that group.".into(),
                            );
                        }
                        self.shared
                            .queue_event(n, "ChatterBoxSessionStartReply", Llsd::Map(body));
                    }
                    5 | 6 | 10 | 11 | 24 | 26 | 38 => {
                        // IM_INVENTORY_ACCEPTED, IM_TASK_INVENTORY_ACCEPTED
                        if matches!(b.dialog, 5 | 10) {
                            self.inventory_accepted(from, b.id, &b.binary_bucket);
                        }
                        self.shared.saw(Seen::ImAnswer {
                            to: b.to_agent_id,
                            dialog: b.dialog,
                            id: b.id,
                        });
                    }
                    0 => self.shared.saw(Seen::Im {
                        to: b.to_agent_id,
                        message: field_str(&b.message),
                    }),
                    4 => self.shared.saw(Seen::InventoryOffer {
                        to: b.to_agent_id,
                        name: field_str(&b.message),
                        entries: b
                            .binary_bucket
                            .chunks(17)
                            .filter(|c| c.len() == 17)
                            .map(|c| (c[0], Uuid::from_slice(&c[1..]).unwrap_or_default()))
                            .collect(),
                    }),
                    // send_group_notice: needs the notices power, as the grid checks
                    32 => self.shared.saw(Seen::GroupNotice {
                        group,
                        message: field_str(&b.message),
                        attached: b.binary_bucket.len() > 1,
                    }),
                    17 => self.shared.saw(Seen::GroupChatSend {
                        group,
                        message: field_str(&b.message),
                    }),
                    18 => {
                        self.shared.state().group_chats.remove(&group);
                        self.shared.saw(Seen::GroupChatLeave { group });
                    }
                    _ => {}
                }
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

    fn balance(&mut self, to: SocketAddr) {
        let mut r = MoneyBalanceReply::default();
        r.money_data.agent_id = self.shared.agent_id;
        r.money_data.transaction_success = true;
        r.money_data.money_balance = self.shared.state().balance;
        self.send(to, &r, true);
    }

    /// AgentDataUpdate alone (an ActivateGroup's answer).
    fn agent_data_only(&mut self, to: SocketAddr) {
        let active = {
            let st = self.shared.state();
            st.active_group.and_then(|id| {
                st.groups
                    .iter()
                    .find(|g| g.id == id && g.bot_member)
                    .cloned()
            })
        };
        let mut m = AgentDataUpdate::default();
        m.agent_data.agent_id = self.shared.agent_id;
        m.agent_data.first_name = str_field(&self.shared.cfg.first);
        m.agent_data.last_name = str_field(&self.shared.cfg.last);
        if let Some(g) = active {
            m.agent_data.active_group_id = g.id;
            m.agent_data.group_name = str_field(&g.name);
            m.agent_data.group_title = str_field(&g.title);
            m.agent_data.group_powers = g.powers;
        }
        self.send(to, &m, true);
    }

    /// The bot joins or leaves a group: the group list goes out again. False when it already was
    /// (or was not) in it.
    fn set_member(&mut self, group: Uuid, member: bool, n: u32) -> bool {
        let body = {
            let mut st = self.shared.state();
            let Some(g) = st.groups.iter_mut().find(|g| g.id == group) else {
                return false;
            };
            if g.bot_member == member {
                return false;
            }
            g.bot_member = member;
            crate::groups::agent_group_data(&st.groups, self.shared.agent_id)
        };
        self.shared.queue_event(n, "AgentGroupDataUpdate", body);
        true
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
        // the avatars in the region (CoarseLocationUpdate): the residents and the bot
        let bot = self.shared.agent_id;
        let ids: Vec<Uuid> = self
            .shared
            .cfg
            .residents
            .iter()
            .map(|r| r.id)
            .chain(std::iter::once(bot))
            .collect();
        let addrs: Vec<SocketAddr> = self.clients.keys().copied().collect();
        for addr in &addrs {
            let m = CoarseLocationUpdate {
                // the residents in a row to the west of the bot, which is last
                location: (0..ids.len())
                    .map(|i| coarse_location_update::Location {
                        x: if i + 1 == ids.len() {
                            128
                        } else {
                            (98 - 10 * i) as u8
                        },
                        y: 128,
                        z: 6,
                    })
                    .collect(),
                index: coarse_location_update::Index {
                    you: (ids.len() - 1) as i16,
                    prey: -1,
                },
                agent_data: ids
                    .iter()
                    .map(|id| coarse_location_update::AgentData { agent_id: *id })
                    .collect(),
            };
            self.send(*addr, &m, false);
            // the friend is online
            let n = OnlineNotification {
                agent_block: vec![online_notification::AgentBlock {
                    agent_id: Uuid::from_u128(0x5eed_0001),
                }],
            };
            self.send(*addr, &n, true);
        }
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
            RegionCommand::GroupChat { from, group, text } => {
                let mut m = ImprovedInstantMessage::default();
                m.agent_data.agent_id = from.id;
                let b = &mut m.message_block;
                b.from_group = true;
                b.to_agent_id = self.shared.agent_id;
                b.dialog = 17; // IM_SESSION_SEND
                b.id = group;
                b.from_agent_name = str_field(&format!("{} {}", from.first, from.last));
                b.message = str_field(&text);
                self.send(to, &m, true);
            }
            RegionCommand::ImFrom {
                id,
                name,
                dialog,
                text,
                session,
                bucket,
            } => {
                let mut m = ImprovedInstantMessage::default();
                m.agent_data.agent_id = id;
                let b = &mut m.message_block;
                b.to_agent_id = self.shared.agent_id;
                b.dialog = dialog;
                b.id = session;
                b.binary_bucket = bucket;
                b.from_agent_name = str_field(&name);
                b.message = str_field(&text);
                self.send(to, &m, true);
            }
            RegionCommand::Dialog {
                object_id,
                object_name,
                owner,
                message,
                buttons,
                channel,
            } => {
                let mut m = ScriptDialog::default();
                m.data.object_id = object_id;
                m.data.first_name = str_field(&owner.first);
                m.data.last_name = str_field(&owner.last);
                m.data.object_name = str_field(&object_name);
                m.data.message = str_field(&message);
                m.data.chat_channel = channel;
                m.buttons = buttons
                    .iter()
                    .map(|b| script_dialog::Buttons {
                        button_label: str_field(b),
                    })
                    .collect();
                m.owner_data = vec![script_dialog::OwnerData { owner_id: owner.id }];
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
            RegionCommand::ScriptQuestion {
                task,
                owner,
                questions,
            } => {
                let mut m = ScriptQuestion::default();
                m.data.task_id = task;
                m.data.item_id = Uuid::from_u128(0x17e3);
                m.data.object_name = str_field("Dance Ball");
                m.data.object_owner = str_field(&owner);
                m.data.questions = questions;
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
        outbox: Vec::new(),
        facing: HashMap::new(),
        controls: HashMap::new(),
        playing: HashMap::new(),
        attached: HashMap::new(),
        avatar_at: HashMap::new(),
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
        for (packet, to) in std::mem::take(&mut r.outbox) {
            if let Err(e) = r.sock.send_to(&packet, to).await {
                log::debug!("testgrid region send: {e}");
            }
        }
    }
}

/// A linkset standing on the parcel: its root and a child prim.
pub const OBJECT: Uuid = Uuid::from_u128(0x0b1e_c701);
pub const OBJECT_CHILD: Uuid = Uuid::from_u128(0x0b1e_c702);
pub const OBJECT_LOCAL_ID: u32 = 0x0b0c;
pub const OBJECT_POSITION: Vec3 = Vec3::new(100.0, 120.0, 22.0);

/// The linkset's owner, its name and its creator, as its ObjectProperties tell them.
pub const OBJECT_OWNER: Uuid = Uuid::from_u128(0x5eed_0001);
pub const OBJECT_NAME: &str = "Garden Bench";
pub const OBJECT_CREATOR: Uuid = Uuid::from_u128(0xc4ea_7041);

/// Local ids in words: "1,2,3".
fn local_ids(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

/// The ObjectProperties of an object of the region's, by its local id: the linkset's root and
/// the seat.
fn properties(local_id: u32) -> Option<object_properties::ObjectData> {
    let (object_id, name, description) = match local_id {
        OBJECT_LOCAL_ID => (OBJECT, OBJECT_NAME, "Two prims"),
        SEAT_LOCAL_ID => (SEAT, "Seat", ""),
        _ => return None,
    };
    Some(object_properties::ObjectData {
        object_id,
        creator_id: OBJECT_CREATOR,
        owner_id: OBJECT_OWNER,
        last_owner_id: OBJECT_CREATOR,
        name: str_field(name),
        description: str_field(description),
        ..Default::default()
    })
}

/// The bot's avatar's local id in the region.
const AVATAR_LOCAL_ID: u32 = 0xa0a0;
/// The object the bot sits on, whatever key it asked to sit on.
const SEAT: Uuid = Uuid::from_u128(0x5ea7);
const SEAT_LOCAL_ID: u32 = 0x5ea7;
pub const SEAT_POSITION: Vec3 = Vec3::new(100.0, 100.0, 20.0);
/// LL_PCODE_VOLUME, LL_PCODE_LEGACY_AVATAR (llprimitive/object_flags.h)
const LL_PCODE_VOLUME: u8 = 9;
const LL_PCODE_AVATAR: u8 = 47;

/// A full ObjectUpdate block: the place (after a foot plane for an avatar), the rest at zero.
fn object_block(
    local_id: u32,
    full_id: Uuid,
    parent: u32,
    pcode: u8,
    position: Vec3,
) -> object_update::ObjectData {
    let mut data = Vec::new();
    if pcode == LL_PCODE_AVATAR {
        data.extend([0u8; 16]);
    }
    for v in [position, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO] {
        for f in v.to_array() {
            data.extend(f.to_le_bytes());
        }
    }
    object_update::ObjectData {
        id: local_id,
        full_id,
        parent_id: parent,
        p_code: pcode,
        scale: Vec3::ONE,
        object_data: data,
        ..Default::default()
    }
}

/// The residents' avatars' local ids, from this one on.
const RESIDENT_LOCAL_ID: u32 = 0xb0b0;
/// The friend whom the bot may find on the map (FindAgent).
pub const MAPPABLE_FRIEND: Uuid = Uuid::from_u128(0x5eed_0001);

/// A region's map tile, by its place on the grid.
pub fn map_tile(x: u32, y: u32) -> Uuid {
    Uuid::from_u128(0x7117_0000_0000 | (u128::from(x) << 16) | u128::from(y))
}

/// The bot's mute list file (LLMuteList's format: type, key, name|flags).
pub fn mute_file() -> Vec<u8> {
    format!(
        "1 {} Some Resident|0\n2 {} Noisy Box|3\n0 {} Spammer|\n",
        Uuid::from_u128(0x5eed_0001),
        Uuid::from_u128(0xb0c5),
        Uuid::nil()
    )
    .into_bytes()
}
