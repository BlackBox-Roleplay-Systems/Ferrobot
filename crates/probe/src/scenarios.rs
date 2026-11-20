//! The scenarios, as the Tranquillity teleport rig ran them with a LibreMetaverse client: one
//! fresh login a round, then teleports and crossings, each judged and told as it ends.

use crate::admin::Admin;
use crate::judge::{self, Sample, Trip, Verdict};
use crate::options::{Options, Scenario};
use crate::report::Record;
use ferro_client::login::{self, LoginRequest, StartLocation};
use ferro_client::{Client, End, Event, Move, WalkKey};
use glam::Vec3;
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

mod bake;
mod bom;
mod group_admin;
mod groups;
mod hypergrid;
mod land;
mod sessions;

/// How long a teleport may take to put the avatar at its target.
const ARRIVAL_WAIT: Duration = Duration::from_secs(15);
/// How long the avatar is watched after arriving, and how often.
const WATCH: Duration = Duration::from_millis(1500);
const SAMPLE_EVERY: Duration = Duration::from_millis(20);
/// How long a crossing may take, flying from near the border.
const CROSSING_WAIT: Duration = Duration::from_secs(45);
/// How long a login may take to arrive.
const LOGIN_WAIT: Duration = Duration::from_secs(60);
/// The restart scenario: the notice the regions get, and how long a region may take to go down
/// and to come back.
const RESTART_NOTICE: u32 = 10;
const RESTART_DOWN_WAIT: Duration = Duration::from_secs(60);
const RESTART_BACK_WAIT: Duration = Duration::from_secs(180);
/// A logout this long had no LogoutReply: ferro-client waits 10 s for it, then ends the session.
const NO_LOGOUT_REPLY: Duration = Duration::from_millis(9500);

/// One bot's probe.
pub struct Probe {
    opts: Arc<Options>,
    /// The bot's account.
    user: String,
    password: String,
    http: reqwest::Client,
    admin: Option<Arc<Admin>>,
    /// Region handles by name, looked up once: a map lookup is not part of a teleport's time.
    handles: HashMap<String, u64>,
    pub records: Vec<Record>,
    json: Option<Arc<Mutex<std::fs::File>>>,
    round: u32,
    /// For the spots of the bounce scenario.
    seed: u64,
    /// Steps are not told (logins tried while a region is down).
    quiet: bool,
}

/// A bot logged in: its client and its session.
struct Online {
    client: Client,
    stop: watch::Sender<bool>,
    session: JoinHandle<End>,
}

impl Probe {
    pub fn new(
        opts: Arc<Options>,
        user: String,
        password: String,
        http: reqwest::Client,
        admin: Option<Arc<Admin>>,
        json: Option<Arc<Mutex<std::fs::File>>>,
    ) -> Probe {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64 | 1)
            ^ user.len() as u64;
        Probe {
            opts,
            user,
            password,
            http,
            admin,
            handles: HashMap::new(),
            records: Vec::new(),
            json,
            round: 0,
            seed,
            quiet: false,
        }
    }

    /// One round: a fresh login, the scenarios, the logout.
    pub async fn round(&mut self, round: u32) {
        self.round = round;
        let Some(mut on) = self.login("login").await else {
            return;
        };
        // the regions settle the arrival (child agents, appearance) before the first teleport
        tokio::time::sleep(Duration::from_secs(3)).await;
        for scenario in self.opts.scenarios.clone() {
            if !on.client.state().online {
                break;
            }
            // these end the bot's session and give back the next one
            let again = match scenario {
                Scenario::Restart => self.restart(on).await,
                Scenario::AbandonedLogin => self.abandoned_login(on).await,
                Scenario::Vanish => self.vanish(on).await,
                Scenario::Kick => self.kick(on).await,
                Scenario::Bake => self.bake(on).await,
                _ => {
                    self.scenario(&on.client, scenario).await;
                    continue;
                }
            };
            match again {
                Some(again) => on = again,
                None => return,
            }
        }
        self.logout(on).await;
    }

    /// Log in at the home region's middle and wait for the arrival; the step is told as `what`.
    async fn login(&mut self, what: &str) -> Option<Online> {
        let user = self.user.clone();
        self.login_as(what, &user).await
    }

    /// As [`Probe::login`], for another account with the bot's password (a scenario's second avatar).
    async fn login_as(&mut self, what: &str, user: &str) -> Option<Online> {
        let (client, mut network) = Client::new(self.http.clone());
        network.set_neighbours(self.opts.neighbours);
        let mut events = client.subscribe();
        let req = self.login_request_for(user);
        let t = Instant::now();
        let resp = match login::login(&self.http, &req).await {
            Ok(resp) => resp,
            Err(e) => {
                self.emit(&client, what, false, t, format!("refused: {e}"));
                return None;
            }
        };
        let (stop, stop_rx) = watch::channel(false);
        let session = tokio::spawn(async move { network.session(&resp, stop_rx).await });
        let arrived = tokio::time::timeout(LOGIN_WAIT, async {
            loop {
                match events.recv().await {
                    Ok(Event::Arrived { .. }) => return true,
                    Ok(Event::Disconnected { .. }) => return false,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                    _ => {}
                }
            }
        })
        .await
        .unwrap_or(false);
        let note = match region_name(&client.state()) {
            r if arrived && r != self.opts.home => {
                format!("arrived in {r}, not {}", self.opts.home)
            }
            _ => String::new(),
        };
        self.emit(&client, what, arrived, t, note);
        let on = Online {
            client,
            stop,
            session,
        };
        if arrived {
            Some(on)
        } else {
            self.logout(on).await;
            None
        }
    }

    /// The bot's login, at the home region's middle.
    fn login_request(&self) -> LoginRequest {
        self.login_request_for(&self.user)
    }

    /// An account's login with the bot's password, at the home region's middle.
    fn login_request_for(&self, user: &str) -> LoginRequest {
        LoginRequest {
            login_uri: self.opts.login_uri.clone(),
            username: user.to_owned(),
            password: self.password.clone(),
            start: StartLocation::Region {
                name: self.opts.home.clone(),
                x: 128,
                y: 128,
                z: 30,
            },
        }
    }

    async fn logout(&mut self, on: Online) {
        let Online {
            client,
            stop,
            session,
        } = on;
        let t = Instant::now();
        let _ = stop.send(true);
        let end = tokio::time::timeout(Duration::from_secs(15), session).await;
        let (ok, note) = match end {
            // the client ends the session itself when the region never answers (10 s)
            Ok(Ok(End::LoggedOut)) if t.elapsed() >= NO_LOGOUT_REPLY => (
                false,
                "no LogoutReply: the client gave up waiting for it".into(),
            ),
            Ok(Ok(End::LoggedOut)) => (true, String::new()),
            Ok(Ok(end)) => (false, end.to_string()),
            Ok(Err(e)) => (false, format!("the session task: {e}")),
            Err(_) => (false, "the session did not end".into()),
        };
        self.emit(&client, "logout", ok, t, note);
    }

    /// The neighbour region restarts: its circuit closes, and opens again once it is back. Then
    /// the home region restarts with the bot in it: the notice comes, the session ends, and the
    /// bot logs in again once the region is back, as Ferrobot does (FERRO_RELOG).
    async fn restart(&mut self, on: Online) -> Option<Online> {
        let Some(admin) = self.admin.clone() else {
            return Some(on);
        };
        let (home, near) = (self.opts.home.clone(), self.opts.near.clone());
        let middle = Vec3::new(128.0, 128.0, 30.0);
        let client = on.client.clone();
        self.teleport(&client, "restart-setup", &home, middle).await;

        if self.opts.neighbours {
            let has_near = |c: &Client| c.state().neighbours.iter().any(|(_, n)| *n == near);
            let t = Instant::now();
            while !has_near(&client) && t.elapsed() < Duration::from_secs(10) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let t = Instant::now();
            match admin.restart(&near, RESTART_NOTICE).await {
                Err(e) => {
                    self.emit(&client, &format!("restart-{near}"), false, t, e);
                }
                Ok(()) => {
                    let gone = wait_for(
                        RESTART_DOWN_WAIT + Duration::from_secs(RESTART_NOTICE.into()),
                        || !has_near(&client),
                    )
                    .await;
                    self.emit(
                        &client,
                        &format!("restart-{near}-circuit-closed"),
                        gone && client.state().online,
                        t,
                        if gone {
                            String::new()
                        } else {
                            format!("the circuit to {near} stayed open")
                        },
                    );
                    let t = Instant::now();
                    let back = wait_for(RESTART_BACK_WAIT, || has_near(&client)).await;
                    self.emit(
                        &client,
                        &format!("restart-{near}-circuit-back"),
                        back && client.state().online,
                        t,
                        if back {
                            String::new()
                        } else {
                            format!(
                                "no circuit to {near} again: no EnableSimulator once it was back"
                            )
                        },
                    );
                    self.teleport(&client, &format!("restart-tp-to-{near}"), &near, middle)
                        .await;
                    self.teleport(&client, &format!("restart-tp-to-{home}"), &home, middle)
                        .await;
                }
            }
        }

        // the home region, with the bot in it
        let mut events = client.subscribe();
        let t = Instant::now();
        if let Err(e) = admin.restart(&home, RESTART_NOTICE).await {
            self.emit(&client, &format!("restart-{home}"), false, t, e);
            return Some(on);
        }
        let notice =
            tokio::time::timeout(Duration::from_secs(u64::from(RESTART_NOTICE) + 20), async {
                loop {
                    match events.recv().await {
                        Ok(Event::RegionRestart { seconds, .. }) => return Some(seconds),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                        _ => {}
                    }
                }
            })
            .await
            .ok()
            .flatten();
        self.emit(
            &client,
            &format!("restart-{home}-notice"),
            notice.is_some(),
            t,
            match notice {
                Some(s) => format!("{s} s"),
                None => "no restart notice".into(),
            },
        );
        let Online {
            client,
            stop,
            mut session,
        } = on;
        let t = Instant::now();
        let ended = tokio::time::timeout(
            RESTART_DOWN_WAIT + Duration::from_secs(RESTART_NOTICE.into()),
            &mut session,
        )
        .await;
        let ended_at = Instant::now();
        match ended {
            Ok(end) => {
                let end = end.map_or_else(|e| format!("the session task: {e}"), |e| e.to_string());
                self.emit(
                    &client,
                    &format!("restart-{home}-session-ends"),
                    true,
                    t,
                    end,
                );
            }
            Err(_) => {
                self.emit(
                    &client,
                    &format!("restart-{home}-session-ends"),
                    false,
                    t,
                    "the session went on through the restart".into(),
                );
                let _ = stop.send(true);
                let _ = tokio::time::timeout(Duration::from_secs(15), session).await;
            }
        }
        drop(stop);
        // log in again until the region is back, as FERRO_RELOG does
        let mut tries = 0;
        loop {
            tries += 1;
            tokio::time::sleep(Duration::from_secs(5)).await;
            if let Some(again) = self.login_quietly().await {
                // the grid puts the bot elsewhere while the home region is still down
                let note = format!("{tries} logins, in {}", region_name(&again.client.state()));
                self.record(
                    &again.client,
                    &format!("restart-{home}-relog"),
                    true,
                    ended_at.elapsed(),
                    note,
                );
                tokio::time::sleep(Duration::from_secs(3)).await;
                self.after(&again.client, "restart-then").await;
                return Some(again);
            }
            if ended_at.elapsed() > RESTART_BACK_WAIT {
                self.record(
                    &client,
                    &format!("restart-{home}-relog"),
                    false,
                    ended_at.elapsed(),
                    format!(
                        "no login in {} s ({tries} tries)",
                        RESTART_BACK_WAIT.as_secs()
                    ),
                );
                return None;
            }
        }
    }

    /// A login that tells nothing when it fails: the region may be down still.
    async fn login_quietly(&mut self) -> Option<Online> {
        let records = self.records.len();
        self.quiet = true;
        let on = self.login("relog").await;
        self.quiet = false;
        self.records.truncate(records);
        on
    }

    async fn scenario(&mut self, client: &Client, scenario: Scenario) {
        let o = self.opts.clone();
        let (home, near, far) = (o.home.clone(), o.near.clone(), o.far.clone());
        let middle = Vec3::new(128.0, 128.0, 30.0);
        match scenario {
            Scenario::Local => {
                self.teleport(client, "local-tp", &home, Vec3::new(60.0, 60.0, 30.0))
                    .await;
                self.teleport(client, "local-tp-back", &home, middle).await;
            }
            Scenario::Near => {
                self.teleport(client, &format!("near-tp-to-{near}"), &near, middle)
                    .await;
                self.teleport(client, &format!("near-tp-to-{home}"), &home, middle)
                    .await;
            }
            Scenario::Far => {
                self.teleport(client, &format!("far-tp-to-{far}"), &far, middle)
                    .await;
                self.teleport(client, &format!("far-tp-to-{home}"), &home, middle)
                    .await;
            }
            Scenario::Cross => {
                self.teleport(client, "cross-setup", &home, Vec3::new(230.0, 128.0, 40.0))
                    .await;
                self.cross(client, "cross-east", &near, true).await;
                self.cross(client, "cross-west", &home, false).await;
            }
            Scenario::Bounce => self.bounce(client).await,
            Scenario::LostCrossing => self.lost_crossing(client).await,
            Scenario::SitCross => self.sit_cross(client).await,
            Scenario::Land => self.land(client).await,
            Scenario::LandGuard => self.land_guard(client).await,
            Scenario::LandFaults => self.land_faults(client).await,
            Scenario::Hypergrid => self.hypergrid(client).await,
            Scenario::Bom => self.bom(client).await,
            Scenario::Groups => self.groups(client).await,
            Scenario::GroupAdmin => self.group_admin(client).await,
            // run by the round: the session ends in them
            Scenario::Restart
            | Scenario::AbandonedLogin
            | Scenario::Vanish
            | Scenario::Kick
            | Scenario::Bake => {}
            Scenario::Unreachable => {
                let unreachable = o.unreachable.clone();
                self.refused(
                    client,
                    &format!("unreachable-tp-to-{unreachable}"),
                    &unreachable,
                    middle,
                )
                .await;
                self.after(client, "after-unreachable").await;
            }
        }
    }

    /// Local teleports from each way of moving: does the avatar snap back toward where it was?
    async fn bounce(&mut self, client: &Client) {
        let home = self.opts.home.clone();
        let _ = client.move_bot(Move::Fly(false)).await;
        let to = self.spot();
        self.teleport(client, "bounce-standing", &home, to).await;

        let _ = client
            .move_bot(Move::Walk(WalkKey::Forward, Duration::from_secs(10)))
            .await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        let to = self.spot();
        self.teleport(client, "bounce-walking", &home, to).await;

        let _ = client.move_bot(Move::Fly(true)).await;
        let _ = client
            .move_bot(Move::Walk(WalkKey::Forward, Duration::from_secs(10)))
            .await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        let to = self.spot();
        self.teleport(client, "bounce-flying", &home, to).await;
        let _ = client.move_bot(Move::Fly(false)).await;

        // falling: land high, then teleport again while still dropping
        let high = self.spot().with_z(120.0);
        if let Some(handle) = self.handle(client, &home).await {
            let _ = client.teleport_to(handle, high).await;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        let to = self.spot();
        self.teleport(client, "bounce-falling", &home, to).await;
    }

    /// The bot drops the CrossedRegion of a crossing east, as a viewer that missed it: does the
    /// crossing still complete (Tranquillity sends it again), and what works afterwards?
    async fn lost_crossing(&mut self, client: &Client) {
        let (home, near) = (self.opts.home.clone(), self.opts.near.clone());
        self.teleport(client, "lost-setup", &home, Vec3::new(230.0, 128.0, 40.0))
            .await;
        client.faults().drop_events("CrossedRegion", 1);
        self.cross(client, "lost-cross-east", &near, true).await;
        let left = client.faults().pending();
        client.faults().drop_events("CrossedRegion", 0);
        tokio::time::sleep(Duration::from_secs(5)).await;
        let st = client.state();
        let note = format!(
            "the bot is in {} at {:.0}; drops left: {left:?}",
            st.region.map(|r| r.name).unwrap_or_else(|| "(none)".into()),
            st.position
        );
        self.emit(client, "lost-after-5s", true, Instant::now(), note);
        self.after(client, "lost-then").await;
    }

    /// A box near the east border, the bot sitting on it, the box moved into the neighbour and
    /// back, as a vehicle crosses with its driver (OpenSim crosses an object moved out of the
    /// region with whoever sits on it: SceneObjectGroup.AbsolutePosition). Then the bot stands
    /// up and the box is deleted.
    async fn sit_cross(&mut self, client: &Client) {
        let (home, near) = (self.opts.home.clone(), self.opts.near.clone());
        // somewhere along the border: other bots may be at it too
        let y = self.spot().y;
        if !self
            .teleport(client, "sit-setup", &home, Vec3::new(240.0, y, 30.0))
            .await
        {
            return;
        }
        // on the ground first: a box rezzed where it was while falling stays in the air
        let mut last = client.state().position.z;
        let landing = Instant::now();
        while landing.elapsed() < Duration::from_secs(10) {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let z = client.state().position.z;
            if (z - last).abs() < 0.05 {
                break;
            }
            last = z;
        }
        let t = Instant::now();
        let before: std::collections::HashSet<Uuid> = match client.root_objects().await {
            Ok(roots) => roots.into_iter().map(|r| r.full_id).collect(),
            Err(e) => {
                self.emit(client, "sit-box", false, t, format!("the objects: {e}"));
                return;
            }
        };
        let at = client.state().position + Vec3::new(3.0, 0.0, 0.0);
        if let Err(e) = client.add_box(at, Vec3::new(1.0, 1.0, 0.5)).await {
            self.emit(client, "sit-box", false, t, e.to_string());
            return;
        }
        // a new object there, the bot's own (its properties tell the owner)
        let me = client.state().agent_id;
        let mut found = None;
        while found.is_none() && t.elapsed() < Duration::from_secs(10) {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let near: Vec<_> = client
                .root_objects()
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|r| !before.contains(&r.full_id) && r.position.distance(at) < 3.0)
                .collect();
            if near.is_empty() {
                continue;
            }
            let keys = near.iter().map(|r| r.full_id).collect();
            let _ = client.learn_objects(keys, Duration::from_secs(3)).await;
            found = client.root_objects().await.ok().and_then(|roots| {
                roots
                    .into_iter()
                    .find(|r| near.iter().any(|n| n.full_id == r.full_id) && r.owner == me)
            });
        }
        let Some(the_box) = found else {
            self.emit(client, "sit-box", false, t, "no box came".into());
            return;
        };
        self.emit(client, "sit-box", true, t, String::new());
        let box_id = the_box.full_id;
        let t = Instant::now();
        let _ = client.move_bot(Move::SitOn(box_id)).await;
        let seated = |c: &Client| c.state().sitting_on == Some(box_id);
        let sat = wait_for(Duration::from_secs(10), || seated(client)).await;
        self.emit(client, "sit-down", sat, t, String::new());
        if sat {
            let z = the_box.position.z;
            if self
                .carry(
                    client,
                    "sit-cross-east",
                    box_id,
                    Vec3::new(260.0, y, z),
                    &near,
                )
                .await
            {
                self.carry(
                    client,
                    "sit-cross-west",
                    box_id,
                    Vec3::new(-4.0, y, z),
                    &home,
                )
                .await;
            }
            let t = Instant::now();
            let _ = client.move_bot(Move::Stand).await;
            let stood = wait_for(Duration::from_secs(10), || {
                client.state().sitting_on.is_none()
            })
            .await;
            self.emit(client, "sit-stand", stood, t, String::new());
        }
        // the box goes, wherever it is
        if let Ok(Some(k)) = client.find_object(box_id).await {
            let _ = client.delete_object(k.local_id).await;
        }
    }

    /// Move the box the bot sits on to `to`, outside the region: the box crosses with the bot.
    async fn carry(
        &mut self,
        client: &Client,
        what: &str,
        box_id: Uuid,
        to: Vec3,
        into: &str,
    ) -> bool {
        let t = Instant::now();
        let Ok(Some(k)) = client.find_object(box_id).await else {
            self.emit(client, what, false, t, "the box is not known here".into());
            return false;
        };
        if let Err(e) = client.move_object(k.local_id, to).await {
            self.emit(client, what, false, t, e.to_string());
            return false;
        }
        let there = |c: &Client| region_name(&c.state()) == into;
        let crossed = wait_for(CROSSING_WAIT, || {
            there(client) && client.state().sitting_on == Some(box_id)
        })
        .await;
        let ms = t.elapsed();
        tokio::time::sleep(WATCH).await;
        let st = client.state();
        let note = if !there(client) {
            format!("never reached {into}")
        } else if st.sitting_on != Some(box_id) {
            format!(
                "in {into}, but not on the box (sitting on {:?})",
                st.sitting_on
            )
        } else if !crossed {
            "late".into()
        } else {
            String::new()
        };
        let ok = note.is_empty();
        self.record(client, what, ok, ms, note);
        ok
    }

    /// What still works after a bad step: a local teleport, to the neighbour and back home.
    async fn after(&mut self, client: &Client, prefix: &str) {
        let (home, near) = (self.opts.home.clone(), self.opts.near.clone());
        let middle = Vec3::new(128.0, 128.0, 30.0);
        self.teleport(
            client,
            &format!("{prefix}-local-tp"),
            &home,
            Vec3::new(100.0, 100.0, 30.0),
        )
        .await;
        self.teleport(client, &format!("{prefix}-tp-to-{near}"), &near, middle)
            .await;
        self.teleport(client, &format!("{prefix}-tp-to-{home}"), &home, middle)
            .await;
    }

    /// A teleport, judged by where the avatar is afterwards (judge.rs).
    async fn teleport(&mut self, client: &Client, what: &str, region: &str, to: Vec3) -> bool {
        self.teleport_via(client, what, region, region, to).await
    }

    /// A teleport to the region the map finds as `lookup`, which calls itself `region` once the
    /// bot is there: another grid's region is looked up as `host:port:Region`.
    async fn teleport_via(
        &mut self,
        client: &Client,
        what: &str,
        lookup: &str,
        region: &str,
        to: Vec3,
    ) -> bool {
        let Some(handle) = self.handle(client, lookup).await else {
            self.emit(
                client,
                what,
                false,
                Instant::now(),
                format!("no region named {lookup} on the map"),
            );
            return false;
        };
        let st = client.state();
        let trip = Trip {
            from_region: region_name(&st),
            from: st.position,
            to_region: region.to_owned(),
            to,
        };
        let t = Instant::now();
        let answer = client.teleport_to(handle, to).await;
        let at_target = |c: &Client| {
            let st = c.state();
            region_name(&st) == trip.to_region && judge::flat(st.position, to) < judge::NEAR
        };
        while t.elapsed() < ARRIVAL_WAIT && !at_target(client) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let arrived = at_target(client);
        let ms = t.elapsed();
        // the user lets go of the movement keys on arrival
        let _ = client.move_bot(Move::WalkStop).await;
        let watched = Instant::now();
        let mut samples = Vec::new();
        if arrived {
            while watched.elapsed() < WATCH {
                samples.push(sample(client, watched));
                tokio::time::sleep(SAMPLE_EVERY).await;
            }
        }
        let end = sample(client, watched);
        let verdict = judge::judge(&trip, arrived, &samples, &end);
        let place = format!(
            "target {region} {:.1}, came from {} {:.1}, at the end {} {:.1}",
            to, trip.from_region, trip.from, end.region, end.at
        );
        let note = match &verdict {
            Verdict::Ok => match &answer {
                Ok(_) => String::new(),
                Err(e) => format!("arrived, but the teleport answered: {e}"),
            },
            Verdict::NotArrived => format!(
                "did not arrive ({}); {place}",
                match &answer {
                    Ok(r) => format!("the teleport answered {r}"),
                    Err(e) => e.to_string(),
                }
            ),
            Verdict::BouncedForAMoment { worst, at, ms } => format!(
                "BOUNCED for a moment: {worst:.1} m back toward the start at {at:.1}, {ms} ms after arriving; {place}"
            ),
            Verdict::BouncedBack => format!("BOUNCED back toward the start; {place}"),
            Verdict::Drifted(d) => format!("DRIFTED {d:.1} m from the target; {place}"),
        };
        let ok = verdict.ok();
        self.record(client, what, ok, ms, note);
        arrived
    }

    /// A teleport that cannot work: right when it fails, and the bot is online where it was.
    async fn refused(&mut self, client: &Client, what: &str, region: &str, to: Vec3) -> bool {
        let Some(handle) = self.handle(client, region).await else {
            self.emit(
                client,
                what,
                false,
                Instant::now(),
                format!("no region named {region} on the map"),
            );
            return false;
        };
        let before = client.state();
        let (from_region, from) = (region_name(&before), before.position);
        let t = Instant::now();
        let answer = client.teleport_to(handle, to).await;
        let ms = t.elapsed();
        let st = client.state();
        let (ok, note) = match answer {
            Ok(r) => (false, format!("the teleport went through, to {r}")),
            Err(e) if !st.online => (false, format!("failed ({e}), and the bot is offline")),
            Err(e) if region_name(&st) != from_region => (
                false,
                format!("failed ({e}), but the bot is in {}", region_name(&st)),
            ),
            Err(e) if judge::flat(st.position, from) >= judge::NEAR => (
                false,
                format!(
                    "failed ({e}), but the bot moved from {from:.1} to {:.1}",
                    st.position
                ),
            ),
            Err(e) => (true, format!("failed as it should: {e}")),
        };
        self.record(client, what, ok, ms, note);
        ok
    }

    /// Fly along the region's middle row over the border, until the bot is in `to`.
    async fn cross(&mut self, client: &Client, what: &str, to: &str, east: bool) -> bool {
        let t = Instant::now();
        let _ = client.move_bot(Move::Fly(true)).await;
        let _ = client
            .move_bot(Move::Face(if east { 90.0 } else { 270.0 }))
            .await;
        let _ = client
            .move_bot(Move::Walk(WalkKey::Forward, CROSSING_WAIT))
            .await;
        let mut crossed = false;
        while t.elapsed() < CROSSING_WAIT {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if region_name(&client.state()) == to {
                crossed = true;
                break;
            }
            if !client.state().online
                && client.state().region.is_none()
                && t.elapsed() > CROSSING_WAIT / 2
            {
                break;
            }
        }
        let ms = t.elapsed();
        let _ = client.move_bot(Move::WalkStop).await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let still_there = region_name(&client.state()) == to;
        let note = if !crossed {
            format!("never reached {to}")
        } else if !still_there {
            "crossed, then back in the old region".into()
        } else {
            String::new()
        };
        self.record(client, what, crossed && still_there, ms, note);
        crossed
    }

    async fn handle(&mut self, client: &Client, region: &str) -> Option<u64> {
        if let Some(h) = self.handles.get(region) {
            return Some(*h);
        }
        let h = client.region_handle(region).await.ok().flatten()?;
        self.handles.insert(region.to_owned(), h);
        Some(h)
    }

    /// A spot of the home region away from its edges (xorshift: no dependency for this).
    fn spot(&mut self) -> Vec3 {
        let mut next = || {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            40.0 + (self.seed % 170) as f32
        };
        Vec3::new(next(), next(), 30.0)
    }

    fn emit(&mut self, client: &Client, what: &str, ok: bool, since: Instant, note: String) {
        self.record(client, what, ok, since.elapsed(), note);
    }

    fn record(&mut self, client: &Client, what: &str, ok: bool, ms: Duration, note: String) {
        let st = client.state();
        let what = match what {
            "login" | "logout" => what.to_owned(),
            _ => format!("{what}#{}", self.round),
        };
        let r = Record {
            bot: self.user.clone(),
            what,
            ok,
            ms: ms.as_millis() as u64,
            region: region_name(&st),
            pos: st.position,
            neighbours: st
                .neighbours
                .iter()
                .map(|(h, n)| {
                    if n.is_empty() {
                        format!("{h:x}")
                    } else {
                        n.clone()
                    }
                })
                .collect(),
            note,
        };
        self.records.push(r);
        if self.quiet {
            return;
        }
        let line = self.records[self.records.len() - 1].json();
        println!("{line}");
        if let Some(f) = &self.json {
            let mut f = f.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = writeln!(f, "{line}") {
                log::warn!("the results file: {e}");
            }
        }
    }
}

/// Wait until `done`, at most `wait`.
async fn wait_for(wait: Duration, done: impl Fn() -> bool) -> bool {
    let t = Instant::now();
    while !done() {
        if t.elapsed() > wait {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    true
}

fn region_name(st: &ferro_client::State) -> String {
    st.region
        .as_ref()
        .map_or_else(|| "(none)".into(), |r| r.name.clone())
}

fn sample(client: &Client, since: Instant) -> Sample {
    let st = client.state();
    Sample {
        region: region_name(&st),
        at: st.position,
        ms: since.elapsed().as_millis() as u64,
    }
}
