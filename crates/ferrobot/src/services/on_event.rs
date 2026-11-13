//! SecondBot's TriggerOnEventService: the events of crate::on_event, watched and run. Its
//! sources: avatars' IMs, the region's alerts, the balance, a group's members joining or leaving
//! (each group compared every 15 minutes, one request a minute at most), the bot changing region,
//! avatars coming into or leaving the region (compared every 10 s), and a clock every minute.
//! Nothing is triggered until `WaitSecsToStart` after each login.
//!
//! The actions are the bot's own settings (the operator's): they run as commands through the
//! commands' dispatcher, with its rate limit, like a master's. What an event brings (an IM's
//! text, a name) only ever fills arguments (crate::on_event).
//!
//! Deliberate difference: an avatar that leaves is placed where it was last seen, so that a
//! `GuestLeaves` event can match its parcel (SecondBot looks for it after it left, finds no
//! parcel, and never runs one).

use crate::commands::{Caller, Ctx};
use crate::event_output::{Roster, now};
use crate::on_event::{self, Action, Lockouts, Source, Trigger, Values, Watch, World};
use ferro_client::events::dialog;
use ferro_client::{Client, Event};
use glam::Vec3;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Settings {
    pub triggers: Vec<Trigger>,
    /// Nothing is triggered this long after a login (WaitSecsToStart, 10 s).
    pub wait: Duration,
    /// Each step logged (DebugMe).
    pub debug: bool,
    /// How often the region's avatars are compared (10 s).
    pub guest_every: Duration,
    /// How often each group's members are compared (15 minutes).
    pub group_every: Duration,
}

struct Service {
    ctx: Ctx,
    s: Settings,
    lockouts: Lockouts,
    /// When the events may run: some time after the login; `None` while offline.
    ready_at: Mutex<Option<Instant>>,
}

impl Service {
    fn ready(&self) -> bool {
        self.ready_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|t| Instant::now() >= t)
    }

    fn set_ready_at(&self, at: Option<Instant>) {
        *self
            .ready_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = at;
    }

    fn watches(&self, source: Source) -> bool {
        self.s.triggers.iter().any(|t| t.source == source)
    }

    fn debug(&self, line: impl FnOnce() -> String) {
        if self.s.debug {
            log::info!("on event (debug): {}", line());
        }
    }
}

fn rounded(v: Vec3) -> [i64; 3] {
    [v.x, v.y, v.z].map(|c| c.round() as i64)
}

/// The world around an event: the bot's place, the avatar's, the clock.
async fn world(
    client: &Client,
    avatar: Option<Uuid>,
    last_seen: Option<Vec3>,
    name: Option<String>,
) -> World {
    let (bot_sim, bot_parcel, bot_at, seen) = {
        let st = client.state();
        (
            st.region
                .as_ref()
                .map(|r| r.name.clone())
                .unwrap_or_default(),
            st.parcel
                .as_ref()
                .map_or_else(|| "-".to_owned(), |p| p.name.clone()),
            st.position,
            avatar.and_then(|a| st.avatars.get(&a).copied()),
        )
    };
    let mut w = World {
        bot_sim,
        bot_parcel,
        bot_at: rounded(bot_at),
        avatar,
        avatar_name: "lookup".into(),
        avatar_parcel: "?".into(),
        avatar_at: [-1; 4],
        clock: on_event::clock(now()),
    };
    if let Some(a) = avatar {
        w.avatar_name = match name {
            Some(n) => n,
            None => crate::event_output::avatar_name(client, a).await,
        };
        if let Some(at) = seen.or(last_seen) {
            let [x, y, z] = rounded(at);
            w.avatar_at = [x, y, z, at.distance(bot_at).round() as i64];
            if let Ok(Some(p)) = client.parcel_at(at).await {
                w.avatar_parcel = p.name;
            }
        }
    }
    w
}

/// An event happened: the events of its source whose checks pass run their actions.
async fn trigger(
    svc: Arc<Service>,
    source: Source,
    args: Vec<(&'static str, String)>,
    avatar: Option<Uuid>,
    last_seen: Option<Vec3>,
) {
    if !svc.ready() {
        return;
    }
    let triggers: Vec<&Trigger> = svc
        .s
        .triggers
        .iter()
        .filter(|t| t.source == source)
        .collect();
    if triggers.is_empty() {
        return;
    }
    svc.debug(|| {
        let shown: Vec<String> = args.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{} {}", source.name(), shown.join(","))
    });
    let name = args
        .iter()
        .find(|(k, _)| *k == "avatarname")
        .map(|(_, v)| v.clone());
    let w = world(&svc.ctx.client, avatar, last_seen, name).await;
    let values = on_event::values(source, &args, &w);
    for t in triggers {
        if !t.applies(&values) {
            svc.debug(|| format!("event {} not for this {}", t.n, source.name()));
            continue;
        }
        if !rules_hold(&svc, t, &values).await {
            continue;
        }
        svc.debug(|| format!("event {}: {} actions", t.n, t.actions.len()));
        run_actions(&svc, t, &values).await;
    }
}

async fn rules_hold(svc: &Service, t: &Trigger, values: &Values) -> bool {
    for (i, rule) in t.rules.iter().enumerate() {
        let left = values.put_in(&rule.left);
        let right = values.put_in(&rule.right);
        let in_group = match (rule.op, on_event::key(&left), on_event::key(&right)) {
            (on_event::Op::InGroup | on_event::Op::NotInGroup, Some(avatar), Some(group)) => svc
                .ctx
                .client
                .is_group_member(group, avatar)
                .await
                .unwrap_or(false),
            _ => false,
        };
        let locked = svc.lockouts.locked(&left, now());
        if !rule.op.holds(&left, &right, in_group, locked) {
            svc.debug(|| format!("event {}: condition {} fails ({:?})", t.n, i + 1, rule.op));
            return false;
        }
    }
    true
}

async fn run_actions(svc: &Service, t: &Trigger, values: &Values) {
    for action in &t.actions {
        match action {
            Action::Lockout { key, minutes } => {
                let key = values.put_in(key);
                // int.TryParse: a lockout without minutes is skipped
                if let Ok(m) = values.put_in(minutes).trim().parse::<i32>() {
                    svc.lockouts.add(&key, i64::from(m), now());
                }
            }
            Action::Command(u) => {
                let r = on_event::ready(u, values);
                // the operator's own trigger, from the settings: run as a trusted caller
                let (ran, value) =
                    crate::api::run_command(&svc.ctx, Caller::Trusted, &r.command, &r.args).await;
                let done = ran && value.contains(r#""status":true"#);
                log::info!(
                    "on event {} ({}): {} ({} args): {}",
                    t.n,
                    t.source.name(),
                    r.command,
                    r.args.len(),
                    if done { "done" } else { "failed" }
                );
                if !ran {
                    log::warn!("on event {}: {}", t.n, value);
                }
                if let Some(target) = &r.reply {
                    crate::smart_reply::send(&svc.ctx, target, &value, &r.command).await;
                }
            }
        }
    }
}

fn spawn(
    svc: &Arc<Service>,
    source: Source,
    args: Vec<(&'static str, String)>,
    avatar: Option<Uuid>,
    last_seen: Option<Vec3>,
) {
    tokio::spawn(trigger(svc.clone(), source, args, avatar, last_seen));
}

/// What the client's events trigger.
async fn on_events(svc: Arc<Service>, mut events: Receiver<Event>) {
    let me = || svc.ctx.client.state().agent_id;
    let mut last_sim: Option<String> = None;
    let mut logged_in = false;
    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(RecvError::Lagged(n)) => {
                log::warn!("on event: {n} client events missed");
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        match event {
            Event::Arrived { region, .. } => {
                if !logged_in {
                    // SecondBot's waitingForTimer, from each login
                    logged_in = true;
                    svc.set_ready_at(Some(Instant::now() + svc.s.wait));
                }
                if let Some(before) = last_sim.as_ref().filter(|b| **b != region) {
                    spawn(
                        &svc,
                        Source::ChangeSim,
                        vec![
                            ("oldsimname", before.clone()),
                            ("newsimname", region.clone()),
                        ],
                        None,
                        None,
                    );
                }
                last_sim = Some(region);
            }
            Event::Disconnected { .. } => {
                logged_in = false;
                svc.set_ready_at(None);
            }
            // an avatar's IM, in a conference too, not a group's (SecondBot's BotImMessage)
            Event::InstantMessage(im)
                if matches!(im.dialog, dialog::NOTHING_SPECIAL | dialog::SESSION_SEND)
                    && !im.from_group
                    && im.from_agent_id != me() =>
            {
                spawn(
                    &svc,
                    Source::AvatarIm,
                    vec![
                        ("message", im.message),
                        ("avatarname", im.from_name),
                        ("avataruuid", im.from_agent_id.to_string()),
                    ],
                    Some(im.from_agent_id),
                    None,
                );
            }
            Event::AlertMessage { message, .. } => {
                spawn(
                    &svc,
                    Source::SimAlert,
                    vec![("alertmessage", message)],
                    None,
                    None,
                );
            }
            Event::Money(m) => {
                spawn(
                    &svc,
                    Source::BalanceUpdate,
                    vec![("funds", m.balance.to_string())],
                    None,
                    None,
                );
            }
            _ => {}
        }
    }
}

/// The timed sources, checked every second: the region's avatars, the groups, the clock.
async fn timed(svc: Arc<Service>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let guests_watched = svc.watches(Source::GuestJoins) || svc.watches(Source::GuestLeaves);
    let mut guests = Roster::default();
    let mut last_places: HashMap<Uuid, Vec3> = HashMap::new();
    let mut guests_checked: Option<Instant> = None;
    let mut clock: Option<Instant> = None;
    let mut cleaned: Option<Instant> = None;
    // the groups the events watch, each with its members and when it was last asked
    let mut groups: Vec<(Uuid, Roster, Option<Instant>)> = Vec::new();
    for t in &svc.s.triggers {
        if let Watch::Group(g) = t.watch
            && !groups.iter().any(|(id, _, _)| *id == g)
        {
            groups.push((g, Roster::default(), None));
        }
    }
    let mut group_asked: Option<Instant> = None;
    // one request a minute at most (fewer in tests)
    let group_gap = svc.s.group_every.min(Duration::from_secs(60));
    let since = |t: Option<Instant>, every: Duration| t.is_none_or(|t| t.elapsed() >= every);
    loop {
        tick.tick().await;
        if !svc.ready() {
            continue;
        }
        let client = &svc.ctx.client;
        if guests_watched && since(guests_checked, svc.s.guest_every) {
            guests_checked = Some(Instant::now());
            let places = client.state().avatars.clone();
            let mut now: Vec<Uuid> = places.keys().copied().collect();
            now.sort();
            svc.debug(|| format!("{} avatars in the region", now.len()));
            let (joined, left) = guests.update(now);
            for a in left {
                let at = last_places.get(&a).copied();
                spawn(
                    &svc,
                    Source::GuestLeaves,
                    vec![("avataruuid", a.to_string())],
                    Some(a),
                    at,
                );
            }
            for a in joined {
                spawn(
                    &svc,
                    Source::GuestJoins,
                    vec![("avataruuid", a.to_string())],
                    Some(a),
                    None,
                );
            }
            last_places = places;
        }
        if since(cleaned, Duration::from_secs(60)) {
            cleaned = Some(Instant::now());
            svc.lockouts.clean(now());
        }
        if since(clock, Duration::from_secs(60)) {
            clock = Some(Instant::now());
            spawn(&svc, Source::Clock, Vec::new(), None, None);
        }
        if !groups.is_empty() && since(group_asked, group_gap) {
            let Some((group, members, asked)) = groups
                .iter_mut()
                .find(|(_, _, asked)| since(*asked, svc.s.group_every))
            else {
                continue;
            };
            group_asked = Some(Instant::now());
            *asked = Some(Instant::now());
            let group = *group;
            let list = match client.group_members_within(group, group_gap).await {
                Ok(list) => list,
                Err(e) => {
                    log::debug!("on event: the members of {group}: {e}");
                    continue;
                }
            };
            svc.debug(|| format!("{} members in {group}", list.len()));
            let name = client
                .state()
                .groups
                .iter()
                .find(|g| g.id == group)
                .map_or_else(|| "none".to_owned(), |g| g.name.clone());
            let (joined, left) = members.update(list.iter().map(|m| m.id).collect());
            for (source, ids) in [
                (Source::GroupMemberLeave, left),
                (Source::GroupMemberJoin, joined),
            ] {
                for a in ids {
                    spawn(
                        &svc,
                        source,
                        vec![
                            ("groupuuid", group.to_string()),
                            ("groupname", name.clone()),
                            ("avataruuid", a.to_string()),
                        ],
                        Some(a),
                        None,
                    );
                }
            }
        }
    }
}

pub async fn run(ctx: Ctx, events: Receiver<Event>, settings: Settings) {
    for t in &settings.triggers {
        log::info!("on event {}: {} is watched", t.n, t.source.name());
    }
    let svc = Arc::new(Service {
        ctx,
        s: settings,
        lockouts: Lockouts::default(),
        ready_at: Mutex::new(None),
    });
    tokio::spawn(timed(svc.clone()));
    on_events(svc, events).await;
}
