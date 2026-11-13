//! SecondBot's Notecard commands: notecards written from a collection kept in the key-value store
//! ("notecard_<collection>"), or straight from a text, and sent.
//!
//! Not here yet: NotecardRead (it needs the asset's download).

use super::reply::{Answer, basic, failure};
use super::{Ctx, avatar, owned};

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
