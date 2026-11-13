//! Offers to the bot, answered by SecondBot's interaction rules (InteractionService.ProcessRequest):
//! for each kind, a switch (accept at all) and a level: "Owner" (the masters, the default),
//! "Friends" (the bot's friends; not for friendship offers), or "Anyone". After those, the grants
//! (crate::grants): a lasting one (SetPermFlag, sticky), else a one-time one, used up (SecondBot's
//! "accept next" list: AcceptNext, or SetPermFlag not sticky). A kind switched off stays off.
//!
//! - teleport offers (IM_LURE_USER): accepted, or ignored (BotTeleportOffer);
//! - teleport requests (IM_TELEPORT_REQUEST): the bot offers a teleport back, or declines;
//! - group invitations (IM_GROUP_INVITATION): accepted, or left unanswered (BotGroupInviteOffer);
//! - inventory offers (IM_INVENTORY_OFFERED, IM_TASK_INVENTORY_OFFERED): accepted into the
//!   inventory's root folder, or declined (BotInventoryOffer);
//! - friendship offers (IM_FRIENDSHIP_OFFERED): accepted, or ignored (BotFriendRequested).
//!
//! With FERRO_INTERACTION_EVENTS on, each of them is told as SecondBot's JSON events
//! (InteractionEvent, then JsonOuputCleaner's once accepted, and BotInventoryAdd's for the items
//! that come into the inventory) to FERRO_INTERACTION_EVENTS_TARGET, as a command's answer goes
//! to its reply target (crate::smart_reply, the command named "interactions").
//!
//! Deliberate differences: masters are recognised by the sender's key where the grid gives it
//! (masters.rs); a group invitation with a membership fee is never accepted; a group invitation's
//! grant is found by the group's key (as SecondBot's, whose sender is the group) or by the
//! inviter's name; an avatar at level "Friends" that is not a friend is told "rejected request"
//! (SecondBot says "on friends list"); a lasting grant is told "on a lasting grant (SetPermFlag)";
//! an inventory offer's "itemname" is the name the offer gives (SecondBot fetches the item); the
//! "InventoryAdd" events come from the items the grid says came in (BulkUpdateInventory), not from
//! every item an inventory fetch shows.

use crate::commands::reply::{cs_bool_text, json, json_pairs};
use crate::config::{ConfigError, Source};
use crate::grants::{Grant, Grants, Used};
use crate::inventory_updates::asset_type_name;
use crate::masters::Masters;
use crate::services::control::Spec;
use crate::services::homebound::Homebound;
use ferro_client::events::{AddedItem, dialog};
use ferro_client::{Client, Event, InstantMessage};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Owner,
    Friends,
    Anyone,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "owner" => Some(Level::Owner),
            "friends" => Some(Level::Friends),
            "anyone" => Some(Level::Anyone),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub enabled: bool,
    pub level: Level,
}

/// SecondBot's InteractionConfig: every kind on, at level "Owner", by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub teleports: Rule,
    pub group_invites: Rule,
    pub inventory: Rule,
    pub friendships: Rule,
}

/// What the service works with.
pub struct Service {
    pub client: Client,
    pub masters: Arc<Masters>,
    pub rules: Rules,
    pub homebound: Arc<Homebound>,
    pub grants: Arc<Grants>,
    pub http: reqwest::Client,
    /// For an events target that is an e-mail address.
    pub smtp: Arc<crate::services::smtp::Mailer>,
    /// Where the JSON events go (JsonOutputEventsTarget), when they are on.
    pub events: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Teleport,
    GroupInvite,
    Inventory,
    Friendship,
}

impl Kind {
    fn grant(self) -> Grant {
        match self {
            Kind::Teleport => Grant::Teleport,
            Kind::GroupInvite => Grant::GroupInvite,
            Kind::Inventory => Grant::Inventory,
            Kind::Friendship => Grant::Friendship,
        }
    }

    /// The event's type, as ProcessRequest's sourceName.
    fn event_type(self) -> &'static str {
        match self {
            Kind::Teleport => "Teleport",
            Kind::GroupInvite => "GroupInvite",
            Kind::Inventory => "InventoryOffer",
            Kind::Friendship => "FriendRequest",
        }
    }
}

/// Why a request was accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    Master,
    Anyone,
    Friend,
    Lasting,
    NextAccept,
}

impl Why {
    /// ProcessRequest's whyAccepted.
    fn info(self) -> &'static str {
        match self {
            Why::Master => "on owners list",
            Why::Anyone => "Accepting from anyone",
            Why::Friend => "on friends list",
            Why::Lasting => "on a lasting grant (SetPermFlag)",
            Why::NextAccept => "was on NextAccept list",
        }
    }
}

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Disabled,
    NotFriend,
    NotMaster,
    Fee(i32),
}

impl Refusal {
    /// For the log.
    fn log(self) -> String {
        match self {
            Refusal::Disabled => "disabled by the settings".into(),
            Refusal::NotFriend => "not a friend".into(),
            Refusal::NotMaster => "not a master".into(),
            Refusal::Fee(fee) => format!("a fee of L${fee}, the bot does not pay to join"),
        }
    }

    /// For the event, as ProcessRequest words it.
    fn info(self) -> String {
        match self {
            Refusal::Disabled => "disabled by config".into(),
            Refusal::NotFriend | Refusal::NotMaster => "rejected request".into(),
            Refusal::Fee(_) => self.log(),
        }
    }
}

/// ProcessRequest: whether the sender may have this. A one-time grant is used up here, so the
/// checks that can refuse anyway (the fee) come first.
async fn allowed(s: &Service, rule: Rule, kind: Kind, im: &InstantMessage) -> Result<Why, Refusal> {
    if !rule.enabled {
        return Err(Refusal::Disabled);
    }
    // a group invitation comes from the group: only the inviter's name tells who sent it
    let master = if kind == Kind::GroupInvite {
        s.masters.by_name(&im.from_name)
    } else {
        s.masters.by_key(&s.client, im.from_agent_id).await
    };
    if master {
        return Ok(Why::Master);
    }
    match rule.level {
        Level::Anyone => return Ok(Why::Anyone),
        Level::Friends
            if kind != Kind::Friendship && s.client.state().friends.contains(&im.from_agent_id) =>
        {
            return Ok(Why::Friend);
        }
        _ => {}
    }
    let grant = kind.grant();
    let mut used = s.grants.use_key(grant, im.from_agent_id);
    if used.is_none() && kind == Kind::GroupInvite {
        used = s.grants.use_name(&s.client, grant, &im.from_name).await;
    }
    match used {
        Some(Used::Lasting) => Ok(Why::Lasting),
        Some(Used::Once) => Ok(Why::NextAccept),
        None if rule.level == Level::Friends => Err(Refusal::NotFriend),
        None => Err(Refusal::NotMaster),
    }
}

/// The membership fee of an invitation (invite_bucket_t: a big-endian S32 then the role); `None`
/// when the bucket is not one.
fn fee(bucket: &[u8]) -> Option<i32> {
    let bytes: [u8; 4] = bucket.get(..4)?.try_into().ok()?;
    (bucket.len() == 20).then(|| i32::from_be_bytes(bytes))
}

/// SecondBot's InteractionEvent: `{from, eventType, accepted, info, misc}`, in that order.
fn interaction_event(
    from: &str,
    event_type: &str,
    accepted: bool,
    info: &str,
    misc: &[(&str, String)],
) -> String {
    format!(
        r#"{{"from":{},"eventType":{},"accepted":{accepted},"info":{},"misc":{}}}"#,
        json(&from),
        json(&event_type),
        json(&info),
        json_pairs(misc)
    )
}

/// What an offer's misc holds before ProcessRequest adds the owner's name, as each handler of
/// SecondBot's builds it.
fn misc(kind: Kind, im: &InstantMessage, root: Uuid) -> Vec<(&'static str, String)> {
    match kind {
        Kind::Teleport => vec![("mode", teleport_mode(im.dialog).to_owned())],
        Kind::GroupInvite => vec![
            ("message", im.message.clone()),
            ("fromuuid", im.from_agent_id.to_string()),
        ],
        Kind::Inventory => {
            let kind = im.binary_bucket.first().map_or(-1, |&t| i32::from(t as i8));
            vec![
                ("transactionid", im.id.to_string()),
                (
                    "itemtype",
                    asset_type_name(kind).map_or_else(|| kind.to_string(), str::to_owned),
                ),
                ("message", im.message.clone()),
                (
                    "fromscript",
                    cs_bool_text(im.dialog == dialog::TASK_INVENTORY_OFFERED),
                ),
                ("targetfolder", root.to_string()),
            ]
        }
        Kind::Friendship => vec![("fromName", im.from_name.clone())],
    }
}

/// BotTeleportOffer's modes.
fn teleport_mode(d: u8) -> &'static str {
    if d == dialog::TELEPORT_REQUEST {
        "Teleport Av to Bot"
    } else {
        "Teleport Bot to Av"
    }
}

/// The item an avatar's offer carries (the bucket: its asset type, then its key).
fn offered_item(bucket: &[u8]) -> Option<Uuid> {
    Uuid::from_slice(bucket.get(1..17)?).ok()
}

impl Service {
    async fn tell(&self, output: &str) {
        let Some(target) = &self.events else {
            return;
        };
        log::debug!("interaction event: {output}");
        crate::smart_reply::send_with(
            &self.client,
            &self.http,
            &self.smtp,
            target,
            output,
            "interactions",
        )
        .await;
    }

    /// DataStoreService.GetAvatarName: the name, or "lookup" when it is not known.
    async fn name_of(&self, id: Uuid) -> String {
        let found = tokio::time::timeout(Duration::from_secs(5), self.client.names(&[id])).await;
        match found {
            Ok(Ok(names)) => names.get(&id).cloned(),
            _ => None,
        }
        .unwrap_or_else(|| "lookup".to_owned())
    }

    async fn on_offer(self: Arc<Self>, im: InstantMessage) {
        let rules = self.rules;
        let (what, rule, kind) = match im.dialog {
            dialog::LURE_USER => ("teleport offer", rules.teleports, Kind::Teleport),
            dialog::TELEPORT_REQUEST => ("teleport request", rules.teleports, Kind::Teleport),
            dialog::GROUP_INVITATION => {
                ("group invitation", rules.group_invites, Kind::GroupInvite)
            }
            dialog::INVENTORY_OFFERED | dialog::TASK_INVENTORY_OFFERED => {
                ("inventory offer", rules.inventory, Kind::Inventory)
            }
            dialog::FRIENDSHIP_OFFERED => ("friendship offer", rules.friendships, Kind::Friendship),
            _ => return,
        };
        let from = &im.from_name;
        let verdict = match fee(&im.binary_bucket) {
            Some(fee) if kind == Kind::GroupInvite && fee > 0 => Err(Refusal::Fee(fee)),
            _ => allowed(&self, rule, kind, &im).await,
        };
        if self.events.is_some() {
            // told on the side: a slow target does not hold the answer up
            tokio::spawn(self.clone().tell_offer(kind, im.clone(), verdict));
        }
        let from_task = im.dialog == dialog::TASK_INVENTORY_OFFERED;
        let client = &self.client;
        let done = match verdict {
            Err(refusal) => {
                log::info!("{what} from {from} refused: {}", refusal.log());
                // the answers SecondBot gives to what it refuses
                match im.dialog {
                    dialog::TELEPORT_REQUEST => {
                        client
                            .decline_teleport_request(im.from_agent_id, im.id)
                            .await
                    }
                    dialog::INVENTORY_OFFERED | dialog::TASK_INVENTORY_OFFERED => {
                        client
                            .answer_inventory_offer(im.from_agent_id, im.id, false, from_task)
                            .await
                    }
                    _ => Ok(()),
                }
            }
            Ok(why) => {
                log::info!("{what} from {from} accepted ({})", why.info());
                match im.dialog {
                    dialog::LURE_USER => {
                        // homebound does not bring the bot back from where it was sent
                        self.homebound.pause();
                        client.accept_lure(im.id).await.map(|region| {
                            log::info!("teleported to {region}");
                        })
                    }
                    dialog::TELEPORT_REQUEST => client.offer_teleport(im.from_agent_id, "").await,
                    // the group's id is the invitation's sender
                    dialog::GROUP_INVITATION => {
                        client
                            .answer_group_invite(im.from_agent_id, im.id, true)
                            .await
                    }
                    dialog::FRIENDSHIP_OFFERED => {
                        client.accept_friendship(im.from_agent_id, im.id).await
                    }
                    _ => {
                        client
                            .answer_inventory_offer(im.from_agent_id, im.id, true, from_task)
                            .await
                    }
                }
            }
        };
        if let Err(e) = done {
            log::warn!("{what} from {from}: {e}");
        }
    }

    /// ProcessRequest's event, then JsonOuputCleaner's when the request is accepted.
    async fn tell_offer(
        self: Arc<Self>,
        kind: Kind,
        im: InstantMessage,
        verdict: Result<Why, Refusal>,
    ) {
        let mut misc = misc(kind, &im, self.client.state().inventory_root);
        // a group invitation's sender is the group: no avatar's name to find
        let owner = if kind == Kind::GroupInvite {
            "lookup".to_owned()
        } else {
            self.name_of(im.from_agent_id).await
        };
        misc.push(("ObjectOwner", owner));
        let (accepted, info) = match verdict {
            Ok(why) => (true, why.info().to_owned()),
            Err(refusal) => (false, refusal.info()),
        };
        let event = interaction_event(&im.from_name, kind.event_type(), accepted, &info, &misc);
        self.tell(&event).await;
        if !accepted {
            return;
        }
        let event = match kind {
            Kind::Teleport => json_pairs(&[
                ("value", im.from_name.clone()),
                ("eventsource", teleport_mode(im.dialog).to_owned()),
            ]),
            Kind::Friendship => json_pairs(&[
                ("value", im.from_name.clone()),
                ("eventsource", kind.event_type().to_owned()),
            ]),
            Kind::GroupInvite | Kind::Inventory => {
                if im.dialog == dialog::INVENTORY_OFFERED {
                    let item = offered_item(&im.binary_bucket).unwrap_or_default();
                    misc.push(("itemuuid", item.to_string()));
                    misc.push(("itemname", im.message.clone()));
                }
                misc.push(("eventsource", kind.event_type().to_owned()));
                json_pairs(&misc)
            }
        };
        self.tell(&event).await;
    }

    /// BotInventoryAdd's event, for each item that came into the inventory.
    async fn on_items_added(&self, items: Vec<AddedItem>) {
        if self.events.is_none() {
            return;
        }
        for item in items {
            let event = json_pairs(&[
                ("itemname", item.name),
                ("itemuuid", item.id.to_string()),
                (
                    "itemtype",
                    asset_type_name(item.asset_type)
                        .map_or_else(|| item.asset_type.to_string(), str::to_owned),
                ),
                ("eventsource", "InventoryAdd".to_owned()),
            ]);
            self.tell(&event).await;
        }
    }
}

/// InteractionService's settings (SecondBot's InteractionConfig, `interaction_…`).
const SETTINGS: &[&str] = &[
    "AcceptTeleports",
    "TeleportRequestLevel",
    "AcceptGroupInvites",
    "GroupInviteLevel",
    "AcceptInventory",
    "InventoryTransferLevel",
    "AcceptFriendRequests",
    "FriendRequestLevel",
    "EnableJsonOutputEvents",
    "JsonOutputEventsTarget",
];

fn settings(s: &Source) -> Result<(Rules, Option<String>), ConfigError> {
    Ok((
        crate::config::interactions(s)?,
        crate::config::interaction_events(s)?,
    ))
}

/// InteractionService, for the service control (services/control.rs). Stopped, the bot leaves
/// every offer unanswered.
pub fn spec(
    client: Client,
    masters: Arc<Masters>,
    homebound: Arc<Homebound>,
    grants: Arc<Grants>,
    http: reqwest::Client,
    smtp: Arc<crate::services::smtp::Mailer>,
) -> Spec {
    Spec::new(
        "InteractionService",
        "interaction",
        SETTINGS,
        settings,
        move |(rules, events)| {
            let service = Arc::new(Service {
                client: client.clone(),
                masters: masters.clone(),
                rules,
                homebound: homebound.clone(),
                grants: grants.clone(),
                http: http.clone(),
                smtp: smtp.clone(),
                events,
            });
            vec![tokio::spawn(run(service, client.subscribe()))]
        },
    )
}

pub async fn run(service: Arc<Service>, mut events: Receiver<Event>) {
    loop {
        match events.recv().await {
            Ok(Event::InstantMessage(im)) => {
                tokio::spawn(service.clone().on_offer(im));
            }
            Ok(Event::InventoryAdded(items)) => {
                let s = service.clone();
                tokio::spawn(async move { s.on_items_added(items).await });
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("interactions: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_as_secondbot_names_them() {
        assert_eq!(Level::parse("Owner"), Some(Level::Owner));
        assert_eq!(Level::parse("anyone"), Some(Level::Anyone));
        assert_eq!(Level::parse(" Friends "), Some(Level::Friends));
        assert_eq!(Level::parse("everyone"), None);
    }

    #[test]
    fn the_fee_of_an_invitation() {
        let mut bucket = 50i32.to_be_bytes().to_vec();
        bucket.extend_from_slice(&[0; 16]);
        assert_eq!(fee(&bucket), Some(50));
        assert_eq!(fee(&[0; 20]), Some(0));
        assert_eq!(fee(&[0]), None);
    }

    fn offer(dialog: u8, bucket: Vec<u8>) -> InstantMessage {
        InstantMessage {
            from_agent_id: Uuid::from_u128(1),
            from_name: "Some Resident".into(),
            dialog,
            id: Uuid::from_u128(2),
            message: "A notecard".into(),
            binary_bucket: bucket,
            to_agent_id: Uuid::nil(),
            from_group: false,
            offline: false,
            timestamp: 0,
            region_id: Uuid::nil(),
            position: glam::Vec3::ZERO,
        }
    }

    #[test]
    fn the_event_as_secondbot_writes_it() {
        let im = offer(dialog::LURE_USER, Vec::new());
        let mut m = misc(Kind::Teleport, &im, Uuid::nil());
        m.push(("ObjectOwner", "Some Resident".into()));
        assert_eq!(
            interaction_event(&im.from_name, "Teleport", true, Why::Master.info(), &m),
            r#"{"from":"Some Resident","eventType":"Teleport","accepted":true,"info":"on owners list","misc":{"mode":"Teleport Bot to Av","ObjectOwner":"Some Resident"}}"#
        );
        let im = offer(dialog::TELEPORT_REQUEST, Vec::new());
        assert_eq!(
            misc(Kind::Teleport, &im, Uuid::nil()),
            vec![("mode", "Teleport Av to Bot".to_owned())]
        );
        assert_eq!(Refusal::NotFriend.info(), "rejected request");
        assert_eq!(Refusal::Disabled.info(), "disabled by config");
    }

    #[test]
    fn an_inventory_offers_misc() {
        let mut bucket = vec![7u8];
        bucket.extend_from_slice(Uuid::from_u128(3).as_bytes());
        let im = offer(dialog::INVENTORY_OFFERED, bucket.clone());
        assert_eq!(
            json_pairs(&misc(Kind::Inventory, &im, Uuid::from_u128(4))),
            r#"{"transactionid":"00000000-0000-0000-0000-000000000002","itemtype":"Notecard","message":"A notecard","fromscript":"False","targetfolder":"00000000-0000-0000-0000-000000000004"}"#
        );
        assert_eq!(offered_item(&bucket), Some(Uuid::from_u128(3)));
        assert_eq!(offered_item(&[7]), None);
        let im = offer(dialog::TASK_INVENTORY_OFFERED, vec![]);
        let m = misc(Kind::Inventory, &im, Uuid::nil());
        assert_eq!(m[1], ("itemtype", "-1".to_owned()));
        assert_eq!(m[3], ("fromscript", "True".to_owned()));
    }
}
