//! SecondBot's ParcelCommands, on the parcel the bot stands on (its SetupCurrentParcel): what the
//! region told of it, asked again when the bot has nothing yet. Changes go to the region as the
//! whole parcel with what changed (the ParcelPropertiesUpdate capability); the region refuses
//! what the bot may not do.
//!
//! Deliberate differences: SetParcelFlag skips a value that is not true or false (SecondBot's
//! Convert.ToBoolean throws, and the command fails); ParcelSetMedia keeps a size between 256 and
//! 1024 (SecondBot's check lets any through); ReturnAllObjectsOnParcelByOwner returns that
//! owner's objects (SecondBot's returns everyone else's); bans never end (SecondBot's end in 3030,
//! which overflows its time and sends garbage).

use super::reply::{Answer, basic, cs_bool_text, cs_float, failure, json_pairs};
use super::{Ctx, avatar, cs_bool, key, owned};
use ferro_client::ParcelAction;
use ferro_client::parcels::{AL_ACCESS, AL_BAN, AccessEntry, FLAG_NAMES, PF_FOR_SALE, ParcelInfo};
use serde::Serialize;
use std::time::Duration;

/// SecondBot waits two seconds for parcel data it does not have yet.
const PARCEL_WAIT: Duration = Duration::from_secs(2);

/// The bot's parcel, or the failure SecondBot answers.
async fn current(ctx: &Ctx, command: &str) -> Result<ParcelInfo, Answer> {
    current_with(ctx, command, &[]).await
}

/// The same, the failure with the command's arguments.
pub(super) async fn current_with(
    ctx: &Ctx,
    command: &str,
    args: &[String],
) -> Result<ParcelInfo, Answer> {
    if !ctx.client.state().online {
        return Err(failure(command, "Error not in a sim", args));
    }
    match ctx.client.parcel(PARCEL_WAIT).await {
        Ok(Some(p)) => Ok(p),
        _ => Err(failure(command, "Parcel data not ready", args)),
    }
}

/// The parcel sent back changed; `ok` the answer when the region took it.
async fn send(ctx: &Ctx, command: &str, p: ParcelInfo, ok: &str, args: &[String]) -> Answer {
    match ctx.client.update_parcel(p).await {
        Ok(()) => basic(command, ok, args),
        Err(e) => failure(command, &e.to_string(), args),
    }
}

/// ParcelStatic.ParcelSetFlag.
fn set_flag(p: &mut ParcelInfo, flag: u32, on: bool) {
    if on {
        p.flags |= flag;
    } else {
        p.flags &= !flag;
    }
}

/// PF_FOR_SALE_OBJECTS
const PF_FOR_SALE_OBJECTS: u32 = 1 << 7;
/// LLParcel::L_LANDING_POINT
const LANDING_POINT: i32 = 1;

pub async fn set_name(ctx: &Ctx, name: &str) -> Answer {
    const C: &str = "SetParcelName";
    let args = owned(&[name]);
    let mut p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    if name.trim().is_empty() {
        return failure(C, "Parcel name is empty", &args);
    }
    p.name = name.to_owned();
    send(ctx, C, p, "ok", &args).await
}

pub async fn set_desc(ctx: &Ctx, desc: &str) -> Answer {
    const C: &str = "SetParcelDesc";
    let args = owned(&[desc]);
    let mut p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    p.desc = desc.to_owned();
    send(ctx, C, p, "ok", &args).await
}

/// SetParcelLandingZone: the landing point, whole metres, the landing mode to it.
pub async fn set_landing(ctx: &Ctx, x: &str, y: &str, z: &str) -> Answer {
    const C: &str = "SetParcelLandingZone";
    let args = owned(&[x, y, z]);
    let mut p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let mut at = [0.0f32; 3];
    for (i, (v, axis)) in [(x, "x"), (y, "y"), (z, "z")].into_iter().enumerate() {
        match v.trim().parse::<i32>() {
            Ok(n) => at[i] = n as f32,
            Err(_) => {
                let why = format!("Unable to process landing point {axis} value");
                return failure(C, &why, &args);
            }
        }
    }
    p.landing_type = LANDING_POINT;
    p.user_location = glam::Vec3::from_array(at);
    send(ctx, C, p, "ok", &args).await
}

/// SetParcelMusic: "True" (SecondBot's helper answers true whatever happens).
pub async fn set_music(ctx: &Ctx, url: &str) -> Answer {
    const C: &str = "SetParcelMusic";
    let args = owned(&[url]);
    let mut p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    if p.music_url == url {
        return basic(C, "True", &args);
    }
    p.music_url = url.to_owned();
    send(ctx, C, p, "True", &args).await
}

/// SetParcelSale: a price from 1 to 9999999 (0 when sold to someone), to an avatar or "none".
pub async fn set_sale(ctx: &Ctx, amount: &str, who: &str) -> Answer {
    const C: &str = "SetParcelSale";
    let args = owned(&[amount, who]);
    let mut p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let Ok(price) = amount.trim().parse::<i32>() else {
        return failure(C, "Invaild amount", &args);
    };
    let buyer = if who == "none" {
        None
    } else {
        avatar(&ctx.client, who).await
    };
    let min = if buyer.is_some() { 0 } else { 1 };
    if !(min..=9_999_999).contains(&price) {
        return failure(C, "Invaild amount", &args);
    }
    p.sale_price = price;
    p.auth_buyer = buyer.unwrap_or_default();
    set_flag(&mut p, PF_FOR_SALE, true);
    set_flag(&mut p, PF_FOR_SALE_OBJECTS, false);
    send(ctx, C, p, "ok", &args).await
}

pub async fn cancel_sale(ctx: &Ctx) -> Answer {
    const C: &str = "CancelParcelSale";
    let mut p = match current(ctx, C).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    set_flag(&mut p, PF_FOR_SALE, false);
    set_flag(&mut p, PF_FOR_SALE_OBJECTS, false);
    send(ctx, C, p, "ok", &[]).await
}

pub async fn set_snapshot(ctx: &Ctx, texture: &str) -> Answer {
    const C: &str = "SetParcelSnapshot";
    let args = owned(&[texture]);
    let mut p = match current(ctx, C).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let Some(id) = key(texture) else {
        return failure(C, "Invaild texture uuid", &args);
    };
    p.snapshot_id = id;
    send(ctx, C, p, "ok", &args).await
}

/// SetParcelAudioControls: who may play sounds; everyone overrides group only.
pub async fn set_audio(ctx: &Ctx, everyone: &str, group_only: &str) -> Answer {
    const C: &str = "SetParcelAudioControls";
    let args = owned(&[everyone, group_only]);
    let mut p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let Some(all) = cs_bool(everyone) else {
        return failure(C, "Unable to process everyone value", &args);
    };
    let Some(group) = cs_bool(group_only) else {
        return failure(C, "Unable to process grouponly value", &args);
    };
    p.any_av_sounds = all;
    p.group_av_sounds = group && !all;
    send(ctx, C, p, "ok", &args).await
}

/// SetParcelFlag: `Flag=True:::Other=False`, by LibreMetaverse's names.
pub async fn set_flags(ctx: &Ctx, data: &str) -> Answer {
    const C: &str = "SetParcelFlag";
    let args = owned(&[data]);
    let mut p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let mut set: Vec<(u32, bool)> = Vec::new();
    for part in data.split(":::") {
        let bits: Vec<&str> = part.split('=').filter(|b| !b.is_empty()).collect();
        let [name, value] = bits[..] else {
            log::warn!("{C}: flag {part} missing \"=\"");
            continue;
        };
        let Some((bit, _)) = FLAG_NAMES.iter().find(|(_, n)| *n == name) else {
            log::warn!("{C}: flag {name} is unknown");
            continue;
        };
        let Some(on) = cs_bool(value) else {
            log::warn!("{C}: unable to set flag {name} to {value}");
            continue;
        };
        set.push((*bit, on));
    }
    if set.is_empty() {
        return failure(C, "No accepted flags", &args);
    }
    // HasParcelPerm: group land needs the bot in the group; other land the region decides
    // (the bot's own, or an estate manager's)
    let allowed = {
        let st = ctx.client.state();
        !p.is_group_owned || st.groups.iter().any(|g| g.id == p.group_id)
    };
    if !allowed {
        return failure(C, "Incorrect perms to control parcel", &args);
    }
    for (bit, on) in set {
        set_flag(&mut p, bit, on);
    }
    send(ctx, C, p, "true", &args).await
}

/// ParcelSetMedia: `MediaURL=…:::MediaLoop=True`, the fields SecondBot's GetMediaList names.
pub async fn set_media(ctx: &Ctx, data: &str) -> Answer {
    const C: &str = "ParcelSetMedia";
    let args = owned(&[data]);
    let mut p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    for part in data.split(":::") {
        let Some((name, value)) = part.split_once('=').filter(|(_, v)| !v.contains('=')) else {
            continue;
        };
        let m = &mut p.media;
        match name {
            "MediaType" => {
                let mime = match value {
                    "IMG-PNG" => Some("image/png".to_owned()),
                    "IMG-JPG" => Some("image/jpeg".to_owned()),
                    "VID-MP4" => Some("video/mp4".to_owned()),
                    "VID-AVI" => Some("video/x-msvideo".to_owned()),
                    v => v.strip_prefix("Custom-").map(str::to_owned),
                };
                if let Some(mime) = mime {
                    m.mime = mime;
                }
            }
            "MediaWidth" | "MediaHeight" => {
                if let Ok(size) = value.trim().parse::<i32>()
                    && (256..=1024).contains(&size)
                {
                    if name == "MediaWidth" {
                        m.width = size;
                    } else {
                        m.height = size;
                    }
                }
            }
            "MediaID" => {
                if let Some(id) = key(value) {
                    m.media_id = id;
                }
            }
            "MediaURL" => p.media_url = value.to_owned(),
            "MediaDesc" => m.desc = value.to_owned(),
            "MediaAutoScale" => {
                if let Some(b) = cs_bool(value) {
                    m.auto_scale = b;
                }
            }
            "MediaLoop" => {
                if let Some(b) = cs_bool(value) {
                    m.looping = b;
                }
            }
            _ => {}
        }
    }
    send(ctx, C, p, "ok", &args).await
}

pub async fn current_id(ctx: &Ctx) -> Answer {
    const C: &str = "GetCurrentParcelId";
    match current(ctx, C).await {
        Ok(p) => basic(C, &p.local_id.to_string(), &[]),
        Err(a) => a,
    }
}

pub async fn current_uuid(ctx: &Ctx) -> Answer {
    const C: &str = "GetCurrentParcelUUID";
    if let Err(a) = current(ctx, C).await {
        return a;
    }
    match ctx.client.parcel_global_id().await {
        Ok(id) => basic(C, &id.to_string(), &[]),
        Err(e) => {
            log::debug!("{C}: {e}");
            failure(C, "Unable to get parcel uuid", &[])
        }
    }
}

/// The size of its bounding box, "width,height".
pub async fn current_size(ctx: &Ctx) -> Answer {
    const C: &str = "GetCurrentParcelSize";
    match current(ctx, C).await {
        Ok(p) => {
            let d = p.aabb_max - p.aabb_min;
            basic(C, &format!("{},{}", cs_float(d.x), cs_float(d.y)), &[])
        }
        Err(a) => a,
    }
}

pub async fn desc(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelDesc";
    match current(ctx, C).await {
        Ok(p) => basic(C, &p.desc, &[]),
        Err(a) => a,
    }
}

/// Every flag by LibreMetaverse's name, "True" or "False".
pub async fn flags(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelFlags";
    match current(ctx, C).await {
        Ok(p) => {
            let pairs: Vec<(&str, String)> = FLAG_NAMES
                .iter()
                .map(|(bit, name)| (*name, cs_bool_text(p.flags & bit != 0)))
                .collect();
            basic(C, &json_pairs(&pairs), &[])
        }
        Err(a) => a,
    }
}

pub async fn prim_counts(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelPrimCounts";
    match current(ctx, C).await {
        Ok(p) => basic(
            C,
            &json_pairs(&[
                ("TotalPrims", p.total_prims.to_string()),
                ("GroupPrims", p.group_prims.to_string()),
                ("OwnerPrims", p.owner_prims.to_string()),
                ("OtherPrims", p.other_prims.to_string()),
                ("MaxPrims", p.max_prims.to_string()),
                ("ParcelPrimBonus", cs_float(p.prim_bonus)),
            ]),
            &[],
        ),
        Err(a) => a,
    }
}

pub async fn sale_details(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelSaleDetails";
    match current(ctx, C).await {
        Ok(p) => basic(
            C,
            &json_pairs(&[
                ("ForSale", cs_bool_text(p.flags & PF_FOR_SALE != 0)),
                ("Cost", p.sale_price.to_string()),
                ("SaleTarget", p.auth_buyer.to_string()),
            ]),
            &[],
        ),
        Err(a) => a,
    }
}

/// The parcel's dwell, asked of the region (SecondBot prints what LibreMetaverse last heard,
/// 0 until something asked).
pub async fn traffic(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelTraffic";
    match current(ctx, C).await {
        Ok(p) => {
            let dwell = ctx.client.parcel_dwell(p.local_id).await.unwrap_or(0.0);
            basic(C, &cs_float(dwell), &[])
        }
        Err(a) => a,
    }
}

async fn act(ctx: &Ctx, command: &str, action: ParcelAction, args: &[String]) -> Answer {
    match ctx.client.parcel_act(action).await {
        Ok(()) => basic(command, "ok", args),
        Err(e) => failure(command, &e.to_string(), args),
    }
}

/// ParcelEject: off the parcel the avatar is on (the region knows which).
pub async fn eject(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "ParcelEject";
    let args = owned(&[who]);
    let Some(target) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar", &args);
    };
    act(ctx, C, ParcelAction::Eject { target, ban: false }, &args).await
}

pub async fn freeze(ctx: &Ctx, who: &str, state: &str) -> Answer {
    const C: &str = "ParcelFreeze";
    let args = owned(&[who, state]);
    let Some(on) = cs_bool(state) else {
        return failure(C, "Invaild state", &args);
    };
    let Some(target) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar UUID", &args);
    };
    act(ctx, C, ParcelAction::Freeze { target, freeze: on }, &args).await
}

/// The parcel's ban list as the region has it now.
async fn ban_list(ctx: &Ctx, p: &ParcelInfo) -> Result<Vec<AccessEntry>, String> {
    ctx.client
        .parcel_access_list(p.local_id, AL_BAN)
        .await
        .map_err(|e| e.to_string())
}

async fn send_bans(ctx: &Ctx, p: &ParcelInfo, entries: Vec<AccessEntry>) -> Result<(), String> {
    let action = ParcelAction::AccessList {
        local_id: p.local_id,
        flags: AL_BAN,
        entries,
    };
    ctx.client
        .parcel_act(action)
        .await
        .map_err(|e| e.to_string())
}

/// ParcelBan: the ban list sent back with the avatar in it, for good.
pub async fn ban(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "ParcelBan";
    let args = owned(&[who]);
    let p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let Some(target) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar", &args);
    };
    let mut list = match ban_list(ctx, &p).await {
        Ok(l) => l,
        Err(e) => return failure(C, &e, &args),
    };
    if list.iter().any(|e| e.id == target) {
        return basic(C, "Avatar is in the blacklist", &args);
    }
    list.push(AccessEntry {
        id: target,
        time: 0,
        flags: AL_BAN,
    });
    match send_bans(ctx, &p, list).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e, &args),
    }
}

pub async fn unban(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "ParcelUnBan";
    let args = owned(&[who]);
    let p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let Some(target) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar", &args);
    };
    let mut list = match ban_list(ctx, &p).await {
        Ok(l) => l,
        Err(e) => return failure(C, &e, &args),
    };
    if !list.iter().any(|e| e.id == target) {
        return basic(C, "Avatar is already unbanned", &args);
    }
    list.retain(|e| e.id != target);
    match send_bans(ctx, &p, list).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e, &args),
    }
}

/// SecondBot's GetParcelBanlistObject, its fields in their order.
#[derive(Serialize)]
struct BanList {
    entrys: std::collections::BTreeMap<String, String>,
    #[serde(rename = "parcelName")]
    parcel_name: String,
    #[serde(rename = "regionName")]
    region_name: String,
    delay: i32,
    #[serde(rename = "reportedEntrys")]
    reported_entrys: usize,
}

pub async fn get_ban_list(ctx: &Ctx) -> Answer {
    const C: &str = "GetParcelBanlist";
    let p = match current(ctx, C).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let list = match ban_list(ctx, &p).await {
        Ok(l) => l,
        Err(e) => return failure(C, &e, &[]),
    };
    let ids: Vec<uuid::Uuid> = list.iter().map(|e| e.id).collect();
    let names = ctx.client.names(&ids).await.unwrap_or_default();
    let value = BanList {
        entrys: list
            .iter()
            .map(|e| {
                let name = names.get(&e.id).cloned().unwrap_or_else(|| "lookup".into());
                (e.id.to_string(), name)
            })
            .collect(),
        parcel_name: p.name.clone(),
        region_name: ctx
            .client
            .state()
            .region
            .map(|r| r.name)
            .unwrap_or_default(),
        delay: 0,
        reported_entrys: list.len(),
    };
    basic(C, &super::reply::json(&value), &[])
}

/// ResetParcelAcccessLists (SecondBot's spelling): both lists emptied.
pub async fn reset_access_lists(ctx: &Ctx) -> Answer {
    const C: &str = "ResetParcelAcccessLists";
    let p = match current(ctx, C).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    for flags in [AL_ACCESS, AL_BAN] {
        let action = ParcelAction::AccessList {
            local_id: p.local_id,
            flags,
            entries: Vec::new(),
        };
        if let Err(e) = ctx.client.parcel_act(action).await {
            return failure(C, &e.to_string(), &[]);
        }
    }
    basic(C, "ok", &[])
}

/// How long SecondBot waits for the object owners.
const OWNERS_WAIT: Duration = Duration::from_secs(13);

/// SecondBot's ReturnAllObjectsOnParcelReal: on another's land the parcel's group must be the
/// bot's active one; the objects of every owner (or of one) sent back.
async fn return_objects(
    ctx: &Ctx,
    command: &str,
    p: &ParcelInfo,
    only: Option<uuid::Uuid>,
) -> Answer {
    let me = ctx.client.state().agent_id;
    if p.owner_id != me {
        if p.group_id.is_nil() {
            return failure(command, "Land is not group owned and im not the owner", &[]);
        }
        let active = |ctx: &Ctx| {
            ctx.client
                .state()
                .active_group
                .as_ref()
                .is_some_and(|g| g.id == p.group_id)
        };
        if !active(ctx) {
            let _ = ctx.client.group_activate(p.group_id).await;
            if !active(ctx) {
                return failure(command, "I do not have access to the group", &[]);
            }
        }
    }
    let owners = match tokio::time::timeout(
        OWNERS_WAIT,
        ctx.client.parcel_object_owners(p.local_id),
    )
    .await
    {
        Ok(Ok(o)) => o,
        _ => return failure(command, "Timeout waiting for object owners reply", &[]),
    };
    let mut ids: Vec<uuid::Uuid> = Vec::new();
    for o in owners {
        if !ids.contains(&o.id) && only.is_none_or(|t| t == o.id) {
            ids.push(o.id);
        }
    }
    if ids.is_empty() {
        return failure(command, "No objects found to return", &[]);
    }
    let action = ParcelAction::ReturnObjects {
        local_id: p.local_id,
        owners: ids,
    };
    act(ctx, command, action, &[]).await
}

pub async fn return_all(ctx: &Ctx) -> Answer {
    const C: &str = "ReturnAllObjectsOnParcel";
    match current(ctx, C).await {
        Ok(p) => return_objects(ctx, C, &p, None).await,
        Err(a) => a,
    }
}

pub async fn return_by_owner(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "ReturnAllObjectsOnParcelByOwner";
    let p = match current(ctx, C).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let Some(owner) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar", &owned(&[who]));
    };
    return_objects(ctx, C, &p, Some(owner)).await
}

/// PF_ALLOW_DEED_TO_GROUP
const PF_ALLOW_DEED_TO_GROUP: u32 = 1 << 13;

/// ParcelDeedToGroup: the parcel set to the group and allowed to be deeded, then deeded.
pub async fn deed_to_group(ctx: &Ctx, group: &str) -> Answer {
    const C: &str = "ParcelDeedToGroup";
    let args = owned(&[group]);
    let mut p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let Some(id) = key(group) else {
        return failure(C, "Invaild group uuid", &args);
    };
    if !ctx.client.state().groups.iter().any(|g| g.id == id) {
        return failure(C, "Not in group", &args);
    }
    set_flag(&mut p, PF_ALLOW_DEED_TO_GROUP, true);
    p.group_id = id;
    let local_id = p.local_id;
    if let Err(e) = ctx.client.update_parcel(p).await {
        return failure(C, &e.to_string(), &args);
    }
    // SecondBot gives the region half a second for the change
    tokio::time::sleep(Duration::from_millis(500)).await;
    act(
        ctx,
        C,
        ParcelAction::DeedToGroup {
            local_id,
            group: id,
        },
        &args,
    )
    .await
}

pub async fn abandon(ctx: &Ctx) -> Answer {
    const C: &str = "AbandonLand";
    match current(ctx, C).await {
        Ok(p) => {
            act(
                ctx,
                C,
                ParcelAction::Release {
                    local_id: p.local_id,
                },
                &[],
            )
            .await
        }
        Err(a) => a,
    }
}

/// ParcelBuy: only at its price, and only when the sale is not to someone else. It spends the
/// bot's money: off unless FERRO_ALLOW_FUNDS, as the pay commands (SecondBot does not gate it).
pub async fn buy(ctx: &Ctx, amount: &str) -> Answer {
    const C: &str = "ParcelBuy";
    let args = owned(&[amount]);
    if !ctx.allow_funds {
        return failure(C, "Funds commands are disabled", &args);
    }
    let p = match current_with(ctx, C, &args).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let me = ctx.client.state().agent_id;
    if !p.auth_buyer.is_nil() && p.auth_buyer != me {
        return failure(C, "Parcel sale locked to other avatars", &args);
    }
    let min = if p.auth_buyer == me { 0 } else { 1 };
    let Ok(price) = amount.trim().parse::<i32>() else {
        return failure(C, "Invaild amount", &args);
    };
    if price < min {
        return failure(C, "Invaild amount", &args);
    }
    if p.flags & PF_FOR_SALE == 0 {
        return failure(C, "Parcel not for sale", &args);
    }
    if p.sale_price != price {
        return failure(C, "Parcel sale price and amount do not match", &args);
    }
    let action = ParcelAction::Buy {
        local_id: p.local_id,
        price,
        area: p.area,
    };
    act(ctx, C, action, &args).await
}

/// EstateParcelReclaim: an estate manager takes the parcel back.
pub async fn reclaim(ctx: &Ctx) -> Answer {
    const C: &str = "EstateParcelReclaim";
    let manager = ctx.client.state().region.is_some_and(|r| r.estate_manager);
    if !manager {
        return failure(C, "Not an estate manager here", &[]);
    }
    match current(ctx, C).await {
        Ok(p) => {
            act(
                ctx,
                C,
                ParcelAction::Reclaim {
                    local_id: p.local_id,
                },
                &[],
            )
            .await
        }
        Err(a) => a,
    }
}
