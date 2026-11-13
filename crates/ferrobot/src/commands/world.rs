//! SecondBot's Movement (teleports), Friends, Avatars and Funds commands that need nothing in
//! world but the client.
//!
//! The Funds commands are off unless FERRO_ALLOW_FUNDS is on (SecondBot's AllowFundsCommands,
//! off by default too). Deliberate difference: Key2Name gives the avatar's name (SecondBot's
//! looks up the nil key).

use super::reply::{Answer, basic, failure, json};
use super::{Ctx, avatar, key, owned};
use serde::Serialize;
use std::collections::BTreeMap;

/// C#'s bool.ToString.
fn cs_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// A place as SecondBot's SimSlURL reads it: a SLurl or `Region/x/y/z`.
fn place(text: &str) -> Option<crate::services::homebound::Place> {
    let t = text
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("maps.secondlife.com/secondlife/");
    crate::services::homebound::Place::parse(t)
}

async fn teleport(ctx: &Ctx, to: &str) -> bool {
    let Some(p) = place(to) else {
        return false;
    };
    match ctx.client.teleport_to_region(&p.region, p.position).await {
        Ok(_) => {
            // homebound leaves the bot where a command sent it (SecondBot's MarkTeleport)
            ctx.homebound.pause();
            true
        }
        Err(e) => {
            log::warn!("teleport to {}: {e}", p.region);
            false
        }
    }
}

pub async fn teleport_to(ctx: &Ctx, region: &str, x: &str, y: &str, z: &str) -> Answer {
    const C: &str = "Teleport";
    let args = owned(&[region, x, y, z]);
    if teleport(ctx, &format!("{region}/{x}/{y}/{z}")).await {
        basic(C, "Accepted", &args)
    } else {
        failure(C, "Error Unable to Teleport to location", &args)
    }
}

pub async fn teleport_slurl(ctx: &Ctx, slurl: &str) -> Answer {
    const C: &str = "TeleportSLURL";
    let args = owned(&[slurl]);
    if slurl.is_empty() {
        return failure(C, "slurl is empty", &args);
    }
    basic(C, cs_bool(teleport(ctx, slurl).await), &args)
}

pub async fn send_teleport_lure(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "SendTeleportLure";
    let args = owned(&[who]);
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar UUID", &args);
    };
    match ctx.client.offer_teleport(to, "").await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub async fn request_teleport(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "RequestTeleport";
    let args = owned(&[who]);
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar UUID", &args);
    };
    match ctx
        .client
        .request_teleport(to, "I would like to teleport to you")
        .await
    {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub async fn key2name(ctx: &Ctx, uuid: &str) -> Answer {
    const C: &str = "Key2Name";
    let Some(id) = key(uuid) else {
        return failure(C, "Not a vaild UUID", &[]);
    };
    match ctx.client.name(id).await {
        Ok(Some(name)) => basic(C, &name, &[]),
        _ => basic(C, "lookup", &[]),
    }
}

pub async fn display_name(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "GetDisplayName";
    let Some(id) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar uuid", &owned(&[who]));
    };
    match ctx.client.display_name(id).await {
        Ok(Some(d)) => basic(C, &d.display_name, &[]),
        _ => basic(C, "?", &[]),
    }
}

/// Nearme: `{key: name}` of the avatars in the region.
pub async fn nearme(ctx: &Ctx) -> Answer {
    let st = ctx.client.state();
    if !st.online {
        return basic("Nearme", "Error not in a sim", &[]);
    }
    let names = ctx.client.names(&st.nearby).await.unwrap_or_default();
    let map: BTreeMap<String, String> = st
        .nearby
        .iter()
        .map(|id| (id.to_string(), names.get(id).cloned().unwrap_or_default()))
        .collect();
    basic("Nearme", &json(&map), &[])
}

/// SecondBot's FriendListEntry, its fields in their order.
#[derive(Serialize)]
struct Friend {
    name: String,
    id: String,
    online: bool,
}

pub async fn friends_list(ctx: &Ctx) -> Answer {
    let st = ctx.client.state();
    let names = ctx.client.names(&st.friends).await.unwrap_or_default();
    let list: Vec<Friend> = st
        .friends
        .iter()
        .map(|id| Friend {
            name: names.get(id).cloned().unwrap_or_default(),
            id: id.to_string(),
            online: st.friends_online.contains(id),
        })
        .collect();
    basic("Friendslist", &json(&list), &[])
}

/// FriendRequest: "true" offers friendship, "false" ends it.
pub async fn friend_request(ctx: &Ctx, who: &str, state: &str) -> Answer {
    const C: &str = "FriendRequest";
    let args = owned(&[who, state]);
    let Some(id) = avatar(&ctx.client, who).await else {
        return failure(C, "avatar lookup", &args);
    };
    let Ok(wanted) = state.trim().to_ascii_lowercase().parse::<bool>() else {
        return failure(C, "state invaild", &args);
    };
    let friend = ctx.client.state().friends.contains(&id);
    if friend && !wanted {
        return match ctx.client.end_friendship(id).await {
            Ok(()) => basic(C, "Removed", &args),
            Err(e) => failure(C, &e.to_string(), &args),
        };
    }
    if !wanted {
        return failure(C, "Not in friendslist", &args);
    }
    match ctx.client.offer_friendship(id).await {
        Ok(()) => basic(C, "Request sent", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

/// FriendFullPerms: online, map and modify rights given ("true") or taken.
pub async fn friend_full_perms(ctx: &Ctx, who: &str, state: &str) -> Answer {
    const C: &str = "FriendFullPerms";
    let args = owned(&[who, state]);
    let Some(id) = avatar(&ctx.client, who).await else {
        return failure(C, "avatar lookup", &args);
    };
    let Ok(grant) = state.trim().to_ascii_lowercase().parse::<bool>() else {
        return failure(C, "state invaild", &args);
    };
    if !ctx.client.state().friends.contains(&id) {
        return failure(C, "Not A friend", &args);
    }
    let rights = if grant { 1 | 2 | 4 } else { 0 };
    match ctx.client.grant_rights(id, rights).await {
        Ok(()) => basic(C, if grant { "granted" } else { "removed" }, &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub async fn balance(ctx: &Ctx) -> Answer {
    const C: &str = "Balance";
    if !ctx.allow_funds {
        return failure(C, "Funds commands are disabled", &[]);
    }
    match ctx.client.balance().await {
        Ok(b) => basic(C, &b.to_string(), &[]),
        Err(e) => failure(C, &e.to_string(), &[]),
    }
}

pub async fn pay_avatar(ctx: &Ctx, who: &str, amount: &str) -> Answer {
    const C: &str = "PayAvatar";
    let args = owned(&[who, amount]);
    if !ctx.allow_funds {
        return failure(C, "Transfer funds to avatars disabled", &args);
    }
    let Some(to) = avatar(&ctx.client, who).await else {
        // SecondBot answers this one as a reply, not a failure
        return basic(C, "avatar lookup", &args);
    };
    let Ok(value) = amount.trim().parse::<i32>() else {
        return failure(C, "Invaild amount", &args);
    };
    let balance = ctx.client.balance().await.unwrap_or(0);
    if value < 0 || value > balance {
        return failure(
            C,
            &format!("Amount out of range current max: {balance}"),
            &args,
        );
    }
    log::info!("paying L${value} to {to}");
    match ctx.client.pay(to, value, None).await {
        Ok(()) => basic(C, "Accepted", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub async fn pay_object(ctx: &Ctx, object: &str, name: &str, amount: &str) -> Answer {
    const C: &str = "PayObject";
    let args = owned(&[object, name, amount]);
    if !ctx.allow_funds {
        return failure(C, "Funds commands are disabled", &args);
    }
    let Some(id) = key(object) else {
        return failure(C, "Invaild object UUID", &args);
    };
    let Ok(value) = amount.trim().parse::<i32>() else {
        return failure(C, "Invaild amount", &args);
    };
    let balance = ctx.client.balance().await.unwrap_or(0);
    if value < 0 || value > balance {
        return failure(C, "Amount out of range", &args);
    }
    if name.is_empty() {
        return failure(C, "Primname is empty", &args);
    }
    log::info!("paying L${value} to the object {name}");
    match ctx.client.pay(id, value, Some(name)).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}
