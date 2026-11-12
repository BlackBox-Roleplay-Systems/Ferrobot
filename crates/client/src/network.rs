//! The network task: one logged-in session at a time, in one region. It owns the UDP socket, the
//! circuit and the event queue; it answers the client's requests and reports events.
//!
//! The connect sequence follows aurora-net/src/session.rs (Aurora Viewer, see NOTICE.md) and
//! `newview/llstartup.cpp`: UseCircuitCode, then once it is acked CompleteAgentMovement and
//! AgentThrottle; RegionHandshake is answered with RegionHandshakeReply; pings both ways; once
//! arrived, AgentDataUpdateRequest (LLAgent's active group and the group list).
//!
//! Teleports and region crossings move the session to another region: on TeleportFinish or
//! CrossedRegion (event queue), a circuit to the new region (UseCircuitCode, then
//! CompleteAgentMovement once acked) and its own seed and event queue, as Aurora's switch_region.
//! The old region is left behind: like LibreMetaverse's bots (AgentSettings.MultipleSims = false),
//! the bot keeps no circuits to neighbour regions.

use crate::caps::{self, EqEvent, EqMessage};
use crate::circuit::{Circuit, Resend};
use crate::events::{ChatMessage, Event, InstantMessage};
use crate::login::LoginResponse;
use crate::names::DisplayName;
use crate::region_restart::{self, Restart};
use crate::state::{ActiveGroup, Group, Region};
use crate::{Request, Shared, TeleportTo};
use ferro_llsd::Llsd;
use ferro_msg::msgs::*;
use ferro_msg::{IncomingPacket, Msg, MsgId, field_str, parse_packet, str_field};
use glam::{Quat, Vec3};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// No packet from the region for this long: the connection is lost. 100 s like the viewer
/// (llstartup.cpp circuit_timeout) and Aurora's SIM_TIMEOUT: 60 s cut sessions that a laggy
/// region or a short network outage would survive.
const REGION_TIMEOUT: Duration = Duration::from_secs(100);
const PING_INTERVAL: Duration = Duration::from_secs(5);
const AGENT_UPDATE_INTERVAL: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(100);
/// How long a logout waits for the region's LogoutReply.
const LOGOUT_WAIT: Duration = Duration::from_secs(10);
/// No AgentMovementComplete this long after CompleteAgentMovement: the arrival failed
/// (STATE_AGENT_WAIT_TIMEOUT, llstartup.cpp; Aurora's LOGIN_ARRIVAL_TIMEOUT).
const ARRIVAL_TIMEOUT: Duration = Duration::from_secs(240);
/// Ids per UUIDNameRequest (Aurora's RequestNames).
const NAMES_PER_REQUEST: usize = 64;
/// TELEPORT_FLAGS_VIA_LURE (lltelepostflags.h); LibreMetaverse's TeleportLureRespond sends it.
const TELEPORT_VIA_LURE: u32 = 1 << 2;

/// How a session ended.
#[derive(Debug, Clone)]
pub enum End {
    /// Logged out on request (shutdown).
    LoggedOut,
    /// The grid ended the session (KickUser), with its reason.
    Kicked(String),
    /// The connection was lost.
    Lost(String),
}

impl std::fmt::Display for End {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            End::LoggedOut => write!(f, "logged out"),
            End::Kicked(reason) => write!(f, "disconnected by the grid: {reason}"),
            End::Lost(reason) => write!(f, "connection lost: {reason}"),
        }
    }
}

/// RegionHandshakeReply flags (LLViewerRegion::unpackRegionHandshake).
const SEND_ALL_CACHEABLE: u32 = 0x1;
const CACHE_EMPTY: u32 = 0x2;
const SUPPORTS_SELF_APPEARANCE: u32 = 0x4;

/// Bandwidth the region may use toward the bot, in bits per second, per channel
/// (resend, land, wind, cloud, task, texture, asset): a bot needs little of it.
const THROTTLES: [f32; 7] = [
    100_000.0, 30_000.0, 1_000.0, 1_000.0, 100_000.0, 30_000.0, 100_000.0,
];

/// The network task. One per client; [`Network::session`] runs each login's session.
pub struct Network {
    http: reqwest::Client,
    requests: mpsc::Receiver<Request>,
    shared: Arc<Shared>,
}

impl Network {
    pub(crate) fn new(
        http: reqwest::Client,
        requests: mpsc::Receiver<Request>,
        shared: Arc<Shared>,
    ) -> Self {
        Network {
            http,
            requests,
            shared,
        }
    }

    /// Connect to the login region and stay there until `shutdown` or the session ends.
    pub async fn session(&mut self, login: &LoginResponse, shutdown: watch::Receiver<bool>) -> End {
        {
            let mut st = self.shared.state_mut();
            st.online = false;
            st.agent_id = login.agent_id;
            st.name = format!("{} {}", login.first_name, login.last_name);
            st.region = None;
            st.home = login.home.or(st.home);
        }
        self.shared
            .names()
            .insert(login.agent_id, &login.first_name, &login.last_name);
        // requests left from before this session have no one waiting any more
        while self.requests.try_recv().is_ok() {}
        let end = run(
            &self.http,
            login,
            shutdown,
            &mut self.requests,
            &self.shared,
        )
        .await;
        {
            let mut st = self.shared.state_mut();
            st.online = false;
            st.region = None;
            st.online_since = None;
        }
        self.shared.emit(Event::Disconnected {
            reason: end.to_string(),
        });
        end
    }
}

type TeleportReply = oneshot::Sender<Result<String, String>>;

struct Session<'a> {
    login: &'a LoginResponse,
    shared: &'a Shared,
    http: &'a reqwest::Client,
    /// The current region's capabilities (secrets of the session, never logged).
    caps: HashMap<String, String>,
    /// The current region's event queue, and the task that polls it.
    eq_rx: mpsc::UnboundedReceiver<EqMessage>,
    eq_open: bool,
    caps_task: Option<JoinHandle<()>>,
    /// Arrivals in a region: the login's, then each teleport's.
    arrivals: u32,
    /// A teleport under way, and who waits for it.
    teleport: Option<Option<TeleportReply>>,
    sock: UdpSocket,
    circuit: Circuit,
    ucc_seq: u32,
    movement_sent: Option<Instant>,
    arrived: bool,
    region_name: String,
    position: Vec3,
    last_ping: Instant,
    last_update: Instant,
    /// Names asked for: what is still missing, and who waits.
    name_waits: Vec<(HashSet<Uuid>, oneshot::Sender<()>)>,
    /// Avatar searches, by query id.
    picker_waits: HashMap<Uuid, oneshot::Sender<Vec<(Uuid, String)>>>,
}

impl Session<'_> {
    fn addr(&self) -> SocketAddr {
        self.circuit.addr
    }

    fn send<M: Msg>(&mut self, msg: &M, reliable: bool) {
        self.circuit.send(&self.sock, msg, reliable);
    }

    fn send_use_circuit_code(&mut self) {
        let mut m = UseCircuitCode::default();
        m.circuit_code.code = self.login.circuit_code;
        m.circuit_code.session_id = self.login.session_id;
        m.circuit_code.id = self.login.agent_id;
        self.ucc_seq =
            self.circuit
                .send_reliable_with(&self.sock, &m, Resend::LOGIN_USE_CIRCUIT_CODE);
    }

    fn send_complete_agent_movement(&mut self) {
        let mut m = CompleteAgentMovement::default();
        m.agent_data.agent_id = self.login.agent_id;
        m.agent_data.session_id = self.login.session_id;
        m.agent_data.circuit_code = self.login.circuit_code;
        self.send(&m, true);

        let mut t = AgentThrottle::default();
        t.agent_data.agent_id = self.login.agent_id;
        t.agent_data.session_id = self.login.session_id;
        t.agent_data.circuit_code = self.login.circuit_code;
        t.throttle.gen_counter = 0;
        t.throttle.throttles = THROTTLES.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.send(&t, true);
        self.movement_sent = Some(Instant::now());
    }

    /// AgentUpdate: standing still, the camera just behind the avatar.
    fn send_agent_update(&mut self) {
        let mut m = AgentUpdate::default();
        let a = &mut m.agent_data;
        a.agent_id = self.login.agent_id;
        a.session_id = self.login.session_id;
        a.body_rotation = Quat::IDENTITY;
        a.head_rotation = Quat::IDENTITY;
        a.camera_center = self.position - Vec3::X * 3.0 + Vec3::Z;
        a.camera_at_axis = Vec3::X;
        a.camera_left_axis = Vec3::Y;
        a.camera_up_axis = Vec3::Z;
        a.far = 64.0;
        self.send(&m, false);
    }

    fn send_logout_request(&mut self) {
        let mut m = LogoutRequest::default();
        m.agent_data.agent_id = self.login.agent_id;
        m.agent_data.session_id = self.login.session_id;
        self.send(&m, true);
    }

    fn on_arrival(&mut self, region_handle: u64) {
        self.arrived = true;
        self.arrivals += 1;
        let first = self.arrivals == 1;
        let p = self.position;
        log::info!(
            "in world: {} {} at {}, {}, {}",
            self.login.first_name,
            self.login.last_name,
            p.x.round(),
            p.y.round(),
            p.z.round()
        );
        {
            let mut st = self.shared.state_mut();
            st.online = true;
            if first {
                st.logins += 1;
                st.online_since = Some(SystemTime::now());
            }
            st.position = p;
            st.region = Some(Region {
                name: self.region_name.clone(),
                handle: region_handle,
            });
        }
        self.shared.emit(Event::Arrived {
            region: self.region_name.clone(),
            position: p,
        });
        self.send_agent_update();
        if let Some(waiting) = self.teleport.take() {
            log::info!("teleported to {}", self.region_name);
            if let Some(reply) = waiting {
                let _ = reply.send(Ok(self.region_name.clone()));
            }
        }
        if first {
            // LLAgent: the active group and title (AgentDataUpdate), and the groups
            // (AgentGroupDataUpdate) come in answer
            let mut r = AgentDataUpdateRequest::default();
            r.agent_data.agent_id = self.login.agent_id;
            r.agent_data.session_id = self.login.session_id;
            self.send(&r, true);
        }
    }

    /// Ask the region's seed for the capabilities, then poll its event queue; the previous
    /// region's queue is dropped.
    fn start_caps(&mut self, seed: String) {
        if let Some(old) = self.caps_task.take() {
            old.abort();
        }
        let (tx, rx) = mpsc::unbounded_channel();
        self.eq_rx = rx;
        self.eq_open = true;
        let http = self.http.clone();
        self.caps_task = Some(tokio::spawn(async move {
            match caps::resolve_seed(&http, &seed).await {
                Ok(caps) => {
                    log::info!("{} capabilities", caps.len());
                    let _ = tx.send(EqMessage::Caps(caps.clone()));
                    match caps.get("EventQueueGet") {
                        Some(url) => caps::event_queue_loop(http, url.clone(), tx).await,
                        None => log::warn!("the region gave no event queue"),
                    }
                }
                Err(e) => {
                    log::warn!("seed capability failed: {e}");
                    let _ = tx.send(EqMessage::Lost);
                }
            }
        }));
    }

    /// TeleportFinish, CrossedRegion: the agent is now in the region at `addr`. A circuit to it,
    /// CompleteAgentMovement once it is up, and its capabilities (Aurora's switch_region,
    /// LibreMetaverse's TeleportFinishEventHandler).
    fn switch_region(&mut self, addr: SocketAddr, region_handle: u64, seed: String) {
        log::info!("moving to region {:x} at {addr}", region_handle);
        self.circuit.flush_acks(&self.sock);
        self.circuit = Circuit::new(addr);
        self.arrived = false;
        self.movement_sent = None;
        self.region_name.clear();
        self.caps.clear();
        self.shared.state_mut().region = None;
        self.send_use_circuit_code();
        self.start_caps(seed);
        // a crossing has no request behind it, but it is under way all the same
        if self.teleport.is_none() {
            self.teleport = Some(None);
        }
    }

    fn teleport_failed(&mut self, reason: String) {
        log::warn!("teleport failed: {reason}");
        if let Some(Some(reply)) = self.teleport.take() {
            let _ = reply.send(Err(reason.clone()));
        }
        self.shared.emit(Event::TeleportFailed { reason });
    }

    fn start_teleport(&mut self, to: TeleportTo, reply: TeleportReply) {
        if self.teleport.is_some() {
            let _ = reply.send(Err("a teleport is already under way".into()));
            return;
        }
        let (agent_id, session_id) = (self.login.agent_id, self.login.session_id);
        match to {
            TeleportTo::Lure(lure) => {
                let mut m = TeleportLureRequest::default();
                m.info.agent_id = agent_id;
                m.info.session_id = session_id;
                m.info.lure_id = lure;
                m.info.teleport_flags = TELEPORT_VIA_LURE;
                self.send(&m, true);
            }
            TeleportTo::Home => {
                // LLAgent::teleportHome: a landmark request without a landmark
                let mut m = TeleportLandmarkRequest::default();
                m.info.agent_id = agent_id;
                m.info.session_id = session_id;
                m.info.landmark_id = Uuid::nil();
                self.send(&m, true);
            }
            TeleportTo::Location {
                region_handle,
                position,
            } => {
                let mut m = TeleportLocationRequest::default();
                m.agent_data.agent_id = agent_id;
                m.agent_data.session_id = session_id;
                m.info.region_handle = region_handle;
                m.info.position = position;
                m.info.look_at = Vec3::X;
                self.send(&m, true);
            }
        }
        self.teleport = Some(Some(reply));
    }

    fn on_request(&mut self, req: Request) {
        match req {
            Request::Names { ids, reply } => {
                let missing: HashSet<Uuid> = {
                    let cache = self.shared.names();
                    ids.into_iter()
                        .filter(|id| cache.name(*id).is_none())
                        .collect()
                };
                if missing.is_empty() {
                    let _ = reply.send(());
                    return;
                }
                let list: Vec<Uuid> = missing.iter().copied().collect();
                for chunk in list.chunks(NAMES_PER_REQUEST) {
                    let m = UUIDNameRequest {
                        uuid_name_block: chunk
                            .iter()
                            .map(|id| uuid_name_request::UUIDNameBlock { id: *id })
                            .collect(),
                    };
                    self.send(&m, true);
                }
                self.name_waits.push((missing, reply));
            }
            Request::Teleport { to, reply } => self.start_teleport(to, reply),
            Request::DisplayNames { ids, reply } => match self.caps.get("GetDisplayNames") {
                Some(cap) => {
                    tokio::spawn(fetch_display_names(
                        self.http.clone(),
                        cap.clone(),
                        ids,
                        reply,
                    ));
                }
                None => {
                    let _ = reply.send(Err("no GetDisplayNames capability".into()));
                }
            },
            Request::FindAvatars { name, reply } => {
                let query = Uuid::new_v4();
                let mut m = AvatarPickerRequest::default();
                m.agent_data.agent_id = self.login.agent_id;
                m.agent_data.session_id = self.login.session_id;
                m.agent_data.query_id = query;
                m.data.name = str_field(&name);
                self.send(&m, true);
                self.picker_waits.insert(query, reply);
            }
        }
    }

    fn on_names(&mut self, names: impl IntoIterator<Item = (Uuid, String, String)>) {
        let mut got = Vec::new();
        {
            let mut cache = self.shared.names();
            for (id, first, last) in names {
                cache.insert(id, &first, &last);
                got.push(id);
            }
        }
        for (missing, _) in &mut self.name_waits {
            for id in &got {
                missing.remove(id);
            }
        }
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.name_waits)
            .into_iter()
            .partition(|(missing, _)| missing.is_empty());
        self.name_waits = waiting;
        for (_, reply) in done {
            let _ = reply.send(());
        }
    }

    fn set_groups(&mut self, groups: Vec<Group>) {
        log::debug!("in {} groups", groups.len());
        self.shared.state_mut().groups = groups;
        self.shared.emit(Event::GroupsChanged);
    }

    /// Timers: resends and acks, the arrival, pings, agent updates, the region's silence.
    fn tick(&mut self) -> Option<End> {
        let now = Instant::now();
        self.circuit.tick(&self.sock);
        if self.circuit.take_dropped().contains(&self.ucc_seq) {
            return Some(End::Lost(
                "the region never acknowledged UseCircuitCode".into(),
            ));
        }
        if self.movement_sent.is_none() && !self.circuit.is_unacked(self.ucc_seq) {
            log::info!("circuit to {} up", self.addr());
            self.send_complete_agent_movement();
        }
        if !self.arrived
            && let Some(sent) = self.movement_sent
            && now.duration_since(sent) > ARRIVAL_TIMEOUT
        {
            // The session exists on the grid: log it out, or the next login finds it still there
            // and is refused for a minute or two (llstartup.cpp STATE_AGENT_WAIT, Aurora's session).
            self.send_logout_request();
            return Some(End::Lost("no AgentMovementComplete from the region".into()));
        }
        if now.duration_since(self.last_ping) >= PING_INTERVAL {
            self.last_ping = now;
            let (id, oldest) = self.circuit.next_ping();
            let mut m = StartPingCheck::default();
            m.ping_id.ping_id = id;
            m.ping_id.oldest_unacked = oldest;
            self.send(&m, false);
        }
        if self.arrived && now.duration_since(self.last_update) >= AGENT_UPDATE_INTERVAL {
            self.last_update = now;
            self.send_agent_update();
        }
        // callers that gave up waiting
        self.name_waits.retain(|(_, reply)| !reply.is_closed());
        self.picker_waits.retain(|_, reply| !reply.is_closed());
        if now.duration_since(self.circuit.last_recv) > REGION_TIMEOUT {
            return Some(End::Lost(format!(
                "nothing from the region for {} s",
                REGION_TIMEOUT.as_secs()
            )));
        }
        None
    }

    fn on_datagram(&mut self, data: &[u8], from: SocketAddr) -> Option<End> {
        if from != self.addr() {
            return None;
        }
        let pkt = match parse_packet(data) {
            Ok(pkt) => pkt,
            Err(e) => {
                log::debug!("bad packet from {from}: {e}");
                return None;
            }
        };
        if !self.circuit.on_receive(pkt.flags, pkt.sequence, &pkt.acks) {
            return None;
        }
        self.shared.state_mut().last_packet = Some(Instant::now());
        match self.dispatch(&pkt) {
            Ok(end) => {
                self.circuit.accept(pkt.flags, pkt.sequence);
                end
            }
            Err(e) => {
                let name = ferro_msg::message_info(pkt.id).map_or("?", |i| i.name);
                log::warn!("failed to decode {name}: {e}");
                None
            }
        }
    }

    fn dispatch(&mut self, pkt: &IncomingPacket) -> Result<Option<End>, ferro_msg::DecodeError> {
        let id: MsgId = pkt.id;
        if id == PacketAck::ID {
            let m: PacketAck = pkt.decode()?;
            self.circuit.ack_received(m.packets.iter().map(|p| p.id));
        } else if id == StartPingCheck::ID {
            let m: StartPingCheck = pkt.decode()?;
            let mut r = CompletePingCheck::default();
            r.ping_id.ping_id = m.ping_id.ping_id;
            self.send(&r, false);
        } else if id == CompletePingCheck::ID {
            let m: CompletePingCheck = pkt.decode()?;
            if let Some(rtt) = self.circuit.on_ping_reply(m.ping_id.ping_id) {
                log::trace!("ping {} ms", rtt.as_millis());
            }
        } else if id == RegionHandshake::ID {
            let m: RegionHandshake = pkt.decode()?;
            self.region_name = field_str(&m.region_info.sim_name);
            log::info!("region handshake: {}", self.region_name);
            let mut reply = RegionHandshakeReply::default();
            reply.agent_data.agent_id = self.login.agent_id;
            reply.agent_data.session_id = self.login.session_id;
            // send everything cacheable, the bot keeps no object cache, and it bakes its own
            // appearance (Aurora's object_cache.rs, LibreMetaverse's NetworkManager both send these)
            reply.region_info.flags = SEND_ALL_CACHEABLE | CACHE_EMPTY | SUPPORTS_SELF_APPEARANCE;
            self.send(&reply, true);
        } else if id == AgentMovementComplete::ID {
            let m: AgentMovementComplete = pkt.decode()?;
            self.position = m.data.position;
            if !self.arrived {
                self.on_arrival(m.data.region_handle);
            } else {
                self.shared.state_mut().position = m.data.position;
            }
        } else if id == ChatFromSimulator::ID {
            let m: ChatFromSimulator = pkt.decode()?;
            let c = m.chat_data;
            self.shared.emit(Event::Chat(ChatMessage {
                from_name: field_str(&c.from_name),
                source_id: c.source_id,
                owner_id: c.owner_id,
                source_type: c.source_type,
                chat_type: c.chat_type,
                audible: c.audible,
                position: c.position,
                message: field_str(&c.message),
            }));
        } else if id == ImprovedInstantMessage::ID {
            let m: ImprovedInstantMessage = pkt.decode()?;
            let b = m.message_block;
            self.shared.emit(Event::InstantMessage(InstantMessage {
                from_agent_id: m.agent_data.agent_id,
                from_name: field_str(&b.from_agent_name),
                to_agent_id: b.to_agent_id,
                dialog: b.dialog,
                id: b.id,
                message: field_str(&b.message),
                from_group: b.from_group,
                offline: b.offline != 0,
                timestamp: b.timestamp,
                region_id: b.region_id,
                position: b.position,
                binary_bucket: b.binary_bucket,
            }));
        } else if id == AlertMessage::ID {
            let m: AlertMessage = pkt.decode()?;
            // OpenSim pads its messages with spaces and newlines
            let message = field_str(&m.alert_data.message).trim().to_owned();
            // a named notification (AlertInfo) first, then OpenSim's system messages
            // (attempt_standard_notification, process_alert_core; Aurora's session)
            let named = m.alert_info.first().and_then(|info| {
                region_restart::from_alert_info(&field_str(&info.message), &info.extra_params)
            });
            match named
                .map(Restart::Notice)
                .or_else(|| region_restart::from_text(&message))
            {
                Some(Restart::Notice(n)) => {
                    log::warn!("the region restarts in {} s", n.seconds);
                    self.shared.emit(Event::RegionRestart {
                        seconds: n.seconds,
                        region: n.region,
                    });
                }
                Some(Restart::Cancelled) => {
                    log::info!("the region restart was cancelled");
                    self.shared.emit(Event::RegionRestartCancelled);
                }
                None if !message.is_empty() => {
                    log::info!("alert: {message}");
                    self.shared.emit(Event::Alert(message));
                }
                None => {}
            }
        } else if id == AgentAlertMessage::ID {
            let m: AgentAlertMessage = pkt.decode()?;
            let message = field_str(&m.alert_data.message).trim().to_owned();
            log::info!("alert: {message}");
            self.shared.emit(Event::Alert(message));
        } else if id == AgentDataUpdate::ID {
            // LLAgent::processAgentDataUpdate
            let m: AgentDataUpdate = pkt.decode()?;
            let a = m.agent_data;
            if a.agent_id == self.login.agent_id {
                self.shared.state_mut().active_group =
                    (!a.active_group_id.is_nil()).then(|| ActiveGroup {
                        id: a.active_group_id,
                        name: field_str(&a.group_name),
                        title: field_str(&a.group_title),
                    });
            }
        } else if id == AgentGroupDataUpdate::ID {
            let m: AgentGroupDataUpdate = pkt.decode()?;
            let groups = m
                .group_data
                .iter()
                .filter(|g| !g.group_id.is_nil())
                .map(|g| Group {
                    id: g.group_id,
                    name: field_str(&g.group_name),
                    insignia: g.group_insignia_id,
                    powers: g.group_powers,
                    accept_notices: g.accept_notices,
                    contribution: g.contribution,
                })
                .collect();
            self.set_groups(groups);
        } else if id == AgentDropGroup::ID {
            let m: AgentDropGroup = pkt.decode()?;
            self.drop_group(m.agent_data.group_id);
        } else if id == UUIDNameReply::ID {
            let m: UUIDNameReply = pkt.decode()?;
            self.on_names(
                m.uuid_name_block
                    .iter()
                    .map(|b| (b.id, field_str(&b.first_name), field_str(&b.last_name))),
            );
        } else if id == AvatarPickerReply::ID {
            let m: AvatarPickerReply = pkt.decode()?;
            let found: Vec<(Uuid, String, String)> = m
                .data
                .iter()
                .filter(|d| !d.avatar_id.is_nil())
                .map(|d| {
                    (
                        d.avatar_id,
                        field_str(&d.first_name),
                        field_str(&d.last_name),
                    )
                })
                .collect();
            let list = found
                .iter()
                .map(|(id, first, last)| (*id, format!("{first} {last}")))
                .collect();
            self.on_names(found);
            if let Some(reply) = self.picker_waits.remove(&m.agent_data.query_id) {
                let _ = reply.send(list);
            }
        } else if id == TeleportStart::ID {
            let m: TeleportStart = pkt.decode()?;
            log::debug!("teleport started, flags {:#x}", m.info.teleport_flags);
            if self.teleport.is_none() {
                // begun by the region (a god teleport, a home teleport from a script…)
                self.teleport = Some(None);
            }
        } else if id == TeleportProgress::ID {
            let m: TeleportProgress = pkt.decode()?;
            log::debug!("teleport: {}", field_str(&m.info.message));
        } else if id == TeleportFailed::ID {
            let m: TeleportFailed = pkt.decode()?;
            self.teleport_failed(field_str(&m.info.reason));
        } else if id == TeleportLocal::ID {
            // process_teleport_local: moved inside the region
            let m: TeleportLocal = pkt.decode()?;
            self.position = m.info.position;
            self.shared.state_mut().position = m.info.position;
            log::info!("teleported inside the region");
            if let Some(Some(reply)) = self.teleport.take() {
                let _ = reply.send(Ok(self.region_name.clone()));
            }
            self.send_agent_update();
        } else if id == KickUser::ID {
            let m: KickUser = pkt.decode()?;
            let reason = field_str(&m.user_info.reason).trim().to_owned();
            return Ok(Some(End::Kicked(reason)));
        } else if id == LogoutReply::ID {
            return Ok(Some(End::LoggedOut));
        } else if id == DisableSimulator::ID {
            return Ok(Some(End::Lost("the region closed the connection".into())));
        }
        Ok(None)
    }

    fn drop_group(&mut self, group: Uuid) {
        if group.is_nil() {
            return;
        }
        let mut groups = self.shared.state().groups.clone();
        groups.retain(|g| g.id != group);
        self.set_groups(groups);
    }

    fn on_event(&mut self, e: EqEvent) -> Option<End> {
        // Only the field names: some bodies hold capability URLs (TeleportFinish,
        // EstablishAgentCommunication…), which are secrets of the session.
        let fields: Vec<&str> = e
            .body
            .as_map()
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default();
        log::debug!("event: {} {fields:?}", e.message);
        match e.message.as_str() {
            "DisableSimulator" => {
                return Some(End::Lost(
                    "the region closed the connection (event queue)".into(),
                ));
            }
            // The grid sends the group list on the event queue (LLAgent::processAgentGroupDataUpdate
            // via LLAgentGroupDataUpdateViewerNode; Aurora's social::parse_group_data)
            "AgentGroupDataUpdate" => self.set_groups(parse_group_data(&e.body)),
            "AgentDropGroup" => self.drop_group(e.body["AgentData"][0]["GroupID"].as_uuid()),
            // LLAgent::processTeleportFinish, LLViewerRegion's CrossedRegion
            "TeleportFinish" | "CrossedRegion" => {
                let key = if e.message == "TeleportFinish" {
                    "Info"
                } else {
                    "RegionData"
                };
                let info = &e.body[key][0];
                let Some(ip) = ip_from_llsd(&info["SimIP"]) else {
                    log::warn!("{} without a region address", e.message);
                    return None;
                };
                let port = u16::try_from(info["SimPort"].as_i32()).unwrap_or(0);
                let seed = info["SeedCapability"].as_str().to_owned();
                if port == 0 || seed.is_empty() {
                    log::warn!("{} without a port or a seed capability", e.message);
                    return None;
                }
                self.switch_region(
                    SocketAddr::from((ip, port)),
                    info["RegionHandle"].as_u64(),
                    seed,
                );
            }
            "TeleportFailed" => {
                self.teleport_failed(e.body["Info"][0]["Reason"].as_str().to_owned());
            }
            _ => {}
        }
        None
    }
}

/// LLAvatarNameCache::requestNamesViaCapability (Aurora's request_display_names): GET
/// `<GetDisplayNames>/?ids=<id>&ids=<id>…`, URLs kept under ~3500 characters.
async fn fetch_display_names(
    http: reqwest::Client,
    cap: String,
    ids: Vec<Uuid>,
    reply: oneshot::Sender<Result<Vec<DisplayName>, String>>,
) {
    const NAME_URL_SEND_THRESHOLD: usize = 3500;
    // caps are granted without the slash the query needs
    let base = if cap.ends_with('/') {
        cap
    } else {
        format!("{cap}/")
    };
    let mut urls: Vec<String> = Vec::new();
    for id in ids {
        match urls.last_mut() {
            Some(url) if url.len() <= NAME_URL_SEND_THRESHOLD => {
                url.push_str("&ids=");
                url.push_str(&id.to_string());
            }
            _ => urls.push(format!("{base}?ids={id}")),
        }
    }
    let mut found = Vec::new();
    for url in urls {
        let resp = match http
            .get(&url)
            .header("Accept", "application/llsd+xml")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => resp,
            Ok(resp) => {
                let _ = reply.send(Err(format!("GetDisplayNames: HTTP {}", resp.status())));
                return;
            }
            Err(e) => {
                let _ = reply.send(Err(format!("GetDisplayNames: {}", e.without_url())));
                return;
            }
        };
        match resp
            .bytes()
            .await
            .ok()
            .and_then(|b| ferro_llsd::from_xml(&b).ok())
        {
            Some(v) if v.is_map() => found.extend(
                v["agents"]
                    .as_array()
                    .iter()
                    .map(DisplayName::from_llsd)
                    .filter(|d| !d.id.is_nil()),
            ),
            _ => {
                let _ = reply.send(Err("GetDisplayNames: an invalid reply".into()));
                return;
            }
        }
    }
    let _ = reply.send(Ok(found));
}

/// An IPv4 address as the event queue carries it (Aurora's ip_from_llsd).
fn ip_from_llsd(v: &Llsd) -> Option<Ipv4Addr> {
    match v {
        Llsd::Binary(b) if b.len() == 4 => Some(Ipv4Addr::new(b[0], b[1], b[2], b[3])),
        Llsd::String(s) => s.parse().ok(),
        Llsd::Integer(i) => Some(Ipv4Addr::from((*i as u32).to_be_bytes())),
        _ => None,
    }
}

/// The groups of an AgentGroupDataUpdate event (aurora-net social::parse_group_data).
fn parse_group_data(body: &Llsd) -> Vec<Group> {
    body["GroupData"]
        .as_array()
        .iter()
        .filter(|g| !g["GroupID"].as_uuid().is_nil())
        .map(|g| Group {
            id: g["GroupID"].as_uuid(),
            name: g["GroupName"].as_str().to_owned(),
            insignia: g["GroupInsigniaID"].as_uuid(),
            powers: g["GroupPowers"].as_u64(),
            accept_notices: g["AcceptNotices"].as_bool(),
            contribution: g["Contribution"].as_i32(),
        })
        .collect()
}

async fn run(
    http: &reqwest::Client,
    login: &LoginResponse,
    mut shutdown: watch::Receiver<bool>,
    requests: &mut mpsc::Receiver<Request>,
    shared: &Shared,
) -> End {
    let addr = SocketAddr::from((login.sim_ip, login.sim_port));
    let sock = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(sock) => sock,
        Err(e) => return End::Lost(format!("no UDP socket: {e}")),
    };
    let (_, eq_rx) = mpsc::unbounded_channel();
    let mut s = Session {
        caps: HashMap::new(),
        login,
        shared,
        http,
        eq_rx,
        eq_open: false,
        caps_task: None,
        arrivals: 0,
        teleport: None,
        sock,
        circuit: Circuit::new(addr),
        ucc_seq: 0,
        movement_sent: None,
        arrived: false,
        region_name: String::new(),
        position: Vec3::new(128.0, 128.0, 0.0),
        last_ping: Instant::now(),
        last_update: Instant::now(),
        name_waits: Vec::new(),
        picker_waits: HashMap::new(),
    };
    s.send_use_circuit_code();
    // capabilities and the event queue, alongside the circuit
    s.start_caps(login.seed_capability.clone());

    let mut buf = vec![0u8; 8192];
    let mut tick = tokio::time::interval(TICK);
    let mut logging_out: Option<Instant> = None;
    let end = loop {
        tokio::select! {
            r = s.sock.recv_from(&mut buf) => match r {
                Ok((n, from)) => {
                    if let Some(end) = s.on_datagram(&buf[..n], from) {
                        break end;
                    }
                }
                Err(e) => log::debug!("udp receive: {e}"),
            },
            _ = tick.tick() => {
                if let Some(at) = logging_out && at.elapsed() > LOGOUT_WAIT {
                    log::warn!("no LogoutReply in {} s", LOGOUT_WAIT.as_secs());
                    break End::LoggedOut;
                }
                if let Some(end) = s.tick() {
                    break end;
                }
            }
            // requests wait for the arrival: the region answers nothing before
            req = requests.recv(), if s.arrived && logging_out.is_none() => {
                if let Some(req) = req {
                    s.on_request(req);
                }
            }
            // The capabilities task drops its sender when it ends; a closed channel is always
            // ready, so its branch is switched off then, or the loop would spin.
            ev = s.eq_rx.recv(), if s.eq_open => match ev {
                Some(EqMessage::Caps(caps)) => s.caps = caps,
                Some(EqMessage::Event(e)) => {
                    if let Some(end) = s.on_event(e) {
                        break end;
                    }
                }
                // Without the region's event queue, IM sessions, group chat and teleports fail
                // and nothing gets better before a new login (LLEventPollImpl, Aurora's
                // on_eq_lost): end the session so that the bot logs in again.
                Some(EqMessage::Lost) => break End::Lost("the region's event queue was lost".into()),
                None => s.eq_open = false,
            },
            changed = shutdown.changed(), if logging_out.is_none() => {
                if changed.is_err() || *shutdown.borrow() {
                    log::info!("logging out");
                    s.send_logout_request();
                    logging_out = Some(Instant::now());
                }
            }
        }
    };
    s.circuit.flush_acks(&s.sock);
    if let Some(task) = s.caps_task.take() {
        task.abort();
    }
    if let Some(Some(reply)) = s.teleport.take() {
        let _ = reply.send(Err("the session ended".into()));
    }
    // Once the logout is asked, however the region ends the session (LogoutReply, a kick,
    // DisableSimulator, its queue closing), the bot is out as it was asked to be.
    if logging_out.is_some() {
        return End::LoggedOut;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn login(sim_port: u16, seed: String) -> LoginResponse {
        LoginResponse {
            agent_id: uuid::Uuid::from_u128(1),
            session_id: uuid::Uuid::from_u128(2),
            first_name: "Test".into(),
            last_name: "Bot".into(),
            sim_ip: Ipv4Addr::LOCALHOST,
            sim_port,
            circuit_code: 3,
            seed_capability: seed,
            message: String::new(),
            secure_session_id: uuid::Uuid::from_u128(4),
            inventory_root: uuid::Uuid::nil(),
            home: None,
        }
    }

    /// A region whose seed capability never answers leaves the bot without its event queue: the
    /// session ends, so that the bot logs in again, instead of staying half connected.
    #[tokio::test(start_paused = true)]
    async fn a_lost_event_queue_ends_the_session() {
        let region = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let seed = format!("http://{}/seed", closed.local_addr().expect("address"));
        drop(closed);
        let port = region.local_addr().expect("address").port();
        let (_stop_tx, stop_rx) = watch::channel(false);
        let (_client, mut network) = crate::Client::new(reqwest::Client::new());
        let end = network.session(&login(port, seed), stop_rx).await;
        assert!(
            matches!(&end, End::Lost(why) if why.contains("event queue")),
            "{end:?}"
        );
    }

    /// Asked to log out, the session ends as logged out even when the region drops the circuit
    /// instead of answering LogoutReply.
    #[tokio::test]
    async fn a_session_closed_during_logout_is_logged_out() {
        let region = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let port = region.local_addr().expect("address").port();
        let (stop_tx, stop_rx) = watch::channel(false);
        let (_client, mut network) = crate::Client::new(reqwest::Client::new());
        let resp = login(port, "http://127.0.0.1:9/seed".into());
        let session = network.session(&resp, stop_rx);
        let region_side = async {
            let mut buf = [0u8; 2048];
            let (_, bot) = region.recv_from(&mut buf).await.expect("UseCircuitCode");
            stop_tx.send(true).expect("stop");
            // wait for the LogoutRequest, then close the circuit
            loop {
                let (n, _) = region.recv_from(&mut buf).await.expect("packet");
                if parse_packet(&buf[..n]).is_ok_and(|p| p.id == LogoutRequest::ID) {
                    break;
                }
            }
            let mut out = Vec::new();
            let mut w = ferro_msg::Writer::new();
            DisableSimulator::ID.encode(&mut w.buf);
            DisableSimulator::default().encode_body(&mut w);
            ferro_msg::packet::build_raw(&w.buf, false, 1, false, false, &[], &mut out);
            region.send_to(&out, bot).await.expect("send");
        };
        let (end, ()) = tokio::join!(session, region_side);
        assert!(matches!(end, End::LoggedOut), "{end:?}");
    }
}
