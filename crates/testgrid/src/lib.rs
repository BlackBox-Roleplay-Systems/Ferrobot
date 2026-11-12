//! A fake Second Life grid on localhost, for tests: a login server (XML-RPC), two regions (UDP
//! circuits, seed capabilities, event queues), and teleports between them.
//!
//! It answers the way the grid does, as far as the bot uses it today: the login reply of
//! `login_to_simulator`, UseCircuitCode then RegionHandshake, CompleteAgentMovement then
//! AgentMovementComplete, pings both ways, LogoutRequest then LogoutReply, the seed request and
//! the held `EventQueueGet` poll (502 when nothing came, as the grid's queue does), the agent's
//! data and groups, names, avatar searches, and teleports (TeleportStart, then TeleportFinish on
//! the event queue, or TeleportFailed).
//!
//! A test starts it, points the bot's login address at [`TestGrid::login_uri`], then watches what
//! the bot sends ([`Seen`]) and acts as the grid would: chat, IMs, teleport offers, a kick, a
//! closed circuit. Each feature that talks to a region adds what it needs here (ROADMAP phase 1).

mod caps;
mod http;
mod login;
mod region;
mod xmlrpc;

use ferro_llsd::Llsd;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{Notify, mpsc};
use uuid::Uuid;

/// The login region and the region teleports go to.
pub const FIRST_REGION: usize = 0;
pub const SECOND_REGION: usize = 1;

/// The account the grid knows, and how it behaves.
#[derive(Debug, Clone)]
pub struct GridConfig {
    pub first: String,
    pub last: String,
    pub password: String,
    /// The login region, at (1000, 1000) on the grid.
    pub region_name: String,
    /// Its neighbour to the east, at (1001, 1000), where teleports go.
    pub second_region_name: String,
    /// How long the event queue holds a poll with nothing to send before its 502.
    pub eq_hold: Duration,
    /// How often the region pings the bot.
    pub ping_interval: Duration,
    /// Other avatars the grid knows, for names and searches.
    pub residents: Vec<Resident>,
    /// The groups the bot is in, and its active group.
    pub groups: Vec<GridGroup>,
    pub active_group: Option<Uuid>,
    /// The login reply's `home`, as the grid writes it (LLSD notation in a string).
    pub home: String,
}

#[derive(Debug, Clone)]
pub struct Resident {
    pub id: Uuid,
    pub first: String,
    pub last: String,
}

#[derive(Debug, Clone)]
pub struct GridGroup {
    pub id: Uuid,
    pub name: String,
    pub title: String,
    pub powers: u64,
}

impl Default for GridConfig {
    fn default() -> Self {
        GridConfig {
            first: "Test".into(),
            last: "Bot".into(),
            password: "a test password".into(),
            region_name: "Test Region".into(),
            second_region_name: "Second Region".into(),
            eq_hold: Duration::from_secs(15),
            ping_interval: Duration::from_secs(2),
            residents: vec![
                Resident {
                    id: Uuid::from_u128(0x5eed_0001),
                    first: "Some".into(),
                    last: "Resident".into(),
                },
                Resident {
                    id: Uuid::from_u128(0x5eed_0002),
                    first: "Other".into(),
                    last: "Avatar".into(),
                },
            ],
            groups: vec![
                GridGroup {
                    id: Uuid::from_u128(0x6000_0001),
                    name: "Test Group".into(),
                    title: "Bot".into(),
                    powers: 0x0000_0002_0000_0002,
                },
                GridGroup {
                    id: Uuid::from_u128(0x6000_0002),
                    name: "Second Group".into(),
                    title: "Member".into(),
                    powers: 0,
                },
            ],
            active_group: Some(Uuid::from_u128(0x6000_0001)),
            // in the second region
            home: "{'region_handle':[r256256, r256000], 'position':[r100, r110, r22], 'look_at':[r1, r0, r0]}".into(),
        }
    }
}

/// What the grid saw the bot do, in order. `session` counts the logins, from 1; `region` is
/// [`FIRST_REGION`] or [`SECOND_REGION`].
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    Login {
        session: u32,
        first: String,
        last: String,
        start: String,
        channel: String,
    },
    LoginRefused {
        reason: String,
    },
    UseCircuitCode {
        session: u32,
        region: usize,
    },
    CompleteAgentMovement {
        session: u32,
        region: usize,
    },
    AgentThrottle {
        session: u32,
    },
    RegionHandshakeReply {
        session: u32,
        flags: u32,
    },
    AgentUpdate {
        session: u32,
    },
    CompletePingCheck {
        session: u32,
    },
    LogoutRequest {
        session: u32,
    },
    AgentDataUpdateRequest {
        session: u32,
    },
    UuidNameRequest {
        session: u32,
        ids: Vec<Uuid>,
    },
    AvatarPickerRequest {
        session: u32,
        name: String,
    },
    TeleportLureRequest {
        session: u32,
        lure: Uuid,
    },
    TeleportLandmarkRequest {
        session: u32,
        landmark: Uuid,
    },
    TeleportLocationRequest {
        session: u32,
        region_handle: u64,
    },
    SeedRequest {
        session: u32,
        region: usize,
        caps: Vec<String>,
    },
    EventQueuePoll {
        session: u32,
        region: usize,
    },
}

/// One login: its ids, where its agent is, and its event queue.
struct Session {
    n: u32,
    session_id: Uuid,
    circuit_code: u32,
    /// Ended by a logout, a kick or a closed circuit: its capabilities answer 404.
    ended: bool,
    /// The region the agent is in, and the one a teleport sends it to.
    region: usize,
    expected: Option<usize>,
    /// Where the agent arrives in the next region (AgentMovementComplete).
    arrival: glam::Vec3,
    /// Events waiting for the next poll of the current region's queue.
    events: VecDeque<(String, Llsd)>,
    /// The next poll answers 404, as a region does when it closes the queue.
    close_queue: bool,
    eq_id: i32,
}

struct State {
    sessions: HashMap<u32, Session>,
    seen: Vec<Seen>,
    /// Teleport offers sent to the bot, by lure id.
    lures: HashSet<Uuid>,
    /// The next teleport fails, with this reason.
    fail_teleport: Option<String>,
}

/// A region as the others see it.
#[derive(Debug, Clone)]
struct RegionInfo {
    name: String,
    addr: SocketAddr,
    /// Corner on the grid, in metres.
    x: u32,
    y: u32,
}

impl RegionInfo {
    fn handle(&self) -> u64 {
        (u64::from(self.x) << 32) | u64::from(self.y)
    }
}

struct Shared {
    cfg: GridConfig,
    agent_id: Uuid,
    http_addr: SocketAddr,
    regions: Vec<RegionInfo>,
    state: Mutex<State>,
    /// Woken when an event is queued or a session ends, for the held polls.
    eq_wake: Notify,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        // a test that panicked while holding the lock leaves the state as it was: keep going
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn saw(&self, seen: Seen) {
        log::debug!("testgrid saw {seen:?}");
        self.state().seen.push(seen);
    }

    fn queue_event(&self, n: u32, message: &str, body: Llsd) {
        if let Some(s) = self.state().sessions.get_mut(&n) {
            s.events.push_back((message.into(), body));
        }
        self.eq_wake.notify_waiters();
    }

    fn end_session(&self, n: u32) {
        if let Some(s) = self.state().sessions.get_mut(&n) {
            s.ended = true;
        }
        self.eq_wake.notify_waiters();
    }

    fn seed_url(&self, n: u32, region: usize) -> String {
        format!("http://{}/cap/{n}/{region}/seed", self.http_addr)
    }
}

/// What a test asks a region to send to the bot.
#[derive(Debug)]
enum RegionCommand {
    Say { from: String, text: String },
    Im { from: String, text: String },
    OfferTeleport { from: Resident, lure: Uuid },
    Kick { reason: String },
    DisableSimulator,
    RestartNotice { seconds: i32 },
}

/// A running fake grid. Dropping it stops it.
pub struct TestGrid {
    shared: Arc<Shared>,
    regions: Vec<mpsc::UnboundedSender<RegionCommand>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for TestGrid {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl TestGrid {
    /// Start the login server and the regions on localhost, on free ports.
    pub async fn start(cfg: GridConfig) -> std::io::Result<TestGrid> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let sockets = [
            UdpSocket::bind("127.0.0.1:0").await?,
            UdpSocket::bind("127.0.0.1:0").await?,
        ];
        let regions = vec![
            RegionInfo {
                name: cfg.region_name.clone(),
                addr: sockets[0].local_addr()?,
                x: 1000 * 256,
                y: 1000 * 256,
            },
            RegionInfo {
                name: cfg.second_region_name.clone(),
                addr: sockets[1].local_addr()?,
                x: 1001 * 256,
                y: 1000 * 256,
            },
        ];
        let shared = Arc::new(Shared {
            cfg,
            agent_id: Uuid::new_v4(),
            http_addr: listener.local_addr()?,
            regions,
            state: Mutex::new(State {
                sessions: HashMap::new(),
                seen: Vec::new(),
                lures: HashSet::new(),
                fail_teleport: None,
            }),
            eq_wake: Notify::new(),
        });
        let mut tasks = vec![tokio::spawn(http::serve(listener, shared.clone()))];
        let mut senders = Vec::new();
        for (index, sock) in sockets.into_iter().enumerate() {
            let (tx, rx) = mpsc::unbounded_channel();
            tasks.push(tokio::spawn(region::run(index, sock, shared.clone(), rx)));
            senders.push(tx);
        }
        Ok(TestGrid {
            shared,
            regions: senders,
            tasks,
        })
    }

    /// The address to give the bot as its login server.
    pub fn login_uri(&self) -> String {
        format!("http://{}/login.cgi", self.shared.http_addr)
    }

    /// The account's name, "First Last".
    pub fn username(&self) -> String {
        format!("{} {}", self.shared.cfg.first, self.shared.cfg.last)
    }

    pub fn password(&self) -> &str {
        &self.shared.cfg.password
    }

    pub fn region_name(&self) -> &str {
        &self.shared.cfg.region_name
    }

    /// The handle of a region ([`FIRST_REGION`], [`SECOND_REGION`]).
    pub fn region_handle(&self, region: usize) -> u64 {
        self.shared.regions[region].handle()
    }

    /// Everything seen so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.shared.state().seen.clone()
    }

    /// How many of what was seen match.
    pub fn count(&self, matches: impl Fn(&Seen) -> bool) -> usize {
        self.shared
            .state()
            .seen
            .iter()
            .filter(|s| matches(s))
            .count()
    }

    /// Wait until `done` holds for what was seen; false when `timeout` passes first.
    pub async fn wait_until(&self, timeout: Duration, done: impl Fn(&[Seen]) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if done(&self.shared.state().seen) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait for the first thing seen that matches.
    pub async fn wait_for(&self, timeout: Duration, matches: impl Fn(&Seen) -> bool) -> bool {
        self.wait_until(timeout, |seen| seen.iter().any(&matches))
            .await
    }

    /// Someone says `text` in local chat, next to the bot.
    pub fn say(&self, from: &str, text: &str) {
        self.command(RegionCommand::Say {
            from: from.into(),
            text: text.into(),
        });
    }

    /// Someone sends the bot an IM.
    pub fn im(&self, from: &str, text: &str) {
        self.command(RegionCommand::Im {
            from: from.into(),
            text: text.into(),
        });
    }

    /// A resident offers the bot a teleport to them (IM_LURE_USER); accepted, it goes to the
    /// second region. Returns the lure's id.
    pub fn offer_teleport(&self, from: &Resident) -> Uuid {
        let lure = Uuid::new_v4();
        self.shared.state().lures.insert(lure);
        self.command(RegionCommand::OfferTeleport {
            from: from.clone(),
            lure,
        });
        lure
    }

    /// The next teleport fails (TeleportFailed on the event queue) with this reason.
    pub fn fail_next_teleport(&self, reason: &str) {
        self.shared.state().fail_teleport = Some(reason.into());
    }

    /// The region kicks the bot (KickUser), as the grid does when the account logs in elsewhere.
    pub fn kick(&self, reason: &str) {
        self.command(RegionCommand::Kick {
            reason: reason.into(),
        });
    }

    /// The region announces a restart in `seconds`, as Second Life does (AlertMessage with a
    /// RegionRestartSeconds or RegionRestartMinutes notification).
    pub fn announce_restart(&self, seconds: i32) {
        self.command(RegionCommand::RestartNotice { seconds });
    }

    /// The region closes the circuit (DisableSimulator), as when it shuts down.
    pub fn disable_simulator(&self) {
        self.command(RegionCommand::DisableSimulator);
    }

    /// Queue an event on the latest session's event queue.
    pub fn queue_event(&self, message: &str, body: Llsd) {
        let n = latest(&mut self.shared.state()).map(|s| s.n);
        if let Some(n) = n {
            self.shared.queue_event(n, message, body);
        }
    }

    /// The latest session's event queue answers its next poll with 404, as a closed queue does.
    pub fn close_event_queue(&self) {
        let mut st = self.shared.state();
        if let Some(s) = latest(&mut st) {
            s.close_queue = true;
        }
        drop(st);
        self.shared.eq_wake.notify_waiters();
    }

    /// A command for the region the latest session's agent is in.
    fn command(&self, c: RegionCommand) {
        let region = latest(&mut self.shared.state()).map_or(FIRST_REGION, |s| s.region);
        // the region tasks only end when the grid is dropped
        let _ = self.regions[region].send(c);
    }
}

fn latest(st: &mut State) -> Option<&mut Session> {
    st.sessions.values_mut().max_by_key(|s| s.n)
}
