//! Commands by IM, as SecondBot's CommandsService.BotImMessage takes them:
//! - from a master (IM_NOTHING_SPECIAL, recognised by key): run without a signature;
//! - from an object (IM_FROM_TASK): signed with the shared secret, unless the object's owner is a
//!   master and FERRO_OBJECT_MASTER_OPTOUT is on.
//!
//! The text is SignedCommand's: `command|||arg~#~arg#|#reply target@@@signature;|;unixtime`, or a
//! JSON object `{cmd, args, reply, signing, unixtime}`. The signature leaves the time out unless
//! FERRO_ENFORCE_TIME_WINDOW is on (then the command must be recent too). The answer goes to the
//! reply target, when there is one (crate::smart_reply).

use crate::api::signing::{same, sign_with};
use crate::commands::{self, Ctx};
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

/// A command as SignedCommand unpacks it.
#[derive(Debug, Default, PartialEq)]
pub struct Unpacked {
    pub command: String,
    pub args: Vec<String>,
    pub reply: Option<String>,
    pub signing: Option<String>,
    pub unixtime: i64,
}

/// SignedCommand.UnpackInput: JSON when the text is an object, else the text form.
pub fn unpack(text: &str) -> Option<Unpacked> {
    let t = text.trim();
    let u = if t.starts_with('{') && t.ends_with('}') {
        let v: serde_json::Value = serde_json::from_str(t).ok()?;
        let s = |k: &str| v[k].as_str().map(str::to_owned);
        Unpacked {
            command: s("cmd").unwrap_or_default(),
            args: v["args"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            reply: s("reply"),
            signing: s("signing"),
            unixtime: v["unixtime"].as_i64().unwrap_or(0),
        }
    } else {
        let mut u = Unpacked::default();
        let mut rest = t;
        if let Some((head, time)) = rest.split_once(";|;") {
            u.unixtime = time.trim().parse().ok()?;
            rest = head;
        }
        if let Some((head, sig)) = rest.split_once("@@@") {
            u.signing = Some(sig.to_owned());
            rest = head;
        }
        if let Some((head, target)) = rest.split_once("#|#") {
            u.reply = Some(target.to_owned());
            rest = head;
        }
        match rest.split_once("|||") {
            Some((command, args)) => {
                u.command = command.to_owned();
                u.args = args.split("~#~").map(str::to_owned).collect();
            }
            None => u.command = rest.to_owned(),
        }
        u
    };
    (!u.command.trim().is_empty()).then_some(u)
}

/// SignedCommand.Validate, for commands that need a signature.
fn signed(u: &Unpacked, s: &Settings, now: i64) -> bool {
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
    let need_signing = match im.dialog {
        dialog::NOTHING_SPECIAL => {
            if !masters.by_key(&ctx.client, im.from_agent_id).await {
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
    let Some(u) = unpack(&im.message) else {
        return;
    };
    if need_signing && !signed(&u, &settings, now()) {
        log::warn!("command by IM from {} refused: not signed", im.from_name);
        return;
    }
    let (ran, value) = match tokio::time::timeout(
        crate::api::COMMAND_DEADLINE,
        commands::run(&ctx, &u.command, &u.args),
    )
    .await
    {
        Ok(answer) => answer,
        Err(_) => (false, "No reply from the grid [Retry later]".to_owned()),
    };
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
    fn the_text_form() {
        let u = unpack("Say|||0~#~hello there#|#42@@@abc;|;1800000000").expect("a command");
        assert_eq!(
            u,
            Unpacked {
                command: "Say".into(),
                args: vec!["0".into(), "hello there".into()],
                reply: Some("42".into()),
                signing: Some("abc".into()),
                unixtime: 1_800_000_000,
            }
        );
        let u = unpack("Name").expect("a bare command");
        assert_eq!((u.command.as_str(), u.args.len()), ("Name", 0));
        assert!(unpack("   ").is_none());
        assert!(unpack("Name;|;soon").is_none(), "a bad time");
    }

    #[test]
    fn the_json_form() {
        let u = unpack(
            r#"{"cmd":"IM","args":["Some Resident","hi"],"reply":"0","signing":"x","unixtime":5}"#,
        )
        .expect("a command");
        assert_eq!(u.command, "IM");
        assert_eq!(u.args, vec!["Some Resident", "hi"]);
        assert_eq!((u.reply.as_deref(), u.unixtime), (Some("0"), 5));
    }

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
