//! What the bot reads from RabbitMQ, as SecondBot's RabbitService: four queues of JSON messages,
//! their fields named as SecondBot's classes name them (System.Text.Json's default: exact case).
//! - notecards: `{Name, Subject, Content, UUID}`: a notecard made and given to that avatar (its
//!   SendNotecard, the subject as the item's description);
//! - commands: `{CommandName, CommandArgs}`: any command;
//! - ims: `{UUID, Message}`: the IM command (an avatar's key or name);
//! - groupims: `{UUID, Message}`: the Groupchat command (a group's key).
//!
//! Deliberate difference: SecondBot runs whatever comes in, unsigned. Here every message carries
//! a signature as a command by IM does (AGENTS.md: every command from outside is authenticated),
//! in `Signing` and, with FERRO_ENFORCE_TIME_WINDOW on, `Unixtime`: sha1 of a name, the arguments
//! joined with `~#~`, the time (when enforced) and the shared secret:
//! - commands: the command's name and arguments;
//! - ims: `IM` with UUID and Message; groupims: `Groupchat` with UUID and Message;
//! - notecards: `Notecard` with Name, Subject, Content and UUID.
//!
//! Without a shared secret the queues are not read at all. Nothing is answered (SecondBot runs
//! these commands with no reply target).

use crate::api::signing::Unpacked;
use crate::commands::{self, Ctx};
use crate::services::im_commands;
use serde::Deserialize;
use std::sync::Arc;

/// Which of SecondBot's queues a message came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Notecards,
    Commands,
    Ims,
    GroupIms,
}

/// The signature's fields, on every message (Ferrobot's; either case).
#[derive(Deserialize, Default)]
struct Signed {
    #[serde(rename = "Signing", alias = "signing")]
    signing: Option<String>,
    #[serde(rename = "Unixtime", alias = "unixtime")]
    unixtime: Option<i64>,
}

/// JsonCommandFormat.
#[derive(Deserialize)]
struct CommandMessage {
    #[serde(rename = "CommandName")]
    command_name: Option<String>,
    #[serde(rename = "CommandArgs")]
    command_args: Option<Vec<String>>,
    #[serde(flatten)]
    signed: Signed,
}

/// JsonMessageFormat.
#[derive(Deserialize)]
struct TextMessage {
    #[serde(rename = "UUID")]
    uuid: Option<String>,
    #[serde(rename = "Message")]
    message: Option<String>,
    #[serde(flatten)]
    signed: Signed,
}

/// JsonNotecardFormat.
#[derive(Deserialize)]
struct NotecardMessage {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Subject")]
    subject: Option<String>,
    #[serde(rename = "Content")]
    content: Option<String>,
    #[serde(rename = "UUID")]
    uuid: Option<String>,
    #[serde(flatten)]
    signed: Signed,
}

/// What became of a message.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not one of the queue's messages (SecondBot's "format is invalid or missing fields").
    Invalid(&'static str),
    /// Not signed with the shared secret.
    Refused,
    /// The command ran (or was refused by the dispatcher); whether it worked.
    Ran { command: String, done: bool },
    /// The notecard was given, or not.
    Notecard { done: bool },
}

/// What the queues' messages are handled with.
#[derive(Clone)]
pub struct Inbound {
    pub ctx: Ctx,
    /// The shared secret and the time window, as for commands by IM.
    pub check: Arc<im_commands::Settings>,
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// A message read as a command: its name, arguments and signature.
fn unpack(kind: Kind, body: &[u8]) -> Result<Unpacked, &'static str> {
    let text = std::str::from_utf8(body).map_err(|_| "not UTF-8")?;
    let as_unpacked = |command: String, args: Vec<String>, s: Signed| Unpacked {
        command,
        args,
        reply: None,
        signing: s.signing,
        unixtime: s.unixtime.unwrap_or(0),
    };
    match kind {
        Kind::Commands => {
            let m: CommandMessage = serde_json::from_str(text).map_err(|_| "not its JSON")?;
            let name = non_empty(m.command_name).ok_or("no CommandName")?;
            Ok(as_unpacked(
                name,
                m.command_args.unwrap_or_default(),
                m.signed,
            ))
        }
        Kind::Ims | Kind::GroupIms => {
            let m: TextMessage = serde_json::from_str(text).map_err(|_| "not its JSON")?;
            let (Some(uuid), Some(message)) = (non_empty(m.uuid), non_empty(m.message)) else {
                return Err("no UUID or no Message");
            };
            let command = if kind == Kind::Ims { "IM" } else { "Groupchat" };
            Ok(as_unpacked(command.into(), vec![uuid, message], m.signed))
        }
        Kind::Notecards => {
            let m: NotecardMessage = serde_json::from_str(text).map_err(|_| "not its JSON")?;
            let (Some(name), Some(subject), Some(content), Some(uuid)) = (
                non_empty(m.name),
                non_empty(m.subject),
                non_empty(m.content),
                non_empty(m.uuid),
            ) else {
                return Err("no Name, Subject, Content or UUID");
            };
            Ok(as_unpacked(
                "Notecard".into(),
                vec![name, subject, content, uuid],
                m.signed,
            ))
        }
    }
}

impl Inbound {
    /// One message from one of the queues.
    pub async fn handle(&self, kind: Kind, queue: &str, body: &[u8]) -> Outcome {
        let u = match unpack(kind, body) {
            Ok(u) => u,
            Err(why) => {
                // the message itself is not logged: it may carry anything
                log::warn!("RabbitMQ: a message on {queue} is not read: {why}");
                return Outcome::Invalid(why);
            }
        };
        if !im_commands::signed(&u, &self.check, now()) {
            log::warn!("RabbitMQ: a message on {queue} refused: not signed");
            return Outcome::Refused;
        }
        if kind == Kind::Notecards {
            return self.notecard(queue, &u.args).await;
        }
        let (ran, value) = match tokio::time::timeout(
            crate::api::COMMAND_DEADLINE,
            commands::run(&self.ctx, &u.command, &u.args),
        )
        .await
        {
            Ok(answer) => answer,
            Err(_) => (false, "No reply from the grid [Retry later]".to_owned()),
        };
        let done = ran && value.contains(r#""status":true"#);
        log::info!(
            "command from RabbitMQ ({queue}): {} ({} args): {}",
            u.command,
            u.args.len(),
            if done { "done" } else { "failed" }
        );
        Outcome::Ran {
            command: u.command,
            done,
        }
    }

    /// SecondBot's SendNotecard(Name, Content, UUID, no date, Subject).
    async fn notecard(&self, queue: &str, args: &[String]) -> Outcome {
        let [name, subject, content, uuid] = args else {
            return Outcome::Invalid("no Name, Subject, Content or UUID");
        };
        let Ok(to) = uuid::Uuid::parse_str(uuid.trim()) else {
            log::warn!("RabbitMQ: a notecard on {queue}: UUID is not valid");
            return Outcome::Invalid("UUID is not valid");
        };
        // it changes things in world: under the rate limit, as the commands that do
        if !commands::admit(&self.ctx).await {
            log::warn!("RabbitMQ: a notecard on {queue}: too many commands, refused");
            return Outcome::Notecard { done: false };
        }
        match self
            .ctx
            .client
            .send_notecard(to, name, subject, content)
            .await
        {
            Ok(()) => {
                log::info!("notecard from RabbitMQ ({queue}) given");
                Outcome::Notecard { done: true }
            }
            Err(e) => {
                log::warn!("RabbitMQ: a notecard on {queue}: {e}");
                Outcome::Notecard { done: false }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::signing::sign_with;

    const SECRET: &str = "a shared secret";

    fn inbound(enforce_time_window: bool) -> Inbound {
        Inbound {
            ctx: commands::test_ctx(),
            check: Arc::new(im_commands::Settings {
                secret: Some(SECRET.into()),
                enforce_time_window,
                time_window: 35,
                object_master_optout: false,
            }),
        }
    }

    fn sig(command: &str, args: &[&str], time: Option<i64>) -> String {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        sign_with(command, &args, time, SECRET)
    }

    #[test]
    fn secondbots_messages_unpacked() {
        let u = unpack(
            Kind::Commands,
            br#"{"CommandName":"Say","CommandArgs":["0","hi"],"Signing":"s","Unixtime":5}"#,
        )
        .expect("a command");
        assert_eq!(
            (u.command.as_str(), u.args.clone()),
            ("Say", vec!["0".into(), "hi".into()])
        );
        assert_eq!((u.signing.as_deref(), u.unixtime), (Some("s"), 5));
        let u =
            unpack(Kind::Commands, br#"{"CommandName":"Name","signing":"s"}"#).expect("no args");
        assert!(u.args.is_empty());
        assert_eq!(u.signing.as_deref(), Some("s"));
        let u = unpack(Kind::Ims, br#"{"UUID":"k","Message":"hello"}"#).expect("an IM");
        assert_eq!(
            (u.command.as_str(), u.args.clone()),
            ("IM", vec!["k".into(), "hello".into()])
        );
        let u = unpack(Kind::GroupIms, br#"{"UUID":"g","Message":"hello"}"#).expect("group");
        assert_eq!(u.command, "Groupchat");
        let u = unpack(
            Kind::Notecards,
            br#"{"Name":"n","Subject":"s","Content":"c","UUID":"k"}"#,
        )
        .expect("a notecard");
        assert_eq!((u.command.as_str(), u.args.len()), ("Notecard", 4));
        // SecondBot's checks: every field there and not empty, names in their exact case
        assert!(unpack(Kind::Commands, br#"{"CommandArgs":[]}"#).is_err());
        assert!(unpack(Kind::Commands, br#"{"commandname":"Name"}"#).is_err());
        assert!(unpack(Kind::Ims, br#"{"UUID":"k","Message":""}"#).is_err());
        assert!(unpack(Kind::Notecards, br#"{"Name":"n","Content":"c","UUID":"k"}"#).is_err());
        assert!(unpack(Kind::Commands, b"not json").is_err());
        assert!(unpack(Kind::Commands, b"\xff\xfe").is_err());
        assert!(
            unpack(
                Kind::Commands,
                br#"{"CommandName":"Say","CommandArgs":[1]}"#
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn signed_commands_run_and_others_do_not() {
        let i = inbound(false);
        let unsigned = br#"{"CommandName":"SetKeyValue","CommandArgs":["k","v"]}"#;
        assert_eq!(
            i.handle(Kind::Commands, "commands", unsigned).await,
            Outcome::Refused
        );
        let forged = format!(
            r#"{{"CommandName":"SetKeyValue","CommandArgs":["k","v"],"Signing":"{}"}}"#,
            sign_with(
                "SetKeyValue",
                &["k".into(), "v".into()],
                None,
                "another secret"
            )
        );
        assert_eq!(
            i.handle(Kind::Commands, "commands", forged.as_bytes())
                .await,
            Outcome::Refused
        );
        assert_eq!(i.ctx.store.get("k"), "", "nothing ran");
        let signed = format!(
            r#"{{"CommandName":"SetKeyValue","CommandArgs":["k","v"],"Signing":"{}"}}"#,
            sig("SetKeyValue", &["k", "v"], None)
        );
        assert_eq!(
            i.handle(Kind::Commands, "commands", signed.as_bytes())
                .await,
            Outcome::Ran {
                command: "SetKeyValue".into(),
                done: true
            }
        );
        assert_eq!(i.ctx.store.get("k"), "v");
        assert_eq!(
            i.handle(Kind::Commands, "commands", b"{}").await,
            Outcome::Invalid("no CommandName")
        );
    }

    #[tokio::test]
    async fn with_the_time_window_the_time_is_signed_too() {
        let i = inbound(true);
        let without_time = format!(
            r#"{{"CommandName":"SetKeyValue","CommandArgs":["k","v"],"Signing":"{}"}}"#,
            sig("SetKeyValue", &["k", "v"], None)
        );
        assert_eq!(
            i.handle(Kind::Commands, "commands", without_time.as_bytes())
                .await,
            Outcome::Refused
        );
        let t = now();
        let with_time = format!(
            r#"{{"CommandName":"SetKeyValue","CommandArgs":["k","v"],"Signing":"{}","Unixtime":{t}}}"#,
            sig("SetKeyValue", &["k", "v"], Some(t))
        );
        assert!(matches!(
            i.handle(Kind::Commands, "commands", with_time.as_bytes())
                .await,
            Outcome::Ran { done: true, .. }
        ));
        let old = t - 100;
        let too_old = format!(
            r#"{{"CommandName":"SetKeyValue","CommandArgs":["k","v"],"Signing":"{}","Unixtime":{old}}}"#,
            sig("SetKeyValue", &["k", "v"], Some(old))
        );
        assert_eq!(
            i.handle(Kind::Commands, "commands", too_old.as_bytes())
                .await,
            Outcome::Refused
        );
    }

    #[tokio::test]
    async fn ims_group_ims_and_notecards_are_signed_as_their_commands() {
        let i = inbound(false);
        let key = "00000000-0000-0000-0000-00005eed0002";
        // the IM signed as the Groupchat command is not an IM
        let im = format!(
            r#"{{"UUID":"{key}","Message":"hello","Signing":"{}"}}"#,
            sig("Groupchat", &[key, "hello"], None)
        );
        assert_eq!(
            i.handle(Kind::Ims, "ims", im.as_bytes()).await,
            Outcome::Refused
        );
        // signed right, it reaches the IM command (which fails here: the bot is not in world)
        let im = format!(
            r#"{{"UUID":"{key}","Message":"hello","Signing":"{}"}}"#,
            sig("IM", &[key, "hello"], None)
        );
        assert!(matches!(
            i.handle(Kind::Ims, "ims", im.as_bytes()).await,
            Outcome::Ran { command, .. } if command == "IM"
        ));
        let group = format!(
            r#"{{"UUID":"{key}","Message":"hello","Signing":"{}"}}"#,
            sig("Groupchat", &[key, "hello"], None)
        );
        assert!(matches!(
            i.handle(Kind::GroupIms, "groupims", group.as_bytes()).await,
            Outcome::Ran { command, .. } if command == "Groupchat"
        ));
        let card = |uuid: &str| {
            format!(
                r#"{{"Name":"n","Subject":"s","Content":"c","UUID":"{uuid}","Signing":"{}"}}"#,
                sig("Notecard", &["n", "s", "c", uuid], None)
            )
        };
        assert_eq!(
            i.handle(Kind::Notecards, "notecards", card("not a key").as_bytes())
                .await,
            Outcome::Invalid("UUID is not valid")
        );
        // not in world: the notecard cannot be made, and that is all
        assert_eq!(
            i.handle(Kind::Notecards, "notecards", card(key).as_bytes())
                .await,
            Outcome::Notecard { done: false }
        );
    }
}
