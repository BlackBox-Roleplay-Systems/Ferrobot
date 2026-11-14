//! SecondBot's EventsService (Services/EventsService.cs, Config/EventsConfig.cs): what happens
//! around the bot, told as SecondBot's OnEvent JSON to an avatar by IM, a channel of local chat
//! and an http(s) address (a signed POST to `<address>/Event/Service`, crate::event_output):
//! - `GroupMembership` (`uuid`, `name`, `action` "join" or "leave"): the members of one group,
//!   compared every 120 s; and, Ferrobot's own (FERRO_EVENTS_GROUPS), the members of several
//!   groups or of every group the bot is in, each event naming its group in a fourth value,
//!   `group`, so that one bot can serve several websites (Black Box's sims share one);
//! - `GuestTracking` (`uuid`, `name`, `action` "enter" or "exit"): the avatars of the region,
//!   compared every 30 s while the bot stands on the parcel named, in the region named;
//! - `ChangedSim` (`lastsim`, `newsim`), `ChangedParcel` (`oldparcel`, `newparcel`): the bot moved;
//! - `AlertMessage` (`Message`, `NotificationId`): every alert the region sends;
//! - `StatusMessage` (`Message`): the bot's state changed;
//! - `MoneyEvent` (`fromuuid`, `touuid`, `amount`, `balance`, `fromname`, `toname`,
//!   `transactionid`): every MoneyBalanceReply.
//!
//! Deliberate differences, each a SecondBot slip: `lastsim` is the region left (SecondBot's lags
//! one change behind); the bot's parcel is followed even with ChangeParcel off (SecondBot's guest
//! tracking then never finds its parcel); the status is Ferrobot's own ("In world: <bot> in
//! <region>", "Disconnected: <why>"), not SecondBot's list of its services.

use crate::event_output::{self, EVENT_SERVICE, Roster, avatar_name};
use ferro_client::{Client, Event};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use uuid::Uuid;

/// SecondBot's EventsConfig.
#[derive(Clone)]
pub struct Settings {
    /// The group whose members are followed (GroupMemberEventsGroupUUID; none: not followed).
    pub group: Option<Uuid>,
    /// Ferrobot's own: more groups followed, their events naming the group (FERRO_EVENTS_GROUPS).
    pub groups: Groups,
    pub group_joins: bool,
    pub group_leaves: bool,
    /// How often the members are compared (120 s).
    pub group_every: Duration,
    pub guest_enters: bool,
    pub guest_leaves: bool,
    /// Where guests are followed: the bot in this region, on this parcel (by their names).
    pub guest_sim: String,
    pub guest_parcel: String,
    /// How often the avatars are compared (30 s).
    pub guest_every: Duration,
    pub alerts: bool,
    pub status: bool,
    pub money: bool,
    pub change_sim: bool,
    pub change_parcel: bool,
    /// The targets: a channel of 0 or more, an avatar, an http(s) address.
    pub channel: Option<i32>,
    pub avatar: Option<Uuid>,
    /// The http(s) addresses, each sent every event (SecondBot takes one).
    pub urls: Vec<String>,
    /// What signs the events' Hash (OutputSecret). Never logged.
    pub secret: String,
}

impl std::fmt::Debug for Settings {
    // the secret is never printed, the address neither (it may carry the receiver's token)
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("group", &self.group)
            .field("groups", &self.groups)
            .field("group_joins", &self.group_joins)
            .field("group_leaves", &self.group_leaves)
            .field("guest_enters", &self.guest_enters)
            .field("guest_leaves", &self.guest_leaves)
            .field("alerts", &self.alerts)
            .field("status", &self.status)
            .field("money", &self.money)
            .field("change_sim", &self.change_sim)
            .field("change_parcel", &self.change_parcel)
            .field("channel", &self.channel)
            .field("avatar", &self.avatar)
            .field("urls_set", &self.urls.len())
            .finish_non_exhaustive()
    }
}

/// The groups followed besides SecondBot's one (FERRO_EVENTS_GROUPS).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Groups {
    #[default]
    None,
    /// These groups.
    These(Vec<Uuid>),
    /// Every group the bot is in, as it joins and leaves them.
    All,
}

impl Groups {
    /// FERRO_EVENTS_GROUPS: `all`, or group keys separated by commas or spaces. None when a key
    /// does not read as one.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return Some(Self::None);
        }
        if value.eq_ignore_ascii_case("all") {
            return Some(Self::All);
        }
        value
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|k| !k.is_empty())
            .map(|k| Uuid::parse_str(k).ok())
            .collect::<Option<Vec<_>>>()
            .map(Self::These)
    }
}

type Data = Vec<(&'static str, Option<String>)>;

/// The events, sent one after the other in the order they came.
#[derive(Clone)]
struct Out(mpsc::UnboundedSender<(&'static str, Data)>);

impl Out {
    fn push(&self, name: &'static str, data: Data) {
        log::debug!("event {name}");
        let _ = self.0.send((name, data));
    }
}

fn text(v: impl Into<String>) -> Option<String> {
    Some(v.into())
}

async fn send_all(
    client: Client,
    http: reqwest::Client,
    s: Arc<Settings>,
    shared_secret: String,
    mut queue: mpsc::UnboundedReceiver<(&'static str, Data)>,
) {
    while let Some((name, data)) = queue.recv().await {
        let bot = client.state().agent_id;
        let output = event_output::on_event(bot, name, &data, event_output::now(), &s.secret);
        if let Some(channel) = s.channel {
            event_output::to_channel(&client, channel, &output).await;
        }
        if let Some(avatar) = s.avatar {
            event_output::to_avatar(&client, avatar, &output).await;
        }
        for url in &s.urls {
            event_output::to_http(&client, &http, &shared_secret, url, EVENT_SERVICE, &output)
                .await;
        }
    }
}

/// What the events of the client say: alerts, money, the bot's region and state.
async fn on_events(client: Client, s: Arc<Settings>, out: Out, mut events: Receiver<Event>) {
    let mut last_sim: Option<String> = None;
    let mut last_status = String::new();
    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(RecvError::Lagged(n)) => {
                log::warn!("events: {n} client events missed");
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        let status = match event {
            Event::Arrived { region, .. } => {
                if s.change_sim
                    && let Some(before) = last_sim.as_ref().filter(|b| **b != region)
                {
                    out.push(
                        "ChangedSim",
                        vec![("lastsim", text(before)), ("newsim", text(&region))],
                    );
                }
                last_sim = Some(region.clone());
                Some(format!("In world: {} in {region}", client.state().name))
            }
            Event::Disconnected { reason } => Some(format!("Disconnected: {reason}")),
            Event::AlertMessage {
                message,
                notification_id,
            } if s.alerts => {
                out.push(
                    "AlertMessage",
                    vec![
                        ("Message", text(message)),
                        ("NotificationId", notification_id),
                    ],
                );
                None
            }
            Event::Money(m) if s.money => {
                let from = avatar_name(&client, m.source_id).await;
                let to = avatar_name(&client, m.dest_id).await;
                out.push(
                    "MoneyEvent",
                    vec![
                        ("fromuuid", text(m.source_id.to_string())),
                        ("touuid", text(m.dest_id.to_string())),
                        ("amount", text(m.amount.to_string())),
                        ("balance", text(m.balance.to_string())),
                        ("fromname", text(from)),
                        ("toname", text(to)),
                        ("transactionid", text(m.transaction_id.to_string())),
                    ],
                );
                None
            }
            _ => None,
        };
        // only a change of state is told (SecondBot's SystemStatusMessage.changed)
        if let Some(status) = status
            && status != last_status
        {
            if s.status {
                out.push("StatusMessage", vec![("Message", text(&status))]);
            }
            last_status = status;
        }
    }
}

/// Who joined or left, as events, the names asked of the grid.
async fn tell(
    client: &Client,
    out: &Out,
    name: &'static str,
    action: &'static str,
    ids: Vec<Uuid>,
) {
    for id in ids {
        let who = avatar_name(client, id).await;
        out.push(
            name,
            vec![
                ("uuid", text(id.to_string())),
                ("name", text(who)),
                ("action", text(action)),
            ],
        );
    }
}

/// The bot's parcel and the guests on it, checked every second (SecondBot's TimedEvents).
async fn watch_place(client: Client, s: Arc<Settings>, out: Out) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_parcel = String::new();
    let mut guests = Roster::default();
    let mut guests_checked: Option<Instant> = None;
    loop {
        tick.tick().await;
        let (online, region, parcel, avatars) = {
            let st = client.state();
            let mut avatars: Vec<Uuid> = st.avatars.keys().copied().collect();
            avatars.sort();
            (
                st.online,
                st.region.as_ref().map(|r| r.name.clone()),
                st.parcel.as_ref().map(|p| p.name.clone()),
                avatars,
            )
        };
        if !online {
            // a new login learns the avatars again, once they had time to show
            guests.reset();
            guests_checked = Some(Instant::now());
            continue;
        }
        // the parcel is known once the region told it
        let (Some(region), Some(parcel)) = (region, parcel) else {
            continue;
        };
        if parcel != last_parcel {
            if s.change_parcel {
                out.push(
                    "ChangedParcel",
                    vec![
                        ("oldparcel", text(&last_parcel)),
                        ("newparcel", text(&parcel)),
                    ],
                );
            }
            last_parcel = parcel;
        }
        if region != s.guest_sim || last_parcel != s.guest_parcel {
            guests.reset();
            continue;
        }
        if guests_checked.is_some_and(|t| t.elapsed() < s.guest_every) {
            continue;
        }
        guests_checked = Some(Instant::now());
        log::debug!("events: {} guests", avatars.len());
        let (entered, left) = guests.update(avatars);
        if s.guest_leaves {
            tell(&client, &out, "GuestTracking", "exit", left).await;
        }
        if s.guest_enters {
            tell(&client, &out, "GuestTracking", "enter", entered).await;
        }
    }
}

/// The group's members, compared every `group_every` (SecondBot asks every 120 s). `named`: the
/// events carry the group (FERRO_EVENTS_GROUPS), SecondBot's one group's do not.
async fn watch_group(client: Client, s: Arc<Settings>, group: Uuid, named: bool, out: Out) {
    let mut members = Roster::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut checked: Option<Instant> = None;
    loop {
        tick.tick().await;
        if !client.health().online || checked.is_some_and(|t| t.elapsed() < s.group_every) {
            continue;
        }
        checked = Some(Instant::now());
        let list = match client.group_members_within(group, s.group_every).await {
            Ok(list) => list,
            Err(e) => {
                log::debug!("events: the group's members: {e}");
                continue;
            }
        };
        log::debug!("events: {} members in the group", list.len());
        let (joined, left) = members.update(list.iter().map(|m| m.id).collect());
        let named = named.then_some(group);
        if s.group_leaves {
            tell_group(&client, &out, named, "leave", left).await;
        }
        if s.group_joins {
            tell_group(&client, &out, named, "join", joined).await;
        }
    }
}

/// GroupMembership for each avatar, with the group's key last when it is named.
async fn tell_group(
    client: &Client,
    out: &Out,
    group: Option<Uuid>,
    action: &'static str,
    who: Vec<Uuid>,
) {
    let Some(group) = group else {
        tell(client, out, "GroupMembership", action, who).await;
        return;
    };
    for id in who {
        let name = avatar_name(client, id).await;
        out.push(
            "GroupMembership",
            vec![
                ("uuid", text(id.to_string())),
                ("name", text(name)),
                ("action", text(action)),
                ("group", text(group.to_string())),
            ],
        );
    }
}

/// FERRO_EVENTS_GROUPS: a watcher per group, started as the bot is found in it (`All`) and
/// stopped when it leaves.
async fn watch_groups(client: Client, s: Arc<Settings>, out: Out) {
    let mut watched: std::collections::HashMap<Uuid, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let wanted: Vec<Uuid> = match &s.groups {
            Groups::None => return,
            Groups::These(list) => list.clone(),
            Groups::All => client.state().groups.iter().map(|g| g.id).collect(),
        };
        watched.retain(|id, task| {
            let keep = wanted.contains(id);
            if !keep {
                task.abort();
                log::debug!("events: no longer following a group");
            }
            keep
        });
        for id in wanted {
            watched.entry(id).or_insert_with(|| {
                tokio::spawn(watch_group(
                    client.clone(),
                    s.clone(),
                    id,
                    true,
                    out.clone(),
                ))
            });
        }
    }
}

pub async fn run(
    client: Client,
    events: Receiver<Event>,
    settings: Settings,
    http: reqwest::Client,
    shared_secret: Option<String>,
) {
    let s = Arc::new(settings);
    let (tx, rx) = mpsc::unbounded_channel();
    let out = Out(tx);
    tokio::spawn(send_all(
        client.clone(),
        http,
        s.clone(),
        shared_secret.unwrap_or_default(),
        rx,
    ));
    tokio::spawn(watch_place(client.clone(), s.clone(), out.clone()));
    if let Some(group) = s.group {
        tokio::spawn(watch_group(
            client.clone(),
            s.clone(),
            group,
            false,
            out.clone(),
        ));
    }
    if s.groups != Groups::None {
        tokio::spawn(watch_groups(client.clone(), s.clone(), out.clone()));
    }
    on_events(client, s, out, events).await;
}

#[cfg(test)]
mod tests {
    use super::Groups;
    use uuid::Uuid;

    #[test]
    fn groups_setting() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        assert_eq!(Groups::parse(""), Some(Groups::None));
        assert_eq!(Groups::parse(" ALL "), Some(Groups::All));
        assert_eq!(
            Groups::parse(&format!("{a}, {b}")),
            Some(Groups::These(vec![a, b]))
        );
        assert_eq!(
            Groups::parse(&format!("{a} {b}")),
            Some(Groups::These(vec![a, b]))
        );
        assert_eq!(Groups::parse(&format!("{a},not-a-key")), None);
    }
}
