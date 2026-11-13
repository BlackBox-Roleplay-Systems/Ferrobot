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
