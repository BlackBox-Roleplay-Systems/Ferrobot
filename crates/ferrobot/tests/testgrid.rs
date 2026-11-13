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

    async fn bot_with_api(grid: &TestGrid, login: Option<&str>) -> (Bot, Api) {
        let port = free_port().to_string();
        let mut env = vec![
            ("FERRO_HTTP_PORT", port.as_str()),
            ("commands_SharedSecret", SECRET),
        ];
        if let Some(uri) = login {
            env.push(("FERRO_LOGIN_URI", uri));
        }
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

        assert_eq!(api.reply("Logoff", &[]).await, "ok");
        assert_eq!(bot.exit_code().await, 0, "{}", bot.log());
        bot.wait_log("logged out").await;
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

/// Relays: local chat to a channel, an avatar's IMs to a webhook (Discord's format).
#[tokio::test]
async fn relays_carry_what_the_bot_hears() {
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
