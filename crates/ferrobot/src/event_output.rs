//! Where SecondBot's events go out, and in what form (its EventsService: PushEvent, OnEvent,
//! InventoryUpdateEvent). Shared by the inventory updates (crate::inventory_updates) and the
//! events service (crate::services::events).
//!
//! A target is an avatar (the event by IM), a channel of local chat, or an http(s) address: a
//! form POST of `token`, `unixtime`, `method`, `action`, `botname` and `event` to
//! `<address>/<resource>`, the token being sha1(unixtime + "EventService" + the bot's key + the
//! event + the shared secret).

use crate::api::signing::sha1_hex;
use crate::commands::reply::json;
use crate::smart_reply::form_encode;
use ferro_client::Client;
use ferro_client::events::chat_type;
use std::collections::HashSet;
use uuid::Uuid;

/// What a POST is sent as: the resource joined to the address, and its `method` and `action`.
#[derive(Debug, Clone, Copy)]
pub struct Post {
    pub resource: &'static str,
    pub method: &'static str,
    pub action: &'static str,
}

/// InventoryUpdateEvent's POST.
pub const INVENTORY_UPDATE: Post = Post {
    resource: "Inventory/Update",
    method: "Inventory",
    action: "Update",
};

/// PushEvent's POST: SecondBot sends its events as "Dialog" "Relay" (sic), and so does Ferrobot,
/// for the receivers written for it.
pub const EVENT_SERVICE: Post = Post {
    resource: "Event/Service",
    method: "Dialog",
    action: "Relay",
};

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The POST's token: sha1(unixtime + "EventService" + the bot's key + the event + the secret).
pub fn token(unixtime: i64, bot: Uuid, output: &str, secret: &str) -> String {
    sha1_hex(&format!("{unixtime}EventService{bot}{output}{secret}"))
}

/// The event by IM, unless the key is nil.
pub async fn to_avatar(client: &Client, avatar: Uuid, output: &str) {
    if !avatar.is_nil()
        && let Err(e) = client.im(avatar, output).await
    {
        log::debug!("event by IM: {e}");
    }
}

/// The event in local chat, on a channel of 0 or more.
pub async fn to_channel(client: &Client, channel: i32, output: &str) {
    if channel >= 0
        && let Err(e) = client.chat(output, channel, chat_type::SAY).await
    {
        log::debug!("event in local chat: {e}");
    }
}

/// The event as a signed form POST to `<url>/<resource>`.
pub async fn to_http(
    client: &Client,
    http: &reqwest::Client,
    secret: &str,
    url: &str,
    post: Post,
    output: &str,
) {
    let (bot, name) = {
        let st = client.state();
        (st.agent_id, st.name.clone())
    };
    let unixtime = now();
    let body = format!(
        "token={}&unixtime={unixtime}&method={}&action={}&botname={}&event={}",
        form_encode(&token(unixtime, bot, output, secret)),
        post.method,
        post.action,
        form_encode(&name),
        form_encode(output)
    );
    // RestSharp's base address joined with the resource
    let url = format!("{}/{}", url.trim_end_matches('/'), post.resource);
    let sent = http
        .post(&url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .timeout(std::time::Duration::from_secs(30))
        .body(body)
        .send()
        .await;
    // the address may carry a token of the receiver's: never logged
    if let Err(e) = sent {
        log::warn!(
            "{} {} event by HTTP failed: {}",
            post.method,
            post.action,
            e.without_url()
        );
    }
}

/// One target as SetInventoryUpdate names it: an avatar's key, an address starting with
/// "http", or a channel.
pub async fn to_target(
    client: &Client,
    http: &reqwest::Client,
    secret: &str,
    target: &str,
    post: Post,
    output: &str,
) {
    if let Ok(avatar) = Uuid::parse_str(target.trim()) {
        to_avatar(client, avatar, output).await;
    } else if target.starts_with("http") {
        to_http(client, http, secret, target, post, output).await;
    } else if let Ok(channel) = target.trim().parse::<i32>() {
        to_channel(client, channel, output).await;
    }
}

/// SecondBot's OnEvent, as System.Text.Json writes it (its fields in order:
/// `{"EventName":…,"Data":{…},"Hash":…,"Unixtime":…,"BotUUID":…}`). The hash is
/// sha1(BotUUID + EventName + every value of Data in order + Unixtime + the output secret); a
/// missing value (null) counts as empty, as C#'s string concatenation reads it.
pub fn on_event(
    bot: Uuid,
    name: &str,
    data: &[(&str, Option<String>)],
    unixtime: i64,
    secret: &str,
) -> String {
    let bot = bot.to_string();
    let values: String = data.iter().filter_map(|(_, v)| v.as_deref()).collect();
    let hash = sha1_hex(&format!("{bot}{name}{values}{unixtime}{secret}"));
    let data: Vec<String> = data
        .iter()
        .map(|(k, v)| format!("{}:{}", json(k), json(v)))
        .collect();
    format!(
        r#"{{"EventName":{},"Data":{{{}}},"Hash":{},"Unixtime":{unixtime},"BotUUID":{}}}"#,
        json(&name),
        data.join(","),
        json(&hash),
        json(&bot)
    )
}

/// Who is in a list now (a group's members, the avatars in a region), against the list before:
/// who joined and who left. Nothing the first time, when the list is only learned (SecondBot's
/// GroupLoaded and GuestListLoaded).
#[derive(Debug, Default)]
pub struct Roster {
    known: Option<Vec<Uuid>>,
}

impl Roster {
    /// The new list; who joined (in its order) and who left (in the old list's order).
    pub fn update(&mut self, now: Vec<Uuid>) -> (Vec<Uuid>, Vec<Uuid>) {
        let changes = match &self.known {
            None => (Vec::new(), Vec::new()),
            Some(before) => {
                // sets: a big group's lists, compared item by item, would be quadratic
                let old: HashSet<Uuid> = before.iter().copied().collect();
                let new: HashSet<Uuid> = now.iter().copied().collect();
                let joined = now.iter().filter(|id| !old.contains(id)).copied();
                let left = before.iter().filter(|id| !new.contains(id)).copied();
                (dedup(joined), dedup(left))
            }
        };
        self.known = Some(now);
        changes
    }

    /// Forget the list: the next one is learned again.
    pub fn reset(&mut self) {
        self.known = None;
    }
}

fn dedup(ids: impl Iterator<Item = Uuid>) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    ids.filter(|id| seen.insert(*id)).collect()
}

/// An avatar's name, as SecondBot's DataStoreService.GetAvatarName: "lookup" when the grid does
/// not know it (or it is no avatar).
pub async fn avatar_name(client: &Client, id: Uuid) -> String {
    if id.is_nil() {
        return "lookup".into();
    }
    match client.name(id).await {
        Ok(Some(name)) => name,
        _ => "lookup".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_on_event_json_and_its_hash() {
        let bot = Uuid::from_u128(1);
        let out = on_event(
            bot,
            "ChangedSim",
            &[
                ("lastsim", Some("A".into())),
                ("newsim", Some("B \"x\"".into())),
            ],
            1_800_000_000,
            "s",
        );
        // python3 -c "import hashlib;print(hashlib.sha1(b'00000000-0000-0000-0000-000000000001'
        //   b'ChangedSimAB \"x\"1800000000s').hexdigest())"
        let hash = sha1_hex("00000000-0000-0000-0000-000000000001ChangedSimAB \"x\"1800000000s");
        assert_eq!(
            out,
            format!(
                r#"{{"EventName":"ChangedSim","Data":{{"lastsim":"A","newsim":"B \"x\""}},"Hash":"{hash}","Unixtime":1800000000,"BotUUID":"00000000-0000-0000-0000-000000000001"}}"#
            )
        );
        let out = on_event(bot, "AlertMessage", &[("NotificationId", None)], 5, "");
        assert!(out.contains(r#""Data":{"NotificationId":null}"#), "{out}");
    }

    #[test]
    fn the_token_is_secondbots() {
        assert_eq!(
            token(1_800_000_000, Uuid::from_u128(1), "{}", "secret"),
            sha1_hex("1800000000EventService00000000-0000-0000-0000-000000000001{}secret")
        );
    }

    #[test]
    fn a_roster_tells_who_joined_and_left() {
        let (a, b, c) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        let mut r = Roster::default();
        assert_eq!(r.update(vec![a, b]), (vec![], vec![]), "learned, not told");
        assert_eq!(r.update(vec![b, c, c]), (vec![c], vec![a]));
        assert_eq!(r.update(vec![b, c]), (vec![], vec![]));
        r.reset();
        assert_eq!(r.update(vec![a]), (vec![], vec![]));
    }
}
