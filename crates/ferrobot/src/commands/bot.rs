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
