//! The scenarios, as the Tranquillity teleport rig ran them with a LibreMetaverse client: one
//! fresh login a round, then teleports and crossings, each judged and told as it ends.

use crate::judge::{self, Sample, Trip, Verdict};
use crate::options::{Options, Scenario};
use crate::report::Record;
use ferro_client::login::{self, LoginRequest, StartLocation};
use ferro_client::{Client, End, Event, Move, WalkKey};
use glam::Vec3;
use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// How long a teleport may take to put the avatar at its target.
const ARRIVAL_WAIT: Duration = Duration::from_secs(15);
/// How long the avatar is watched after arriving, and how often.
const WATCH: Duration = Duration::from_millis(1500);
const SAMPLE_EVERY: Duration = Duration::from_millis(20);
/// How long a crossing may take, flying from near the border.
const CROSSING_WAIT: Duration = Duration::from_secs(45);
/// How long a login may take to arrive.
const LOGIN_WAIT: Duration = Duration::from_secs(60);

pub struct Probe<'a> {
    opts: &'a Options,
    password: String,
    http: reqwest::Client,
    /// Region handles by name, looked up once: a map lookup is not part of a teleport's time.
    handles: HashMap<String, u64>,
    pub records: Vec<Record>,
    json: Option<std::fs::File>,
    round: u32,
    /// For the spots of the bounce scenario.
    seed: u64,
}

impl<'a> Probe<'a> {
    pub fn new(opts: &'a Options, password: String, json: Option<std::fs::File>) -> Probe<'a> {
        let http = reqwest::Client::builder()
            .redirect(login::redirect_policy())
            .build()
            .unwrap_or_default();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64 | 1);
        Probe {
            opts,
            password,
            http,
            handles: HashMap::new(),
            records: Vec::new(),
            json,
            round: 0,
            seed,
        }
    }

    /// One round: a fresh login, the scenarios, the logout.
    pub async fn round(&mut self, round: u32) {
        self.round = round;
        let (client, mut network) = Client::new(self.http.clone());
        network.set_neighbours(self.opts.neighbours);
        let mut events = client.subscribe();
        let req = LoginRequest {
            login_uri: self.opts.login_uri.clone(),
            username: self.opts.user.clone(),
            password: self.password.clone(),
            start: StartLocation::Region {
                name: self.opts.home.clone(),
                x: 128,
                y: 128,
                z: 30,
            },
        };
        let t = Instant::now();
        let resp = match login::login(&self.http, &req).await {
            Ok(resp) => resp,
            Err(e) => {
                self.emit(&client, "login", false, t, format!("refused: {e}"));
                return;
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
        self.emit(&client, "login", arrived, t, String::new());
        if arrived {
            // the regions settle the arrival (child agents, appearance) before the first teleport
            tokio::time::sleep(Duration::from_secs(3)).await;
            for scenario in self.opts.scenarios.clone() {
                if !client.state().online {
                    break;
                }
                self.scenario(&client, scenario).await;
            }
        }
        let t = Instant::now();
        let _ = stop.send(true);
        let end = tokio::time::timeout(Duration::from_secs(15), session).await;
        let (ok, note) = match end {
            Ok(Ok(End::LoggedOut)) => (true, String::new()),
            Ok(Ok(end)) => (false, end.to_string()),
            Ok(Err(e)) => (false, format!("the session task: {e}")),
            Err(_) => (false, "the session did not end".into()),
        };
        self.emit(&client, "logout", ok, t, note);
    }

    async fn scenario(&mut self, client: &Client, scenario: Scenario) {
        let o = self.opts;
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
            Scenario::Unreachable => {
                let unreachable = o.unreachable.clone();
                self.teleport(
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
        let line = r.json();
        println!("{line}");
        if let Some(f) = self.json.as_mut()
            && let Err(e) = writeln!(f, "{line}")
        {
            log::warn!("the results file: {e}");
        }
        self.records.push(r);
    }
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
