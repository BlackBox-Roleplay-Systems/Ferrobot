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

mod estate;
mod groups;
mod inventory;
mod objects;
mod parcels;
mod profile;
pub(crate) use estate::EstateRequest;
pub(crate) use objects::ObjectRequest;
pub(crate) use parcels::ParcelRequest;
pub use parcels::{ObjectOwner, ParcelAction};
pub(crate) use profile::ProfileRequest;

pub(crate) use groups::GroupRequest;
pub(crate) use inventory::InventoryRequest;

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

/// How long the client bears with a region that fails it. The defaults are the viewer's and
/// Aurora's; shorter ones are for tests, so that they need not wait minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// No packet from the region this long: the connection is lost (100 s).
    pub region_silence: Duration,
    /// Fast event queue failures in a row before the queue is lost and the session ends (15,
    /// with 1 s + 3 s per error between them).
    pub event_queue_errors: u32,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            region_silence: REGION_TIMEOUT,
            event_queue_errors: caps::MAX_ERRORS,
        }
    }
}
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
    timeouts: Timeouts,
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
            timeouts: Timeouts::default(),
        }
    }

    /// From the next session on.
    pub fn set_timeouts(&mut self, timeouts: Timeouts) {
        self.timeouts = timeouts;
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
            // chat sessions end with the login
            st.group_chats.clear();
            st.friends = login.buddies.clone();
            st.friends_online.clear();
            st.nearby.clear();
            st.nearby_positions.clear();
            st.sitting_on = None;
            st.avatars.clear();
            st.parcel = None;
            st.region_parcels = None;
            st.mute_list = None;
            st.away = false;
            st.inventory_root = login.inventory_root;
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
            self.timeouts,
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
    timeouts: Timeouts,
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
    /// Group replies being gathered.
    groups: groups::GroupWaits,
    /// Control flags in every agent update (fly), and the ones for the next only (stand up…).
    controls: u32,
    controls_once: u32,
    /// The body's rotation in the agent updates.
    rotation: Quat,
    /// Inventory items being created, by callback number.
    created: HashMap<u32, oneshot::Sender<Uuid>>,
    next_callback: u32,
    /// The outfit was asked to be baked this session.
    appearance_asked: bool,
    /// Region names asked of the world map (lower case), and who waits.
    map_waits: Vec<(String, oneshot::Sender<Option<crate::MapRegion>>)>,
    /// The region's id (RegionHandshake).
    region_id: Uuid,
    /// The bot manages the region's estate (RegionHandshake).
    estate_manager: bool,
    region_product: String,
    /// The invoice of the last estate `getinfo`.
    estate_invoice: Uuid,
    estate_waits: estate::EstateWaits,
    profile_waits: profile::ProfileWaits,
    /// Parcel replies being waited for.
    parcel_waits: parcels::ParcelWaits,
    /// The mute list's file, being downloaded.
    mute_xfer: Option<crate::mutes::XferDownload>,
    /// Where the region's root objects and the bot's avatar stand (the object updates).
    places: crate::objects::Places,
    /// Protected assets being downloaded (TransferRequest).
    transfers: crate::transfer::Transfers,
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
        a.body_rotation = self.rotation;
        a.head_rotation = self.rotation;
        // the camera behind the avatar, looking where it looks
        let at = self.rotation * Vec3::X;
        a.camera_center = self.position - at * 3.0 + Vec3::Z;
        a.camera_at_axis = at;
        a.camera_left_axis = self.rotation * Vec3::Y;
        a.camera_up_axis = Vec3::Z;
        a.far = 64.0;
        a.control_flags = self.controls | std::mem::take(&mut self.controls_once);
        self.send(&m, false);
    }

    /// An instant message of any dialog, from the bot (LLIMModel::sendMessage's pack_instant_message).
    fn send_im(&mut self, to: Uuid, dialog: u8, id: Uuid, message: &str, bucket: Vec<u8>) {
        let mut m = ImprovedInstantMessage::default();
        m.agent_data.agent_id = self.login.agent_id;
        m.agent_data.session_id = self.login.session_id;
        let b = &mut m.message_block;
        b.to_agent_id = to;
        b.dialog = dialog;
        b.id = id;
        b.position = self.position;
        b.from_agent_name = str_field(&format!(
            "{} {}",
            self.login.first_name, self.login.last_name
        ));
        b.message = str_field(message);
        // an empty bucket is sent as one zero byte, as the viewer does
        b.binary_bucket = if bucket.is_empty() { vec![0] } else { bucket };
        self.send(&m, true);
    }

    fn interact(&mut self, i: crate::Interaction) {
        use crate::Interaction;
        use crate::events::dialog;
        let (agent, session) = (self.login.agent_id, self.login.session_id);
        let root = self.login.inventory_root;
        match i {
            Interaction::AcceptFriendship { transaction, .. } => {
                // LLAvatarActions / LibreMetaverse AcceptFriendship: the calling card's folder
                let mut m = AcceptFriendship::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.transaction_block.transaction_id = transaction;
                m.folder_data = vec![accept_friendship::FolderData { folder_id: root }];
                self.send(&m, true);
            }
            Interaction::AnswerInventory {
                to,
                transaction,
                accept,
                from_task,
            } => {
                // inventory_offer_callback: the destination folder in the bucket
                let d = match (from_task, accept) {
                    (false, true) => dialog::INVENTORY_ACCEPTED,
                    (false, false) => dialog::INVENTORY_DECLINED,
                    (true, true) => dialog::TASK_INVENTORY_ACCEPTED,
                    (true, false) => dialog::TASK_INVENTORY_DECLINED,
                };
                let bucket = if accept {
                    root.as_bytes().to_vec()
                } else {
                    Vec::new()
                };
                self.send_im(to, d, transaction, "", bucket);
            }
            Interaction::RequestBalance => self.request_balance(),
            Interaction::Move(mv) => self.move_body(mv),
            Interaction::Pay {
                to,
                amount,
                object_name,
            } => {
                // LLGiveMoney / LLPayObject: a gift (5001) or an object's pay (5008)
                let mut m = MoneyTransferRequest::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.money_data.source_id = agent;
                m.money_data.dest_id = to;
                m.money_data.amount = amount;
                m.money_data.transaction_type = if object_name.is_some() { 5008 } else { 5001 };
                m.money_data.description = str_field(object_name.as_deref().unwrap_or(""));
                self.send(&m, true);
            }
            Interaction::RequestTeleport { to, message } => {
                self.send_im(
                    to,
                    dialog::TELEPORT_REQUEST,
                    Uuid::new_v4(),
                    &message,
                    Vec::new(),
                );
            }
            Interaction::OfferFriendship { to } => {
                self.send_im(
                    to,
                    dialog::FRIENDSHIP_OFFERED,
                    Uuid::new_v4(),
                    "",
                    Vec::new(),
                );
            }
            Interaction::EndFriendship { to } => {
                let mut m = TerminateFriendship::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.ex_block.other_id = to;
                self.send(&m, true);
            }
            Interaction::GrantRights { to, rights } => {
                let mut m = GrantUserRights::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.rights = vec![grant_user_rights::Rights {
                    agent_related: to,
                    related_rights: rights,
                }];
                self.send(&m, true);
            }
            Interaction::DialogReply {
                object_id,
                channel,
                index,
                label,
            } => {
                let mut m = ScriptDialogReply::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.data.object_id = object_id;
                m.data.chat_channel = channel;
                m.data.button_index = index;
                m.data.button_label = str_field(&label);
                self.send(&m, true);
            }
            Interaction::OfferTeleport { to, message } => {
                // LLAvatarActions::offerTeleport: StartLure, lure type 0
                let mut m = StartLure::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.info.lure_type = 0;
                m.info.message = str_field(&message);
                m.target_data = vec![start_lure::TargetData { target_id: to }];
                self.send(&m, true);
            }
            Interaction::DeclineTeleportRequest { to, transaction } => {
                self.send_im(to, dialog::LURE_DECLINED, transaction, "", Vec::new());
            }
            Interaction::FindAgent { prey } => {
                // LLAvatarTracker::findAgent: the region fills in the place
                let mut m = FindAgent::default();
                m.agent_block.hunter = agent;
                m.agent_block.prey = prey;
                m.location_block.push(find_agent::LocationBlock {
                    global_x: 0.0,
                    global_y: 0.0,
                });
                self.send(&m, true);
            }
            Interaction::RequestMuteList => self.request_mute_list(),
            Interaction::Animate { anims } => {
                let mut m = AgentAnimation::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.animation_list = anims
                    .into_iter()
                    .map(|(anim_id, start_anim)| agent_animation::AnimationList {
                        anim_id,
                        start_anim,
                    })
                    .collect();
                self.send(&m, true);
            }
            Interaction::Sound { asset } => {
                // LLGestureMgr's sound step: send_sound_trigger at the agent
                let handle = self.shared.state().region.as_ref().map_or(0, |r| r.handle);
                let mut m = SoundTrigger::default();
                m.sound_data.sound_id = asset;
                m.sound_data.owner_id = agent;
                m.sound_data.handle = handle;
                m.sound_data.position = self.position;
                m.sound_data.gain = 1.0;
                self.send(&m, false);
            }
            Interaction::PointAt { target } => {
                // LLHUDEffectPointAt (15, POINTAT_TARGET_SELECT) and LLHUDEffectBeam (7), as
                // LibreMetaverse's PointAtEffect and BeamEffect; Aurora's pointat_effect
                let attention = |kind: u8, target: Uuid, offset: [f64; 3], extra: Option<u8>| {
                    let mut d = Vec::with_capacity(57);
                    d.extend_from_slice(agent.as_bytes());
                    d.extend_from_slice(target.as_bytes());
                    for v in offset {
                        d.extend_from_slice(&v.to_le_bytes());
                    }
                    if let Some(b) = extra {
                        d.push(b);
                    }
                    viewer_effect::Effect {
                        id: Uuid::new_v4(),
                        agent_id: agent,
                        type_: kind,
                        duration: if kind == 15 { f32::MAX / 4.0 } else { 3.0 },
                        color: vec![255, 255, 255, 255],
                        type_data: d,
                    }
                };
                let mut m = ViewerEffect::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.effect = vec![
                    attention(15, target, [0.0; 3], Some(1)),
                    attention(7, target, [0.0, 0.0, 2.0], None),
                ];
                self.send(&m, false);
            }
            Interaction::ScriptAnswer {
                task,
                item,
                questions,
            } => {
                let mut m = ScriptAnswerYes::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.data.task_id = task;
                m.data.item_id = item;
                m.data.questions = questions;
                self.send(&m, true);
            }
        }
    }

    /// Movement (LLAgent, LibreMetaverse's AgentMovement): control flags in the agent update,
    /// sent at once, or the messages the viewer sends.
    fn move_body(&mut self, mv: crate::Move) {
        use crate::Move;
        // AGENT_CONTROL_* (agent.h)
        const FLY: u32 = 0x0000_2000;
        const STOP: u32 = 0x0000_4000;
        const STAND_UP: u32 = 0x0001_0000;
        const SIT_ON_GROUND: u32 = 0x0002_0000;
        // AGENT_CONTROL_AWAY; ANIM_AGENT_AWAY (llanimationstates.h)
        const AWAY: u32 = 0x0200_0000;
        const AWAY_ANIMATION: Uuid = Uuid::from_u128(0xfd037134_85d4_f241_72c6_4f42164fedee);
        let (agent, session) = (self.login.agent_id, self.login.session_id);
        match mv {
            Move::Away(on) => {
                // LLAgent::setAFK / clearAFK: the away animation, and the flag in every update
                let mut m = AgentAnimation::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.animation_list.push(agent_animation::AnimationList {
                    anim_id: AWAY_ANIMATION,
                    start_anim: on,
                });
                self.send(&m, true);
                if on {
                    self.controls |= AWAY;
                    self.controls_once |= STOP;
                } else {
                    self.controls &= !AWAY;
                }
                self.shared.state_mut().away = on;
            }
            Move::Fly(on) => {
                if on {
                    self.controls |= FLY;
                } else {
                    self.controls &= !FLY;
                }
                self.shared.state_mut().flying = on;
            }
            Move::AlwaysRun(on) => {
                let mut m = SetAlwaysRun::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.agent_data.always_run = on;
                self.send(&m, true);
                return;
            }
            Move::SitOnGround => self.controls_once |= SIT_ON_GROUND,
            Move::SitOn(target) => {
                let mut m = AgentRequestSit::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.target_object.target_id = target;
                self.send(&m, true);
                return;
            }
            Move::Stand => {
                self.controls_once |= STAND_UP;
                self.shared.state_mut().sitting_on = None;
            }
            Move::AutoPilot(target) => {
                // global coordinates: the region's corner plus the place
                let handle = self.shared.state().region.as_ref().map_or(0, |r| r.handle);
                let (gx, gy) = ((handle >> 32) as f64, (handle & 0xffff_ffff) as f64);
                let mut m = GenericMessage::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.method_data.method = str_field("autopilot");
                m.param_list = [
                    format!("{}", (gx + f64::from(target.x)) as u64),
                    format!("{}", (gy + f64::from(target.y)) as u64),
                    format!("{}", target.z),
                ]
                .iter()
                .map(|p| generic_message::ParamList {
                    parameter: str_field(p),
                })
                .collect();
                self.send(&m, true);
                return;
            }
            Move::Stop => self.controls_once |= STOP,
            Move::Face(heading) => {
                // a heading clockwise from north is a yaw counter-clockwise from east
                let yaw = (90.0 - heading).to_radians();
                self.rotation = Quat::from_rotation_z(yaw);
            }
        }
        self.send_agent_update();
    }

    /// Object updates: the bot's own place follows its avatar's (process_object_update).
    fn on_places(&mut self, placed: &[crate::objects::Placed]) {
        for p in placed {
            self.places.update(p, self.login.agent_id);
        }
        if let Some(at) = self.places.own_position()
            && at != self.position
        {
            self.position = at;
            self.shared.state_mut().position = at;
        }
        if placed.iter().any(|p| p.avatar) || !self.shared.state().avatars.is_empty() {
            let avatars = self.places.avatar_positions();
            let mut st = self.shared.state_mut();
            if st.avatars != avatars {
                st.avatars = avatars;
            }
        }
        if let Some(seat) = self.places.own_seat() {
            let mut st = self.shared.state_mut();
            if st.sitting_on != seat {
                // a nil seat: seated on something not seen yet; AvatarSitResponse named it
                if seat != Some(Uuid::nil()) || st.sitting_on.is_none() {
                    st.sitting_on = seat;
                }
            }
        }
    }

    /// TransferInfo, TransferPacket, TransferAbort: a download of ours, gathered; a failed one is
    /// aborted (LLTransferTargetChannel).
    fn on_transfer(&mut self, pkt: &IncomingPacket) -> Result<(), ferro_msg::DecodeError> {
        use crate::transfer::Reply;
        let (transfer, channel, reply) = if pkt.id == TransferInfo::ID {
            let t = pkt.decode::<TransferInfo>()?.transfer_info;
            let r = Reply::Info {
                status: t.status,
                size: t.size,
            };
            (t.transfer_id, t.channel_type, r)
        } else if pkt.id == TransferPacket::ID {
            let t = pkt.decode::<TransferPacket>()?.transfer_data;
            let r = Reply::Packet {
                status: t.status,
                packet: t.packet,
                data: t.data,
            };
            (t.transfer_id, t.channel_type, r)
        } else {
            let t = pkt.decode::<TransferAbort>()?.transfer_info;
            (t.transfer_id, t.channel_type, Reply::Abort)
        };
        let (_, failed) = self.transfers.receive(transfer, channel, reply);
        if failed {
            self.abort_transfer(transfer);
        }
        Ok(())
    }

    fn abort_transfer(&mut self, transfer: Uuid) {
        let mut m = TransferAbort::default();
        m.transfer_info.transfer_id = transfer;
        m.transfer_info.channel_type = crate::transfer::ASSET_CHANNEL;
        self.send(&m, true);
    }

    /// LLXferManager::processReceiveData for the mute list's file.
    fn on_xfer_packet(&mut self, m: &SendXferPacket) {
        use crate::mutes::Step;
        let Some(x) = self.mute_xfer.as_mut() else {
            return;
        };
        let step = x.packet(m.xfer_id.id, m.xfer_id.packet, &m.data_packet.data);
        let id = x.id;
        let confirm = |s: &mut Self, packet: u32| {
            let mut c = ConfirmXferPacket::default();
            c.xfer_id.id = id;
            c.xfer_id.packet = packet;
            s.send(&c, false);
        };
        match step {
            Step::Ignore => {}
            Step::Confirm(n) => confirm(self, n),
            Step::Done(n, text) => {
                confirm(self, n);
                self.mute_xfer = None;
                let list = crate::mutes::parse_file(&text);
                log::info!("mute list: {} entries", list.len());
                self.shared.state_mut().mute_list = Some(list);
            }
            Step::Fail(why) => {
                log::warn!("mute list: {why}");
                self.mute_xfer = None;
            }
        }
    }

    /// MuteListRequest, without a cached copy (LLMuteList::requestFromServer).
    fn request_mute_list(&mut self) {
        let mut m = MuteListRequest::default();
        m.agent_data.agent_id = self.login.agent_id;
        m.agent_data.session_id = self.login.session_id;
        m.mute_data.mute_crc = 0;
        self.send(&m, true);
    }

    fn request_balance(&mut self) {
        let mut m = MoneyBalanceRequest::default();
        m.agent_data.agent_id = self.login.agent_id;
        m.agent_data.session_id = self.login.session_id;
        self.send(&m, true);
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
                id: self.region_id,
                estate_manager: self.estate_manager,
                product: self.region_product.clone(),
            });
        }
        self.shared.emit(Event::Arrived {
            region: self.region_name.clone(),
            position: p,
        });
        self.send_agent_update();
        self.appearance_once();
        if let Some(waiting) = self.teleport.take() {
            log::info!("teleported to {}", self.region_name);
            if let Some(reply) = waiting {
                let _ = reply.send(Ok(self.region_name.clone()));
            }
        }
        if first {
            // the balance, as the viewer asks for it once arrived (Aurora's session)
            self.request_balance();
            // LLAgent: the active group and title (AgentDataUpdate), and the groups
            // (AgentGroupDataUpdate) come in answer
            let mut r = AgentDataUpdateRequest::default();
            r.agent_data.agent_id = self.login.agent_id;
            r.agent_data.session_id = self.login.session_id;
            self.send(&r, true);
            // the mute list, as the viewer asks for it once logged in (LLMuteList)
            self.request_mute_list();
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
        let max_errors = self.timeouts.event_queue_errors;
        self.caps_task = Some(tokio::spawn(async move {
            match caps::resolve_seed(&http, &seed).await {
                Ok(caps) => {
                    log::info!("{} capabilities", caps.len());
                    let _ = tx.send(EqMessage::Caps(caps.clone()));
                    match caps.get("EventQueueGet") {
                        Some(url) => {
                            caps::event_queue_loop(http, url.clone(), tx, max_errors).await
                        }
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
        self.places.clear();
        self.transfers.clear();
        {
            let mut st = self.shared.state_mut();
            st.region = None;
            st.sitting_on = None;
            st.avatars.clear();
            st.parcel = None;
            st.region_parcels = None;
        }
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
            Request::Group(r) => self.on_group_request(r),
            Request::Parcel(r) => self.on_parcel_request(r),
            Request::Estate(r) => self.on_estate_request(r),
            Request::Profile(r) => self.on_profile_request(r),
            Request::Object(r) => self.on_object_request(r),
            Request::Inventory(r) => self.on_inventory_request(r),
            Request::Rebake { reply } => self.request_appearance(Some(reply)),
            Request::HoverHeight { height, reply } => {
                let Some(url) = self.caps.get("AgentPreferences").cloned() else {
                    let _ = reply.send(Err("no AgentPreferences capability".into()));
                    return;
                };
                let http = self.http.clone();
                tokio::spawn(async move {
                    let body = ferro_llsd::llsd_map! { "hover_height" => height.clamp(-2.0, 2.0) };
                    let answer = caps::post_llsd(&http, &url, &body)
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("AgentPreferences: {e}"));
                    let _ = reply.send(answer);
                });
            }
            Request::ViewerAsset { kind, asset, reply } => {
                // LLViewerAssetStorage: GET <ViewerAsset>?<kind>=<asset>
                let Some(cap) = self.caps.get("ViewerAsset").cloned() else {
                    let _ = reply.send(Err("no ViewerAsset capability".into()));
                    return;
                };
                let http = self.http.clone();
                tokio::spawn(async move {
                    let sep = if cap.contains('?') { '&' } else { '?' };
                    let url = format!("{cap}{sep}{kind}={asset}");
                    let answer = match http.get(&url).timeout(Duration::from_secs(30)).send().await
                    {
                        Ok(r) if r.status().is_success() => r
                            .bytes()
                            .await
                            .map(|b| b.to_vec())
                            .map_err(|e| e.without_url().to_string()),
                        Ok(r) => Err(format!("ViewerAsset: status {}", r.status().as_u16())),
                        Err(e) => Err(format!("ViewerAsset: {}", e.without_url())),
                    };
                    let _ = reply.send(answer);
                });
            }
            Request::FindRegion { name, reply } => {
                // LLWorldMapMessage::sendNamedRegionRequest; the reply gives the grid position
                let mut m = MapNameRequest::default();
                m.agent_data.agent_id = self.login.agent_id;
                m.agent_data.session_id = self.login.session_id;
                m.agent_data.flags = 2; // LAYER_FLAG
                m.name_data.name = str_field(&name);
                self.send(&m, true);
                self.map_waits.push((name.to_lowercase(), reply));
            }
            Request::Interact { interaction, reply } => {
                self.interact(interaction);
                let _ = reply.send(());
            }
            Request::Chat {
                message,
                channel,
                chat_type,
                reply,
            } => {
                // LLChatBar::sendChatFromViewer, a long text in parts
                for part in groups::split_message(&message) {
                    let mut m = ChatFromViewer::default();
                    m.agent_data.agent_id = self.login.agent_id;
                    m.agent_data.session_id = self.login.session_id;
                    m.chat_data.message = str_field(part);
                    m.chat_data.type_ = chat_type;
                    m.chat_data.channel = channel;
                    self.send(&m, true);
                }
                let _ = reply.send(());
            }
            Request::Im { to, message, reply } => {
                // a 1:1 session's id: the two agents' ids xored (LLIMMgr::computeSessionID)
                let session = Uuid::from_u128(self.login.agent_id.as_u128() ^ to.as_u128());
                for part in groups::split_message(&message) {
                    self.send_im(
                        to,
                        crate::events::dialog::NOTHING_SPECIAL,
                        session,
                        part,
                        Vec::new(),
                    );
                }
                let _ = reply.send(());
            }
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
        for transfer in self.transfers.expire() {
            self.abort_transfer(transfer);
        }
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
        self.groups.retain_open();
        self.map_waits.retain(|(_, tx)| !tx.is_closed());
        self.parcel_waits.retain_open();
        self.estate_waits.retain_open();
        self.profile_waits.retain_open();
        self.created.retain(|_, tx| !tx.is_closed());
        if now.duration_since(self.circuit.last_recv) > self.timeouts.region_silence {
            return Some(End::Lost(format!(
                "nothing from the region for {} s",
                self.timeouts.region_silence.as_secs()
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
        if self.dispatch_groups(pkt)? {
            return Ok(None);
        }
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
            self.region_id = m.region_info2.region_id;
            self.estate_manager = m.region_info.is_estate_manager;
            self.region_product = field_str(&m.region_info3.product_name);
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
            let im = InstantMessage {
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
            };
            self.on_session_message(&im);
            self.shared.emit(Event::InstantMessage(im));
        } else if id == ObjectUpdate::ID {
            let m: ObjectUpdate = pkt.decode()?;
            let placed: Vec<_> = m
                .object_data
                .iter()
                .filter_map(crate::objects::from_full)
                .collect();
            self.on_places(&placed);
        } else if id == ObjectUpdateCompressed::ID {
            let m: ObjectUpdateCompressed = pkt.decode()?;
            let placed: Vec<_> = m
                .object_data
                .iter()
                .filter_map(|b| crate::objects::from_compressed(&b.data))
                .collect();
            self.on_places(&placed);
        } else if id == ImprovedTerseObjectUpdate::ID {
            let m: ImprovedTerseObjectUpdate = pkt.decode()?;
            let placed: Vec<_> = m
                .object_data
                .iter()
                .filter_map(crate::objects::from_terse)
                .collect();
            self.on_places(&placed);
        } else if id == TransferInfo::ID || id == TransferPacket::ID || id == TransferAbort::ID {
            self.on_transfer(pkt)?;
        } else if id == KillObject::ID {
            let m: KillObject = pkt.decode()?;
            for o in &m.object_data {
                self.places.kill(o.id);
            }
            let avatars = self.places.avatar_positions();
            self.shared.state_mut().avatars = avatars;
        } else if id == AvatarPropertiesReply::ID {
            let m: AvatarPropertiesReply = pkt.decode()?;
            self.on_avatar_properties(&m);
        } else if id == AvatarAnimation::ID {
            // the animations the region plays on the bot (SignaledAnimations)
            let m: AvatarAnimation = pkt.decode()?;
            if m.sender.id == self.login.agent_id {
                self.shared.state_mut().animations =
                    m.animation_list.iter().map(|a| a.anim_id).collect();
            }
        } else if id == ScriptQuestion::ID {
            let m: ScriptQuestion = pkt.decode()?;
            let d = &m.data;
            self.shared
                .emit(Event::ScriptQuestion(crate::events::ScriptQuestion {
                    task: d.task_id,
                    item: d.item_id,
                    object_name: field_str(&d.object_name),
                    owner_name: field_str(&d.object_owner),
                    questions: d.questions,
                }));
        } else if id == EstateOwnerMessage::ID {
            let m: EstateOwnerMessage = pkt.decode()?;
            self.on_estate_owner_message(&m);
        } else if id == EstateCovenantReply::ID {
            let m: EstateCovenantReply = pkt.decode()?;
            self.on_covenant_reply(&m);
        } else if id == ParcelAccessListReply::ID {
            let m: ParcelAccessListReply = pkt.decode()?;
            self.on_access_list_reply(&m);
        } else if id == ParcelObjectOwnersReply::ID {
            let m: ParcelObjectOwnersReply = pkt.decode()?;
            let owners = m
                .data
                .iter()
                .filter(|d| !d.owner_id.is_nil())
                .map(|d| parcels::ObjectOwner {
                    id: d.owner_id,
                    is_group: d.is_group_owned,
                    count: d.count,
                })
                .collect();
            self.on_object_owners(owners);
        } else if id == DirLandReply::ID {
            let m: DirLandReply = pkt.decode()?;
            self.on_land_reply(&m);
        } else if id == ParcelDwellReply::ID {
            let m: ParcelDwellReply = pkt.decode()?;
            self.on_dwell_reply(&m);
        } else if id == FindAgent::ID {
            // LLAvatarTracker::processAgentFound: the answer is the request, the place filled in;
            // the region and the place in it, as LibreMetaverse's GlobalPosToRegionHandle
            let m: FindAgent = pkt.decode()?;
            if let Some(l) = m.location_block.first() {
                let (gx, gy) = (l.global_x.max(0.0), l.global_y.max(0.0));
                let (rx, ry) = ((gx / 256.0).floor() * 256.0, (gy / 256.0).floor() * 256.0);
                let handle = ((rx as u64) << 32) | (ry as u64);
                let position = Vec3::new((gx - rx) as f32, (gy - ry) as f32, 0.0);
                self.shared.state_mut().friend_locations.insert(
                    m.agent_block.prey,
                    crate::state::FriendLocation {
                        region_handle: handle,
                        position,
                        at: std::time::SystemTime::now(),
                    },
                );
            }
        } else if id == MuteListUpdate::ID {
            // the file is fetched by xfer from the region that announced it
            let m: MuteListUpdate = pkt.decode()?;
            if m.mute_data.agent_id == self.login.agent_id {
                let id = crate::mutes::xfer_id();
                let mut r = RequestXfer::default();
                r.xfer_id.id = id;
                r.xfer_id.filename = m.mute_data.filename.clone();
                r.xfer_id.file_path = crate::mutes::LL_PATH_CACHE;
                r.xfer_id.delete_on_completion = true;
                r.xfer_id.use_big_packets = false;
                r.xfer_id.v_file_id = Uuid::nil();
                r.xfer_id.v_file_type = -1;
                self.send(&r, true);
                self.mute_xfer = Some(crate::mutes::XferDownload::new(id));
            }
        } else if id == UseCachedMuteList::ID {
            // the bot keeps no copy across logins: what it has is current
            let mut st = self.shared.state_mut();
            if st.mute_list.is_none() {
                st.mute_list = Some(Vec::new());
            }
        } else if id == SendXferPacket::ID {
            let m: SendXferPacket = pkt.decode()?;
            self.on_xfer_packet(&m);
        } else if id == AbortXfer::ID {
            let m: AbortXfer = pkt.decode()?;
            if self
                .mute_xfer
                .as_ref()
                .is_some_and(|x| x.id == m.xfer_id.id)
            {
                log::warn!("mute list: the region gave up its download");
                self.mute_xfer = None;
            }
        } else if id == GenericMessage::ID {
            let m: GenericMessage = pkt.decode()?;
            if field_str(&m.method_data.method) == crate::mutes::EMPTY_MUTE_LIST
                && m.agent_data.agent_id == self.login.agent_id
            {
                self.shared.state_mut().mute_list = Some(Vec::new());
            }
        } else if id == AvatarSitResponse::ID {
            // process_avatar_sit_response: the region seats the bot; AgentSit when it asks
            let m: AvatarSitResponse = pkt.decode()?;
            self.shared.state_mut().sitting_on = Some(m.sit_object.id);
            if m.sit_transform.auto_pilot {
                let mut s = AgentSit::default();
                s.agent_data.agent_id = self.login.agent_id;
                s.agent_data.session_id = self.login.session_id;
                self.send(&s, true);
            }
        } else if id == MoneyBalanceReply::ID {
            let m: MoneyBalanceReply = pkt.decode()?;
            let mut st = self.shared.state_mut();
            st.balance = Some(m.money_data.money_balance);
            st.balance_updates += 1;
        } else if id == CoarseLocationUpdate::ID {
            // the avatars in the region (LLWorld::processCoarseUpdate), the bot left out
            let m: CoarseLocationUpdate = pkt.decode()?;
            let me = self.login.agent_id;
            // the n-th location is the n-th avatar's; heights come divided by 4
            let positions: std::collections::HashMap<Uuid, Vec3> = m
                .agent_data
                .iter()
                .zip(&m.location)
                .filter(|(a, _)| a.agent_id != me && !a.agent_id.is_nil())
                .map(|(a, l)| {
                    let at = Vec3::new(f32::from(l.x), f32::from(l.y), f32::from(l.z) * 4.0);
                    (a.agent_id, at)
                })
                .collect();
            let mut st = self.shared.state_mut();
            st.nearby = m
                .agent_data
                .iter()
                .map(|a| a.agent_id)
                .filter(|id| *id != me && !id.is_nil())
                .collect();
            st.nearby_positions = positions;
        } else if id == OnlineNotification::ID || id == OfflineNotification::ID {
            let online = id == OnlineNotification::ID;
            let ids: Vec<Uuid> = if online {
                pkt.decode::<OnlineNotification>()?
                    .agent_block
                    .iter()
                    .map(|b| b.agent_id)
                    .collect()
            } else {
                pkt.decode::<OfflineNotification>()?
                    .agent_block
                    .iter()
                    .map(|b| b.agent_id)
                    .collect()
            };
            let mut st = self.shared.state_mut();
            st.friends_online.retain(|f| !ids.contains(f));
            if online {
                st.friends_online.extend(ids);
            }
        } else if id == BulkUpdateInventory::ID {
            let m: BulkUpdateInventory = pkt.decode()?;
            let added = inventory::bulk_update(&m, self.login.agent_id);
            self.on_inventory_added(added);
        } else if id == UpdateCreateInventoryItem::ID {
            let m: UpdateCreateInventoryItem = pkt.decode()?;
            for it in &m.inventory_data {
                if let Some(tx) = self.created.remove(&it.callback_id) {
                    let _ = tx.send(it.item_id);
                }
            }
        } else if id == MapBlockReply::ID {
            let m: MapBlockReply = pkt.decode()?;
            self.on_map_blocks(&m);
        } else if id == ScriptDialog::ID {
            let m: ScriptDialog = pkt.decode()?;
            let d = &m.data;
            let first = field_str(&d.first_name);
            let last = field_str(&d.last_name);
            self.shared
                .emit(Event::ScriptDialog(crate::events::ScriptDialog {
                    object_id: d.object_id,
                    object_name: field_str(&d.object_name),
                    owner_id: m.owner_data.first().map_or(Uuid::nil(), |o| o.owner_id),
                    owner_name: format!("{first} {last}").trim().to_owned(),
                    message: field_str(&d.message),
                    channel: d.chat_channel,
                    buttons: m
                        .buttons
                        .iter()
                        .map(|b| field_str(&b.button_label))
                        .collect(),
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
                    // a leading slash marks a system message (process_alert_core)
                    let message = message
                        .strip_prefix('/')
                        .map_or(message.clone(), str::to_owned);
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

    /// Once per login, arrived and with the capabilities: ask for the outfit to be baked, or the
    /// bot stays a cloud.
    fn appearance_once(&mut self) {
        if self.appearance_asked || !self.arrived || self.caps.is_empty() {
            return;
        }
        self.appearance_asked = true;
        self.request_appearance(None);
    }

    /// LLAppearanceMgr::serverAppearanceUpdateCoro (Aurora's request_server_appearance): POST
    /// the outfit's version to UpdateAvatarAppearance; the region bakes the current outfit. The
    /// version is not known here: a wrong one is answered with the one `expected`, asked again.
    fn request_appearance(&mut self, reply: Option<oneshot::Sender<Result<i32, String>>>) {
        let Some(url) = self.caps.get("UpdateAvatarAppearance").cloned() else {
            log::warn!("no UpdateAvatarAppearance capability: the outfit is not baked");
            if let Some(reply) = reply {
                let _ = reply.send(Err("no UpdateAvatarAppearance capability".into()));
            }
            return;
        };
        let http = self.http.clone();
        tokio::spawn(async move {
            let mut version = 0;
            let mut answer = Err("no answer".to_owned());
            for _ in 0..2 {
                let body = ferro_llsd::llsd_map! { "cof_version" => version };
                match tokio::time::timeout(
                    Duration::from_secs(60),
                    caps::post_llsd(&http, &url, &body),
                )
                .await
                {
                    Ok(Ok(v)) if v["success"].as_bool() => {
                        answer = Ok(version);
                        break;
                    }
                    Ok(Ok(v)) if v.has("expected") && v["expected"].as_i32() != version => {
                        version = v["expected"].as_i32();
                    }
                    Ok(Ok(v)) => {
                        answer = Err(v["error"].to_string_value());
                        break;
                    }
                    Ok(Err(e)) => {
                        answer = Err(e.to_string());
                        break;
                    }
                    Err(_) => {
                        answer = Err("no answer in 60 s".into());
                        break;
                    }
                }
            }
            match &answer {
                Ok(v) => log::info!("outfit baked (version {v})"),
                Err(e) => log::warn!("outfit not baked: {e}"),
            }
            if let Some(reply) = reply {
                let _ = reply.send(answer);
            }
        });
    }

    /// MapBlockReply for the names asked (Aurora's teleport by name): the region of that very
    /// name, the end-of-list block (access 255, MapSearchModule's) never one; a reply with only
    /// other names starting like it, or empty ones, means there is no such region.
    fn on_map_blocks(&mut self, m: &MapBlockReply) {
        let names: Vec<(String, u8, crate::MapRegion)> = m
            .data
            .iter()
            .map(|d| {
                let handle = ((u64::from(d.x) * 256) << 32) | (u64::from(d.y) * 256);
                let name = field_str(&d.name);
                let region = crate::MapRegion {
                    name: name.clone(),
                    x: d.x,
                    y: d.y,
                    handle,
                    map_image: d.map_image_id,
                };
                (name.to_lowercase(), d.access, region)
            })
            .collect();
        let waits = std::mem::take(&mut self.map_waits);
        for (want, tx) in waits {
            let found = names
                .iter()
                .find(|(n, access, _)| *access != 255 && *n == want)
                .map(|(_, _, r)| r.clone());
            if let Some(region) = found {
                let _ = tx.send(Some(region));
            } else if names
                .iter()
                .all(|(n, _, _)| n.is_empty() || n.starts_with(&want))
            {
                let _ = tx.send(None);
            } else {
                // a reply to something else (map browsing): still waiting
                self.map_waits.push((want, tx));
            }
        }
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
            "ChatterBoxSessionStartReply" => self.on_session_start_reply(&e.body),
            "ChatterBoxInvitation" => self.on_session_invitation(&e.body),
            "ParcelProperties" => self.on_parcel_properties(&e.body),
            "BulkUpdateInventory" => {
                let added = inventory::bulk_update_llsd(&e.body, self.login.agent_id);
                self.on_inventory_added(added);
            }
            // Second Life sends it on the event queue (Aurora's land::parse_object_owners)
            "ParcelObjectOwnersReply" => {
                let owners = e.body["Data"]
                    .as_array()
                    .iter()
                    .filter(|d| !d["OwnerID"].as_uuid().is_nil())
                    .map(|d| parcels::ObjectOwner {
                        id: d["OwnerID"].as_uuid(),
                        is_group: d["IsGroupOwned"].as_bool(),
                        count: d["Count"].as_i32(),
                    })
                    .collect();
                self.on_object_owners(owners);
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
    timeouts: Timeouts,
) -> End {
    let addr = SocketAddr::from((login.sim_ip, login.sim_port));
    let sock = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(sock) => sock,
        Err(e) => return End::Lost(format!("no UDP socket: {e}")),
    };
    // tokio only knows the socket is writable once its reactor has seen it: until then a send
    // "would block" and the packet is dropped (UseCircuitCode then waited for its resend, 5 s)
    if let Err(e) = sock.writable().await {
        return End::Lost(format!("UDP socket: {e}"));
    }
    let (_, eq_rx) = mpsc::unbounded_channel();
    let mut s = Session {
        caps: HashMap::new(),
        login,
        shared,
        http,
        timeouts,
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
        groups: groups::GroupWaits::default(),
        map_waits: Vec::new(),
        mute_xfer: None,
        region_id: Uuid::nil(),
        estate_manager: false,
        region_product: String::new(),
        estate_invoice: Uuid::new_v4(),
        estate_waits: Default::default(),
        profile_waits: Default::default(),
        parcel_waits: Default::default(),
        places: Default::default(),
        transfers: Default::default(),
        appearance_asked: false,
        created: HashMap::new(),
        next_callback: 0,
        controls: 0,
        controls_once: 0,
        rotation: Quat::IDENTITY,
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
                Some(EqMessage::Caps(caps)) => {
                    s.caps = caps;
                    s.appearance_once();
                }
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
            buddies: Vec::new(),
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
