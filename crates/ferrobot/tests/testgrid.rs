//! The real bot, run as a process against the test grid (`crates/testgrid`): login, arrival,
//! chat and IMs, pings, logout on SIGTERM, a kick, a closed circuit, a silent region and a failing
//! event queue and the relog after them, a refused login. ROADMAP phase 1.
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
    health_file: std::path::PathBuf,
}

impl Drop for Bot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.health_file);
    }
}

/// A health file of its own for each bot: the tests run side by side.
fn health_file() -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("ferrobot-test-health-{}-{n}", std::process::id()))
}

impl Bot {
    fn start(grid: &TestGrid, env: &[(&str, &str)]) -> Bot {
        let health_file = health_file();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrobot"));
        // nothing from the environment of whoever runs the tests: no real account by mistake
        cmd.env_clear()
            .env("FERRO_LOGIN_URI", grid.login_uri())
            .env("FERRO_USERNAME", grid.username())
            .env("FERRO_PASSWORD", grid.password())
            .env("FERRO_RELOG_DELAY", "0")
            .env("RUST_LOG", "debug,ferrobot=trace,ferro_client=trace")
            .env("FERRO_HEALTH_FILE", &health_file)
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
        Bot {
            child,
            log,
            health_file,
        }
    }

    /// `ferrobot healthcheck`, as Docker runs it.
    async fn healthcheck(&self) -> i32 {
        Command::new(env!("CARGO_BIN_EXE_ferrobot"))
            .arg("healthcheck")
            .env_clear()
            .env("FERRO_HEALTH_FILE", &self.health_file)
            .stderr(Stdio::null())
            .status()
            .await
            .expect("healthcheck runs")
            .code()
            .expect("an exit code")
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
        position(
            &seen,
            &Seen::UseCircuitCode {
                session: 1,
                region: 0
            }
        ) < position(
            &seen,
            &Seen::CompleteAgentMovement {
                session: 1,
                region: 0
            }
        )
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
        matches!(s, Seen::SeedRequest { session: 1, caps, .. } if caps.iter().any(|c| c == "EventQueueGet"))
    })
    .await;
    wait_for(&grid, &bot, "an event queue poll", |s| {
        *s == Seen::EventQueuePoll {
            session: 1,
            region: 0,
        }
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
        *s == Seen::EventQueuePoll {
            session: 1,
            region: 0,
        }
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

/// A region that goes silent is lost once nothing came from it for FERRO_REGION_TIMEOUT (100 s on
/// a grid, as the viewer and Aurora; 3 s here), and the bot logs in again. The region pings every
/// 2 s, so a quiet but living region keeps the session.
#[tokio::test]
async fn a_silent_region_is_lost_then_the_bot_logs_in_again() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[("FERRO_REGION_TIMEOUT", "3")]);
    arrived(&grid, &bot, 1).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(!bot.log().contains("connection lost"), "{}", bot.log());

    grid.silence_region();
    let silent_since = tokio::time::Instant::now();
    bot.wait_log("connection lost: nothing from the region for 3 s")
        .await;
    assert!(
        silent_since.elapsed() >= Duration::from_millis(2900),
        "lost after {:?}\n{}",
        silent_since.elapsed(),
        bot.log()
    );
    arrived(&grid, &bot, 2).await;
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// An event queue that keeps failing at once is polled again after a back-off, then given up after
/// FERRO_EVENT_QUEUE_ERRORS failures in a row (15 on a grid, as lleventpoll.cpp; 1 here): the
/// session ends and the bot logs in again (Aurora's on_eq_lost).
#[tokio::test]
async fn an_event_queue_that_fails_is_given_up_then_the_bot_logs_in_again() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[("FERRO_EVENT_QUEUE_ERRORS", "1")]);
    arrived(&grid, &bot, 1).await;
    wait_for(&grid, &bot, "an event queue poll", |s| {
        *s == Seen::EventQueuePoll {
            session: 1,
            region: 0,
        }
    })
    .await;
    grid.fail_event_queue();
    bot.wait_log("error 1 of 1 in a row, next poll in 1 s")
        .await;
    bot.wait_log("giving up after 1 errors in a row").await;
    bot.wait_log("connection lost: the region's event queue was lost")
        .await;
    arrived(&grid, &bot, 2).await;
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// Without relog, a lost region ends the bot with exit code 1, as a kick does.
#[tokio::test]
async fn without_relog_a_silent_region_ends_the_bot() {
    let grid = grid().await;
    let mut bot = Bot::start(
        &grid,
        &[("FERRO_REGION_TIMEOUT", "2"), ("FERRO_RELOG", "false")],
    );
    arrived(&grid, &bot, 1).await;
    grid.silence_region();
    assert_eq!(bot.exit_code().await, 1, "{}", bot.log());
    assert!(
        bot.log()
            .contains("connection lost: nothing from the region for 2 s")
    );
    assert_eq!(grid.count(|s| matches!(s, Seen::Login { .. })), 1);
}

#[tokio::test]
async fn the_healthcheck_follows_the_bot() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[]);
    arrived(&grid, &bot, 1).await;
    // written every 5 s
    let deadline = tokio::time::Instant::now() + WAIT;
    while !std::fs::read_to_string(&bot.health_file).is_ok_and(|t| t.contains("online=true")) {
        assert!(tokio::time::Instant::now() < deadline, "{}", bot.log());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(bot.healthcheck().await, 0);
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    // no file: unhealthy
    std::fs::remove_file(&bot.health_file).expect("remove");
    assert_eq!(bot.healthcheck().await, 1);
}

/// A master's teleport offer is accepted: the bot arrives in the other region and keeps working
/// there; anyone else's is ignored.
#[tokio::test]
async fn a_master_teleports_the_bot_and_others_cannot() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[("FERRO_MASTERS", "Other Avatar, some")]);
    arrived(&grid, &bot, 1).await;

    let stranger = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0003),
        first: "Someone".into(),
        last: "Else".into(),
    };
    grid.offer_teleport(&stranger);
    bot.wait_log("teleport offer from Someone Else refused: not a master")
        .await;
    assert_eq!(
        grid.count(|s| matches!(s, Seen::TeleportLureRequest { .. })),
        0
    );

    let master = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0001),
        first: "Some".into(),
        last: "Resident".into(),
    };
    let lure = grid.offer_teleport(&master);
    wait_for(&grid, &bot, "the TeleportLureRequest", |s| {
        *s == Seen::TeleportLureRequest { session: 1, lure }
    })
    .await;
    wait_for(&grid, &bot, "the arrival in the second region", |s| {
        *s == Seen::CompleteAgentMovement {
            session: 1,
            region: ferro_testgrid::SECOND_REGION,
        }
    })
    .await;
    bot.wait_log("teleported to Second Region").await;
    // chat and IMs in the new region reach the bot
    grid.say("Other Avatar", "you made it");
    bot.wait_log("[chat] Other Avatar: you made it").await;

    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    assert_eq!(grid.count(|s| matches!(s, Seen::Login { .. })), 1);
}

fn uuid_from(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

/// A restart announced: home is in the other region, so the bot goes there at once.
#[tokio::test]
async fn before_a_region_restart_the_bot_goes_home() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[]);
    arrived(&grid, &bot, 1).await;
    grid.announce_restart(300);
    bot.wait_log("the region restarts in 300 s: going home")
        .await;
    wait_for(
        &grid,
        &bot,
        "the home teleport",
        |s| matches!(s, Seen::TeleportLandmarkRequest { landmark, .. } if landmark.is_nil()),
    )
    .await;
    bot.wait_log("home, in Second Region, before the restart")
        .await;
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// Group invitations: a master's is accepted, a stranger's and one with a fee are not.
#[tokio::test]
async fn group_invitations_from_masters_only_and_never_with_a_fee() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[("FERRO_MASTERS", "Some Resident")]);
    arrived(&grid, &bot, 1).await;
    let stranger = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0003),
        first: "Someone".into(),
        last: "Else".into(),
    };
    let master = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0001),
        first: "Some".into(),
        last: "Resident".into(),
    };
    grid.invite_bot(&stranger, ferro_testgrid::THIRD_GROUP, 0);
    bot.wait_log("group invitation from Someone Else refused: not a master")
        .await;
    grid.invite_bot(&master, ferro_testgrid::THIRD_GROUP, 50);
    bot.wait_log("refused: a fee of L$50, the bot does not pay to join")
        .await;
    assert_eq!(
        grid.count(|s| matches!(s, Seen::GroupInviteAnswer { .. })),
        0
    );

    grid.invite_bot(&master, ferro_testgrid::THIRD_GROUP, 0);
    wait_for(&grid, &bot, "the acceptance", |s| {
        *s == Seen::GroupInviteAnswer {
            group: ferro_testgrid::THIRD_GROUP,
            accept: true,
        }
    })
    .await;
    bot.wait_log("group invitation from Some Resident accepted")
        .await;
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

#[tokio::test]
async fn group_chat_is_logged() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[]);
    arrived(&grid, &bot, 1).await;
    // the group list first
    bot.wait_log("in 2 groups").await;
    let some = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0001),
        first: "Some".into(),
        last: "Resident".into(),
    };
    grid.group_chat(&some, ferro_testgrid::TEST_GROUP, "anyone there?");
    bot.wait_log("[group Test Group] Some Resident: anyone there?")
        .await;
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// SecondBot's HTTP API, called the way BB's SlBot.php calls it: form fields, sha1 signature,
/// `{Key, Value}` answers with the command's JSON inside.
mod api {
    use super::*;
    use ferro_testgrid::{BUILDERS, TEST_GROUP};
    use serde_json::Value;
    use sha1::{Digest, Sha1};

    const SECRET: &str = "a shared secret";
    const SOME: &str = "00000000-0000-0000-0000-00005eed0001";
    const OTHER: &str = "00000000-0000-0000-0000-00005eed0002";

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .expect("bind")
            .local_addr()
            .expect("address")
            .port()
    }

    fn encode(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64
    }

    struct Api {
        url: String,
        http: reqwest::Client,
    }

    impl Api {
        /// POST /api/Run with these fields, the body as it comes.
        async fn post(&self, fields: &[(&str, String)]) -> String {
            let body: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect();
            self.http
                .post(&self.url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(body.join("&"))
                .send()
                .await
                .expect("the API answers")
                .text()
                .await
                .expect("a body")
        }

        /// A command signed as SlBot::run signs it; the raw body.
        async fn raw(&self, command: &str, args: &[&str], unixtime: i64, secret: &str) -> String {
            let joined = args.join("~#~");
            let sig: String = Sha1::digest(format!("{command}{joined}{unixtime}{secret}"))
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let mut fields = vec![
                ("commandName", command.to_owned()),
                ("unixtime", unixtime.to_string()),
                ("signing", sig),
            ];
            if !joined.is_empty() {
                fields.push(("args", joined));
            }
            self.post(&fields).await
        }

        /// A command's `{Key, Value}`, and its inner JSON when it has one.
        async fn run(&self, command: &str, args: &[&str]) -> (bool, Value) {
            let body = self.raw(command, args, now(), SECRET).await;
            let outer: Value = serde_json::from_str(&body).unwrap_or_else(|_| panic!("{body}"));
            let key = outer["Key"].as_bool().expect("Key");
            let value = outer["Value"].as_str().expect("Value");
            (
                key,
                serde_json::from_str(value).unwrap_or(Value::String(value.into())),
            )
        }

        /// The `reply` of a command that worked.
        async fn reply(&self, command: &str, args: &[&str]) -> String {
            let (key, inner) = self.run(command, args).await;
            assert!(key && inner["status"] == true, "{command}: {inner}");
            // the answer names the command as SecondBot spells it, whatever the case asked
            assert!(
                inner["command"]
                    .as_str()
                    .is_some_and(|c| c.eq_ignore_ascii_case(command)),
                "{inner}"
            );
            inner["reply"].as_str().expect("reply").to_owned()
        }

        /// Asks GetPosition until it answers `expected`.
        async fn wait_position(&self, expected: Value) {
            let deadline = tokio::time::Instant::now() + WAIT;
            loop {
                let pos: Value =
                    serde_json::from_str(&self.reply("GetPosition", &[]).await).expect("map");
                if pos == expected {
                    return;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "{pos}, not {expected}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        /// The `reply` and `args` of a command that failed.
        async fn failure(&self, command: &str, args: &[&str]) -> (String, Value) {
            let (key, inner) = self.run(command, args).await;
            assert!(key && inner["status"] == false, "{command}: {inner}");
            assert!(inner["whyfailed"].is_null());
            (
                inner["reply"].as_str().expect("reply").to_owned(),
                inner["args"].clone(),
            )
        }
    }

    /// SecondBot's inventory extras: SetPermFlag, the inventory tree and its counts, purging old
    /// notecards, the sculpted prims, the inventory update events and FetchNextNotecard.
    #[tokio::test]
    async fn inventory_extras() {
        use ferro_testgrid::inventory as inv;
        let grid = grid().await;
        let (mut bot, api) = bot_with_settings(&grid, &[("FERRO_MASTERS", "Some Resident")]).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;
        let some = ferro_testgrid::Resident {
            id: uuid_from(0x5eed_0001),
            first: "Some".into(),
            last: "Resident".into(),
        };
        let who = some.id.to_string();

        // SetPermFlag answers as SecondBot's, and changes nothing
        assert_eq!(
            api.reply("SetPermFlag", &[&who, "group", "true", "false"])
                .await,
            "Added perm: group Sticky: False"
        );
        assert_eq!(
            api.reply("SetPermFlag", &[&who, "inventory", "False", "TRUE"])
                .await,
            "Removed perm: inventory Sticky: True"
        );
        let nil = uuid::Uuid::nil().to_string();
        for (args, why) in [
            ([nil.as_str(), "group", "true", "true"], "avatar lookup"),
            ([who.as_str(), "group", "yes", "true"], "Invaild state"),
            ([who.as_str(), "group", "true", "1"], "Invaild sticky"),
            ([who.as_str(), "Group", "true", "true"], "Invaild flag"),
        ] {
            let (reply, sent) = api.failure("SetPermFlag", &args).await;
            assert_eq!(reply, why);
            assert_eq!(sent, serde_json::json!(args));
        }

        // no preload: always ready; the counts grow with what is fetched
        assert_eq!(api.reply("InventoryPreloadDone", &[]).await, "Ready");
        let tree: Value =
            serde_json::from_str(&api.reply("InventoryContentsCatched", &[]).await).expect("json");
        assert_eq!(tree["Name"], "My Inventory");
        assert_eq!(tree["UUID"], inv::ROOT.to_string());
        assert_eq!(tree["InventoryType"], "Folder");
        let notecards = tree["Children"]
            .as_array()
            .expect("children")
            .iter()
            .find(|c| c["Name"] == "Notecards")
            .expect("the Notecards folder");
        assert_eq!(
            notecards["Children"][0],
            serde_json::json!({
                "UUID": inv::NOTECARD.to_string(),
                "Name": "Welcome",
                "Children": null,
                "InventoryType": "Notecard",
            })
        );
        let party = tree["Children"]
            .as_array()
            .expect("children")
            .iter()
            .find(|c| c["Name"] == "My Outfits")
            .expect("My Outfits");
        assert_eq!(party["Children"][0]["Name"], "Party");
        let count = api.reply("InventoryPreloadCount", &[]).await;
        let (folders, items) = count
            .strip_prefix("folders:")
            .and_then(|c| c.split_once(",items:"))
            .unwrap_or_else(|| panic!("{count}"));
        assert!(folders.parse::<usize>().expect("folders") >= 9, "{count}");
        assert!(items.parse::<usize>().expect("items") >= 10, "{count}");

        // purging: the old notecard goes for good, the Welcome notecard stays
        assert_eq!(api.reply("InventoryPurgeNotecards", &[]).await, "Ok");
        let old_asset = uuid_from(0x3000_0008);
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::Object {
                    what: format!("unlink {old_asset}")
                })
            .await
        );
        let left = api
            .reply("InventoryContents", &[&inv::NOTECARDS.to_string()])
            .await;
        assert!(
            left.contains("Welcome") && !left.contains("Old Note"),
            "{left}"
        );
        assert_eq!(
            grid.count(|s| matches!(s, Seen::Object { what } if what.starts_with("unlink"))),
            1
        );

        // the sculpted child prim, reported by its root
        let sculpts: Value =
            serde_json::from_str(&api.reply("ListSculptys", &[]).await).expect("json");
        assert_eq!(
            sculpts,
            serde_json::json!([{
                "name": "?",
                "isLinked": "True",
                "owner": who,
                "uuid": ferro_testgrid::OBJECT.to_string(),
                "pos": "<100, 120, 22>",
            }])
        );

        // inventory updates: a notecard accepted is told on a channel and by HTTP
        let (hook, caught) = super::catch_a_post().await;
        assert_eq!(
            api.reply("SetInventoryUpdate", &["notecard", "7"]).await,
            "Ok"
        );
        assert_eq!(
            api.reply("SetInventoryUpdate", &["Notecard", &hook]).await,
            "Ok"
        );
        assert_eq!(
            api.reply("SetInventoryUpdate", &["Object", "8"]).await,
            "Ok"
        );
        grid.offer_inventory(&some, None);
        assert!(
            grid.wait_for(WAIT, |s| matches!(s, Seen::Chat { message, channel: 7, .. }
                if message.starts_with(r#"{"itemname":"A notecard","itemuuid":""#)
                    && message.ends_with(r#"","itemtype":"Notecard"}"#)))
                .await,
            "{}",
            bot.log()
        );
        let body = tokio::time::timeout(WAIT, caught)
            .await
            .expect("the update in time")
            .expect("an update");
        assert_eq!(
            super::form_field(&body, "method").as_deref(),
            Some("Inventory")
        );
        assert_eq!(
            super::form_field(&body, "action").as_deref(),
            Some("Update")
        );
        let event = super::form_field(&body, "event").expect("event");
        let unixtime = super::form_field(&body, "unixtime").expect("unixtime");
        let token: String = Sha1::digest(format!(
            "{unixtime}EventService{}{event}{SECRET}",
            grid.agent_id()
        ))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
        assert_eq!(super::form_field(&body, "token"), Some(token));
        assert!(event.contains(r#""itemtype":"Notecard""#), "{event}");
        assert_eq!(
            grid.count(|s| matches!(s, Seen::Chat { channel: 8, .. })),
            0
        );

        // FetchNextNotecard: the server's next notecard, sent to whom it names
        let answer = format!(
            r#"{{"status":true,"message":"ok","AvatarUUID":"{who}","NotecardTitle":"Stream details","NotecardContent":"port 8000"}}"#
        );
        let (endpoint, asked) = super::serve_a_post("200 OK", answer).await;
        assert_eq!(
            api.reply("FetchNextNotecard", &[&endpoint, "a code"]).await,
            "ok"
        );
        let (path, body) = asked.await.expect("the request");
        assert_eq!(path, "/sys.php/Notecard/Next");
        let unixtime = super::form_field(&body, "unixtime").expect("unixtime");
        let token: String = Sha1::digest(format!("{unixtime}NotecardNexta code"))
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(super::form_field(&body, "token"), Some(token));
        assert_eq!(
            super::form_field(&body, "method").as_deref(),
            Some("Notecard")
        );
        assert!(
            grid.wait_for(
                WAIT,
                |s| matches!(s, Seen::InventoryOffer { to, name, entries }
                if *to == some.id && name == "Stream details" && entries[0].0 == 7)
            )
            .await
        );
        let (endpoint, _) = super::serve_a_post(
            "200 OK",
            r#"{"status":false,"message":"nothing queued"}"#.into(),
        )
        .await;
        assert_eq!(
            api.failure("FetchNextNotecard", &[&endpoint, "a code"])
                .await
                .0,
            "Bad reply: nothing queued"
        );
        let (endpoint, _) = super::serve_a_post("404 Not Found", String::new()).await;
        assert_eq!(
            api.failure("FetchNextNotecard", &[&endpoint, "a code"])
                .await
                .0,
            "HTTP status code: NotFound"
        );
        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    async fn bot_with_api(grid: &TestGrid, login: Option<&str>) -> (Bot, Api) {
        let login: Vec<(&str, &str)> = login
            .map(|uri| ("FERRO_LOGIN_URI", uri))
            .into_iter()
            .collect();
        bot_with_settings(grid, &login).await
    }

    /// The bot with its API, and these settings too.
    async fn bot_with_settings(grid: &TestGrid, settings: &[(&str, &str)]) -> (Bot, Api) {
        let port = free_port().to_string();
        let mut env = vec![
            ("FERRO_HTTP_PORT", port.as_str()),
            ("commands_SharedSecret", SECRET),
        ];
        env.extend_from_slice(settings);
        let bot = Bot::start(grid, &env);
        bot.wait_log("HTTP API on port").await;
        let api = Api {
            url: format!("http://127.0.0.1:{port}/api/Run"),
            http: reqwest::Client::new(),
        };
        (bot, api)
    }

    /// `GET /health` for checks through a tunnel: 503 until in world, then 200.
    #[tokio::test]
    async fn the_health_endpoint_follows_the_bot() {
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, Some("http://127.0.0.1:1/")).await;
        let health = api.url.replace("/api/Run", "/health");
        let resp = api.http.get(&health).send().await.expect("an answer");
        assert_eq!(resp.status().as_u16(), 503);
        assert_eq!(resp.text().await.expect("body"), "offline");
        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());

        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        let health = api.url.replace("/api/Run", "/health");
        let resp = api.http.get(&health).send().await.expect("an answer");
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(resp.text().await.expect("body"), "ok");
        // anything else answers as SecondBot's catch-all does
        let other = api.url.replace("/api/Run", "/nothing");
        let body = api
            .http
            .get(&other)
            .send()
            .await
            .expect("an answer")
            .text()
            .await
            .expect("body");
        assert_eq!(body, r#"{"Message":"Error"}"#);
        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    #[tokio::test]
    async fn refusals_are_plain_text() {
        let grid = grid().await;
        // nowhere to log in: the API answers, the bot is not connected
        let (mut bot, api) = bot_with_api(&grid, Some("http://127.0.0.1:1/")).await;
        assert_eq!(
            api.raw("Name", &[], now(), SECRET).await,
            "No bot connected yet"
        );
        assert_eq!(
            api.post(&[("commandName", "Name".into()), ("unixtime", "soon".into())])
                .await,
            "Invaild unixtime was sent"
        );
        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());

        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        assert_eq!(
            api.raw("Name", &[], now(), "wrong").await,
            "Command request rejected"
        );
        assert_eq!(
            api.raw("Name", &[], now() - 36, SECRET).await,
            "Command request rejected",
            "older than 35 s"
        );
        assert_eq!(
            api.run("NoSuchCommand", &[]).await,
            (false, Value::String("Unknown command".into()))
        );
        assert_eq!(
            api.run("Name", &["extra"]).await,
            (
                false,
                Value::String("incorrect number of args expected: 0 but got: 1".into())
            )
        );
        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    #[tokio::test]
    async fn bbs_commands_answer_as_secondbot() {
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("in 2 groups").await;
        let group = TEST_GROUP.to_string();
        let builders = BUILDERS.to_string();

        assert_eq!(api.reply("SimName", &[]).await, "Test Region");
        assert_eq!(api.reply("name", &[]).await, "Test Bot");
        assert_eq!(api.run("name", &[]).await.1["command"], "Name");
        assert_eq!(api.reply("Name2Key", &["Other Avatar"]).await, OTHER);
        assert_eq!(api.reply("Name2Key", &["Nobody Here"]).await, "lookup");

        let list: Value = serde_json::from_str(&api.reply("GetGroupList", &[]).await).expect("map");
        assert_eq!(list[&group], "Test Group");
        let roles: Value =
            serde_json::from_str(&api.reply("GetGroupRoles", &[&group]).await).expect("map");
        assert_eq!(roles[&builders], "Builders");
        assert_eq!(roles["00000000-0000-0000-0000-000000000000"], "Everyone");
        let members: Vec<String> =
            serde_json::from_str(&api.reply("GetGroupMembers", &[&group]).await).expect("list");
        assert_eq!(members.len(), 3);
        assert!(members.contains(&OTHER.to_owned()));
        let holders: Vec<String> =
            serde_json::from_str(&api.reply("GetGroupRoleMembers", &[&group, &builders]).await)
                .expect("list");
        assert_eq!(holders, vec![SOME.to_owned()]);
        assert_eq!(
            api.reply("IsGroupMember", &[&group, "Other Avatar"]).await,
            "True"
        );
        assert_eq!(
            api.reply("IsGroupMember", &[&group, "Nobody Here"]).await,
            "False"
        );

        // an invitation with "everyone", then one with a role on top
        let newcomer = "00000000-0000-0000-0000-00005eed0009";
        assert_eq!(
            api.reply("GroupInvite", &[&group, newcomer, "everyone"])
                .await,
            "Invite sent"
        );
        assert_eq!(
            api.reply("GroupInvite", &[&group, newcomer, &builders])
                .await,
            "Invite sent"
        );
        let invitee = uuid_from(0x5eed_0009);
        assert!(
            grid.wait_until(WAIT, |seen| {
                seen.iter()
                    .filter(|s| matches!(s, Seen::GroupInvite { invitee: i, .. } if *i == invitee))
                    .count()
                    == 3
            })
            .await,
            "{:?}",
            grid.seen()
        );

        assert_eq!(
            api.reply("GroupAddRole", &[&group, OTHER, &builders]).await,
            "Adding role"
        );
        assert!(
            grid.wait_until(WAIT, |_| grid
                .group_members(TEST_GROUP)
                .iter()
                .any(|m| m.id == uuid_from(0x5eed_0002) && m.roles.contains(&BUILDERS)))
                .await
        );
        assert_eq!(
            api.reply("GroupRemoveRole", &[&group, OTHER, &builders])
                .await,
            "Removing role"
        );
        assert_eq!(api.reply("GroupEject", &[&group, OTHER]).await, "Accepted");
        assert_eq!(
            api.failure("GroupEject", &[&group, OTHER]).await.0,
            "Not in group"
        );
        // an avatar not in the group is invited instead
        assert_eq!(
            api.reply("GroupAddRole", &[&group, OTHER, &builders]).await,
            "Invite sent"
        );

        let (why, args) = api.failure("GetGroupRoles", &["nonsense"]).await;
        assert_eq!(
            (why.as_str(), args),
            ("Invaild group UUID", serde_json::json!(["nonsense"]))
        );
        let (why, args) = api
            .failure("GroupAddRole", &["nonsense", OTHER, &builders])
            .await;
        assert_eq!(
            (why.as_str(), args),
            ("Invaild group UUID", serde_json::json!(["nonsense", OTHER])),
            "two args, as SecondBot"
        );
        assert_eq!(
            api.failure(
                "GroupRemoveRole",
                &[&group, OTHER, "00000000-0000-0000-0000-000000000000"]
            )
            .await
            .0,
            "The everyone role cannot be removed"
        );
        let third = ferro_testgrid::THIRD_GROUP.to_string();
        assert_eq!(
            api.failure("GetGroupRoles", &[&third]).await.0,
            "Unknown group"
        );

        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// SecondBot's Inventory commands, against the grid's inventory.
    #[tokio::test]
    async fn inventory_commands() {
        use ferro_testgrid::inventory as inv;
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;

        let tree: Value =
            serde_json::from_str(&api.reply("InventoryFolders", &[]).await).expect("tree");
        assert_eq!(tree["name"], "My Inventory");
        let names: Vec<&str> = tree["subfolders"]
            .as_array()
            .expect("subfolders")
            .iter()
            .map(|f| f["name"].as_str().expect("name"))
            .collect();
        assert!(
            names.contains(&"Notecards") && names.contains(&"Trash"),
            "{names:?}"
        );
        let stuff = tree["subfolders"]
            .as_array()
            .expect("subfolders")
            .iter()
            .find(|f| f["name"] == "Stuff")
            .expect("Stuff");
        assert_eq!(stuff["subfolders"][0]["name"], "Sub");

        let contents: Vec<Value> = serde_json::from_str(
            &api.reply("InventoryContents", &[&inv::NOTECARDS.to_string()])
                .await,
        )
        .expect("list");
        assert_eq!(contents[0]["typename"], "Folder");
        assert_eq!(contents[0]["name"], "Notecards");
        assert_eq!(contents[1]["name"], "Welcome");
        assert_eq!(contents[1]["typename"], "InventoryNotecard");

        // the asset behind a link
        assert_eq!(
            api.reply("GetRealUUID", &[&inv::LINK.to_string()]).await,
            inv::NOTECARD_ASSET.to_string()
        );

        // sending an item and a folder
        assert_eq!(
            api.reply("SendItem", &[&inv::NOTECARD.to_string(), "Other Avatar"])
                .await,
            "ok"
        );
        assert_eq!(
            api.reply("SendFolder", &[&inv::OBJECTS.to_string(), OTHER])
                .await,
            "ok"
        );
        let other = uuid_from(0x5eed_0002);
        assert!(
            grid.wait_until(WAIT, |seen| {
                seen.contains(&Seen::InventoryOffer {
                    to: other,
                    name: "Welcome".into(),
                    entries: vec![(7, inv::NOTECARD)],
                }) && seen.contains(&Seen::InventoryOffer {
                    to: other,
                    name: "Objects".into(),
                    entries: vec![(8, inv::OBJECTS), (6, inv::BOX)],
                })
            })
            .await,
            "{:?}",
            grid.seen()
        );
        assert_eq!(
            api.failure("SendItem", &["00000000-0000-0000-0000-0000000000ff", OTHER])
                .await
                .0,
            "Unable to find item"
        );

        // by path, and directly by key, name and type
        assert_eq!(
            api.reply("SendItemByPath", &["Objects/Box", OTHER]).await,
            "ok"
        );
        assert_eq!(
            api.reply("SendFolderByPath", &["/Stuff/Sub/", OTHER]).await,
            "ok"
        );
        let direct = uuid_from(0xd1ec7);
        let (_, inner) = api
            .run(
                "SendItemDirect",
                &[&direct.to_string(), "A Gift", "Object", "Other Avatar"],
            )
            .await;
        assert_eq!(inner["reply"], "ok", "{inner}");
        assert_eq!(
            inner["args"][3],
            other.to_string(),
            "the avatar's key, as SecondBot"
        );
        assert!(
            grid.wait_until(WAIT, |seen| {
                seen.contains(&Seen::InventoryOffer {
                    to: other,
                    name: "Box".into(),
                    entries: vec![(6, inv::BOX)],
                }) && seen.contains(&Seen::InventoryOffer {
                    to: other,
                    name: "Sub".into(),
                    entries: vec![(8, inv::SUB)],
                }) && seen.contains(&Seen::InventoryOffer {
                    to: other,
                    name: "A Gift".into(),
                    entries: vec![(6, direct)],
                })
            })
            .await,
            "{:?}",
            grid.seen()
        );
        assert_eq!(
            api.failure("SendItemByPath", &["Objects/Nothing", OTHER])
                .await
                .0,
            "Unable to find item via path"
        );
        assert_eq!(
            api.failure("SendItemByPath", &["Nowhere/Box", OTHER])
                .await
                .0,
            "Unable to find item via path"
        );
        assert_eq!(
            api.failure("SendFolderByPath", &["Stuff/None", OTHER])
                .await
                .0,
            "Unable to find folder"
        );
        assert_eq!(
            api.failure(
                "SendItemDirect",
                &[&direct.to_string(), "A Gift", "folder", OTHER]
            )
            .await
            .0,
            "Invaild item type"
        );

        // a new folder, an item renamed and moved, a folder moved, things thrown away
        let (key, inner) = api
            .run(
                "CreateInventoryFolder",
                &[&inv::ROOT.to_string(), "Deliveries"],
            )
            .await;
        assert!(key && inner["reply"] == "Ok");
        let new_id: uuid::Uuid = inner["args"][0]
            .as_str()
            .expect("its key")
            .parse()
            .expect("a key");
        assert_eq!(
            api.reply("RenameInventory", &[&inv::BOX.to_string(), "Big Box"])
                .await,
            "Ok"
        );
        assert_eq!(
            api.reply(
                "MoveInventoryItem",
                &[&inv::BOX.to_string(), &new_id.to_string()]
            )
            .await,
            "Ok"
        );
        assert_eq!(
            api.reply(
                "MoveInventoryFolder",
                &[&inv::SUB.to_string(), &new_id.to_string()]
            )
            .await,
            "Ok"
        );
        assert_eq!(
            api.reply("DeleteInventoryItem", &[&inv::NOTECARD.to_string()])
                .await,
            "Ok"
        );
        assert!(
            grid.wait_until(WAIT, |_| {
                let now = grid.inventory();
                let item = |id| now.items.iter().find(|i| i.id == id).cloned();
                now.folders
                    .iter()
                    .any(|f| f.id == new_id && f.name == "Deliveries")
                    && item(inv::BOX).is_some_and(|b| b.name == "Big Box" && b.parent == new_id)
                    && now
                        .folders
                        .iter()
                        .any(|f| f.id == inv::SUB && f.parent == new_id)
                    && item(inv::NOTECARD).is_some_and(|n| n.parent == inv::TRASH)
            })
            .await,
            "{:?}",
            grid.inventory()
        );
        assert_eq!(
            api.failure("RenameInventory", &[&inv::BOX.to_string(), "ab"])
                .await
                .0,
            "Item name is to short"
        );

        // notecards: straight from a text, and from a collection
        assert_eq!(
            api.reply(
                "NotecardDirectSend",
                &["Other Avatar", "Hello there", "Note"]
            )
            .await,
            "ok"
        );
        let (_, inner) = api.run("NotecardAdd", &["list", "first line, "]).await;
        assert_eq!(inner["args"], serde_json::json!(["list", "12"]));
        api.reply("NotecardAdd", &["list", "second line"]).await;
        assert_eq!(
            api.reply("NotecardSend", &[OTHER, "list", "Lines"]).await,
            "ok"
        );
        assert_eq!(
            api.failure("NotecardSend", &[OTHER, "list", "Lines"])
                .await
                .0,
            "No content in notecard storage ?"
        );
        let asset = |text: &str| {
            format!(
                "Linden text version 2\n{{\nLLEmbeddedItems version 1\n{{\ncount 0\n}}\nText length {}\n{text}}}\n",
                text.len()
            )
        };
        assert!(
            grid.wait_until(WAIT, |seen| {
                let saved = |text: &str| seen.iter().any(|s| matches!(s, Seen::NotecardSaved { asset: a, .. } if *a == asset(text)));
                let sent = |name: &str| seen.iter().any(|s| matches!(s, Seen::InventoryOffer { to, name: n, entries } if *to == other && n == name && entries[0].0 == 7));
                saved("Hello there") && saved("first line, second line") && sent("Note") && sent("Lines")
            })
            .await,
            "{:?}",
            grid.seen()
        );
        // created in the Notecards folder
        assert!(
            grid.inventory()
                .items
                .iter()
                .any(|i| i.name == "Note" && i.parent == inv::NOTECARDS)
        );

        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// Teleports, names, friends, avatars nearby, and the money commands (off, then on).
    #[tokio::test]
    async fn world_commands() {
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        let other = uuid_from(0x5eed_0002);
        let some = uuid_from(0x5eed_0001);

        assert_eq!(api.reply("Key2Name", &[OTHER]).await, "Other Avatar");
        assert_eq!(
            api.reply("Key2Name", &["00000000-0000-0000-0000-00000000dead"])
                .await,
            "lookup"
        );
        assert_eq!(api.reply("GetDisplayName", &[SOME]).await, "Sunny");

        // the region tells who is here (every ping, 2 s)
        let deadline = tokio::time::Instant::now() + WAIT;
        let near = loop {
            let near: Value = serde_json::from_str(&api.reply("Nearme", &[]).await).expect("map");
            if near.as_object().is_some_and(|m| m.len() == 2) {
                break near;
            }
            assert!(tokio::time::Instant::now() < deadline, "{near}");
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        assert_eq!(near[OTHER], "Other Avatar");

        // the friend from the login, online
        let friends: Vec<Value> =
            serde_json::from_str(&api.reply("Friendslist", &[]).await).expect("list");
        assert_eq!(friends[0]["name"], "Some Resident");
        assert_eq!(friends[0]["online"], true);
        assert_eq!(
            api.reply("FriendFullPerms", &[SOME, "true"]).await,
            "granted"
        );
        assert_eq!(
            api.failure("FriendFullPerms", &[OTHER, "true"]).await.0,
            "Not A friend"
        );
        assert_eq!(
            api.reply("FriendRequest", &[OTHER, "true"]).await,
            "Request sent"
        );
        assert_eq!(
            api.reply("FriendRequest", &[SOME, "false"]).await,
            "Removed"
        );
        assert!(
            grid.wait_until(WAIT, |seen| {
                seen.contains(&Seen::GrantRights {
                    to: some,
                    rights: 7,
                }) && seen.contains(&Seen::EndFriendship { other: some })
                    && seen
                        .iter()
                        .any(|s| matches!(s, Seen::ImAnswer { to, dialog: 38, .. } if *to == other))
            })
            .await
        );

        // teleports by command, a lure, a request
        assert_eq!(
            api.reply("Teleport", &["Second Region", "10", "20", "30"])
                .await,
            "Accepted"
        );
        assert_eq!(
            api.reply(
                "TeleportSLURL",
                &["http://maps.secondlife.com/secondlife/Test%20Region/40/50/60"]
            )
            .await,
            "True"
        );
        assert_eq!(
            api.failure("Teleport", &["Nowhere", "1", "2", "3"]).await.0,
            "Error Unable to Teleport to location"
        );
        assert_eq!(api.reply("SendTeleportLure", &[OTHER]).await, "ok");
        assert_eq!(api.reply("RequestTeleport", &[OTHER]).await, "ok");
        assert!(
            grid.wait_until(WAIT, |seen| {
                seen.contains(&Seen::StartLure { target: other })
                    && seen
                        .iter()
                        .any(|s| matches!(s, Seen::ImAnswer { to, dialog: 26, .. } if *to == other))
            })
            .await
        );

        // money: off by default
        assert_eq!(
            api.failure("Balance", &[]).await.0,
            "Funds commands are disabled"
        );
        assert_eq!(
            api.failure("PayAvatar", &[OTHER, "10"]).await.0,
            "Transfer funds to avatars disabled"
        );
        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());

        // money on
        let port = free_port().to_string();
        let mut bot = Bot::start(
            &grid,
            &[
                ("FERRO_HTTP_PORT", port.as_str()),
                ("commands_SharedSecret", SECRET),
                ("FERRO_ALLOW_FUNDS", "true"),
            ],
        );
        bot.wait_log("in world").await;
        let api = Api {
            url: format!("http://127.0.0.1:{port}/api/Run"),
            http: reqwest::Client::new(),
        };
        assert_eq!(api.reply("Balance", &[]).await, "500");
        assert_eq!(api.reply("PayAvatar", &[OTHER, "25"]).await, "Accepted");
        assert_eq!(
            api.failure("PayAvatar", &[OTHER, "100000"]).await.0,
            "Amount out of range current max: 475"
        );
        let object = "00000000-0000-0000-0000-0000000000bb";
        assert_eq!(
            api.reply("PayObject", &[object, "Tip jar", "5"]).await,
            "ok"
        );
        assert!(
            grid.wait_until(WAIT, |seen| {
                seen.contains(&Seen::Pay {
                    to: other,
                    amount: 25,
                    transaction_type: 5001,
                }) && seen.contains(&Seen::Pay {
                    to: object.parse().expect("key"),
                    amount: 5,
                    transaction_type: 5008,
                })
            })
            .await
        );
        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// SecondBot's Info, Core and Self commands, and BotStatus; Logoff stops the bot.
    #[tokio::test]
    async fn bot_commands() {
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        assert_eq!(api.reply("Hello", &[]).await, "world");
        assert!(api.reply("Version", &[]).await.starts_with("0."));
        let t: i64 = api.reply("UnixTimeNow", &[]).await.parse().expect("a time");
        assert!((t - now()).abs() < 5);
        let pos: Value = serde_json::from_str(&api.reply("GetPosition", &[]).await).expect("map");
        assert_eq!(pos, serde_json::json!({"x": 128, "y": 128, "z": 25}));

        assert_eq!(api.reply("ReadKeyValue", &["colour"]).await, "");
        assert_eq!(api.reply("SetKeyValue", &["colour", "blue"]).await, "ok");
        assert_eq!(api.reply("ReadKeyValue", &["colour"]).await, "blue");
        assert_eq!(api.reply("ClearKeyValue", &["colour"]).await, "ok");
        assert_eq!(api.reply("ReadKeyValue", &["colour"]).await, "");

        let (_, inner) = api.run("GetLastCommands", &[]).await;
        let last: Vec<Value> = inner["args"]
            .as_array()
            .expect("args")
            .iter()
            .map(|a| serde_json::from_str(a.as_str().expect("text")).expect("json"))
            .collect();
        assert_eq!(last[0]["Command"], "ReadKeyValue", "the newest first");
        assert_eq!(last[1]["Command"], "ClearKeyValue");

        let status: Value = serde_json::from_str(&api.reply("BotStatus", &[]).await).expect("map");
        assert_eq!(status["online"], true);
        assert_eq!(status["region"], "Test Region");
        assert_eq!(status["logins"], 1);
        assert!(status["commands"].as_u64().expect("a count") >= 10);

        // the outfit, baked by itself after the login (the version corrected), then on demand
        assert!(
            grid.wait_until(WAIT, |seen| seen
                .contains(&Seen::AppearanceRequest { cof_version: 0 })
                && seen.contains(&Seen::AppearanceRequest { cof_version: 7 }))
                .await
        );
        bot.wait_log("outfit baked (version 7)").await;
        assert_eq!(api.reply("RebakeAppearance", &[]).await, "ok");

        assert_eq!(
            api.failure("Discord_RoleList", &["800"]).await.0,
            "Discord client not ready",
            "Discord is off"
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
        bot.wait_log("logged out").await;
    }

    /// SecondBot's movement commands: fly, run, sit and stand, the autopilot, turning, hovering.
    #[tokio::test]
    async fn movement_commands() {
        const FLY: u32 = 0x2000;
        const STOP: u32 = 0x4000;
        const STAND_UP: u32 = 0x1_0000;
        const SIT_ON_GROUND: u32 = 0x2_0000;
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        let flags =
            |f: u32| move |s: &Seen| matches!(s, Seen::Controls { flags } if flags & f != 0);

        assert_eq!(api.reply("Fly", &["True"]).await, "ok");
        assert!(grid.wait_for(WAIT, flags(FLY)).await);
        assert_eq!(api.failure("Fly", &["maybe"]).await.0, "Invaild mode");
        assert_eq!(api.reply("Fly", &["false"]).await, "ok");
        assert!(
            grid.wait_for(
                WAIT,
                |s| matches!(s, Seen::Controls { flags } if flags & FLY == 0)
            )
            .await
        );

        assert_eq!(api.reply("SetAlwaysRun", &["true"]).await, "ok");
        assert!(
            grid.wait_for(WAIT, |s| *s == Seen::AlwaysRun { on: true })
                .await
        );

        // on an object: the region answers, and the bot sits where it is sent
        let chair = uuid_from(0xc4a1);
        assert_eq!(api.reply("Sit", &[&chair.to_string()]).await, "ok");
        assert!(
            grid.wait_for(WAIT, |s| *s == Seen::AgentRequestSit { target: chair })
                .await
        );
        assert!(grid.wait_for(WAIT, |s| *s == Seen::AgentSit).await);
        // seated, the bot is where its seat is, a metre up
        let seat = ferro_testgrid::SEAT_POSITION;
        let at = |x: f32, y: f32, z: f32| serde_json::json!({"x": x.round() as i64, "y": y.round() as i64, "z": z.round() as i64});
        api.wait_position(at(seat.x, seat.y, seat.z + 1.0)).await;
        assert_eq!(api.reply("Stand", &[]).await, "ok");
        assert!(grid.wait_for(WAIT, flags(STAND_UP)).await);
        api.wait_position(at(seat.x + 1.0, seat.y, seat.z)).await;
        assert_eq!(api.reply("Sit", &["ground"]).await, "ok");
        assert!(grid.wait_for(WAIT, flags(SIT_ON_GROUND)).await);
        assert_eq!(
            api.failure("Sit", &["a chair"]).await.0,
            "Invaild object uuid and not ground"
        );

        // the bot stands next to the seat, facing east: it turns north-east, then walks back
        assert_eq!(api.reply("AutoPilot", &["128", "128", "25"]).await, "ok");
        assert!(
            grid.wait_for(WAIT, |s| *s == Seen::Facing { heading: 44 })
                .await
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::AutoPilot {
                    params: vec!["256128".into(), "256128".into(), "25".into()]
                })
            .await
        );
        // the region moves it there (a terse update)
        api.wait_position(at(128.0, 128.0, 25.0)).await;
        assert_eq!(
            api.failure("AutoPilot", &["300", "128", "25"]).await.0,
            "x value out of range 0-255"
        );
        assert_eq!(
            api.failure("AutoPilot", &["here", "128", "25"]).await.0,
            "Convert to vector has failed"
        );
        assert_eq!(api.reply("AutoPilotStop", &[]).await, "ok");
        assert!(grid.wait_for(WAIT, flags(STOP)).await);

        assert_eq!(api.reply("RotateTo", &["180"]).await, "ok");
        assert!(
            grid.wait_for(WAIT, |s| *s == Seen::Facing { heading: 180 })
                .await
        );
        assert_eq!(
            api.failure("RotateTo", &["south"]).await.0,
            "Unable to process rotation"
        );
        // a resident stands to the west, once the region has said so (every ping, 2 s)
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let (_, inner) = api.run("RotateToFace", &["Some Resident"]).await;
            if inner["status"] == true {
                assert_eq!(inner["reply"], "True");
                break;
            }
            assert_eq!(inner["reply"], "Unable to see avatar");
            assert!(tokio::time::Instant::now() < deadline, "{inner}");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        assert!(
            grid.wait_for(WAIT, |s| *s == Seen::Facing { heading: 270 })
                .await
        );
        assert_eq!(
            api.failure("RotateToFace", &[&uuid_from(0xdead).to_string()])
                .await
                .0,
            "Unable to see avatar"
        );
        assert_eq!(
            api.reply("RotateToFaceVector", &["<200, 200, 25>"]).await,
            "True"
        );
        assert!(
            grid.wait_for(WAIT, |s| *s == Seen::Facing { heading: 45 })
                .await
        );
        assert_eq!(
            api.failure("RotateToFaceVector", &["<300, 1, 1>"]).await.0,
            "Vector x value is out of range 0-255"
        );
        assert_eq!(
            api.failure("RotateToFaceVector", &["north"]).await.0,
            "Invaild vector"
        );

        assert_eq!(api.reply("HoverHeight", &["0.5"]).await, "ok");
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::HoverHeight {
                    height: "0.5".into()
                })
            .await
        );
        assert_eq!(
            api.failure("HoverHeight", &["3"]).await.0,
            "height value out of range(-2.0 to 2.0)"
        );
        assert_eq!(
            api.failure("HoverHeight", &["high"]).await.0,
            "Convert to float has failed"
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// The smaller commands: avatars near with their places, a friend on the map, regions on the
    /// map, the mute list, away, and the Async… inventory commands.
    #[tokio::test]
    async fn info_and_async_commands() {
        use ferro_testgrid::inventory as inv;
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;
        let (some, other) = (uuid_from(0x5eed_0001), uuid_from(0x5eed_0002));

        // the residents stand 30 and 40 m to the west
        let deadline = tokio::time::Instant::now() + WAIT;
        let near = loop {
            let r = api.reply("NearmeWithDetails", &[]).await;
            if r.matches("\"id\"").count() == 2 {
                break r;
            }
            assert!(tokio::time::Instant::now() < deadline, "{r}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!(
            near,
            format!(
                r#"[{{"id":"{some}","name":"Some Resident","x":98,"y":128,"z":25,"range":30}},{{"id":"{other}","name":"Other Avatar","x":88,"y":128,"z":25,"range":40}}]"#
            )
        );

        // a friend found on the map; someone else is no friend
        let loc: Value =
            serde_json::from_str(&api.reply("FriendsGetLocation", &["Some Resident"]).await)
                .expect("json");
        assert_eq!(loc["Name"], "Some Resident");
        assert_eq!(
            loc["Pos"],
            serde_json::json!({"X": 100.5, "Y": 60.25, "Z": 0})
        );
        assert_eq!(
            loc["RegionPos"],
            serde_json::json!({"X": 1000, "Y": 1000, "Z": 10})
        );
        assert!((loc["time"].as_i64().expect("a time") - now()).abs() < 5);
        assert_eq!(
            api.failure("FriendsGetLocation", &[OTHER]).await.0,
            "not in friends list (updating)"
        );

        // regions on the map
        assert_eq!(
            api.reply("GetSimGlobalPos", &["Test Region"]).await,
            r#"{"region":"Test Region","X":"1000","Y":"1000"}"#
        );
        assert_eq!(
            api.reply("GetSimTexture", &["Test Region"]).await,
            ferro_testgrid::map_tile(1000, 1000).to_string()
        );
        assert_eq!(
            api.failure("GetSimTexture", &["Nowhere"]).await.0,
            "Unable to find region"
        );

        // the mute list, downloaded after the login
        let deadline = tokio::time::Instant::now() + WAIT;
        let mutes = loop {
            let r = api.reply("GetMuteList", &["false"]).await;
            if r != "[]" {
                break r;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!(
            mutes,
            format!(
                r#"[{{"type":"Resident","id":"{some}","name":"Some Resident","flags":"Default"}},{{"type":"Object","id":"{}","name":"Noisy Box","flags":"TextChat, VoiceChat"}},{{"type":"ByName","id":"{}","name":"Spammer","flags":"Default"}}]"#,
                uuid_from(0xb0c5),
                uuid::Uuid::nil()
            )
        );
        assert!(
            grid.wait_for(WAIT, |s| *s == Seen::XferConfirmed { packet: 1 })
                .await
        );
        assert_eq!(
            api.reply("GetMuteList", &["True"]).await,
            "Requesting mute list [Retry later]"
        );
        assert!(
            grid.wait_until(WAIT, |seen| seen
                .iter()
                .filter(|s| **s == Seen::MuteListRequest)
                .count()
                == 2)
                .await
        );

        // away: the animation and the flag
        assert_eq!(api.reply("SetAway", &["true"]).await, "ok");
        let away = uuid_from(0xfd037134_85d4_f241_72c6_4f42164fedee);
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::Animation {
                    id: away,
                    start: true
                })
            .await
        );
        assert!(
            grid.wait_for(
                WAIT,
                |s| matches!(s, Seen::Controls { flags } if flags & 0x0200_0000 != 0)
            )
            .await
        );
        assert_eq!(api.failure("SetAway", &["soon"]).await.0, "Invaild mode");

        // the Async… commands: a request id now, the outcome to the reply target
        let said = |request: String, data: String| {
            move |s: &Seen| {
                matches!(s, Seen::Chat { message, channel: 7, .. }
                if message.contains(&format!(r#""requestUUID":"{request}","status":true"#))
                    && message.contains(&data))
            }
        };
        let r = api
            .reply("AsyncSendItem", &[&inv::BOX.to_string(), OTHER, "7"])
            .await;
        assert!(
            grid.wait_for(WAIT, said(r, format!(r#""data":"{}""#, inv::BOX)))
                .await
        );
        let r = api
            .reply("AsyncSendItemByPath", &["Objects/Box", OTHER, "7"])
            .await;
        assert!(
            grid.wait_for(WAIT, said(r, r#""message":"Objects/Box""#.into()))
                .await
        );
        let r = api
            .reply(
                "AsyncInventoryContents",
                &[&inv::NOTECARDS.to_string(), "7"],
            )
            .await;
        assert!(grid.wait_for(WAIT, said(r, "Welcome".into())).await);
        assert!(
            grid.wait_until(WAIT, |seen| seen
                .iter()
                .filter(|s| matches!(s, Seen::InventoryOffer { to, .. } if *to == other))
                .count()
                == 2)
                .await
        );
        let (_, inner) = api.run("AsyncSendItem", &["nothing", OTHER, "7"]).await;
        assert_eq!(inner["reply"], "Invaild item uuid");
        assert_eq!(
            inner["args"],
            serde_json::json!(["nothing", OTHER]),
            "as SecondBot, without the target"
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// The parcel the bot stands on, as SecondBot's parcel getters tell it.
    #[tokio::test]
    async fn parcel_getters() {
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;

        assert_eq!(api.reply("GetCurrentParcelId", &[]).await, "7");
        assert_eq!(
            api.reply("GetCurrentParcelUUID", &[]).await,
            ferro_testgrid::parcels::PARCEL_ID.to_string()
        );
        assert_eq!(api.reply("GetCurrentParcelSize", &[]).await, "128,128");
        assert_eq!(api.reply("GetParcelDesc", &[]).await, "The test parcel");
        let flags: Value =
            serde_json::from_str(&api.reply("GetParcelFlags", &[]).await).expect("json");
        assert_eq!(flags["AllowFly"], "True");
        assert_eq!(flags["AllowLandmark"], "True");
        assert_eq!(flags["ForSale"], "False");
        assert_eq!(flags.as_object().expect("map").len(), 31);
        assert!(
            api.reply("GetParcelFlags", &[])
                .await
                .starts_with(r#"{"AllowFly":"True","AllowOtherScripts":"False","ForSale""#),
            "in LibreMetaverse's order"
        );
        assert_eq!(
            api.reply("GetParcelPrimCounts", &[]).await,
            r#"{"TotalPrims":"12","GroupPrims":"0","OwnerPrims":"10","OtherPrims":"2","MaxPrims":"3750","ParcelPrimBonus":"1.5"}"#
        );
        assert_eq!(
            api.reply("GetParcelSaleDetails", &[]).await,
            format!(
                r#"{{"ForSale":"False","Cost":"0","SaleTarget":"{}"}}"#,
                uuid::Uuid::nil()
            )
        );
        assert_eq!(api.reply("GetParcelTraffic", &[]).await, "42.5");
        assert_eq!(
            api.failure("ParcelBuy", &["0"]).await.0,
            "Funds commands are disabled",
            "it spends money"
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// SecondBot's parcel setters: each sends the parcel back changed, the rest kept.
    #[tokio::test]
    async fn parcel_setters() {
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;
        let snapshot = uuid_from(0x5a95);

        assert_eq!(api.reply("SetParcelName", &["New Plaza"]).await, "ok");
        assert_eq!(api.reply("SetParcelDesc", &["All are welcome"]).await, "ok");
        assert_eq!(
            api.reply("SetParcelLandingZone", &["100", "110", "25"])
                .await,
            "ok"
        );
        assert_eq!(
            api.reply("SetParcelMusic", &["http://music.example/stream"])
                .await,
            "True"
        );
        assert_eq!(api.reply("SetParcelSale", &["500", "none"]).await, "ok");
        assert_eq!(
            api.reply("SetParcelSnapshot", &[&snapshot.to_string()])
                .await,
            "ok"
        );
        assert_eq!(
            api.reply("SetParcelAudioControls", &["true", "true"]).await,
            "ok"
        );
        assert_eq!(
            api.reply(
                "SetParcelFlag",
                &["AllowFly=False:::AllowDamage=True:::Nonsense=True"]
            )
            .await,
            "true"
        );
        assert_eq!(
            api.reply(
                "ParcelSetMedia",
                &["MediaURL=http://media.example/:::MediaType=IMG-PNG:::MediaWidth=5000"]
            )
            .await,
            "ok"
        );
        let p = grid.parcel();
        assert_eq!(
            (p.name.as_str(), p.desc.as_str()),
            ("New Plaza", "All are welcome")
        );
        assert_eq!((p.landing_type, p.user_location), (1, [100.0, 110.0, 25.0]));
        assert_eq!(p.music_url, "http://music.example/stream");
        assert_eq!(p.sale_price, 500);
        assert_eq!(p.snapshot_id, snapshot);
        assert!(
            p.any_av_sounds && !p.group_av_sounds,
            "everyone overrides group only"
        );
        // ForSale and AllowDamage set, AllowFly cleared, AllowLandmark and CreateObjects kept
        assert_eq!(p.flags, 4 | 32 | 8 | 64);
        assert_eq!(p.media_url, "http://media.example/");

        assert_eq!(api.reply("CancelParcelSale", &[]).await, "ok");
        assert_eq!(grid.parcel().flags & 4, 0);
        assert_eq!(
            api.reply("GetParcelSaleDetails", &[]).await,
            format!(
                r#"{{"ForSale":"False","Cost":"500","SaleTarget":"{}"}}"#,
                uuid::Uuid::nil()
            )
        );

        // what SecondBot refuses
        assert_eq!(
            api.failure("SetParcelSale", &["0", "none"]).await.0,
            "Invaild amount"
        );
        assert_eq!(
            api.failure("SetParcelLandingZone", &["1", "here", "3"])
                .await
                .0,
            "Unable to process landing point y value"
        );
        assert_eq!(
            api.failure("SetParcelFlag", &["Fly"]).await.0,
            "No accepted flags"
        );
        assert_eq!(
            api.failure("SetParcelAudioControls", &["maybe", "true"])
                .await
                .0,
            "Unable to process everyone value"
        );
        assert_eq!(
            api.failure("SetParcelSnapshot", &["nothing"]).await.0,
            "Invaild texture uuid"
        );
        assert_eq!(
            grid.count(|s| matches!(s, Seen::ParcelUpdate { .. })),
            10,
            "one update per change"
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// SecondBot's parcel access commands: eject, freeze, bans, returns, deed, abandon, buy,
    /// reclaim.
    #[tokio::test]
    async fn parcel_access() {
        let grid = grid().await;
        let port = free_port().to_string();
        let mut bot = Bot::start(
            &grid,
            &[
                ("FERRO_HTTP_PORT", port.as_str()),
                ("commands_SharedSecret", SECRET),
                ("FERRO_ALLOW_FUNDS", "true"),
            ],
        );
        let api = Api {
            url: format!("http://127.0.0.1:{port}/api/Run"),
            http: reqwest::Client::new(),
        };
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;
        let (some, other) = (uuid_from(0x5eed_0001), uuid_from(0x5eed_0002));
        let did =
            |what: String| move |s: &Seen| matches!(s, Seen::Parcel { what: w } if *w == what);

        assert_eq!(api.reply("ParcelEject", &[OTHER]).await, "ok");
        assert!(grid.wait_for(WAIT, did(format!("eject {other} 0"))).await);
        assert_eq!(
            api.reply("ParcelFreeze", &["Other Avatar", "true"]).await,
            "ok"
        );
        assert!(grid.wait_for(WAIT, did(format!("freeze {other} 0"))).await);
        assert_eq!(api.reply("ParcelFreeze", &[OTHER, "False"]).await, "ok");
        assert!(grid.wait_for(WAIT, did(format!("freeze {other} 1"))).await);
        assert_eq!(
            api.failure("ParcelFreeze", &[OTHER, "maybe"]).await.0,
            "Invaild state"
        );

        // bans: the list read, then sent back whole
        assert_eq!(api.reply("ParcelBan", &[OTHER]).await, "ok");
        assert_eq!(api.reply("ParcelBan", &["Some Resident"]).await, "ok");
        assert_eq!(
            api.reply("ParcelBan", &[OTHER]).await,
            "Avatar is in the blacklist"
        );
        let bans: Value =
            serde_json::from_str(&api.reply("GetParcelBanlist", &[]).await).expect("json");
        assert_eq!(bans["entrys"][&other.to_string()], "Other Avatar");
        assert_eq!(bans["entrys"][&some.to_string()], "Some Resident");
        assert_eq!(
            (bans["reportedEntrys"].as_i64(), bans["parcelName"].as_str()),
            (Some(2), Some("Plaza"))
        );
        assert_eq!(bans["regionName"], "Test Region");
        assert_eq!(api.reply("ParcelUnBan", &[OTHER]).await, "ok");
        assert_eq!(
            api.reply("ParcelUnBan", &[OTHER]).await,
            "Avatar is already unbanned"
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::AccessListUpdate {
                    flags: 2,
                    ids: vec![some]
                })
            .await
        );
        assert_eq!(api.reply("ResetParcelAcccessLists", &[]).await, "ok");
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::AccessListUpdate {
                    flags: 1,
                    ids: vec![]
                })
            .await
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::AccessListUpdate {
                    flags: 2,
                    ids: vec![]
                })
            .await
        );

        // returns: everyone's, then one owner's only
        assert_eq!(api.reply("ReturnAllObjectsOnParcel", &[]).await, "ok");
        assert!(
            grid.wait_for(WAIT, did(format!("return 16 {some},{other}")))
                .await
        );
        assert_eq!(
            api.reply("ReturnAllObjectsOnParcelByOwner", &[OTHER]).await,
            "ok"
        );
        assert!(grid.wait_for(WAIT, did(format!("return 16 {other}"))).await);

        // the parcel to a group the bot is in, not to one it is not in
        let group = ferro_testgrid::TEST_GROUP;
        assert_eq!(
            api.reply("ParcelDeedToGroup", &[&group.to_string()]).await,
            "ok"
        );
        assert!(grid.wait_for(WAIT, did(format!("deed 7 {group}"))).await);
        assert_eq!(
            grid.parcel().flags & (1 << 13),
            1 << 13,
            "deeding allowed first"
        );
        assert_eq!(
            api.failure(
                "ParcelDeedToGroup",
                &[&ferro_testgrid::THIRD_GROUP.to_string()]
            )
            .await
            .0,
            "Not in group"
        );

        // buying: only for sale, at its price
        assert_eq!(
            api.failure("ParcelBuy", &["10"]).await.0,
            "Parcel not for sale"
        );
        assert_eq!(api.reply("SetParcelSale", &["300", "none"]).await, "ok");
        assert_eq!(
            api.failure("ParcelBuy", &["10"]).await.0,
            "Parcel sale price and amount do not match"
        );
        assert_eq!(api.reply("ParcelBuy", &["300"]).await, "ok");
        assert!(grid.wait_for(WAIT, did("buy 7 300 16384".into())).await);

        assert_eq!(api.reply("AbandonLand", &[]).await, "ok");
        assert!(grid.wait_for(WAIT, did("release 7".into())).await);
        assert_eq!(api.reply("EstateParcelReclaim", &[]).await, "ok");
        assert!(grid.wait_for(WAIT, did("reclaim 7".into())).await);

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// SecondBot's estate commands, for an estate manager.
    #[tokio::test]
    async fn estate_commands() {
        use ferro_testgrid::inventory as inv;
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;
        let other = uuid_from(0x5eed_0002);
        let sent = |method: &'static str, params: Vec<String>| {
            move |s: &Seen| {
                matches!(s, Seen::EstateMessage { method: m, params: p }
                if m == method && *p == params)
            }
        };
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let me = || {
            grid.seen()
                .iter()
                .find_map(|s| match s {
                    Seen::EstateMessage { method, params } if method == "teleporthomeuser" => {
                        params.first().cloned()
                    }
                    _ => None,
                })
                .expect("the bot's key")
        };

        assert_eq!(api.reply("EstateKickUser", &[OTHER]).await, "ok");
        assert!(grid.wait_for(WAIT, sent("kickestate", s(&[OTHER]))).await);
        assert_eq!(
            api.reply("EstateTeleportHomeUser", &["Other Avatar"]).await,
            "ok"
        );
        assert!(
            grid.wait_for(WAIT, |s| matches!(s, Seen::EstateMessage { method, params }
                if method == "teleporthomeuser" && params[1] == other.to_string()))
                .await
        );
        let bot_key = me();

        assert_eq!(api.reply("AddEstateManager", &[OTHER, "true"]).await, "ok");
        assert!(
            grid.wait_for(
                WAIT,
                sent("estateaccessdelta", s(&[&bot_key, "257", OTHER]))
            )
            .await
        );
        assert_eq!(
            api.reply("RemoveEstateManager", &[OTHER, "false"]).await,
            "ok"
        );
        assert!(
            grid.wait_for(
                WAIT,
                sent("estateaccessdelta", s(&[&bot_key, "512", OTHER]))
            )
            .await
        );
        assert_eq!(
            api.failure("AddEstateManager", &["Other Avatar", "true"])
                .await
                .0,
            "Unable to find avatar UUID"
        );
        assert_eq!(
            api.reply("UpdateEstateBanlist", &[OTHER, "add", "true"])
                .await,
            "Ban request accepted"
        );
        assert!(
            grid.wait_for(WAIT, sent("estateaccessdelta", s(&[&bot_key, "66", OTHER])))
                .await
        );
        assert_eq!(
            api.reply("UpdateEstateBanlist", &[OTHER, "remove", "false"])
                .await,
            "Unban request accepted"
        );
        assert!(
            grid.wait_for(
                WAIT,
                sent("estateaccessdelta", s(&[&bot_key, "128", OTHER]))
            )
            .await
        );
        assert_eq!(
            api.reply("GetEstateBanList", &[]).await,
            format!(r#"["{}"]"#, uuid_from(0xba11_0001))
        );

        // restarts: 30 to 240 s, else 240; "false" cancels
        assert_eq!(api.reply("SimRestart", &["60", "true"]).await, "restarting");
        assert!(grid.wait_for(WAIT, sent("restart", s(&["60"]))).await);
        assert_eq!(api.reply("SimRestart", &["5", "true"]).await, "restarting");
        assert!(grid.wait_for(WAIT, sent("restart", s(&["240"]))).await);
        assert_eq!(api.reply("SimRestart", &["0", "false"]).await, "canceled");
        assert!(grid.wait_for(WAIT, sent("restart", s(&["-1"]))).await);
        assert_eq!(api.reply("SimMessage", &["Hello region"]).await, "ok");
        assert!(
            grid.wait_for(
                WAIT,
                sent(
                    "simulatormessage",
                    s(&["-1", "-1", &bot_key, "Test Bot", "Hello region"])
                )
            )
            .await
        );

        // the covenant: set to a notecard, read back with its text
        assert_eq!(
            api.reply("SetEstateCovenant", &[&inv::NOTECARD.to_string()])
                .await,
            "changed Covenant to notecard: Welcome"
        );
        assert!(
            grid.wait_for(
                WAIT,
                sent(
                    "estatechangecovenantid",
                    vec![inv::NOTECARD_ASSET.to_string()]
                )
            )
            .await
        );
        assert_eq!(
            api.failure("SetEstateCovenant", &[&inv::BOX.to_string()])
                .await
                .0,
            "Item is not a notecard"
        );
        let c: Value =
            serde_json::from_str(&api.reply("GetEstateCovenant", &[]).await).expect("json");
        assert_eq!(c["Fetched"], true);
        assert_eq!(c["CovenantID"], inv::NOTECARD_ASSET.to_string());
        assert_eq!(c["Timestamp"], "1700000000");
        assert_eq!(c["EstateName"], "Test Estate");
        assert_eq!(
            c["CovenantText"],
            inv::welcome_text().replace('\u{F0000}', "[ATTACHMENT]")
        );

        // the region's rules, by its capability
        let values = [
            "true", "false", "false", "true", "true", "40", "1.5", "true", "false", "false",
            "true", "false",
        ];
        assert_eq!(api.reply("SetExtendedRegionInfo", &values).await, "ok");
        let posted = |cap: &'static str, needle: &'static str| move |s: &Seen| matches!(s, Seen::EstatePost { cap: c, body } if c == cap && body.contains(needle));
        assert!(
            grid.wait_for(WAIT, posted("regioninfo", "<integer>21</integer>"))
                .await
        );
        let mut bad = values;
        bad[5] = "lots";
        let (why, args) = api.failure("SetExtendedRegionInfo", &bad).await;
        assert_eq!(
            why,
            "Unable to process agent limit value please use a number"
        );
        assert_eq!(
            args.as_array().map(Vec::len),
            Some(6),
            "the values up to it"
        );
        // SetRegionFlags answers as SetExtendedRegionInfo, as SecondBot's calls it
        let (key, inner) = api
            .run(
                "SetRegionFlags",
                &[
                    "false", "true", "false", "true", "false", "true", "20", "1", "false", "true",
                ],
            )
            .await;
        assert!(key && inner["status"] == true, "{inner}");
        assert_eq!(inner["command"], "SetExtendedRegionInfo");
        assert!(
            grid.wait_for(WAIT, posted("regioninfo", "<integer>42</integer>"))
                .await
        );

        // the estate's settings: its name from getinfo, its other flags kept
        let config = [
            "12", "true", "true", "true", "false", "false", "false", "true", "false", "false",
        ];
        assert_eq!(api.reply("SetEstateConfig", &config).await, "ok");
        assert!(
            grid.wait_for(WAIT, posted("estatechangeinfo", "Test Estate"))
                .await
        );
        assert!(
            grid.wait_for(WAIT, posted("estatechangeinfo", "<real>12"))
                .await
        );
        let mut late = config;
        late[0] = "30";
        assert_eq!(
            api.failure("SetEstateConfig", &late).await.0,
            "Sun hour value must be between 0 and 23"
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// Gestures, animations, pointing, and which scripts may animate the bot.
    #[tokio::test]
    async fn animation_commands() {
        use ferro_testgrid::inventory as inv;
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;
        let some = uuid_from(0x5eed_0001);

        // the gesture: downloaded, then its animation, sound and chat
        assert_eq!(
            api.reply("PlayGesture", &[&inv::GESTURE.to_string()]).await,
            "Accepted"
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::ViewerAsset {
                    asset: inv::GESTURE_ASSET
                })
            .await
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::Animation {
                    id: inv::GESTURE_ANIMATION,
                    start: true
                })
            .await
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::SoundTrigger {
                    asset: inv::GESTURE_SOUND
                })
            .await
        );
        assert!(
            grid.wait_for(WAIT, |s| matches!(s, Seen::Chat { message, channel: 7, .. }
                if message == "hello from a gesture"))
                .await
        );
        assert_eq!(
            api.reply("PlayGestureDirect", &["nothing"]).await,
            "Error with gesture"
        );
        assert_eq!(
            api.failure("PlayGesture", &[&inv::BOX.to_string()]).await.0,
            "ViewerAsset: status 404"
        );

        // what the region says the bot plays, stopped
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let r = api.reply("ResetAnimations", &[]).await;
            if r == "Accepted - 1 stopped animations" {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "{r}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::Animation {
                    id: inv::GESTURE_ANIMATION,
                    start: false
                })
            .await
        );

        // pointing at a resident: the animation, the point-at effect and a beam
        assert_eq!(api.reply("PointAt", &["Some Resident"]).await, "ok");
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::Effect {
                    kind: 15,
                    target: some
                })
            .await
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::Effect {
                    kind: 7,
                    target: some
                })
            .await
        );
        assert_eq!(
            api.failure("PointAt", &[&uuid_from(0xdead).to_string()])
                .await
                .0,
            "Cant find UUID in sim"
        );

        // a script of an allowed avatar may animate the bot, and nothing more
        assert_eq!(
            api.reply("AddToAllowAnimations", &["Some Resident"]).await,
            "ok"
        );
        let (task, other_task) = (uuid_from(0x7a5c), uuid_from(0x7a5d));
        grid.script_question(other_task, "Other Avatar", 0x10);
        grid.script_question(task, "Some Resident", 0x10 | 0x4);
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::ScriptAnswer {
                    task,
                    questions: 0x10
                })
            .await
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            grid.count(|s| matches!(s, Seen::ScriptAnswer { .. })),
            1,
            "another's object is not answered"
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// Profiles: read, and the bot's own changed (the whole profile sent, as OpenSim needs).
    #[tokio::test]
    async fn profile_commands() {
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;

        assert_eq!(
            api.reply("GetProfileAbout", &["Some Resident"]).await,
            "I like tests"
        );
        assert_eq!(
            api.reply("GetProfileImage", &["Some Resident"]).await,
            uuid_from(0x1a6e_0001).to_string()
        );
        assert_eq!(
            api.failure("GetProfileAbout", &[&uuid_from(0xdead).to_string()])
                .await
                .0,
            "Requesting avatar details  [Retry later]"
        );

        assert_eq!(
            api.reply("SetProfileAbout", &["A bot that helps"]).await,
            "ok"
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::ProfileUpdate {
                    about: "A bot that helps".into(),
                    image: uuid::Uuid::nil(),
                    rest_kept: true
                })
            .await,
            "the rest of the profile sent back as it was"
        );
        let picture = uuid_from(0x91c7);
        assert_eq!(
            api.reply("SetProfileImage", &[&picture.to_string()]).await,
            "ok"
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::ProfileUpdate {
                    about: "A bot that helps".into(),
                    image: picture,
                    rest_kept: true
                })
            .await
        );
        assert_eq!(
            api.failure("SetProfileAbout", &["short"]).await.0,
            "About text to short"
        );
        assert_eq!(
            api.failure("SetProfileImage", &["picture"]).await.0,
            "Invaild texture uuid\""
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// Objects: rezzing items, clicking, taking back, listing, and putting items in them.
    #[tokio::test]
    async fn object_commands() {
        use ferro_testgrid::inventory as inv;
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;
        let did =
            |what: String| move |s: &Seen| matches!(s, Seen::Object { what: w } if *w == what);
        let (object, local) = (ferro_testgrid::OBJECT, ferro_testgrid::OBJECT_LOCAL_ID);
        let boxed = inv::BOX.to_string();

        // rezzing: where the bot stands, at a place, at the parcel's centre
        let id = api.reply("RezObject", &[&boxed]).await;
        assert!(uuid::Uuid::parse_str(&id).is_ok(), "{id}");
        assert!(
            grid.wait_for(
                WAIT,
                did(format!("rez {} at 128,128,25 bypass 1", inv::BOX))
            )
            .await
        );
        api.reply("RezObjectOnPos", &[&boxed, "10", "20", "30"])
            .await;
        assert!(
            grid.wait_for(WAIT, did(format!("rez {} at 10,20,30 bypass 1", inv::BOX)))
                .await
        );
        api.reply("RezObjectParcelCenter", &[&boxed, "1"]).await;
        assert!(
            grid.wait_for(
                WAIT,
                did(format!("rez {} at 128,128,26 bypass 1", inv::BOX))
            )
            .await
        );
        assert_eq!(
            api.failure("RezObjectOnPos", &[&boxed, "here", "20", "30"])
                .await
                .0,
            "Unable to unpack X cord"
        );
        assert_eq!(
            api.failure("RezObjectParcelCenter", &[&boxed, "5"]).await.0,
            "Z offset must be between -2 and 2"
        );
        assert_eq!(
            api.failure("RezObject", &["nothing"]).await.0,
            "Invaild item UUID: nothing"
        );
        let missing = uuid_from(0x404);
        assert_eq!(
            api.failure("RezObject", &[&missing.to_string()]).await.0,
            format!("Unable to find item: {missing}")
        );
        let request = api.reply("AsyncRezObject", &[&boxed, "7"]).await;
        assert!(
            grid.wait_for(WAIT, |s| matches!(s, Seen::Chat { message, channel: 7, .. }
                if message.contains(&format!(r#""requestUUID":"{request}","status":true"#))))
                .await
        );

        // clicking a prim of the linkset
        assert_eq!(
            api.reply("ClickObject", &[&ferro_testgrid::OBJECT_CHILD.to_string()])
                .await,
            "ok"
        );
        assert!(
            grid.wait_for(WAIT, did(format!("grab {}", local + 1)))
                .await
        );
        assert!(
            grid.wait_for(WAIT, did(format!("degrab {}", local + 1)))
                .await
        );
        assert_eq!(
            api.failure("ClickObject", &[&missing.to_string()]).await.0,
            "Unable to see object"
        );

        // the objects on the parcel: the linkset's root
        let listed: Value =
            serde_json::from_str(&api.reply("GetObjectsInParcel", &[]).await).expect("json");
        assert_eq!(
            listed,
            serde_json::json!([{
                "ID": object.to_string(),
                "LocalID": local,
                "OwnerID": uuid_from(0x5eed_0001).to_string(),
                "Position": {"X": 100, "Y": 120, "Z": 22},
            }])
        );

        // items into it: a script by RezScript, anything else into its contents
        assert_eq!(
            api.reply(
                "TransferInventoryToObject",
                &[&inv::SCRIPT.to_string(), &object.to_string(), "true"]
            )
            .await,
            "Transfering script [state: True]"
        );
        assert!(
            grid.wait_for(
                WAIT,
                did(format!("script {local} {} running true", inv::SCRIPT))
            )
            .await
        );
        assert_eq!(
            api.reply(
                "TransferInventoryToObject",
                &[&boxed, &object.to_string(), "true"]
            )
            .await,
            "Transfering inventory"
        );
        assert!(
            grid.wait_for(WAIT, did(format!("contents {local} {}", inv::BOX)))
                .await
        );
        assert_eq!(
            api.failure(
                "TransferInventoryToObject",
                &[&boxed, &object.to_string(), "yes"]
            )
            .await
            .0,
            "Invaild running"
        );
        let request = api
            .reply(
                "AsyncTransferInventoryToObject",
                &[&boxed, &missing.to_string(), "false", "7"],
            )
            .await;
        assert!(
            grid.wait_for(WAIT, |s| matches!(s, Seen::Chat { message, channel: 7, .. }
                if message.contains(&request) && message.contains("Unable to find object")))
                .await
        );

        // taken back into the Objects folder
        assert_eq!(api.reply("UnRezObject", &[&object.to_string()]).await, "ok");
        assert!(
            grid.wait_for(WAIT, did(format!("derez {local} to 4 in {}", inv::OBJECTS)))
                .await
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// Attachments and outfits, through the Current Outfit Folder.
    #[tokio::test]
    async fn outfit_commands() {
        use ferro_testgrid::inventory as inv;
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;
        let did =
            |what: String| move |s: &Seen| matches!(s, Seen::Object { what: w } if *w == what);

        // the hat worn at login, taken off by its point's name
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let (_, inner) = api.run("Detach", &["Skull"]).await;
            if inner["status"] == true {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "{inner}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            grid.wait_for(WAIT, did(format!("detach {}", inv::HAT)))
                .await
        );
        assert!(
            grid.wait_for(WAIT, did(format!("unlink {}", inv::HAT)))
                .await
        );

        // an object worn and kept worn, then taken off by its key
        assert_eq!(
            api.reply("Attach", &[&inv::NECKLACE.to_string()]).await,
            "Ok"
        );
        assert!(
            grid.wait_for(WAIT, did(format!("link {} in {}", inv::NECKLACE, inv::COF)))
                .await
        );
        assert!(
            grid.wait_for(WAIT, did(format!("attach {} pt 128", inv::NECKLACE)))
                .await
        );
        assert_eq!(
            api.reply("Detach", &[&inv::NECKLACE.to_string()]).await,
            "Ok"
        );
        assert!(
            grid.wait_for(WAIT, did(format!("unlink {}", inv::NECKLACE)))
                .await
        );
        assert_eq!(
            api.failure("Attach", &[&inv::SHIRT.to_string()]).await.0,
            "not an object"
        );
        assert_eq!(
            api.failure("Detach", &["Chest"]).await.0,
            "invaild item uuid pr invaild attach point"
        );

        // an outfit: its shirt and hair replace those worn, the other body parts stay
        let bakes = grid.count(|s| matches!(s, Seen::AppearanceRequest { .. }));
        let (key, inner) = api.run("Outfit", &["Party"]).await;
        assert!(key && inner["status"] == true, "{inner}");
        assert_eq!(inner["command"], "SwapOutfit", "as SecondBot's answers");
        for gone in [inv::SHIRT, inv::HAIR] {
            assert!(grid.wait_for(WAIT, did(format!("unlink {gone}"))).await);
        }
        for new in [inv::PARTY_SHIRT, inv::PARTY_HAIR, inv::NECKLACE] {
            assert!(
                grid.wait_for(WAIT, did(format!("link {new} in {}", inv::COF)))
                    .await
            );
        }
        assert!(
            grid.wait_for(WAIT, did(format!("attach {} pt 128", inv::NECKLACE)))
                .await
        );
        for kept in [inv::SHAPE, inv::SKIN, inv::EYES] {
            assert_eq!(
                grid.count(did(format!("unlink {kept}"))),
                0,
                "a body part stays"
            );
        }
        assert!(
            grid.wait_until(WAIT, |seen| seen
                .iter()
                .filter(|s| matches!(s, Seen::AppearanceRequest { .. }))
                .count()
                > bakes)
                .await,
            "baked again"
        );
        let (_, inner) = api.run("Outfit", &["Nowhere"]).await;
        assert_eq!(inner["command"], "SwapOutfitByPath");
        assert_eq!(inner["reply"], "Named folder value is empty");

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// SecondBot's Discord commands, on the stand-in Discord's server.
    #[tokio::test]
    async fn discord_commands() {
        use ferro_testgrid::discord::{MASTER, MODERATOR, StandIn, VISITOR};
        let grid = grid().await;
        let discord = StandIn::start().await.expect("the stand-in Discord");
        let host = discord.host();
        let port = free_port().to_string();
        let mut bot = Bot::start(
            &grid,
            &[
                ("FERRO_HTTP_PORT", port.as_str()),
                ("commands_SharedSecret", SECRET),
                ("discord_Enabled", "true"),
                ("discord_ServerID", "800"),
                ("discord_ClientToken", "a-test-token"),
                ("FERRO_DISCORD_STAND_IN", host.as_str()),
            ],
        );
        let api = Api {
            url: format!("http://127.0.0.1:{port}/api/Run"),
            http: reqwest::Client::new(),
        };
        arrived(&grid, &bot, 1).await;
        bot.wait_log("discord: channels ready").await;
        let (m, v) = (MASTER.to_string(), VISITOR.to_string());
        let moderator = MODERATOR.to_string();

        // roles
        let roles: Value =
            serde_json::from_str(&api.reply("Discord_RoleList", &["800"]).await).expect("json");
        assert_eq!(roles[&moderator], "Moderator");
        assert_eq!(
            api.reply("Discord_GetMemberRoles", &["800", &m]).await,
            format!(r#"["{moderator}"]"#)
        );
        assert_eq!(
            api.reply("Discord_AddRole", &["800", &moderator, &v]).await,
            "True"
        );
        assert!(
            discord
                .state()
                .members
                .iter()
                .any(|x| x.id == VISITOR && x.roles == vec![MODERATOR])
        );
        let made = api.reply("Discord_RoleCreate", &["800", "Buyers"]).await;
        let new_role = made
            .strip_prefix("[ok, ")
            .and_then(|r| r.strip_suffix(']'))
            .expect("[ok, id]")
            .to_owned();
        assert!(
            discord
                .state()
                .roles
                .iter()
                .any(|r| r.name == "Buyers" && r.color == 0x99_2D_22)
        );
        assert_eq!(
            api.reply(
                "Discord_Role_UpdatePerms",
                &["800", &new_role, "Speak=True,SendMessages=False"]
            )
            .await,
            "True"
        );
        assert!(
            discord
                .state()
                .roles
                .iter()
                .any(|r| r.name == "Buyers" && r.permissions == 1 << 21)
        );
        let settings = api
            .reply("Discord_Role_GetSettings", &["800", &moderator])
            .await;
        assert!(
            settings.starts_with(
                r#"{"Config|Color":"0,255,0","Config|Name":"Moderator","Info|AssignedCount":"2","Perm|AttachFiles":"False""#
            ),
            "{settings}"
        );
        assert!(settings.contains(r#""Perm|ManageMessages":"True""#));
        assert_eq!(
            api.reply("Discord_RoleRemove", &["800", &new_role]).await,
            "True"
        );
        assert_eq!(
            api.reply("Discord_RoleRemove", &["800", &new_role]).await,
            "False"
        );

        // channels, messages, members
        let channels: Value =
            serde_json::from_str(&api.reply("Discord_TextChannels_List", &["800"]).await)
                .expect("json");
        let general = discord
            .channel("general", None)
            .expect("general")
            .id
            .to_string();
        assert_eq!(channels[&general], "general");
        assert!(
            channels
                .as_object()
                .expect("map")
                .values()
                .any(|n| n == "localchat")
        );
        assert_eq!(
            api.reply(
                "Discord_MessageChannel",
                &["800", &general, "true", "hello everyone"]
            )
            .await,
            "True"
        );
        assert!(
            discord
                .state()
                .messages
                .iter()
                .any(|x| x.content == "hello everyone" && x.tts)
        );
        assert_eq!(
            api.reply("Discord_MessageChannel", &["800", "12345", "false", "lost"])
                .await,
            "False"
        );
        assert_eq!(
            api.reply("Discord_MembersList", &["800"]).await,
            format!(r#"{{"{m}":"The Boss|boss","{v}":"visitor"}}"#)
        );
        assert_eq!(
            api.reply("Discord_Dm_Member", &["800", &v, "a word"]).await,
            "True"
        );
        assert!(
            discord
                .state()
                .messages
                .iter()
                .any(|x| x.content == "a word")
        );
        assert_eq!(
            api.reply("Discord_MuteMember", &["800", &v, "true"]).await,
            "True"
        );
        assert!(
            discord
                .state()
                .members
                .iter()
                .any(|x| x.id == VISITOR && x.mute)
        );
        discord.say(general.parse().expect("id"), VISITOR, "spam");
        discord.say(general.parse().expect("id"), VISITOR, "more spam");
        assert_eq!(
            api.reply("Discord_BulkClear_Messages", &["800", &v]).await,
            "[True, 2]"
        );
        assert_eq!(
            api.reply("Discord_BanMember", &["800", &v, "spam"]).await,
            "True"
        );
        assert!(discord.state().bans.iter().any(|(u, _)| *u == VISITOR));

        // no voice in the container
        assert_eq!(
            api.failure("Discord_VoiceJoin", &["General"]).await.0,
            "Voice service is not enabled"
        );
        assert_eq!(
            api.failure("Discord_VoiceLeave", &[]).await.0,
            "Not in a Discord voice channel"
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// The region's parcels, its type, and the land sale search.
    #[tokio::test]
    async fn region_parcel_commands() {
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;

        assert_eq!(api.reply("ParcelName", &[]).await, "Plaza");
        assert_eq!(
            api.reply("GetCurrentRegionType", &[]).await,
            "Estate / Full Region"
        );
        assert_eq!(
            api.reply("GetListOfParcels", &[]).await,
            r#"{"groupuuids":[],"parcels":[]}"#,
            "nothing before UpdateListOfParcels"
        );

        // two parcels: one request each, the bitmaps cover the rest
        assert_eq!(api.reply("UpdateListOfParcels", &[]).await, "ok");
        assert_eq!(
            grid.count(
                |s| matches!(s, Seen::ParcelPropertiesRequest { sequence } if *sequence > 0)
            ),
            2
        );
        let list: Value =
            serde_json::from_str(&api.reply("GetListOfParcels", &[]).await).expect("json");
        assert_eq!(
            list,
            serde_json::json!({
                "groupuuids": [ferro_testgrid::TEST_GROUP.to_string(), uuid::Uuid::nil().to_string()],
                "parcels": [
                    {"area": 49152, "groupindex": 0, "name": "Outskirts", "maxprims": 11250,
                     "currentprims": 40, "startbox": "<0, 0, 0>", "endbox": "<256, 256, 0>"},
                    {"area": 16384, "groupindex": 1, "name": "Plaza", "maxprims": 3750,
                     "currentprims": 12, "startbox": "<64, 64, 0>", "endbox": "<192, 192, 0>"},
                ]
            })
        );
        assert_eq!(
            api.reply("GetParcelListSnapshots", &[]).await,
            format!(
                r#"{{"Outskirts":"{}","Plaza":"{}"}}"#,
                uuid_from(0x5a95_0008),
                uuid::Uuid::nil()
            )
        );
        let svg = api.reply("GetSimParcelLayers", &[]).await;
        assert!(
            svg.starts_with(r#"<svg width="256" height="256" xmlns="http://www.w3.org/2000/svg">"#)
        );
        assert!(svg.ends_with("</svg>"));
        // a column per 4 m: one run outside the Plaza, three across it
        assert_eq!(svg.matches("<rect ").count(), 16 + 32 * 3 + 16);
        assert!(svg.contains(r#"data-parcel="Plaza" width="4" height="128" x="64" y="64""#));
        assert!(svg.contains(r#"data-parcel="Outskirts" width="4" height="256" x="0" y="0""#));

        // land for sale: each parcel as its own JSON text
        let found: Vec<String> = serde_json::from_str(
            &api.reply(
                "QueryLandSaleData",
                &["Mainland", "5000", "100", "0", "3000"],
            )
            .await,
        )
        .expect("a list");
        let first: Value = serde_json::from_str(&found[0]).expect("json");
        assert_eq!(
            first,
            serde_json::json!({"ID": uuid_from(0x1a4d_0001).to_string(), "Name": "Beach Lot",
                "ActualArea": 512, "SalePrice": 1200, "Auction": false, "ForSale": true, "Dwell": 0})
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::LandQuery {
                    flags: (1 << 15) | (1 << 17) | (1 << 20) | (1 << 21),
                    kind: 1 << 3,
                    price: 5000,
                    area: 100,
                    start: 0
                })
            .await
        );
        assert_eq!(
            api.failure(
                "QueryLandSaleData",
                &["Nowhere", "5000", "100", "0", "3000"]
            )
            .await
            .0,
            "Invaild filterRegion"
        );
        assert_eq!(
            api.failure("QueryLandSaleData", &["Any", "5000", "-1", "0", "3000"])
                .await
                .0,
            "filterArea must be >= 0"
        );
        assert_eq!(
            api.failure("QueryLandSaleData", &["Any", "5000", "100", "0", "100"])
                .await
                .0,
            "timeout must be >= 2000 and <= 7000"
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// NotecardRead: the notecard's text, downloaded, goes to the reply target.
    #[tokio::test]
    async fn notecard_read() {
        use ferro_testgrid::inventory as inv;
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("capabilities").await;

        let (hook, caught) = super::catch_a_post().await;
        assert_eq!(
            api.reply("NotecardRead", &[&inv::NOTECARD.to_string(), &hook])
                .await,
            "Making request to get data from notecard"
        );
        let body = tokio::time::timeout(WAIT, caught)
            .await
            .expect("the reply in time")
            .expect("a reply");
        assert_eq!(
            super::form_field(&body, "command").as_deref(),
            Some("NotecardRead")
        );
        let reply: Value =
            serde_json::from_str(&super::form_field(&body, "reply").expect("reply")).expect("json");
        let text = inv::welcome_text().replace('\u{F0000}', "[ATTACHMENT]");
        assert_eq!(
            reply,
            serde_json::json!({
                "uuid": inv::NOTECARD.to_string(),
                "name": "Welcome",
                "content": text,
                "status": true,
            })
        );
        assert!(
            grid.count(|s| *s
                == Seen::TransferRequest {
                    item: inv::NOTECARD
                })
                == 1
        );

        // not a notecard: refused, and the reply target is told
        let (why, args) = api
            .failure("NotecardRead", &[&inv::BOX.to_string(), "7"])
            .await;
        assert_eq!(why, "Inventory item is not a notecard");
        assert_eq!(args, serde_json::json!([inv::BOX.to_string()]));
        assert!(
            grid.wait_for(WAIT, |s| matches!(s, Seen::Chat { message, channel: 7, .. }
                if message.contains(r#""status":false"#) && message.contains("not a notecard")))
                .await
        );
        assert_eq!(
            api.failure("NotecardRead", &["nothing", "7"]).await.0,
            "Not a vaild UUID"
        );

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// SecondBot's other group commands: notices, bans, titles, the active group, join and leave.
    #[tokio::test]
    async fn more_group_commands() {
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("in 2 groups").await;
        let group = TEST_GROUP.to_string();
        let third = ferro_testgrid::THIRD_GROUP.to_string();
        let officers = ferro_testgrid::OFFICERS.to_string();

        assert_eq!(
            api.reply("Groupnotice", &[&group, "Hello", "A notice"])
                .await,
            "Sending notice"
        );
        let item = "00000000-0000-0000-0000-0000000000aa";
        assert_eq!(
            api.reply(
                "GroupnoticeWithAttachment",
                &[&group, "Hi", "With an item", item]
            )
            .await,
            "Sending notice with attachment"
        );
        assert!(
            grid.wait_until(WAIT, |seen| {
                seen.contains(&Seen::GroupNotice {
                    group: TEST_GROUP,
                    message: "Hello|A notice".into(),
                    attached: false,
                }) && seen.contains(&Seen::GroupNotice {
                    group: TEST_GROUP,
                    message: "Hi|With an item".into(),
                    attached: true,
                })
            })
            .await
        );
        assert_eq!(
            api.failure("Groupnotice", &[&third, "x", "y"]).await.0,
            "Unknown group"
        );

        // bans: an avatar banned (ejected first), listed, unbanned
        assert_eq!(api.reply("GetGroupBanList", &[&group]).await, "[]");
        assert_eq!(
            api.reply("GroupBan", &[&group, OTHER, "true"]).await,
            format!("Accepted Ban for {OTHER}")
        );
        assert!(
            !grid
                .group_members(TEST_GROUP)
                .iter()
                .any(|m| m.id == uuid_from(0x5eed_0002))
        );
        assert_eq!(
            api.reply("GetGroupBanList", &[&group]).await,
            format!(r#"["{OTHER}"]"#)
        );
        assert_eq!(
            api.reply("GroupBan", &[&group, OTHER, "false"]).await,
            format!("Accepted Unban for {OTHER}")
        );
        assert_eq!(api.reply("GetGroupBanList", &[&group]).await, "[]");

        // titles and the active group
        let titles: Value =
            serde_json::from_str(&api.reply("GetGroupTitles", &[&group]).await).expect("map");
        assert_eq!(titles[&officers]["title"], "Bot");
        assert_eq!(titles[&officers]["selected"], true);
        assert_eq!(
            api.reply("GroupActiveTitle", &[&group, &officers]).await,
            "Switching title"
        );
        let second = ferro_testgrid::SECOND_GROUP.to_string();
        assert_eq!(
            api.reply("GroupActiveGroup", &[&second]).await,
            "Switching active group"
        );
        assert!(
            grid.wait_until(WAIT, |seen| {
                seen.contains(&Seen::GroupTitleUpdate {
                    group: TEST_GROUP,
                    role: ferro_testgrid::OFFICERS,
                }) && seen.contains(&Seen::ActivateGroup {
                    group: ferro_testgrid::SECOND_GROUP,
                })
            })
            .await
        );

        // a member's roles
        let roles: Value =
            serde_json::from_str(&api.reply("GetGroupMemberRoles", &[&group, SOME]).await)
                .expect("map");
        assert_eq!(roles[ferro_testgrid::BUILDERS.to_string()], "Builders");
        assert_eq!(roles[ferro_testgrid::OWNERS.to_string()], "Owners");

        // join the open group, leave it
        assert_eq!(api.reply("GroupJoin", &[&third]).await, "Joined group");
        bot.wait_log("in 3 groups").await;
        assert_eq!(api.reply("GroupJoin", &[&third]).await, "Already in group");
        assert_eq!(api.reply("GroupLeave", &[&third]).await, "Left group");
        assert!(
            grid.wait_until(WAIT, |seen| seen.contains(&Seen::LeaveGroup {
                group: ferro_testgrid::THIRD_GROUP
            }))
            .await
        );
        assert_eq!(api.reply("ForceLoadGroups", &[]).await, "Working on it");

        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }

    /// SecondBot's Chat and Groupchat commands, and the history behind them.
    #[tokio::test]
    async fn chat_commands_and_their_history() {
        let grid = grid().await;
        let (mut bot, api) = bot_with_api(&grid, None).await;
        arrived(&grid, &bot, 1).await;
        bot.wait_log("in 2 groups").await;

        // local chat, both ways
        grid.say("Other Avatar", "hello bot");
        bot.wait_log("[chat] Other Avatar: hello bot").await;
        assert_eq!(api.reply("Say", &["0", "hello world"]).await, "ok");
        assert_eq!(api.reply("Shout", &["0", "LOUD"]).await, "ok");
        assert_eq!(api.reply("Whisper", &["7", "psst"]).await, "ok");
        assert_eq!(
            api.failure("Say", &["zero", "x"]).await.0,
            "Invaild channel"
        );
        assert!(
            grid.wait_until(WAIT, |seen| {
                [("hello world", 0, 1), ("LOUD", 0, 2), ("psst", 7, 0)]
                    .iter()
                    .all(|(m, ch, t)| {
                        seen.contains(&Seen::Chat {
                            message: (*m).into(),
                            channel: *ch,
                            chat_type: *t,
                        })
                    })
            })
            .await,
            "{:?}",
            grid.seen()
        );
        let local: Vec<String> =
            serde_json::from_str(&api.reply("LocalChatHistory", &[]).await).expect("list");
        assert_eq!(local[0], "{Av} Other Avatar: hello bot");
        assert_eq!(local[1], "{BOT} Test Bot: hello world");

        // IMs: one received opens a window, unread until read
        assert_eq!(api.reply("haveunreadims", &[]).await, "False");
        grid.im("Other Avatar", "are you there?");
        bot.wait_log("[im] Other Avatar: are you there?").await;
        assert_eq!(api.reply("haveunreadims", &[]).await, "True");
        let unread: Vec<String> =
            serde_json::from_str(&api.reply("listwithunread", &[]).await).expect("list");
        assert_eq!(unread.len(), 1);
        let windows: Value =
            serde_json::from_str(&api.reply("chatwindows", &[]).await).expect("map");
        assert_eq!(windows[&unread[0]], "Other Avatar");
        let lines: Vec<String> =
            serde_json::from_str(&api.reply("getimchat", &[&unread[0]]).await).expect("list");
        assert_eq!(lines, vec!["Other Avatar: are you there?"]);
        assert_eq!(api.reply("haveunreadims", &[]).await, "False");
        assert_eq!(
            api.failure("getimchat", &["nope"]).await.0,
            "avatar UUID invaild"
        );

        // an IM sent by name
        assert_eq!(api.reply("IM", &["Other Avatar", "yes, here"]).await, "ok");
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::Im {
                    to: uuid_from(0x5eed_0002),
                    message: "yes, here".into()
                })
            .await
        );
        assert_eq!(
            api.failure("IM", &["Nobody Here", "x"]).await.0,
            "avatar lookup"
        );

        // group chat, both ways
        let group = TEST_GROUP.to_string();
        assert_eq!(
            api.reply("Groupchat", &[&group, "hello group"]).await,
            "Sending"
        );
        assert!(
            grid.wait_for(WAIT, |s| *s
                == Seen::GroupChatSend {
                    group: TEST_GROUP,
                    message: "hello group".into()
                })
            .await
        );
        assert_eq!(api.reply("GroupchatAnyUnread", &[]).await, "False");
        let some = ferro_testgrid::Resident {
            id: uuid_from(0x5eed_0001),
            first: "Some".into(),
            last: "Resident".into(),
        };
        grid.group_chat(&some, TEST_GROUP, "welcome");
        bot.wait_log("[group Test Group] Some Resident: welcome")
            .await;
        assert_eq!(api.reply("GroupchatAnyUnread", &[]).await, "True");
        assert_eq!(
            api.reply("GroupchatGroupHasUnread", &[&group]).await,
            "True"
        );
        let groups: Vec<String> =
            serde_json::from_str(&api.reply("GroupchatListAllUnreadGroups", &[]).await)
                .expect("list");
        assert_eq!(groups, vec![group.clone()]);
        let lines: Vec<String> =
            serde_json::from_str(&api.reply("GroupchatHistory", &[&group]).await).expect("list");
        assert_eq!(lines, vec!["Bot: hello group", "Some Resident: welcome"]);
        assert_eq!(
            api.reply("GroupchatGroupHasUnread", &[&group]).await,
            "False"
        );
        assert_eq!(api.reply("GroupchatClearAll", &[]).await, "ok");
        assert_eq!(api.reply("GroupchatHistory", &[&group]).await, "[]");
        let third = ferro_testgrid::THIRD_GROUP.to_string();
        assert_eq!(
            api.failure("Groupchat", &[&third, "x"]).await.0,
            "Unknown group"
        );

        bot.terminate();
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
    }
}

/// Commands by IM, as SecondBot takes them: a master's run, the answer goes to the reply target;
/// a stranger's are ignored, even under a master's name; an object's must be signed.
#[tokio::test]
async fn commands_by_im() {
    use sha1::Digest;
    let grid = grid().await;
    let mut bot = Bot::start(
        &grid,
        &[
            ("FERRO_MASTERS", "Some Resident"),
            ("FERRO_SHARED_SECRET", "a shared secret"),
            ("FERRO_HTTP", "false"),
        ],
    );
    arrived(&grid, &bot, 1).await;
    let some = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0001),
        first: "Some".into(),
        last: "Resident".into(),
    };
    let other = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0002),
        first: "Other".into(),
        last: "Avatar".into(),
    };
    let said = |text: &'static str| move |s: &Seen| matches!(s, Seen::Chat { message, .. } if message == text);

    // not masters: a stranger, and a stranger's IM under a master's name
    grid.im_from(&other, "Say|||0~#~from a stranger");
    grid.im("Some Resident", "Say|||0~#~from an impostor");
    // a master: runs, and the answer of Name goes to channel 5
    grid.im_from(&some, "Say|||0~#~from a master");
    wait_for(&grid, &bot, "the master's command", said("from a master")).await;
    grid.im_from(&some, "Name#|#5");
    wait_for(&grid, &bot, "the answer on channel 5", |s| {
        matches!(s, Seen::Chat { message, channel: 5, .. } if message.contains(r#""reply":"Test Bot""#))
    })
    .await;
    assert_eq!(grid.count(said("from a stranger")), 0);
    assert_eq!(grid.count(said("from an impostor")), 0);

    // an object: signed with the shared secret (no time window by default)
    let sig: String = sha1::Sha1::digest("Say0~#~from an objecta shared secret")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    grid.object_im(&other, "Vendor", "Say|||0~#~unsigned object");
    grid.object_im(
        &other,
        "Vendor",
        &format!("Say|||0~#~from an object@@@{sig}"),
    );
    wait_for(&grid, &bot, "the object's command", said("from an object")).await;
    bot.wait_log("command by IM from Vendor refused: not signed")
        .await;
    assert_eq!(grid.count(said("unsigned object")), 0);

    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// SecondBot's interaction rules: friendships, inventory, teleport requests, each by its level.
#[tokio::test]
async fn offers_follow_the_interaction_rules() {
    let grid = grid().await;
    let mut bot = Bot::start(
        &grid,
        &[
            ("FERRO_MASTERS", "Some Resident"),
            ("interaction_InventoryTransferLevel", "Anyone"),
        ],
    );
    arrived(&grid, &bot, 1).await;
    let some = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0001),
        first: "Some".into(),
        last: "Resident".into(),
    };
    let other = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0002),
        first: "Other".into(),
        last: "Avatar".into(),
    };

    // friendship: from the master only (level Owner)
    let refused = grid.offer_friendship(&other);
    let accepted = grid.offer_friendship(&some);
    wait_for(&grid, &bot, "the friendship accepted", |s| {
        *s == Seen::FriendshipAccepted {
            transaction: accepted,
        }
    })
    .await;
    bot.wait_log("friendship offer from Other Avatar refused: not a master")
        .await;
    assert_eq!(
        grid.count(|s| *s
            == Seen::FriendshipAccepted {
                transaction: refused
            }),
        0
    );

    // inventory: from anyone (level Anyone), from an avatar and from an object
    let item = grid.offer_inventory(&other, None);
    let object_item = grid.offer_inventory(&other, Some("Vendor"));
    wait_for(&grid, &bot, "the item accepted", |s| {
        *s == Seen::ImAnswer {
            to: other.id,
            dialog: 5,
            id: item,
        }
    })
    .await;
    wait_for(&grid, &bot, "the object's item accepted", |s| {
        *s == Seen::ImAnswer {
            to: other.id,
            dialog: 10,
            id: object_item,
        }
    })
    .await;

    // teleport requests: a stranger's declined, the master's answered with a teleport offer
    let request = grid.request_teleport(&other);
    wait_for(&grid, &bot, "the request declined", |s| {
        *s == Seen::ImAnswer {
            to: other.id,
            dialog: 24,
            id: request,
        }
    })
    .await;
    grid.request_teleport(&some);
    wait_for(&grid, &bot, "a teleport offered", |s| {
        *s == Seen::StartLure { target: some.id }
    })
    .await;

    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// Inventory at the default level (Owner): a stranger's item is declined.
#[tokio::test]
async fn a_strangers_item_is_declined_by_default() {
    let grid = grid().await;
    let mut bot = Bot::start(&grid, &[("FERRO_MASTERS", "Some Resident")]);
    arrived(&grid, &bot, 1).await;
    let other = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0002),
        first: "Other".into(),
        last: "Avatar".into(),
    };
    let item = grid.offer_inventory(&other, None);
    wait_for(&grid, &bot, "the item declined", |s| {
        *s == Seen::ImAnswer {
            to: other.id,
            dialog: 6,
            id: item,
        }
    })
    .await;
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// Script dialogs: relayed on a channel as SecondBot's JSON, answered with DialogResponce.
#[tokio::test]
async fn script_dialogs_are_relayed_and_answered() {
    let grid = grid().await;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port()
        .to_string();
    let mut bot = Bot::start(
        &grid,
        &[
            ("FERRO_HTTP_PORT", port.as_str()),
            ("FERRO_SHARED_SECRET", "s"),
            ("FERRO_DIALOG_CHANNEL", "7"),
        ],
    );
    arrived(&grid, &bot, 1).await;
    bot.wait_log("HTTP API on port").await;
    let some = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0001),
        first: "Some".into(),
        last: "Resident".into(),
    };
    let object = grid.dialog("Door", &some, "Open the door?", &["Yes", "No"], -42);
    wait_for(&grid, &bot, "the dialog relayed on channel 7", |s| {
        matches!(s, Seen::Chat { message, channel: 7, .. }
            if message.starts_with(r#"{"message":"Open the door?","buttons":["Yes","No"],"objectname":"Door","dialogid":1,"#))
    })
    .await;

    // DialogResponce through the API, signed as BB signs
    use sha1::Digest;
    let http = reqwest::Client::new();
    let run = |args: &'static str| {
        let http = http.clone();
        let port = port.clone();
        async move {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_secs();
            let sig: String = sha1::Sha1::digest(format!("DialogResponce{args}{t}s"))
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let body = format!(
                "commandName=DialogResponce&args={}&unixtime={t}&signing={sig}",
                args.replace("~#~", "%7E%23%7E")
            );
            let text = http
                .post(format!("http://127.0.0.1:{port}/api/Run"))
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(body)
                .send()
                .await
                .expect("answer")
                .text()
                .await
                .expect("body");
            let outer: serde_json::Value = serde_json::from_str(&text).expect("json");
            let inner: serde_json::Value =
                serde_json::from_str(outer["Value"].as_str().expect("Value")).expect("inner");
            inner["reply"].as_str().expect("reply").to_owned()
        }
    };
    assert_eq!(run("1~#~Maybe").await, "Invaild dialog button");
    assert_eq!(run("1~#~No").await, "action");
    wait_for(&grid, &bot, "the button pressed", |s| {
        *s == Seen::DialogReply {
            object_id: object,
            channel: -42,
            index: 1,
            label: "No".into(),
        }
    })
    .await;
    assert_eq!(
        run("1~#~No").await,
        "Invaild dialog window",
        "answered once"
    );

    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// Homebound: logged in away from home, the bot goes home; a restart at home sends it to the
/// backup.
/// At home, homebound sits the bot on its seat once, and leaves it seated (AtHomeAutoSitUuid).
#[tokio::test]
async fn homebound_sits_on_its_seat_at_home() {
    let grid = grid().await;
    let seat = uuid_from(0x5ea7_0001);
    let seat_text = seat.to_string();
    let mut bot = Bot::start(
        &grid,
        &[
            ("FERRO_HOME_SIM", "Test Region/128/128/25"),
            ("FERRO_HOME_SEEK", "false"),
            ("homebound_AtHomeAutoSitUuid", seat_text.as_str()),
        ],
    );
    arrived(&grid, &bot, 1).await;
    wait_for(&grid, &bot, "the sit request", |s| {
        *s == Seen::AgentRequestSit { target: seat }
    })
    .await;
    wait_for(&grid, &bot, "the bot seated", |s| *s == Seen::AgentSit).await;
    bot.wait_log("homebound: sitting on").await;
    // seated, it does not ask again (a check every 5 s)
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(
        grid.count(|s| matches!(s, Seen::AgentRequestSit { .. })),
        1,
        "{}",
        bot.log()
    );
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0);
}

#[tokio::test]
async fn homebound_goes_home_and_leaves_before_a_restart() {
    let grid = grid().await;
    let mut bot = Bot::start(
        &grid,
        &[
            ("FERRO_HOME_SIM", "Second%20Region/100/110/22"),
            ("FERRO_BACKUP_SIM", "Test Region/128/128/25"),
            ("FERRO_HOME_AFTER", "0"),
        ],
    );
    arrived(&grid, &bot, 1).await;
    wait_for(&grid, &bot, "the arrival at home", |s| {
        *s == Seen::CompleteAgentMovement {
            session: 1,
            region: ferro_testgrid::SECOND_REGION,
        }
    })
    .await;
    bot.wait_log("homebound: away from home, to Second Region")
        .await;
    assert!(
        grid.count(|s| matches!(s, Seen::MapNameRequest { name } if name == "Second Region")) >= 1
    );

    grid.announce_restart(120);
    bot.wait_log("homebound: the region restarts, to Test Region")
        .await;
    wait_for(&grid, &bot, "the arrival at the backup", |s| {
        // the login's arrival in the first region, then this one
        matches!(
            s,
            Seen::CompleteAgentMovement {
                session: 1,
                region: 0
            }
        )
    })
    .await;
    assert!(
        grid.wait_until(WAIT, |seen| seen
            .iter()
            .filter(|s| matches!(
                s,
                Seen::CompleteAgentMovement {
                    session: 1,
                    region: 0
                }
            ))
            .count()
            == 2)
            .await,
        "{}",
        bot.log()
    );
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// A one-call HTTP endpoint: its address, and the body of the first POST it gets.
async fn catch_a_post() -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let hook = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let hook_url = format!("http://{}/hook", hook.local_addr().expect("address"));
    let caught = tokio::spawn(async move {
        let (mut s, _) = hook.accept().await.expect("the webhook's call");
        let mut buf = vec![0u8; 8192];
        let mut got = Vec::new();
        // the head, then the body by its length
        loop {
            let n = s.read(&mut buf).await.expect("read");
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).to_string();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let len: usize = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if body.len() >= len {
                    s.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                        .await
                        .expect("answer");
                    return body.to_owned();
                }
            }
            if n == 0 {
                return String::new();
            }
        }
    });
    (hook_url, caught)
}

/// A one-call HTTP server that answers with this status and JSON body: its base address
/// (ending in "/"), and the path and body of the POST it got.
async fn serve_a_post(
    status: &str,
    body: String,
) -> (String, tokio::task::JoinHandle<(String, String)>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}/", listener.local_addr().expect("address"));
    let answer = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let served = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.expect("the call");
        let mut buf = vec![0u8; 8192];
        let mut got = Vec::new();
        loop {
            let n = s.read(&mut buf).await.expect("read");
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).to_string();
            if let Some((head, sent)) = text.split_once("\r\n\r\n") {
                let len: usize = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if sent.len() >= len || n == 0 {
                    s.write_all(answer.as_bytes()).await.expect("answer");
                    let path = head
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_owned();
                    return (path, sent.to_owned());
                }
            }
        }
    });
    (base, served)
}

/// A form field's value (application/x-www-form-urlencoded).
fn form_field(body: &str, name: &str) -> Option<String> {
    let raw = body
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{name}=")))?;
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Relays: local chat to a channel, an avatar's IMs to a webhook (Discord's format).
#[tokio::test]
async fn relays_carry_what_the_bot_hears() {
    let (hook_url, caught) = catch_a_post().await;
    let other = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0002),
        first: "Other".into(),
        last: "Avatar".into(),
    };
    let im_relay = format!(
        "sType::avatar{{}}sFilter::{}{{}}tType::webhook{{}}tConfig::{hook_url}",
        other.id
    );
    let grid = grid().await;
    let mut bot = Bot::start(
        &grid,
        &[
            (
                "FERRO_RELAY_1",
                "sType::localchat{}sFilter::avatar{}tType::localchat{}tConfig::9",
            ),
            ("FERRO_RELAY_2", im_relay.as_str()),
        ],
    );
    arrived(&grid, &bot, 1).await;
    bot.wait_log("2 relays").await;
    grid.say("Other Avatar", "hello there");
    wait_for(&grid, &bot, "the line relayed on channel 9", |s| {
        matches!(s, Seen::Chat { message, channel: 9, .. }
            if message.contains(r#""message":"{Relay} hello there""#) && message.contains(r#""source":"localchat""#))
    })
    .await;
    grid.im_from(&other, "ping");
    let body = tokio::time::timeout(WAIT, caught)
        .await
        .expect("the webhook called")
        .expect("the hook task");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["content"], "**Other Avatar**: ping");
    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}

/// Discord both ways: the bot's channels on the server, what it hears in Second Life posted
/// there, and what a Discord master writes there sent back (a visitor is refused).
#[tokio::test]
async fn discord_both_ways() {
    use ferro_testgrid::discord::{MASTER, StandIn, VISITOR};
    let grid = grid().await;
    let discord = StandIn::start().await.expect("the stand-in Discord");
    let host = discord.host();
    let master = MASTER.to_string();
    let mut bot = Bot::start(
        &grid,
        &[
            ("FERRO_DISCORD", "true"),
            ("FERRO_DISCORD_SERVER", "800"),
            ("FERRO_DISCORD_TOKEN", "a-test-token"),
            ("FERRO_DISCORD_STAND_IN", host.as_str()),
            ("FERRO_DISCORD_MASTERS", master.as_str()),
            ("FERRO_DISCORD_COMMANDS", "true"),
        ],
    );
    arrived(&grid, &bot, 1).await;
    bot.wait_log("discord: channels ready").await;
    assert!(
        !bot.log().contains("a-test-token"),
        "the token is never logged"
    );

    // the bot's channels, its state, a channel per group, its activity
    let status = discord.channel("status", Some("bot")).expect("status");
    let localchat = discord
        .channel("localchat", Some("bot"))
        .expect("localchat");
    let commands = discord.channel("commands", Some("bot")).expect("commands");
    assert!(
        discord
            .wait(WAIT, |d| d
                .posts(status.id)
                .iter()
                .any(|p| p.content == "In world: Test Bot in Test Region"))
            .await
    );
    assert!(
        discord
            .wait(WAIT, |d| d.channel("test-group", Some("group")).is_some())
            .await
    );
    let group_channel = discord
        .channel("test-group", Some("group"))
        .expect("a group's channel");
    assert!(
        group_channel
            .topic
            .starts_with(&format!("group={}|||", ferro_testgrid::TEST_GROUP))
    );
    assert!(
        discord
            .wait(WAIT, |d| d
                .state()
                .presences
                .iter()
                .any(|p| p == "Test Region"))
            .await
    );

    // Second Life to Discord: local chat, an IM, a group's chat
    let some = ferro_testgrid::Resident {
        id: uuid_from(0x5eed_0001),
        first: "Some".into(),
        last: "Resident".into(),
    };
    grid.say("Some Resident", "hello from the region");
    assert!(
        discord
            .wait(WAIT, |d| d.posts(localchat.id).iter().any(
                |p| p.content == ":person_bald:Some Resident: hello from the region"
            ))
            .await
    );
    grid.im_from(&some, "hello bot");
    assert!(
        discord
            .wait(WAIT, |d| d.channel("someresident", Some("im")).is_some())
            .await
    );
    let im_channel = discord
        .channel("someresident", Some("im"))
        .expect("the avatar's channel");
    assert_eq!(
        im_channel.topic,
        format!("avatar={}||| !close !offertp !clear", some.id)
    );
    assert!(
        discord
            .wait(WAIT, |d| d
                .posts(im_channel.id)
                .iter()
                .any(|p| p.content == "Some Resident:hello bot"))
            .await
    );
    grid.group_chat(&some, ferro_testgrid::TEST_GROUP, "hi group");
    assert!(
        discord
            .wait(WAIT, |d| d
                .posts(group_channel.id)
                .iter()
                .any(|p| p.content == "Some Resident: hi group"))
            .await
    );

    // Discord to Second Life, from the master
    let said = discord.say(localchat.id, MASTER, "hello from Discord");
    assert!(
        grid.wait_for(WAIT, |s| matches!(s, Seen::Chat { message, channel: 0, .. }
            if message == "hello from Discord"))
            .await
    );
    assert!(
        discord
            .wait(WAIT, |d| d.state().deleted_messages.contains(&said))
            .await
    );
    let replied = discord.say(im_channel.id, MASTER, "hello back");
    assert!(
        grid.wait_for(WAIT, |s| *s
            == Seen::Im {
                to: some.id,
                message: "boss: hello back".into()
            })
        .await
    );
    assert!(
        discord
            .wait(WAIT, |d| d
                .state()
                .reactions
                .contains(&(replied, "✅".into())))
            .await
    );
    discord.say(im_channel.id, MASTER, "!offertp");
    assert!(
        grid.wait_for(WAIT, |s| *s == Seen::StartLure { target: some.id })
            .await
    );
    discord.say(group_channel.id, MASTER, "hello group");
    assert!(
        grid.wait_for(WAIT, |s| *s
            == Seen::GroupChatSend {
                group: ferro_testgrid::TEST_GROUP,
                message: "boss: hello group".into()
            })
        .await
    );
    let invite = discord.say(group_channel.id, MASTER, "!invite Other Avatar");
    assert!(
        grid.wait_for(WAIT, |s| matches!(s, Seen::GroupInvite { invitee, .. }
            if *invitee == uuid_from(0x5eed_0002)))
            .await
    );
    assert!(
        discord
            .wait(WAIT, |d| d
                .state()
                .reactions
                .contains(&(invite, "✅".into())))
            .await
    );

    // the commands channel: the answer as a reply
    let asked = discord.say(commands.id, MASTER, "Hello");
    assert!(
        discord
            .wait(WAIT, |d| d.state().messages.iter().any(|m| m.reply_to
                == Some(asked)
                && m.content.contains(r#""reply":"world""#)))
            .await
    );

    // a visitor: refused everywhere, nothing reaches Second Life
    let chats = grid.count(|s| matches!(s, Seen::Chat { channel: 0, .. }));
    let refused = discord.say(localchat.id, VISITOR, "let me in");
    let refused_command = discord.say(commands.id, VISITOR, "Logoff");
    assert!(
        discord
            .wait(WAIT, |d| {
                let st = d.state();
                st.reactions.contains(&(refused, "❌".into()))
                    && st.reactions.contains(&(refused_command, "❌".into()))
            })
            .await
    );
    assert_eq!(
        grid.count(|s| matches!(s, Seen::Chat { channel: 0, .. })),
        chats
    );

    // !close: the avatar's channel goes
    discord.say(im_channel.id, MASTER, "!close");
    assert!(
        discord
            .wait(WAIT, |d| d
                .state()
                .deleted_channels
                .contains(&im_channel.id))
            .await
    );

    bot.terminate();
    assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
}
