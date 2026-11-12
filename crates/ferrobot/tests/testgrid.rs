//! The real bot, run as a process against the test grid (`crates/testgrid`): login, arrival,
//! chat and IMs, pings, logout on SIGTERM, a kick, a closed circuit and the relog after them,
//! a refused login. ROADMAP phase 1.
#![cfg(unix)]

use ferro_testgrid::{GridConfig, Seen, TestGrid};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const WAIT: Duration = Duration::from_secs(15);

/// The bot's process, and its log lines as they come.
struct Bot {
    child: Child,
    log: Arc<Mutex<Vec<String>>>,
}

impl Bot {
    fn start(grid: &TestGrid, env: &[(&str, &str)]) -> Bot {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrobot"));
        // nothing from the environment of whoever runs the tests: no real account by mistake
        cmd.env_clear()
            .env("FERRO_LOGIN_URI", grid.login_uri())
            .env("FERRO_USERNAME", grid.username())
            .env("FERRO_PASSWORD", grid.password())
            .env("FERRO_RELOG_DELAY", "0")
            .env("RUST_LOG", "debug,ferrobot=trace")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("the bot starts");
        let log = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("piped stderr");
        let lines = log.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                lines.lock().expect("log lock").push(line);
            }
        });
        Bot { child, log }
    }

    fn log(&self) -> String {
        self.log.lock().expect("log lock").join("\n")
    }

    async fn wait_log(&self, needle: &str) {
        let deadline = tokio::time::Instant::now() + WAIT;
        while tokio::time::Instant::now() < deadline {
            if self.log().contains(needle) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("no \"{needle}\" in the bot's log:\n{}", self.log());
    }

    /// SIGTERM, as `docker stop` sends.
    fn terminate(&self) {
        let pid = self.child.id().expect("the bot is running").to_string();
        let status = std::process::Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .expect("kill runs");
        assert!(status.success());
    }

    async fn exit_code(&mut self) -> i32 {
        match tokio::time::timeout(WAIT, self.child.wait()).await {
            Ok(Ok(status)) => status.code().expect("an exit code, not a signal"),
            _ => panic!("the bot did not exit:\n{}", self.log()),
        }
    }
}

async fn grid() -> TestGrid {
    TestGrid::start(GridConfig::default())
        .await
        .expect("the test grid starts")
}

async fn wait_for(grid: &TestGrid, bot: &Bot, what: &str, matches: impl Fn(&Seen) -> bool) {
    if !grid.wait_for(WAIT, matches).await {
        panic!(
            "the grid never saw {what}; it saw {:?}\nthe bot's log:\n{}",
            grid.seen(),
            bot.log()
        );
    }
}

async fn arrived(grid: &TestGrid, bot: &Bot, session: u32) {
    wait_for(grid, bot, "an AgentUpdate", |s| {
        *s == Seen::AgentUpdate { session }
    })
    .await;
}

fn position(seen: &[Seen], what: &Seen) -> usize {
    seen.iter()
        .position(|s| s == what)
        .unwrap_or_else(|| panic!("{what:?} not seen in {seen:?}"))
}

#[tokio::test]
async fn logs_in_arrives_talks_and_logs_out_on_sigterm() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[]);
    arrived(&grid, &bot, 1).await;

    let seen = grid.seen();
    assert_eq!(
        seen[0],
        Seen::Login {
            session: 1,
            first: "Test".into(),
            last: "Bot".into(),
            start: "last".into(),
            channel: "Ferrobot".into(),
        }
    );
    // CompleteAgentMovement only once UseCircuitCode is acked (llstartup.cpp)
    assert!(
        position(&seen, &Seen::UseCircuitCode { session: 1 })
            < position(&seen, &Seen::CompleteAgentMovement { session: 1 })
    );
    position(&seen, &Seen::AgentThrottle { session: 1 });
    position(
        &seen,
        &Seen::RegionHandshakeReply {
            session: 1,
            flags: 0x7,
        },
    );
    bot.wait_log("region handshake: Test Region").await;
    bot.wait_log("in world: Test Bot at 128, 128, 25").await;

    wait_for(&grid, &bot, "the seed request", |s| {
        matches!(s, Seen::SeedRequest { session: 1, caps } if caps.iter().any(|c| c == "EventQueueGet"))
    })
    .await;
    wait_for(&grid, &bot, "an event queue poll", |s| {
        *s == Seen::EventQueuePoll { session: 1 }
    })
    .await;
    grid.queue_event("TestEvent", ferro_llsd::llsd_map! { "answer" => 42 });
    bot.wait_log("event: TestEvent [\"answer\"]").await;

    grid.say("Someone Else", "hello there");
    bot.wait_log("[chat] Someone Else: hello there").await;
    grid.im("Someone Else", "hi bot");
    bot.wait_log("[im] Someone Else: hi bot").await;

    // the region's pings are answered, and the bot's own get their reply
    wait_for(&grid, &bot, "a CompletePingCheck", |s| {
        *s == Seen::CompletePingCheck { session: 1 }
    })
    .await;
    bot.wait_log("ping ").await;

    bot.terminate();
    wait_for(&grid, &bot, "the LogoutRequest", |s| {
        *s == Seen::LogoutRequest { session: 1 }
    })
    .await;
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    bot.wait_log("logged out").await;
    assert_eq!(grid.count(|s| matches!(s, Seen::Login { .. })), 1);
}

#[tokio::test]
async fn a_kick_logs_in_again() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[]);
    arrived(&grid, &bot, 1).await;
    grid.kick("You have been logged out by the grid.");
    bot.wait_log("disconnected by the grid: You have been logged out by the grid.")
        .await;
    arrived(&grid, &bot, 2).await;
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

#[tokio::test]
async fn a_closed_circuit_logs_in_again() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[]);
    arrived(&grid, &bot, 1).await;
    grid.disable_simulator();
    bot.wait_log("connection lost: the region closed the connection")
        .await;
    arrived(&grid, &bot, 2).await;
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

#[tokio::test]
async fn without_relog_a_kick_ends_the_bot() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[("FERRO_RELOG", "false")]);
    arrived(&grid, &bot, 1).await;
    grid.kick("Kicked for a test.");
    assert_eq!(bot.exit_code().await, 1, "{}", bot.log());
    assert_eq!(grid.count(|s| matches!(s, Seen::Login { .. })), 1);
}

#[tokio::test]
async fn a_wrong_password_ends_with_exit_code_2() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[("FERRO_PASSWORD", "not the password")]);
    assert_eq!(bot.exit_code().await, 2, "{}", bot.log());
    assert_eq!(
        grid.seen(),
        vec![Seen::LoginRefused {
            reason: "key".into()
        }]
    );
}

#[tokio::test]
async fn a_start_location_goes_in_the_login() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[("FERRO_START", "Test Region/10/20/30")]);
    wait_for(
        &grid,
        &bot,
        "the login",
        |s| matches!(s, Seen::Login { start, .. } if start == "uri:Test Region&10&20&30"),
    )
    .await;
    arrived(&grid, &bot, 1).await;
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// CPU time (user + system) of a process, in clock ticks, from /proc.
#[cfg(target_os = "linux")]
fn cpu_ticks(pid: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("/proc stat");
    // after the command name, which ends with the last ')': utime and stime are fields 14 and 15
    let rest = &stat[stat.rfind(')').expect("comm") + 2..];
    let fields: Vec<&str> = rest.split(' ').collect();
    fields[11].parse::<u64>().expect("utime") + fields[12].parse::<u64>().expect("stime")
}

/// A region that closes its event queue (404) does not end the session (Aurora), and the
/// session loop does not spin on the closed channel.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_closed_event_queue_neither_ends_the_session_nor_spins() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[]);
    arrived(&grid, &bot, 1).await;
    wait_for(&grid, &bot, "an event queue poll", |s| {
        *s == Seen::EventQueuePoll { session: 1 }
    })
    .await;
    grid.close_event_queue();
    bot.wait_log("event queue closed").await;

    let pid = bot.child.id().expect("running");
    let updates = grid.count(|s| *s == Seen::AgentUpdate { session: 1 });
    let before = cpu_ticks(pid);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let used = cpu_ticks(pid) - before;
    // a spinning loop burns a whole core: about 300 ticks in 3 s at the usual 100 Hz
    assert!(used < 100, "{used} ticks of CPU in 3 s\n{}", bot.log());
    assert!(grid.count(|s| *s == Seen::AgentUpdate { session: 1 }) > updates);
    assert_eq!(grid.count(|s| matches!(s, Seen::Login { .. })), 1);

    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}
