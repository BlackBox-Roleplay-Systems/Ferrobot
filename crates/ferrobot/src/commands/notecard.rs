//! SecondBot's Notecard commands: notecards written from a collection kept in the key-value store
//! ("notecard_<collection>"), or straight from a text, and sent; and NotecardRead, whose text
//! goes to a reply target.
//!
//! Deliberate difference: NotecardRead puts "[ATTACHMENT]" where each embedded item is
//! (SecondBot's loop throws away the text before the last one).

use super::reply::{Answer, basic, failure};
use super::{Ctx, avatar, key as uuid_key, owned};
use serde::Serialize;

/// SecondBot's NotecardReadReply, its fields in their order.
#[derive(Serialize)]
struct ReadReply {
    uuid: String,
    name: String,
    content: String,
    status: bool,
}

/// INVTYPE_NOTECARD (llinventorytype.h)
const NOTECARD: i32 = 7;

/// NotecardRead: answers at once; the text, or why there is none, goes to the reply target.
pub async fn read(ctx: &Ctx, item: &str, target: &str) -> Answer {
    const C: &str = "NotecardRead";
    if target.trim().is_empty() {
        return failure(C, "replytarget not set", &[]);
    }
    let args = owned(&[item]);
    let mut reply = ReadReply {
        uuid: item.to_owned(),
        name: "?".into(),
        content: String::new(),
        status: false,
    };
    let refuse = |ctx: &Ctx, mut reply: ReadReply, why: &str| {
        reply.content = why.to_owned();
        let (ctx, target) = (ctx.clone(), target.to_owned());
        tokio::spawn(async move { smart(&ctx, &target, &reply).await });
        failure(C, why, &args)
    };
    let Some(id) = uuid_key(item) else {
        return refuse(ctx, reply, "Not a vaild UUID");
    };
    let found = match ctx.client.inventory_item(id).await {
        Ok(Some(i)) => i,
        _ => return refuse(ctx, reply, "Unable to find inventory item"),
    };
    if found.inv_type != NOTECARD {
        return refuse(ctx, reply, "Inventory item is not a notecard");
    }
    let (ctx, target) = (ctx.clone(), target.to_owned());
    tokio::spawn(async move {
        match ctx.client.read_notecard(&found, "[ATTACHMENT]").await {
            Ok(text) => {
                reply.content = text;
                reply.name = found.name.clone();
                reply.status = true;
            }
            Err(e) => {
                log::warn!("NotecardRead: {e}");
                reply.content = "!ERROR! - unable to read notecard".into();
            }
        }
        smart(&ctx, &target, &reply).await;
    });
    basic(C, "Making request to get data from notecard", &[])
}

async fn smart(ctx: &Ctx, target: &str, reply: &ReadReply) {
    let json = serde_json::to_string(reply).unwrap_or_default();
    crate::smart_reply::send(ctx, target, &json, "NotecardRead").await;
}

fn key(collection: &str) -> String {
    format!("notecard_{collection}")
}

pub fn add(ctx: &Ctx, collection: &str, content: &str) -> Answer {
    const C: &str = "NotecardAdd";
    let args = owned(&[collection, content]);
    if collection.is_empty() {
        return failure(C, "Collection value is empty", &args);
    }
    if content.is_empty() {
        return failure(C, "Content value is empty", &args);
    }
    let len = ctx.store.append(&key(collection), content);
    basic(C, "ok", &[collection.to_owned(), len.to_string()])
}

pub fn clear(ctx: &Ctx, collection: &str) -> Answer {
    if collection.is_empty() {
        return failure(
            "NotecardClear",
            "Collection value is empty",
            &owned(&[collection]),
        );
    }
    ctx.store.clear(&key(collection));
    basic("NotecardClear", "ok", &[])
}

/// NotecardSend: the collection written to a notecard and sent; the collection is cleared after.
pub async fn send(ctx: &Ctx, who: &str, collection: &str, name: &str) -> Answer {
    const C: &str = "NotecardSend";
    let args = owned(&[who, collection, name]);
    if collection.is_empty() {
        return failure(C, "Collection value is empty", &args);
    }
    if name.is_empty() {
        return failure(C, "Notecardname value is empty", &args);
    }
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar uuid", &args);
    };
    let content = ctx.store.get(&key(collection));
    if content.len() <= 3 {
        return failure(C, "No content in notecard storage ?", &args);
    }
    match ctx.client.send_notecard(to, name, "", &content).await {
        Ok(()) => {
            ctx.store.clear(&key(collection));
            basic(C, "ok", &[])
        }
        Err(e) => {
            log::warn!("NotecardSend: {e}");
            failure(
                C,
                "Failed to create/send notecard",
                &owned(&[who, &content, name]),
            )
        }
    }
}

/// SecondBot's NotecardEndpoint: what the StreamAdmin server answers.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct NotecardEndpoint {
    status: bool,
    message: Option<String>,
    #[serde(rename = "AvatarUUID")]
    avatar: Option<String>,
    #[serde(rename = "NotecardTitle")]
    title: Option<String>,
    #[serde(rename = "NotecardContent")]
    content: Option<String>,
}

/// C#'s HttpStatusCode name of a status ("NotFound"), its number when it has none.
fn status_name(status: u16) -> String {
    reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .map_or_else(
            || status.to_string(),
            |r| r.split([' ', '-']).collect::<String>(),
        )
}

/// FetchNextNotecard (SecondBot's StreamAdmin command): asks the server at `endpoint` for the
/// next notecard to send (a signed form POST to `<endpoint>sys.php/Notecard/Next`), then sends
/// it.
pub async fn fetch_next(ctx: &Ctx, endpoint: &str, code: &str) -> Answer {
    const C: &str = "FetchNextNotecard";
    let args = owned(&[endpoint, code]);
    if endpoint.is_empty() {
        return failure(C, "Endpoint is empty", &args);
    }
    if code.is_empty() {
        return failure(C, "Endpointcode is empty", &args);
    }
    let unixtime = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // sha1(unixtime + "NotecardNext" + the endpoint's code)
    let token = crate::api::signing::sha1_hex(&format!("{unixtime}NotecardNext{code}"));
    let body = format!(
        "token={}&unixtime={unixtime}&method=Notecard&action=Next",
        crate::smart_reply::form_encode(&token)
    );
    // RestSharp joins the base address and the resource with a slash
    let url = format!(
        "{}/Notecard/Next",
        format!("{endpoint}sys.php").trim_end_matches('/')
    );
    let sent = ctx
        .http
        .post(&url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .timeout(std::time::Duration::from_secs(30))
        .body(body)
        .send()
        .await;
    let resp = match sent {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            let why = format!("HTTP status code: {}", status_name(r.status().as_u16()));
            return failure(C, &why, &args);
        }
        // RestSharp's status when nothing answered
        Err(e) => {
            log::warn!("FetchNextNotecard: {}", e.without_url());
            return failure(C, "HTTP status code: 0", &args);
        }
    };
    let reply = match resp.text().await {
        Ok(t) => t,
        Err(e) => return failure(C, &format!("Error: {}", e.without_url()), &args),
    };
    let reply: NotecardEndpoint = match serde_json::from_str(&reply) {
        Ok(r) => r,
        Err(e) => return failure(C, &format!("Error: {e}"), &args),
    };
    if !reply.status {
        let why = format!("Bad reply: {}", reply.message.unwrap_or_default());
        return failure(C, &why, &args);
    }
    let title = reply.title.unwrap_or_default();
    if title.chars().count() < 3 {
        return failure(C, "Notecard title is to short", &args);
    }
    let who = reply.avatar.unwrap_or_default();
    let Some(to) = avatar(&ctx.client, &who).await else {
        return failure(C, "Unable to unpack avatar", &args);
    };
    let content = reply.content.unwrap_or_default();
    match ctx.client.send_notecard(to, &title, "", &content).await {
        Ok(()) => basic(C, "ok", &[]),
        Err(e) => {
            log::warn!("FetchNextNotecard: {e}");
            failure(C, "Failed to create/send notecard", &args)
        }
    }
}

pub async fn direct_send(ctx: &Ctx, who: &str, content: &str, name: &str) -> Answer {
    const C: &str = "NotecardDirectSend";
    let args = owned(&[who, content, name]);
    if name.is_empty() {
        return failure(C, "notecardname value is empty", &args);
    }
    if content.is_empty() {
        return failure(C, "content value is empty", &args);
    }
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar uuid", &args);
    };
    match ctx.client.send_notecard(to, name, "", content).await {
        Ok(()) => basic(C, "ok", &[]),
        Err(e) => {
            log::warn!("NotecardDirectSend: {e}");
            failure(C, "Failed to create/send notecard", &args)
        }
    }
}

#[cfg(test)]
mod fetch_tests {
    use super::*;

    #[test]
    fn status_names_as_csharp_writes_them() {
        assert_eq!(status_name(404), "NotFound");
        assert_eq!(status_name(500), "InternalServerError");
        assert_eq!(status_name(599), "599");
    }

    #[test]
    fn the_endpoint_answer() {
        let r: NotecardEndpoint = serde_json::from_str(
            r#"{"status":true,"message":"ok","AvatarUUID":"x","NotecardTitle":"Card","NotecardContent":"text"}"#,
        )
        .expect("json");
        assert!(r.status);
        assert_eq!(r.title.as_deref(), Some("Card"));
        let r: NotecardEndpoint = serde_json::from_str("{}").expect("json");
        assert!(!r.status);
    }
}
