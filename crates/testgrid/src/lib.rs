//! A fake Second Life grid on localhost, for tests: a login server (XML-RPC), one region (the UDP
//! circuit), its seed capability and its event queue.
//!
//! It answers the way the grid does, as far as the bot uses it today: the login reply of
//! `login_to_simulator`, UseCircuitCode then RegionHandshake, CompleteAgentMovement then
//! AgentMovementComplete, pings both ways, LogoutRequest then LogoutReply, the seed request and
//! the held `EventQueueGet` poll (502 when nothing came, as the grid's queue does).
//!
//! A test starts it, points the bot's login address at [`TestGrid::login_uri`], then watches what
//! the bot sends ([`Seen`]) and acts as the grid would: chat, IMs, a kick, a closed circuit.
//! Each feature that talks to a region adds what it needs here (ROADMAP phase 1).

mod caps;
mod http;
mod login;
mod region;
mod xmlrpc;

use ferro_llsd::Llsd;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{Notify, mpsc};
use uuid::Uuid;

/// The account the grid knows, and how it behaves.
#[derive(Debug, Clone)]
pub struct GridConfig {
    pub first: String,
    pub last: String,
    pub password: String,
    pub region_name: String,
    /// How long the event queue holds a poll with nothing to send before its 502.
    pub eq_hold: Duration,
    /// How often the region pings the bot.
    pub ping_interval: Duration,
}

impl Default for GridConfig {
    fn default() -> Self {
        GridConfig {
            first: "Test".into(),
            last: "Bot".into(),
            password: "a test password".into(),
            region_name: "Test Region".into(),
            eq_hold: Duration::from_secs(15),
            ping_interval: Duration::from_secs(2),
        }
    }
}

/// What the grid saw the bot do, in order. `session` counts the logins, from 1.
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
    },
    CompleteAgentMovement {
        session: u32,
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
    SeedRequest {
        session: u32,
        caps: Vec<String>,
    },
    EventQueuePoll {
        session: u32,
    },
}

impl Seen {
    pub fn session(&self) -> Option<u32> {
        match self {
            Seen::Login { session, .. }
            | Seen::UseCircuitCode { session }
            | Seen::CompleteAgentMovement { session }
            | Seen::AgentThrottle { session }
            | Seen::RegionHandshakeReply { session, .. }
            | Seen::AgentUpdate { session }
            | Seen::CompletePingCheck { session }
            | Seen::LogoutRequest { session }
            | Seen::SeedRequest { session, .. }
            | Seen::EventQueuePoll { session } => Some(*session),
            Seen::LoginRefused { .. } => None,
        }
    }
}

/// One login: its ids, and its event queue.
struct Session {
    n: u32,
    session_id: Uuid,
    circuit_code: u32,
    /// Ended by a logout, a kick or a closed circuit: its capabilities answer 404.
    ended: bool,
    /// Events waiting for the next poll.
    events: VecDeque<(String, Llsd)>,
    /// The next poll answers 404, as a region does when it closes the queue.
    close_queue: bool,
    eq_id: i32,
}

struct State {
    sessions: HashMap<u32, Session>,
    seen: Vec<Seen>,
}

struct Shared {
    cfg: GridConfig,
    agent_id: Uuid,
    http_addr: SocketAddr,
    region_addr: SocketAddr,
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

    fn end_session(&self, n: u32) {
        if let Some(s) = self.state().sessions.get_mut(&n) {
            s.ended = true;
        }
        self.eq_wake.notify_waiters();
    }
}

/// What a test asks the region to send to the bot.
#[derive(Debug)]
enum RegionCommand {
    Say { from: String, text: String },
    Im { from: String, text: String },
    Kick { reason: String },
    DisableSimulator,
}

/// A running fake grid. Dropping it stops it.
pub struct TestGrid {
    shared: Arc<Shared>,
    region: mpsc::UnboundedSender<RegionCommand>,
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
    /// Start the login server and the region on localhost, on free ports.
    pub async fn start(cfg: GridConfig) -> std::io::Result<TestGrid> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let udp = UdpSocket::bind("127.0.0.1:0").await?;
        let shared = Arc::new(Shared {
            cfg,
            agent_id: Uuid::new_v4(),
            http_addr: listener.local_addr()?,
            region_addr: udp.local_addr()?,
            state: Mutex::new(State {
                sessions: HashMap::new(),
                seen: Vec::new(),
            }),
            eq_wake: Notify::new(),
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let tasks = vec![
            tokio::spawn(http::serve(listener, shared.clone())),
            tokio::spawn(region::run(udp, shared.clone(), rx)),
        ];
        Ok(TestGrid {
            shared,
            region: tx,
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

    /// The region kicks the bot (KickUser), as the grid does when the account logs in elsewhere.
    pub fn kick(&self, reason: &str) {
        self.command(RegionCommand::Kick {
            reason: reason.into(),
        });
    }

    /// The region closes the circuit (DisableSimulator), as when it shuts down.
    pub fn disable_simulator(&self) {
        self.command(RegionCommand::DisableSimulator);
    }

    /// Queue an event on the latest session's event queue.
    pub fn queue_event(&self, message: &str, body: Llsd) {
        let mut st = self.shared.state();
        if let Some(s) = latest(&mut st) {
            s.events.push_back((message.into(), body));
        }
        drop(st);
        self.shared.eq_wake.notify_waiters();
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

    fn command(&self, c: RegionCommand) {
        // the region task only ends when the grid is dropped
        let _ = self.region.send(c);
    }
}

fn latest(st: &mut State) -> Option<&mut Session> {
    st.sessions.values_mut().max_by_key(|s| s.n)
}
