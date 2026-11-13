//! Relays, as SecondBot's RelayService: what the bot hears, sent on elsewhere. Each relay has a
//! source and a target:
//! - sources: `localchat` (filtered by `avatar`, `object`, `system` or `ownersay`, or a
//!   sender's key), `avatar` (IMs, filtered by the sender's key), `object` (objects' IMs,
//!   filtered by the owner's key), `group` (group chat, filtered by the group's name or key);
//! - targets: `localchat` (a channel), `avatar` (a key, by IM), `group` (a key, its chat),
//!   `http` (a POST to `<url>/Relay/Service`, signed with the shared secret, as SecondBot's), and
//!   Ferrobot's `webhook` (a POST of `{"content": …}`, which Discord's webhooks take).
//!
//! The text goes out as SecondBot's JSON (`{name, message, sourcetype, sourceoption}`, or the
//! extended one), marked "{Relay}" so that a relayed line is never relayed again. SecondBot's
//! Discord bot and RabbitMQ targets are not here (TASKS.md).

use crate::api::signing::sign_with;
use ferro_client::events::{chat_type, dialog};
use ferro_client::{Client, Event};
use serde::Serialize;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relay {
    pub source_type: String,
    pub source_option: String,
    pub target_type: String,
    pub target_option: String,
    /// The extended JSON (SecondBot's `_extended`; always with the short form).
    pub extended: bool,
}

impl Relay {
    /// SecondBot's short form: `sType::localchat{}sFilter::avatar{}tType::http{}tConfig::<url>`.
    pub fn parse_short(s: &str) -> Option<Relay> {
        let mut found = std::collections::HashMap::new();
        for part in s.split("{}").filter(|p| !p.is_empty()) {
            if let Some((k, v)) = part.split_once("::")
                && !k.is_empty()
                && !v.is_empty()
            {
                found.entry(k.to_owned()).or_insert_with(|| v.to_owned());
            }
        }
        let mut get = |k: &str| found.remove(k);
        Some(Relay {
            source_type: get("sType")?,
            source_option: get("sFilter")?,
            target_type: get("tType")?,
            target_option: get("tConfig")?,
            extended: true,
        })
    }
}

/// relayEvent, its fields in their order.
#[derive(Serialize)]
struct Basic<'a> {
    name: &'a str,
    message: &'a str,
    sourcetype: &'a str,
    sourceoption: &'a str,
}

/// relayEventExtended, its fields in their order.
#[derive(Serialize)]
struct Extended<'a> {
    source: &'a str,
    sourceid: &'a str,
    sourcename: &'a str,
    triggerid: &'a str,
    triggername: &'a str,
    triggerdisplayname: &'a str,
    message: &'a str,
    unixtime: String,
}

/// One thing heard, as triggerRelayEvent takes it.
struct Heard {
    source: &'static str,
    filter: String,
    sourceuuid: String,
    triggername: String,
    message: String,
    sourcename: String,
    triggerdisplayname: String,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn form_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_owned(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// EChatType's names as SecondBot writes them (ChatType.ToString().ToLower()).
fn chat_name(t: u8) -> &'static str {
    match t {
        chat_type::WHISPER => "whisper",
        chat_type::SHOUT => "shout",
        chat_type::OWNER_SAY => "ownersay",
        9 => "regionsayto",
        _ => "normal",
    }
}

async fn relay(client: &Client, http: &reqwest::Client, relays: &[Relay], secret: &str, h: Heard) {
    if h.message.contains("{Relay}") {
        return;
    }
    let message = format!("{{Relay}} {}", h.message);
    let basic = serde_json::to_string(&Basic {
        name: &h.triggername,
        message: &message,
        sourcetype: h.source,
        sourceoption: &h.filter,
    })
    .unwrap_or_default();
    let extended = serde_json::to_string(&Extended {
        source: h.source,
        sourceid: &h.filter,
        sourcename: &h.sourcename,
        triggerid: &h.sourceuuid,
        triggername: &h.triggername,
        triggerdisplayname: &h.triggerdisplayname,
        message: &message,
        unixtime: now().to_string(),
    })
    .unwrap_or_default();
    for r in relays {
        if r.source_type != h.source
            || (!r.source_option.eq_ignore_ascii_case(&h.filter) && r.source_option != h.sourceuuid)
        {
            continue;
        }
        let event = if r.extended { &extended } else { &basic };
        let target = r.target_option.trim();
        let done = match r.target_type.as_str() {
            "localchat" => match target.parse::<i32>() {
                Ok(ch) if ch >= 0 => client
                    .chat(event, ch, chat_type::SAY)
                    .await
                    .map_err(|e| e.to_string()),
                _ => Err("not a channel".into()),
            },
            "avatar" => match Uuid::parse_str(target) {
                Ok(id) => client.im(id, event).await.map_err(|e| e.to_string()),
                Err(_) => Err("not a key".into()),
            },
            "group" => match Uuid::parse_str(target) {
                Ok(id) => client
                    .group_chat_send(id, event)
                    .await
                    .map_err(|e| e.to_string()),
                Err(_) => Err("not a key".into()),
            },
            "http" => {
                let unixtime = now();
                let st = client.state();
                // sha1(unixtime + "RelayService" + the bot's key + the basic event + the secret)
                let token = sign_with(
                    &format!("{unixtime}RelayService{}{basic}", st.agent_id),
                    &[],
                    None,
                    secret,
                );
                let body = format!(
                    "token={}&unixtime={unixtime}&method=Relay&action=Service&botname={}&event={}",
                    form_encode(&token),
                    form_encode(&st.name),
                    form_encode(event)
                );
                post(
                    http,
                    &format!("{}/Relay/Service", target.trim_end_matches('/')),
                    "application/x-www-form-urlencoded",
                    body,
                )
                .await
            }
            "webhook" => {
                let content = format!("**{}**: {}", h.triggername, h.message);
                let body = serde_json::json!({ "content": content }).to_string();
                post(http, target, "application/json", body).await
            }
            other => Err(format!("the target {other} is not supported")),
        };
        if let Err(e) = done {
            log::warn!("relay {} -> {}: {e}", r.source_type, r.target_type);
        }
    }
}

async fn post(http: &reqwest::Client, url: &str, kind: &str, body: String) -> Result<(), String> {
    let resp = http
        .post(url)
        .header("Content-Type", kind)
        .timeout(std::time::Duration::from_secs(30))
        .body(body)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("HTTP {}", resp.status()))
    }
}

async fn display_name(client: &Client, id: Uuid) -> String {
    match client.display_name(id).await {
        Ok(Some(d)) => d.display_name,
        _ => id.to_string(),
    }
}

pub async fn run(client: Client, relays: Vec<Relay>, secret: Option<String>) {
    if relays.is_empty() {
        return;
    }
    log::info!("{} relays", relays.len());
    let http = reqwest::Client::new();
    let secret = secret.unwrap_or_default();
    let mut events = client.subscribe();
    loop {
        let heard = match events.recv().await {
            Ok(Event::Chat(c)) => {
                let spoken = matches!(
                    c.chat_type,
                    chat_type::WHISPER
                        | chat_type::SAY
                        | chat_type::SHOUT
                        | chat_type::OWNER_SAY
                        | 9
                );
                let lower = c.from_name.to_lowercase();
                if !spoken
                    || c.source_id == client.state().agent_id
                    || lower == "secondlife"
                    || lower == "second life"
                {
                    continue;
                }
                let filter = match (c.chat_type, c.source_type) {
                    (chat_type::OWNER_SAY, _) => "ownersay",
                    (_, 0) => "system",
                    (_, 2) => "object",
                    _ => "avatar",
                };
                let display = if filter == "avatar" {
                    display_name(&client, c.source_id).await
                } else {
                    c.source_id.to_string()
                };
                Heard {
                    source: "localchat",
                    filter: filter.into(),
                    sourceuuid: c.source_id.to_string(),
                    triggername: c.from_name,
                    message: c.message,
                    sourcename: chat_name(c.chat_type).into(),
                    triggerdisplayname: display,
                }
            }
            Ok(Event::InstantMessage(im)) => match im.dialog {
                dialog::FROM_TASK => Heard {
                    source: "object",
                    filter: im.from_agent_id.to_string(),
                    sourceuuid: im.id.to_string(),
                    triggername: im.from_name,
                    message: im.message,
                    sourcename: "MessageFromObject".into(),
                    triggerdisplayname: "objectim".into(),
                },
                dialog::NOTHING_SPECIAL if im.from_agent_id != client.state().agent_id => {
                    let display = display_name(&client, im.from_agent_id).await;
                    Heard {
                        source: "avatar",
                        filter: im.from_agent_id.to_string(),
                        sourceuuid: im.id.to_string(),
                        triggername: im.from_name,
                        message: im.message,
                        sourcename: "MessageFromAgent".into(),
                        triggerdisplayname: display,
                    }
                }
                _ => continue,
            },
            Ok(Event::GroupChat {
                group,
                from_id,
                from_name,
                message,
            }) => {
                if from_id == client.state().agent_id {
                    continue;
                }
                let name = client
                    .state()
                    .groups
                    .iter()
                    .find(|g| g.id == group)
                    .map_or_else(|| "?".to_owned(), |g| g.name.clone());
                let display = display_name(&client, from_id).await;
                Heard {
                    source: "group",
                    filter: name,
                    sourceuuid: group.to_string(),
                    triggername: from_name,
                    message,
                    sourcename: "SessionSend".into(),
                    triggerdisplayname: display,
                }
            }
            Ok(_) => continue,
            Err(RecvError::Lagged(n)) => {
                log::warn!("relays: {n} events missed");
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        relay(&client, &http, &relays, &secret, heard).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_short_form() {
        let r =
            Relay::parse_short("sType::localchat{}sFilter::avatar{}tType::localchat{}tConfig::9")
                .expect("a relay");
        assert_eq!(
            r,
            Relay {
                source_type: "localchat".into(),
                source_option: "avatar".into(),
                target_type: "localchat".into(),
                target_option: "9".into(),
                extended: true,
            }
        );
        assert!(Relay::parse_short("sType::localchat{}tType::http").is_none());
        // a URL keeps its own colons
        let r = Relay::parse_short(
            "sType::group{}sFilter::Test Group{}tType::http{}tConfig::https://example.invalid/x",
        )
        .expect("a relay");
        assert_eq!(r.target_option, "https://example.invalid/x");
    }
}
