//! SecondBot's inventory update events (EventsService.InventoryUpdateEvent, fed by
//! InteractionService.BotInventoryAdd): the targets SetInventoryUpdate named for a type of item
//! hear of each item of that type that comes into the inventory, as
//! `{"itemname":…,"itemuuid":…,"itemtype":…}`. A target is an avatar's key (by IM), an http(s)
//! address (a signed form POST to `<address>/Inventory/Update`) or a channel (in local chat).
//!
//! Deliberate differences: the watch list is always on (SecondBot's needs its events service),
//! and a type may be named as SecondBot's hints name it ("Callcard", "Lslbyte") as well as by
//! LibreMetaverse's name; both mean the same type.

use crate::api::signing::sign_with;
use crate::commands::reply::json_pairs;
use crate::smart_reply::form_encode;
use ferro_client::events::{AddedItem, chat_type};
use ferro_client::{Client, Event};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

/// LibreMetaverse's AssetType name of a type (what SecondBot's events name it).
pub fn asset_type_name(t: i32) -> Option<&'static str> {
    Some(match t {
        0 => "Texture",
        1 => "Sound",
        2 => "CallingCard",
        3 => "Landmark",
        4 => "Script",
        5 => "Clothing",
        6 => "Object",
        7 => "Notecard",
        8 => "Folder",
        10 => "LSLText",
        11 => "LSLBytecode",
        12 => "TextureTGA",
        13 => "Bodypart",
        17 => "SoundWAV",
        18 => "ImageTGA",
        19 => "ImageJPEG",
        20 => "Animation",
        21 => "Gesture",
        22 => "Simstate",
        24 => "Link",
        25 => "LinkFolder",
        40 => "Widget",
        45 => "Person",
        49 => "Mesh",
        56 => "Settings",
        57 => "Material",
        _ => return None,
    })
}

/// SecondBot's `ToLower().FirstCharToUpper()`: "LSLText" is "Lsltext".
pub fn event_type_name(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let mut chars = lower.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The targets, by the event's type name.
#[derive(Default)]
pub struct InventoryUpdates(Mutex<HashMap<String, Vec<String>>>);

impl InventoryUpdates {
    /// SecondBot's AddInventoryEvent: a target added once to a type.
    pub fn add(&self, type_name: &str, target: &str) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let targets = map.entry(type_name.to_owned()).or_default();
        if !targets.iter().any(|t| t == target) {
            targets.push(target.to_owned());
        }
    }

    fn targets(&self, type_name: &str) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(type_name)
            .cloned()
            .unwrap_or_default()
    }
}

/// The event's JSON, its fields in SecondBot's order.
fn event_json(item: &AddedItem, type_name: &str) -> String {
    json_pairs(&[
        ("itemname", item.name.clone()),
        ("itemuuid", item.id.to_string()),
        ("itemtype", type_name.to_owned()),
    ])
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

async fn send(client: &Client, http: &reqwest::Client, secret: &str, target: &str, output: &str) {
    if let Ok(avatar) = Uuid::parse_str(target.trim()) {
        if !avatar.is_nil() {
            let _ = client.im(avatar, output).await;
        }
        return;
    }
    if target.starts_with("http") {
        let st = client.state();
        let unixtime = now();
        // sha1(unixtime + "EventService" + the bot's key + the event + the secret)
        let token = sign_with(
            &format!("{unixtime}EventService{}{output}", st.agent_id),
            &[],
            None,
            secret,
        );
        let body = format!(
            "token={}&unixtime={unixtime}&method=Inventory&action=Update&botname={}&event={}",
            form_encode(&token),
            form_encode(&st.name),
            form_encode(output)
        );
        // RestSharp's base address joined with the resource
        let url = format!("{}/Inventory/Update", target.trim_end_matches('/'));
        let sent = http
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .timeout(std::time::Duration::from_secs(30))
            .body(body)
            .send()
            .await;
        if let Err(e) = sent {
            log::warn!("inventory update by HTTP failed: {}", e.without_url());
        }
        return;
    }
    if let Ok(channel) = target.trim().parse::<i32>()
        && channel >= 0
    {
        let _ = client.chat(output, channel, chat_type::SAY).await;
    }
}

/// Tells the targets of each item that comes into the inventory.
pub async fn run(
    client: Client,
    mut events: Receiver<Event>,
    updates: Arc<InventoryUpdates>,
    http: reqwest::Client,
    secret: Option<String>,
) {
    let secret = secret.unwrap_or_default();
    loop {
        match events.recv().await {
            Ok(Event::InventoryAdded(items)) => {
                for item in items {
                    let Some(name) = asset_type_name(item.asset_type) else {
                        continue;
                    };
                    let type_name = event_type_name(name);
                    let targets = updates.targets(&type_name);
                    if targets.is_empty() {
                        continue;
                    }
                    let output = event_json(&item, &type_name);
                    for target in targets {
                        send(&client, &http, &secret, &target, &output).await;
                    }
                }
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("inventory updates: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names_as_secondbot_writes_them() {
        assert_eq!(event_type_name("LSLText"), "Lsltext");
        assert_eq!(event_type_name("notecard"), "Notecard");
        assert_eq!(event_type_name(""), "");
        assert_eq!(
            asset_type_name(2).map(event_type_name),
            Some("Callingcard".into())
        );
        assert_eq!(asset_type_name(9), None);
    }

    #[test]
    fn targets_are_added_once() {
        let u = InventoryUpdates::default();
        u.add("Notecard", "7");
        u.add("Notecard", "7");
        u.add("Notecard", "http://example.invalid/");
        assert_eq!(u.targets("Notecard").len(), 2);
        assert!(u.targets("Object").is_empty());
    }

    #[test]
    fn the_event_in_order() {
        let item = AddedItem {
            id: Uuid::from_u128(1),
            name: "Note".into(),
            asset_type: 7,
        };
        assert_eq!(
            event_json(&item, "Notecard"),
            r#"{"itemname":"Note","itemuuid":"00000000-0000-0000-0000-000000000001","itemtype":"Notecard"}"#
        );
    }
}
