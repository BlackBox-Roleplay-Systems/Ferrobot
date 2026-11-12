//! One logged-in session in one region: the UDP circuit, the event queue, keep-alive and logout.
//!
//! The connect sequence follows aurora-net/src/session.rs (Aurora Viewer, see NOTICE.md) and
//! `newview/llstartup.cpp`: UseCircuitCode, then once it is acked CompleteAgentMovement and
//! AgentThrottle; RegionHandshake is answered with RegionHandshakeReply; pings both ways.
//! A bot stays in its region: crossings and teleports come later.

use crate::caps::{self, EqEvent, EqMessage};
use crate::circuit::{Circuit, Resend};
use crate::login::LoginResponse;
use ferro_msg::msgs::*;
use ferro_msg::{IncomingPacket, Msg, MsgId, parse_packet};
use glam::{Quat, Vec3};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};

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

/// How a session ended.
#[derive(Debug)]
pub enum End {
    /// Logged out on request (shutdown).
    LoggedOut,
    /// The grid ended the session (KickUser), with its reason.
    Kicked(String),
    /// The connection was lost.
    Lost(String),
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

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .to_owned()
}

struct Session<'a> {
    login: &'a LoginResponse,
    sock: UdpSocket,
    circuit: Circuit,
    ucc_seq: u32,
    movement_sent: Option<Instant>,
    arrived: bool,
    region_name: String,
    position: Vec3,
    last_ping: Instant,
    last_update: Instant,
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
            self.region_name = text(&m.region_info.sim_name);
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
                self.arrived = true;
                let p = m.data.position;
                log::info!(
                    "in world: {} {} at {}, {}, {}",
                    self.login.first_name,
                    self.login.last_name,
                    p.x.round(),
                    p.y.round(),
                    p.z.round()
                );
                self.send_agent_update();
            }
        } else if id == ChatFromSimulator::ID {
            let m: ChatFromSimulator = pkt.decode()?;
            let c = &m.chat_data;
            // 0 whisper, 1 say, 2 shout; the others are typing, start/stop and debug
            if c.chat_type <= 2 && c.source_id != self.login.agent_id {
                log::info!("[chat] {}: {}", text(&c.from_name), text(&c.message));
            }
        } else if id == ImprovedInstantMessage::ID {
            let m: ImprovedInstantMessage = pkt.decode()?;
            let b = &m.message_block;
            // dialog 0: a plain IM
            if b.dialog == 0 {
                log::info!("[im] {}: {}", text(&b.from_agent_name), text(&b.message));
            } else {
                log::debug!(
                    "instant message, dialog {} from {}",
                    b.dialog,
                    text(&b.from_agent_name)
                );
            }
        } else if id == AlertMessage::ID {
            let m: AlertMessage = pkt.decode()?;
            log::info!("alert: {}", text(&m.alert_data.message));
        } else if id == KickUser::ID {
            let m: KickUser = pkt.decode()?;
            return Ok(Some(End::Kicked(text(&m.user_info.reason))));
        } else if id == LogoutReply::ID {
            return Ok(Some(End::LoggedOut));
        } else if id == DisableSimulator::ID {
            return Ok(Some(End::Lost("the region closed the connection".into())));
        }
        Ok(None)
    }

    fn on_event(&mut self, e: EqEvent) -> Option<End> {
        match e.message.as_str() {
            "DisableSimulator" => Some(End::Lost(
                "the region closed the connection (event queue)".into(),
            )),
            other => {
                // Only the field names: some bodies hold capability URLs (TeleportFinish,
                // EstablishAgentCommunication…), which are secrets of the session.
                let fields: Vec<&str> = e
                    .body
                    .as_map()
                    .map(|m| m.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                log::debug!("event: {other} {fields:?}");
                None
            }
        }
    }
}

/// Connect to the login region and stay there until shutdown or the connection ends.
pub async fn run(
    http: &reqwest::Client,
    login: &LoginResponse,
    mut shutdown: watch::Receiver<bool>,
) -> End {
    let addr = SocketAddr::from((login.sim_ip, login.sim_port));
    let sock = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(sock) => sock,
        Err(e) => return End::Lost(format!("no UDP socket: {e}")),
    };
    let mut s = Session {
        login,
        sock,
        circuit: Circuit::new(addr),
        ucc_seq: 0,
        movement_sent: None,
        arrived: false,
        region_name: String::new(),
        position: Vec3::new(128.0, 128.0, 0.0),
        last_ping: Instant::now(),
        last_update: Instant::now(),
    };
    s.send_use_circuit_code();

    // capabilities and the event queue, alongside the circuit
    let (eq_tx, mut eq_rx) = mpsc::unbounded_channel();
    let seed_http = http.clone();
    let seed = login.seed_capability.clone();
    let caps_task = tokio::spawn(async move {
        match caps::resolve_seed(&seed_http, &seed).await {
            Ok(caps) => {
                log::info!("{} capabilities", caps.len());
                match caps.get("EventQueueGet") {
                    Some(url) => caps::event_queue_loop(seed_http, url.clone(), eq_tx).await,
                    None => log::warn!("the region gave no event queue"),
                }
            }
            Err(e) => {
                log::warn!("seed capability failed: {e}");
                let _ = eq_tx.send(EqMessage::Lost);
            }
        }
    });

    let mut buf = vec![0u8; 8192];
    let mut tick = tokio::time::interval(TICK);
    let mut logging_out: Option<Instant> = None;
    // The capabilities task drops its sender when it ends; a closed channel is always ready, so
    // its branch is switched off then, or the loop would spin.
    let mut eq_open = true;
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
            ev = eq_rx.recv(), if eq_open => match ev {
                Some(EqMessage::Event(e)) => {
                    if let Some(end) = s.on_event(e) {
                        break end;
                    }
                }
                // Without the region's event queue, IM sessions, group chat and teleports fail
                // and nothing gets better before a new login (LLEventPollImpl, Aurora's
                // on_eq_lost): end the session so that the bot logs in again.
                Some(EqMessage::Lost) => break End::Lost("the region's event queue was lost".into()),
                None => eq_open = false,
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
    caps_task.abort();
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
        let end = run(&reqwest::Client::new(), &login(port, seed), stop_rx).await;
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
        let http = reqwest::Client::new();
        let resp = login(port, "http://127.0.0.1:9/seed".into());
        let session = run(&http, &resp, stop_rx);
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
