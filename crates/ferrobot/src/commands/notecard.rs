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
    match ctx.client.send_notecard(to, name, &content).await {
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
    match ctx.client.send_notecard(to, name, content).await {
        Ok(()) => basic(C, "ok", &[]),
        Err(e) => {
            log::warn!("NotecardDirectSend: {e}");
            failure(C, "Failed to create/send notecard", &args)
        }
    }
}
