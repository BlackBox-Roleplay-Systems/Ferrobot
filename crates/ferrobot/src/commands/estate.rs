//! SecondBot's Estate commands: estate managers' tools, as EstateOwnerMessage and the estate's
//! capabilities (LibreMetaverse's EstateTools, the viewer's LLFloaterRegionInfo, Aurora's
//! estate.rs).
//!
//! Deliberate differences: SetEstateConfig keeps the estate's other flags (SecondBot's sets them
//! from nothing) and sends the sun hour to the capability as the viewer does (SecondBot's times
//! 1024 there too). Not here yet: SetEstateTerrainRaw (a terrain upload by xfer).

use super::reply::{Answer, basic, failure, json};
use super::{Ctx, avatar, cs_bool, key, owned};
use ferro_client::estate::{
    RegionRules, access_delta_strings, estate_access as access, estate_change_body,
    estate_change_strings, estate_flags as flags, region_message_strings,
};
use serde::Serialize;
use std::time::Duration;

fn manager(ctx: &Ctx) -> bool {
    ctx.client.state().region.is_some_and(|r| r.estate_manager)
}

fn region_name(ctx: &Ctx) -> String {
    ctx.client
        .state()
        .region
        .map(|r| r.name)
        .unwrap_or_default()
}

async fn message(
    ctx: &Ctx,
    c: &str,
    method: &str,
    strings: Vec<String>,
    ok: &str,
    args: &[String],
) -> Answer {
    match ctx.client.estate_message(method, strings).await {
        Ok(()) => basic(c, ok, args),
        Err(e) => failure(c, &e.to_string(), args),
    }
}

pub async fn kick(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "EstateKickUser";
    let args = owned(&[who]);
    if !manager(ctx) {
        let why = format!("Not an estate manager on region {}", region_name(ctx));
        return failure(C, &why, &args);
    }
    let Some(id) = avatar(&ctx.client, who).await else {
        return failure(C, "avatar lookup", &args);
    };
    message(ctx, C, "kickestate", vec![id.to_string()], "ok", &args).await
}

pub async fn teleport_home(ctx: &Ctx, who: &str) -> Answer {
    const C: &str = "EstateTeleportHomeUser";
    let args = owned(&[who]);
    if !manager(ctx) {
        let why = format!("Not an estate manager on region {}", region_name(ctx));
        return failure(C, &why, &args);
    }
    let Some(id) = avatar(&ctx.client, who).await else {
        return failure(C, "avatar lookup", &args);
    };
    let me = ctx.client.state().agent_id;
    let strings = vec![me.to_string(), id.to_string()];
    message(ctx, C, "teleporthomeuser", strings, "ok", &args).await
}

/// AddEstateManager and RemoveEstateManager: an avatar's key, for this estate or all of them.
pub async fn manager_change(ctx: &Ctx, add: bool, who: &str, all: &str) -> Answer {
    let c = if add {
        "AddEstateManager"
    } else {
        "RemoveEstateManager"
    };
    if !manager(ctx) {
        return failure(c, "Not an estate manager here", &owned(&[who]));
    }
    let Some(all) = cs_bool(all) else {
        let why = "Unable to process all estates value please use true or false";
        return failure(c, why, &owned(&[who, all]));
    };
    let Some(id) = key(who) else {
        return failure(c, "Unable to find avatar UUID", &owned(&[who]));
    };
    let base = if add {
        access::MANAGER_ADD
    } else {
        access::MANAGER_REMOVE
    };
    let f = if all {
        base | access::APPLY_TO_ALL_ESTATES
    } else {
        base
    };
    let me = ctx.client.state().agent_id;
    let strings = access_delta_strings(me, f, id);
    message(ctx, c, "estateaccessdelta", strings, "ok", &owned(&[who])).await
}

/// UpdateEstateBanlist: "add" bans, anything else unbans; `global` for every estate the bot
/// manages.
pub async fn update_ban_list(ctx: &Ctx, who: &str, mode: &str, global: &str) -> Answer {
    const C: &str = "UpdateEstateBanlist";
    let args = owned(&[who, mode, global]);
    if !manager(ctx) {
        let why = format!("Not an estate manager on region {}", region_name(ctx));
        return failure(C, &why, &args);
    }
    let Some(id) = key(who) else {
        return failure(C, "Unable to find avatar UUID", &args);
    };
    let Some(global) = cs_bool(global) else {
        let why = "Unable to process global value please use true or false";
        return failure(C, why, &args);
    };
    let (base, ok) = if mode == "add" {
        (access::BANNED_AGENT_ADD, "Ban request accepted")
    } else {
        (access::BANNED_AGENT_REMOVE, "Unban request accepted")
    };
    let f = if global {
        base | access::APPLY_TO_MANAGED_ESTATES
    } else {
        base
    };
    let me = ctx.client.state().agent_id;
    message(
        ctx,
        C,
        "estateaccessdelta",
        access_delta_strings(me, f, id),
        ok,
        &args,
    )
    .await
}

/// GetEstateBanList: the banned avatars' keys (an empty list when the region does not say).
pub async fn ban_list(ctx: &Ctx) -> Answer {
    const C: &str = "GetEstateBanList";
    let banned = match ctx.client.estate_access().await {
        Ok(a) => a.banned,
        Err(e) => {
            log::warn!("{C}: {e}");
            Vec::new()
        }
    };
    let ids: Vec<String> = banned.iter().map(ToString::to_string).collect();
    basic(C, &json(&ids), &[])
}

/// SimRestart: a delay of 30 to 240 s (240 otherwise), or "false" cancels.
pub async fn restart(ctx: &Ctx, delay: &str, mode: &str) -> Answer {
    const C: &str = "SimRestart";
    let args = owned(&[delay, mode]);
    if !manager(ctx) {
        return failure(C, "Not an estate manager here", &args);
    }
    if cs_bool(mode) != Some(true) {
        return message(ctx, C, "restart", vec!["-1".into()], "canceled", &args).await;
    }
    let mut seconds = delay.trim().parse::<i32>().unwrap_or(0);
    if !(30..=240).contains(&seconds) {
        seconds = 240;
    }
    message(
        ctx,
        C,
        "restart",
        vec![seconds.to_string()],
        "restarting",
        &args,
    )
    .await
}

/// SimMessage: a message to everyone in the region.
pub async fn sim_message(ctx: &Ctx, text: &str) -> Answer {
    const C: &str = "SimMessage";
    let args = owned(&[text]);
    if text.trim().is_empty() {
        return failure(C, "Message empty", &args);
    }
    if !manager(ctx) {
        return failure(C, "Not an estate manager here", &args);
    }
    let (me, name) = {
        let st = ctx.client.state();
        (st.agent_id, st.name.clone())
    };
    let strings = region_message_strings(me, &name, text);
    message(ctx, C, "simulatormessage", strings, "ok", &args).await
}

/// SetEstateCovenant: the covenant set to a notecard of the bot's.
pub async fn set_covenant(ctx: &Ctx, item: &str) -> Answer {
    const C: &str = "SetEstateCovenant";
    let args = owned(&[item]);
    if !manager(ctx) {
        return failure(C, "Not an estate manager here", &args);
    }
    let Some(id) = key(item) else {
        return failure(C, "Invaild inventoryNotecardUUID", &args);
    };
    let Ok(Some(found)) = ctx.client.inventory_item(id).await else {
        return failure(C, "Unable to find notecard", &args);
    };
    if found.asset_type != 7 {
        return failure(C, "Item is not a notecard", &args);
    }
    let ok = format!("changed Covenant to notecard: {}", found.name);
    let strings = vec![found.asset_id.to_string()];
    message(ctx, C, "estatechangecovenantid", strings, &ok, &args).await
}

/// SecondBot's answer of GetEstateCovenant, its fields in their order.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct CovenantReply {
    fetched: bool,
    #[serde(rename = "CovenantID")]
    covenant_id: String,
    timestamp: String,
    estate_name: String,
    #[serde(rename = "EstateOwnerID")]
    estate_owner_id: String,
    covenant_text: String,
}

const COVENANT_WAIT: Duration = Duration::from_secs(10);

pub async fn get_covenant(ctx: &Ctx) -> Answer {
    const C: &str = "GetEstateCovenant";
    let c = match tokio::time::timeout(COVENANT_WAIT, ctx.client.estate_covenant()).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => return failure(C, &e.to_string(), &[]),
        Err(_) => return failure(C, "Timed out waiting for EstateCovenantReply", &[]),
    };
    let (fetched, text) = if c.id.is_nil() {
        (
            false,
            "There is no Covenant provided for this Estate".to_owned(),
        )
    } else {
        match tokio::time::timeout(COVENANT_WAIT, ctx.client.estate_covenant_text()).await {
            Ok(Ok(t)) => (true, t),
            Ok(Err(e)) => {
                log::warn!("{C}: {e}");
                (false, "!ERROR! - unable to read notecard".to_owned())
            }
            Err(_) => (
                false,
                "Timed out waiting for estate covenant asset".to_owned(),
            ),
        }
    };
    let reply = CovenantReply {
        fetched,
        covenant_id: c.id.to_string(),
        timestamp: c.timestamp.to_string(),
        estate_name: c.estate_name,
        estate_owner_id: c.estate_owner.to_string(),
        covenant_text: text,
    };
    basic(C, &json(&reply), &[])
}

/// SIM_ACCESS_PG, _MATURE, _ADULT (RegionMaturity).
const PG: u8 = 13;
const MATURE: u8 = 21;
const ADULT: u8 = 42;

/// SetExtendedRegionInfo's twelve values, in their order.
pub struct RegionValues<'a> {
    pub values: [&'a str; 12],
}

/// SetExtendedRegionInfo: the region's rules, by the DispatchRegionInfo capability, else
/// `setregioninfo` (LibreMetaverse's SetRegionInfoAsync). Each value is checked in turn; a
/// failure names the values up to the one that failed.
pub async fn set_region_info(ctx: &Ctx, command: &str, v: RegionValues<'_>) -> Answer {
    let all = v.values;
    let upto = |n: usize| owned(&all[..=n]);
    if !manager(ctx) {
        return failure(command, "Not an estate manager here", &[]);
    }
    const BOOLS: [(usize, &str); 10] = [
        (0, "block terraform"),
        (1, "block fly"),
        (2, "block fly over"),
        (3, "allow damage"),
        (4, "allow land resell"),
        (7, "mature"),
        (8, "adult"),
        (9, "block object push"),
        (10, "allow parcel changes"),
        (11, "block parcel search"),
    ];
    let mut b = [false; 12];
    let mut read = |range: std::ops::RangeInclusive<usize>| -> Result<(), Answer> {
        for (i, what) in BOOLS.iter().filter(|(i, _)| range.contains(i)) {
            match cs_bool(all[*i]) {
                Some(x) => b[*i] = x,
                None => {
                    let why = format!("Unable to process {what} value please use true or false");
                    return Err(failure(command, &why, &upto(*i)));
                }
            }
        }
        Ok(())
    };
    // in the order of the arguments: five switches, the two numbers, five switches
    if let Err(a) = read(0..=4) {
        return a;
    }
    let (agent_limit, prim_bonus) = match numbers(command, &all, &upto) {
        Ok(n) => n,
        Err(a) => return a,
    };
    if let Err(a) = read(7..=11) {
        return a;
    }
    let sim_access = if b[8] {
        ADULT
    } else if b[7] {
        MATURE
    } else {
        PG
    };
    let rules = RegionRules {
        block_terraform: b[0],
        block_fly: b[1],
        block_fly_over: b[2],
        allow_damage: b[3],
        allow_land_resell: b[4],
        restrict_pushobject: b[9],
        allow_parcel_changes: b[10],
        block_parcel_search: b[11],
        agent_limit: agent_limit as f32,
        object_bonus: prim_bonus,
        sim_access,
    };
    let args = owned(&all);
    if ctx
        .client
        .estate_post("DispatchRegionInfo", rules.body())
        .await
        .is_err()
    {
        log::info!("{command}: no DispatchRegionInfo, setregioninfo instead");
        return message(ctx, command, "setregioninfo", rules.strings(), "ok", &args).await;
    }
    basic(command, "ok", &args)
}

/// The agent limit (an int) and the prim bonus (a float) of SetExtendedRegionInfo.
fn numbers(
    command: &str,
    all: &[&str; 12],
    upto: &dyn Fn(usize) -> Vec<String>,
) -> Result<(i32, f32), Answer> {
    let Ok(limit) = all[5].trim().parse::<i32>() else {
        let why = "Unable to process agent limit value please use a number";
        return Err(failure(command, why, &upto(5)));
    };
    let Ok(bonus) = all[6].trim().parse::<f32>() else {
        let why = "Unable to process prim bonus value please use a number";
        return Err(failure(command, why, &upto(6)));
    };
    Ok((limit, bonus))
}

/// SetEstateConfig: the estate's settings, by the EstateChangeInfo capability, else
/// `estatechangeinfo`; the estate's name from its info.
pub async fn set_estate_config(ctx: &Ctx, v: [&str; 10]) -> Answer {
    const C: &str = "SetEstateConfig";
    if !manager(ctx) {
        return failure(C, "Not an estate manager here", &[]);
    }
    let Ok(sun) = v[0].trim().parse::<f32>() else {
        let why = "Unable to process sun hour value please use a number between 0 and 23";
        return failure(C, why, &owned(&v[..1]));
    };
    if !(0.0..=23.0).contains(&sun) {
        return failure(
            C,
            "Sun hour value must be between 0 and 23",
            &owned(&v[..1]),
        );
    }
    const NAMES: [(&str, u64); 9] = [
        ("is sun fixed", flags::SUN_FIXED),
        ("is externally visible", flags::EXTERNALLY_VISIBLE),
        ("allow direct teleport", flags::ALLOW_DIRECT_TELEPORT),
        ("deny anonymous", flags::DENY_ANONYMOUS),
        ("deny age unverified", flags::DENY_AGEUNVERIFIED),
        ("block bots", flags::DENY_BOTS),
        ("allow voice chat", flags::ALLOW_VOICE),
        ("override public access", flags::ALLOW_ACCESS_OVERRIDE),
        ("override environment", flags::ALLOW_ENVIRONMENT_OVERRIDE),
    ];
    let mut set = Vec::new();
    for (i, (what, flag)) in NAMES.iter().enumerate() {
        match cs_bool(v[i + 1]) {
            Some(on) => set.push((*flag, on)),
            None => {
                let why = format!("Unable to process {what} value please use true or false");
                return failure(C, &why, &owned(&v[..i + 2]));
            }
        }
    }
    let mut info =
        match tokio::time::timeout(Duration::from_secs(3), ctx.client.estate_info()).await {
            Ok(Ok(i)) => i,
            _ => return failure(C, "Timed out waiting for EstateInfoReply", &[]),
        };
    if info.name.is_empty() {
        return failure(C, "No estate info reply received", &[]);
    }
    for (flag, on) in set {
        info.set(flag, on);
    }
    info.sun_hour = sun;
    let args = owned(&v);
    let body = estate_change_body(&info, uuid::Uuid::new_v4());
    match ctx.client.estate_post("EstateChangeInfo", body).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) if e.to_string().contains("no EstateChangeInfo") => {
            let strings = estate_change_strings(&info);
            message(
                ctx,
                C,
                "estatechangeinfo",
                strings,
                "ok using fallback",
                &args,
            )
            .await
        }
        Err(e) => failure(C, &e.to_string(), &args),
    }
}
