//! Commands by IM, as SecondBot's CommandsService.BotImMessage takes them:
//! - from a master (IM_NOTHING_SPECIAL, recognised by key): run without a signature;
//! - from an avatar granted "command" by SetPermFlag (crate::grants), recognised by key the same
//!   way: run without a signature, its next command only or until the bot stops, but never the
//!   commands that hand out grants (commands::GRANTING). Deliberate difference: SecondBot's
//!   SetPermFlag does nothing. The grant is made by an authenticated party (a signed command, or
//!   a master), the sender's key comes from the grid, not from the message, and the grantee
//!   cannot widen its grant or pass it on: nobody gets more than what a master or the holder of
//!   the shared secret gave. Its objects still sign their commands;
//! - from an object (IM_FROM_TASK): signed with the shared secret, unless the object's owner is a
//!   master and FERRO_OBJECT_MASTER_OPTOUT is on.
//!
//! The text is SignedCommand's: `command|||arg~#~arg#|#reply target@@@signature;|;unixtime`, or a
//! JSON object `{cmd, args, reply, signing, unixtime}`. The signature leaves the time out unless
//! FERRO_ENFORCE_TIME_WINDOW is on (then the command must be recent too). The answer goes to the
//! reply target, when there is one (crate::smart_reply).

use crate::api::signing::{Unpacked, same, sign_with, unpack};
use crate::commands::{self, Ctx};
use crate::grants::Grant;
use crate::masters::Masters;
use ferro_client::events::dialog;
use ferro_client::{Event, InstantMessage};
use std::sync::Arc;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

/// IM_FROM_TASK (llinstantmessage.h): llInstantMessage from an object.
const FROM_TASK: u8 = 19;

pub struct Settings {
    pub secret: Option<String>,
    pub enforce_time_window: bool,
    pub time_window: u64,
    pub object_master_optout: bool,
}

/// SignedCommand.Validate, for commands that need a signature (by IM, and from RabbitMQ).
pub fn signed(u: &Unpacked, s: &Settings, now: i64) -> bool {
    let (Some(secret), Some(signing)) = (&s.secret, &u.signing) else {
        return false;
    };
    let time = if s.enforce_time_window {
        let window = i64::try_from(s.time_window).unwrap_or(i64::MAX);
        if u.unixtime == 0 || now.saturating_sub(u.unixtime) > window {
            return false;
        }
        Some(u.unixtime)
    } else {
        None
    };
    same(&sign_with(&u.command, &u.args, time, secret), signing)
}

/// Whether an avatar that is not a master may run this command on its "command" grant. A
/// one-time grant is used up here, before the command runs, and only by a command that exists:
/// plain talk keeps it.
async fn granted(ctx: &Ctx, u: &Unpacked, im: &InstantMessage) -> bool {
    let Some(command) = commands::known(&u.command) else {
        return false;
    };
    let id = im.from_agent_id;
    if !ctx.grants.has_lasting(Grant::Command, id) && !ctx.grants.has_once(Grant::Command, id) {
        return false;
    }
    if commands::GRANTING.contains(&command) {
        log::warn!(
            "command by IM from {}: {command} refused, a command grant does not reach it",
            im.from_name
        );
        return false;
    }
    match ctx.grants.use_key(Grant::Command, id) {
        Some(used) => {
            log::info!("command by IM from {} on a {used:?} grant", im.from_name);
            true
        }
        None => false,
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

async fn on_im(ctx: Ctx, masters: Arc<Masters>, settings: Arc<Settings>, im: InstantMessage) {
    let me = ctx.client.state().agent_id;
    if im.from_agent_id == me || im.from_group {
        return;
    }
    let Some(u) = unpack(&im.message) else {
        return;
    };
    let need_signing = match im.dialog {
        dialog::NOTHING_SPECIAL => {
            if !masters.by_key(&ctx.client, im.from_agent_id).await && !granted(&ctx, &u, &im).await
            {
                if commands::known(&u.command).is_some() {
                    log::debug!("command by IM from {} refused: not a master", im.from_name);
                }
                return;
            }
            false
        }
        FROM_TASK => {
            // the IM's sender id is the object's owner (IM_FROM_TASK)
            !(settings.object_master_optout && masters.by_key(&ctx.client, im.from_agent_id).await)
        }
        _ => return,
    };
    if need_signing && !signed(&u, &settings, now()) {
        log::warn!("command by IM from {} refused: not signed", im.from_name);
        return;
    }
    let (ran, value) = crate::api::run_command(&ctx, &u.command, &u.args).await;
    // a master's IM is tried as a command, as SecondBot does: plain talk is no failure
    if !ran && value == "Unknown command" {
        log::debug!("IM from {} is not a command", im.from_name);
        return;
    }
    log::info!(
        "command by IM from {}: {} ({} args): {}",
        im.from_name,
        u.command,
        u.args.len(),
        if ran && value.contains(r#""status":true"#) {
            "done"
        } else {
            "failed"
        }
    );
    if let Some(target) = &u.reply {
        crate::smart_reply::send(&ctx, target, &value, &u.command).await;
    }
}

pub async fn run(ctx: Ctx, mut events: Receiver<Event>, masters: Arc<Masters>, settings: Settings) {
    let settings = Arc::new(settings);
    loop {
        match events.recv().await {
            Ok(Event::InstantMessage(im))
                if matches!(im.dialog, dialog::NOTHING_SPECIAL | FROM_TASK) =>
            {
                tokio::spawn(on_im(ctx.clone(), masters.clone(), settings.clone(), im));
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("IM commands: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_without_and_with_the_time() {
        let s = |enforce| Settings {
            secret: Some("secret".into()),
            enforce_time_window: enforce,
            time_window: 35,
            object_master_optout: false,
        };
        let args = vec!["0".to_owned(), "hi".to_owned()];
        let mut u = Unpacked {
            command: "Say".into(),
            args: args.clone(),
            signing: Some(sign_with("Say", &args, None, "secret")),
            ..Unpacked::default()
        };
        assert!(signed(&u, &s(false), 1_000));
        assert!(!signed(&u, &s(true), 1_000), "the time is asked for");
        u.unixtime = 990;
        u.signing = Some(sign_with("Say", &args, Some(990), "secret"));
        assert!(signed(&u, &s(true), 1_000));
        assert!(!signed(&u, &s(true), 1_100), "too old");
        let no_secret = Settings {
            secret: None,
            ..s(false)
        };
        assert!(!signed(&u, &no_secret, 1_000));
    }
}
