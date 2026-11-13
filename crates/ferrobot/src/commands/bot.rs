//! SecondBot's Self commands that need nothing more than the bot (GoHome, Logoff,
//! GetLastCommands, the key-value store), and Ferrobot's own BotStatus.

use super::reply::{Answer, basic, failure, json};
use super::{Ctx, owned};

/// GoHome: homebound's home sim when it is on, else the Second Life home. "ok" at once, as
/// SecondBot answers before the teleport.
pub fn go_home(ctx: &Ctx) -> Answer {
    let client = ctx.client.clone();
    let home = ctx.home.clone();
    tokio::spawn(async move {
        let done = match home {
            Some(p) => client.teleport_to_region(&p.region, p.position).await,
            None => client.teleport_home().await,
        };
        if let Err(e) = done {
            log::warn!("GoHome: {e}");
        }
    });
    basic("GoHome", "ok", &[])
}

/// Logoff: the bot logs out and stops, as on SIGTERM (no relog).
pub fn logoff(ctx: &Ctx) -> Answer {
    log::info!("Logoff asked by command");
    let _ = ctx.stop.send(true);
    basic("Logoff", "ok", &[])
}

/// GetLastCommands: the last 5 commands, each as its JSON, in `args`. The newest first; SecondBot
/// gives the oldest 5 it kept, which its name does not mean.
pub fn get_last_commands(ctx: &Ctx) -> Answer {
    let last: Vec<String> = ctx.store.last(5).iter().map(json).collect();
    basic("GetLastCommands", "ok", &last)
}

pub fn read_key_value(ctx: &Ctx, key: &str) -> Answer {
    basic("ReadKeyValue", &ctx.store.get(key), &[])
}

/// SetKeyValue: SecondBot's refuses every key that is not empty (an inverted check); this one
/// refuses the empty ones, as its texts say.
pub fn set_key_value(ctx: &Ctx, key: &str, value: &str) -> Answer {
    let args = owned(&[key, value]);
    if key.is_empty() {
        return failure("SetKeyValue", "Key is empty", &args);
    }
    if value.is_empty() {
        return failure("SetKeyValue", "Value is empty", &args);
    }
    ctx.store.set(key, value);
    basic("SetKeyValue", "ok", &[])
}

pub fn clear_key_value(ctx: &Ctx, key: &str) -> Answer {
    if key.is_empty() {
        return failure("ClearKeyValue", "Key is empty", &owned(&[key]));
    }
    ctx.store.clear(key);
    basic("ClearKeyValue", "ok", &[])
}

/// BotStatus (Ferrobot's): in world or not, where, since when, the logins, the commands run and
/// failed, the time up.
pub fn bot_status(ctx: &Ctx) -> Answer {
    let st = ctx.client.state();
    let (commands, failures) = ctx.store.counts();
    let since = st
        .online_since
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let status = serde_json::json!({
        "online": st.online,
        "region": st.region.map(|r| r.name),
        "online_since": since,
        "logins": st.logins,
        "uptime": ctx.store.started.elapsed().as_secs(),
        "commands": commands,
        "failures": failures,
        "version": ferro_client::login::version(),
    });
    basic("BotStatus", &status.to_string(), &[])
}

/// RebakeAppearance: the region bakes the outfit again.
pub async fn rebake(ctx: &Ctx) -> Answer {
    const C: &str = "RebakeAppearance";
    match ctx.client.rebake().await {
        Ok(_) => basic(C, "ok", &[]),
        Err(ferro_client::ClientError::Timeout) => {
            failure(C, "Rebake did not finish in 60 secs", &[])
        }
        Err(e) => failure(C, &format!("Rebake failed: {e}"), &[]),
    }
}

/// LibreMetaverse's MuteType names, which SecondBot prints.
fn mute_type(kind: i32) -> String {
    match kind {
        0 => "ByName".into(),
        1 => "Resident".into(),
        2 => "Object".into(),
        3 => "Group".into(),
        4 => "External".into(),
        n => n.to_string(),
    }
}

/// LibreMetaverse's MuteFlags as C# writes a [Flags] enum: "Default", "All", or the names of its
/// bits ("TextChat, VoiceChat"); a value with other bits is its number.
fn mute_flags(flags: u32) -> String {
    const NAMES: [(u32, &str); 4] = [
        (1, "TextChat"),
        (2, "VoiceChat"),
        (4, "Particles"),
        (8, "ObjectSounds"),
    ];
    match flags {
        0 => "Default".into(),
        0xf => "All".into(),
        f if f & !0xf != 0 => f.to_string(),
        f => NAMES
            .iter()
            .filter(|(bit, _)| f & bit != 0)
            .map(|(_, n)| *n)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

#[derive(serde::Serialize)]
struct MuteEntry {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    name: String,
    flags: String,
}

/// GetMuteList: what the grid sent (asked for at each login); "true" asks again.
pub async fn mute_list(ctx: &Ctx, refresh: &str) -> Answer {
    const C: &str = "GetMuteList";
    let args = owned(&[refresh]);
    let Some(again) = super::cs_bool(refresh) else {
        return failure(C, "Invaild refresh value", &args);
    };
    if again {
        let _ = ctx.client.request_mute_list().await;
        return basic(C, "Requesting mute list [Retry later]", &args);
    }
    let list: Vec<MuteEntry> = ctx
        .client
        .state()
        .mute_list
        .unwrap_or_default()
        .into_iter()
        .map(|m| MuteEntry {
            kind: mute_type(m.kind),
            id: m.id.to_string(),
            name: m.name,
            flags: mute_flags(m.flags),
        })
        .collect();
    basic(C, &json(&list), &args)
}

/// SetAway: away (or back), shown next to the name in the viewer.
pub async fn set_away(ctx: &Ctx, mode: &str) -> Answer {
    const C: &str = "SetAway";
    let args = owned(&[mode]);
    let Some(away) = super::cs_bool(mode) else {
        return failure(C, "Invaild mode", &args);
    };
    match ctx.client.move_bot(ferro_client::Move::Away(away)).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

#[cfg(test)]
mod mute_tests {
    use super::*;

    #[test]
    fn mute_flags_as_csharp_writes_them() {
        assert_eq!(mute_flags(0), "Default");
        assert_eq!(mute_flags(15), "All");
        assert_eq!(mute_flags(3), "TextChat, VoiceChat");
        assert_eq!(mute_flags(12), "Particles, ObjectSounds");
        assert_eq!(mute_flags(16), "16");
        assert_eq!(mute_type(1), "Resident");
    }
}
